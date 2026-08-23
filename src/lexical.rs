//! Lexical extraction backend: blob-free packs for the long tail of
//! languages.
//!
//! Instead of a compiled grammar, a `source = "lexical"` pack declares a
//! small lexical profile — comment and string syntax, nesting brackets —
//! plus regex openers for blocks and tests. Extraction then:
//!
//! 1. masks comments and strings, so openers only match real code and
//!    bracket counting never sees a brace inside a string;
//! 2. matches the opener regexes, rejecting any match whose keyword is
//!    masked (a decoy in a comment) or whose title is not a real string;
//! 3. derives each match's span from its call bracket's matching closer,
//!    and nesting from span containment — the same structural rule the
//!    tree-sitter path uses (shared `Capture`/`build_nodes`).
//!
//! The robustness ceiling is lower than a real grammar: syntax the profile
//! cannot see (JS regex literals, template-literal interpolation, exotic
//! line terminators) is invisible to a lexical scan. Extraction therefore
//! fails closed — unbalanced brackets or an opener without a call bracket
//! are errors, never silent partial results — and `tests/lexical.rs`
//! differentially fuzzes this backend against the native grammar.

use crate::error::{Error, Result};
use crate::extract::{ActualKind, ActualNode, Capture, build_nodes, decode_name};
use crate::pack::{Lexical, Pack};
use regex::Regex;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// What a byte of source belongs to, per the pack's lexical profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Code,
    Comment,
    Str,
}

/// Extract the actual test structure using the pack's lexical profile.
pub(crate) fn extract(pack: &Pack, cfg: &Lexical, source: &str) -> Result<Vec<ActualNode>> {
    let err = |message: String| Error::Lexical {
        pack: pack.name().to_string(),
        message,
    };

    let pairs = bracket_pairs(cfg).map_err(err)?;
    let states = mask(source.as_bytes(), cfg);
    let brackets = match_brackets(source.as_bytes(), &states, &pairs)
        .map_err(|(pos, what)| err(format!("{what} (line {})", line_of(source, pos))))?;

    let mut captures: Vec<Capture> = Vec::new();
    for (kind, pattern) in [
        (ActualKind::Block, cfg.block.open.as_str()),
        (ActualKind::Test, cfg.test.open.as_str()),
    ] {
        collect(kind, pattern, pack, source, &states, &brackets, &pairs)
            .map(|found| captures.extend(found))
            .map_err(err)?;
    }

    captures.sort_by_key(|c| (c.start, std::cmp::Reverse(c.end)));
    captures.dedup_by_key(|c| (c.start, c.end, c.kind));
    let mut i = 0;
    let top = build_nodes(&captures, &mut i, usize::MAX);
    Ok(ActualNode::prune_empty_blocks(top))
}

/// The profile's bracket pairs as single bytes (multi-byte brackets are
/// not supported).
fn bracket_pairs(cfg: &Lexical) -> std::result::Result<Vec<(u8, u8)>, String> {
    cfg.nest
        .iter()
        .map(|(open, close)| match (open.as_bytes(), close.as_bytes()) {
            (&[o], &[c]) => Ok((o, c)),
            _ => Err(format!(
                "nest pair [{open:?}, {close:?}] must be single characters"
            )),
        })
        .collect()
}

/// Classify every byte as code, comment, or string content. Comments and
/// strings are matched greedily left-to-right; string escapes consume the
/// following byte so an escaped delimiter does not close the literal.
fn mask(source: &[u8], cfg: &Lexical) -> Vec<State> {
    let mut states = vec![State::Code; source.len()];
    let mut i = 0;
    'outer: while i < source.len() {
        if let Some(lc) = &cfg.line_comment
            && source[i..].starts_with(lc.as_bytes())
        {
            while i < source.len() && source[i] != b'\n' {
                states[i] = State::Comment;
                i += 1;
            }
            continue;
        }
        if let Some((open, close)) = &cfg.block_comment
            && source[i..].starts_with(open.as_bytes())
        {
            let start = i;
            i += open.len();
            while i < source.len() && !source[i..].starts_with(close.as_bytes()) {
                i += 1;
            }
            i = (i + close.len()).min(source.len());
            states[start..i].fill(State::Comment);
            continue;
        }
        for rule in &cfg.strings {
            if source[i..].starts_with(rule.delim.as_bytes()) {
                let start = i;
                i += rule.delim.len();
                while i < source.len() {
                    if let Some(esc) = &rule.escape
                        && source[i..].starts_with(esc.as_bytes())
                    {
                        i += esc.len() + 1;
                        continue;
                    }
                    if source[i..].starts_with(rule.delim.as_bytes()) {
                        i += rule.delim.len();
                        break;
                    }
                    i += 1;
                }
                states[start..i.min(source.len())].fill(State::Str);
                continue 'outer;
            }
        }
        i += 1;
    }
    states
}

/// Map every code-position open bracket to its matching close. Fails
/// closed: brackets the scan cannot balance mean the profile does not
/// understand this file, and partial extraction would silently mis-nest.
fn match_brackets(
    source: &[u8],
    states: &[State],
    pairs: &[(u8, u8)],
) -> std::result::Result<HashMap<usize, usize>, (usize, String)> {
    let mut stack: Vec<(u8, usize)> = Vec::new(); // (expected close, open pos)
    let mut map = HashMap::new();
    for (i, (&b, &state)) in source.iter().zip(states).enumerate() {
        if state != State::Code {
            continue;
        }
        if let Some(&(_, close)) = pairs.iter().find(|(open, _)| *open == b) {
            stack.push((close, i));
        } else if pairs.iter().any(|(_, close)| *close == b) {
            match stack.pop() {
                Some((expected, open)) if expected == b => {
                    map.insert(open, i);
                }
                _ => return Err((i, format!("unbalanced `{}`", char::from(b)))),
            }
        }
    }
    if let Some(&(expected, open)) = stack.last() {
        return Err((
            open,
            format!("unclosed delimiter, expected `{}`", char::from(expected)),
        ));
    }
    Ok(map)
}

/// Run one opener pattern over the source, keeping matches whose keyword
/// is real code and whose title is a real string literal. A rejected
/// match (a decoy inside a comment or string) resumes the search one byte
/// past its start so it cannot shadow a real opener it overlapped.
fn collect(
    kind: ActualKind,
    pattern: &str,
    pack: &Pack,
    source: &str,
    states: &[State],
    brackets: &HashMap<usize, usize>,
    pairs: &[(u8, u8)],
) -> std::result::Result<Vec<Capture>, String> {
    let re = compiled(pattern)?;
    let group = |want: &str| {
        re.capture_names()
            .position(|n| n == Some(want))
            .ok_or_else(|| format!("opener pattern must define a (?<{want}>...) group"))
    };
    let (kw_i, name_i) = (group("kw")?, group("name")?);
    let syntax = pack.manifest.extract.name_syntax;
    let bytes = source.as_bytes();

    let mut out = Vec::new();
    let mut locs = re.capture_locations();
    let mut at = 0;
    while at <= source.len() {
        let Some(m) = re.captures_read_at(&mut locs, source, at) else {
            break;
        };
        let Some((kw_start, _)) = locs.get(kw_i) else {
            return Err("(?<kw>...) did not participate in a match".to_string());
        };
        let Some((name_start, name_end)) = locs.get(name_i) else {
            return Err("(?<name>...) did not participate in a match".to_string());
        };
        if states[kw_start] != State::Code || states[name_start] != State::Str {
            at = m.start() + 1;
            continue;
        }
        // The span runs to the matching closer of the opener's call
        // bracket (mirrors the tree-sitter node span of the whole call).
        let open_pos = (kw_start..name_start)
            .find(|&i| states[i] == State::Code && pairs.iter().any(|(open, _)| *open == bytes[i]))
            .ok_or_else(|| {
                format!(
                    "opener at line {} has no call bracket",
                    line_of(source, kw_start)
                )
            })?;
        let end = brackets[&open_pos] + 1;
        out.push(Capture {
            kind,
            name: decode_name(syntax, &source[name_start..name_end]),
            line: line_of(source, kw_start),
            start: kw_start,
            end,
        });
        at = m.end();
    }
    Ok(out)
}

/// Compile an opener pattern once per process; extraction runs per file,
/// and regex compilation would otherwise dominate a scan of many files.
fn compiled(pattern: &str) -> std::result::Result<Regex, String> {
    static CACHE: OnceLock<Mutex<HashMap<String, Regex>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Mutex::default);
    let mut cache = cache.lock().expect("regex cache poisoned");
    if let Some(re) = cache.get(pattern) {
        return Ok(re.clone());
    }
    let re = Regex::new(pattern).map_err(|e| format!("invalid opener pattern: {e}"))?;
    cache.insert(pattern.to_string(), re.clone());
    Ok(re)
}

/// 1-based line number of a byte offset (always a char boundary: regex
/// match positions and ASCII bracket positions).
fn line_of(source: &str, pos: usize) -> usize {
    source[..pos].matches('\n').count() + 1
}
