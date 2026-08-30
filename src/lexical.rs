//! Lexical extraction backend: blob-free packs for the long tail of
//! languages.
//!
//! Instead of a compiled grammar, a `source = "lexical"` pack declares a
//! small lexical profile — comment and string syntax, nesting brackets —
//! plus regex openers for blocks and tests. Extraction then:
//!
//! 1. masks comments and strings, failing on anything unterminated;
//! 2. matches the opener regexes against a copy of the source with
//!    comments blanked to spaces (so legal trivia like
//!    `describe/* c */("x")` matches as whitespace), rejecting matches
//!    whose keyword is not real code or whose title is not in the state
//!    `name_syntax` promises (a string literal for `js-string`, plain
//!    code for `raw` identifiers);
//! 3. derives each match's span from the first bracket the match itself
//!    contains (`it(` → the call's parens; `mod foo {` → the body), and
//!    nesting from span containment — the same structural rule the
//!    tree-sitter path uses (shared `Capture`/`build_nodes`).
//!
//! ## Scope, honestly
//!
//! This is deliberately not a lexer-generator. It models two shapes —
//! call-pattern tests (`it("title", ...)`) and declaration-pattern tests
//! (`function test_x(`, `mod when_y {`) — because those cover the bulk
//! of real test conventions with a profile that is 100% reviewable text.
//! Syntax the profile cannot see is out of scope by design: JS regex
//! literals and template interpolation, Python's indentation nesting,
//! Ruby's `do … end`, attribute markers (`#[test]`). Those need either
//! future profile tiers or a real grammar (`wasm:`). The contract that
//! makes the tradeoff safe is failing closed: malformed profiles fail at
//! pack load, and files the scan cannot fully account for (unbalanced
//! brackets, unterminated strings or comments) are tool errors, never
//! silent partial extractions. `tests/lexical.rs` differentially fuzzes
//! the typescript profile against the native grammar.

use crate::error::{Error, Result};
use crate::extract::{
    ActualKind, ActualNode, Capture, Extraction, Unsupported, build_nodes, decode_name,
};
use crate::pack::{Lexical, NameSyntax, Pack};
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

/// Validate a lexical profile at pack-load time: every token non-empty,
/// bracket pairs usable, opener regexes compiling and defining the
/// required capture groups. A malformed profile must fail the pack load —
/// never hang or panic extraction later.
pub(crate) fn validate_profile(cfg: &Lexical) -> std::result::Result<(), String> {
    if cfg.line_comment.as_deref() == Some("") {
        return Err("line_comment must not be empty".to_string());
    }
    if let Some((open, close)) = &cfg.block_comment
        && (open.is_empty() || close.is_empty())
    {
        return Err("block_comment tokens must not be empty".to_string());
    }
    for rule in &cfg.strings {
        if rule.delim.is_empty() {
            return Err("string delim must not be empty".to_string());
        }
        if rule.escape.as_deref() == Some("") {
            return Err("string escape must not be empty".to_string());
        }
    }
    if cfg.nest.is_empty() {
        return Err("nest must declare at least one bracket pair".to_string());
    }
    bracket_pairs(cfg)?;
    for (which, opener) in [("block", &cfg.block), ("test", &cfg.test)] {
        let re = compiled(&opener.open).map_err(|e| format!("[lexical.{which}] {e}"))?;
        for group in ["kw", "name"] {
            if !re.capture_names().any(|n| n == Some(group)) {
                return Err(format!(
                    "[lexical.{which}] open must define a (?<{group}>...) group"
                ));
            }
        }
    }
    if let Some(pattern) = &cfg.unsupported {
        compiled(&pattern.open).map_err(|e| format!("[lexical.unsupported] {e}"))?;
    }
    Ok(())
}

/// Extract the actual test structure using the pack's lexical profile.
pub(crate) fn extract(pack: &Pack, cfg: &Lexical, source: &str) -> Result<Extraction> {
    let err = |message: String| Error::Lexical {
        pack: pack.name().to_string(),
        message,
    };
    let located = |pos: usize, what: String| format!("{what} (line {})", line_of(source, pos));

    // Profiles are validated at pack load; these stay as defensive errors.
    let pairs = bracket_pairs(cfg).map_err(err)?;
    let states = mask(source.as_bytes(), cfg).map_err(|(pos, what)| err(located(pos, what)))?;
    let brackets = match_brackets(source.as_bytes(), &states, &pairs)
        .map_err(|(pos, what)| err(located(pos, what)))?;
    let haystack = blank_comments(source, &states);

    let mut captures: Vec<Capture> = Vec::new();
    for (kind, pattern) in [
        (ActualKind::Block, cfg.block.open.as_str()),
        (ActualKind::Test, cfg.test.open.as_str()),
    ] {
        collect(
            kind, pattern, pack, source, &haystack, &states, &brackets, &pairs,
        )
        .map(|found| captures.extend(found))
        .map_err(err)?;
    }

    captures.sort_by_key(|c| (c.start, std::cmp::Reverse(c.end)));
    captures.dedup_by_key(|c| (c.start, c.end, c.kind));
    let mut i = 0;
    let top = build_nodes(&captures, &mut i, usize::MAX);
    let unsupported = cfg
        .unsupported
        .as_ref()
        .map_or_else(
            || Ok(Vec::new()),
            |pattern| collect_unsupported(&pattern.open, source, &haystack, &states),
        )
        .map_err(err)?;
    Ok(Extraction {
        nodes: ActualNode::prune_empty_blocks(top),
        unsupported,
    })
}

/// Collect line-only findings whose match contains real, non-trivia code.
fn collect_unsupported(
    pattern: &str,
    source: &str,
    haystack: &str,
    states: &[State],
) -> std::result::Result<Vec<Unsupported>, String> {
    let re = compiled(pattern)?;
    let mut out = Vec::new();
    for found in re.find_iter(haystack) {
        let code = (found.start()..found.end()).find(|&at| {
            states.get(at) == Some(&State::Code) && !source.as_bytes()[at].is_ascii_whitespace()
        });
        if let Some(code) = code {
            out.push(Unsupported {
                line: line_of(source, code),
            });
        }
    }
    out.sort_by_key(|finding| finding.line);
    Ok(out)
}

/// The profile's bracket pairs as single bytes (multi-byte brackets are
/// not supported).
fn bracket_pairs(cfg: &Lexical) -> std::result::Result<Vec<(u8, u8)>, String> {
    cfg.nest
        .iter()
        .map(|(open, close)| match (open.as_bytes(), close.as_bytes()) {
            (&[o], &[c]) if o != c => Ok((o, c)),
            _ => Err(format!(
                "nest pair [{open:?}, {close:?}] must be two distinct single characters"
            )),
        })
        .collect()
}

/// Classify every byte as code, comment, or string content. Comments and
/// strings are matched greedily left-to-right; string escapes consume the
/// following byte so an escaped delimiter does not close the literal.
///
/// Fails closed: an unterminated string or block comment means the
/// profile does not understand this file, and masking through EOF would
/// let extraction silently drop everything after the error.
fn mask(source: &[u8], cfg: &Lexical) -> std::result::Result<Vec<State>, (usize, String)> {
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
            if i >= source.len() {
                return Err((start, "unterminated block comment".to_string()));
            }
            i += close.len();
            states[start..i].fill(State::Comment);
            continue;
        }
        for rule in &cfg.strings {
            if source[i..].starts_with(rule.delim.as_bytes()) {
                let start = i;
                i += rule.delim.len();
                let mut closed = false;
                while i < source.len() {
                    if let Some(esc) = &rule.escape
                        && source[i..].starts_with(esc.as_bytes())
                    {
                        i += esc.len() + 1;
                        continue;
                    }
                    if source[i..].starts_with(rule.delim.as_bytes()) {
                        i += rule.delim.len();
                        closed = true;
                        break;
                    }
                    i += 1;
                }
                if !closed {
                    return Err((start, "unterminated string".to_string()));
                }
                states[start..i.min(source.len())].fill(State::Str);
                continue 'outer;
            }
        }
        i += 1;
    }
    Ok(states)
}

/// A copy of the source with comment bytes blanked to spaces, byte
/// offsets unchanged. Opener regexes match against this, so comment
/// trivia between tokens behaves as the whitespace the language says it
/// is, while the state mask still knows what was really a comment.
fn blank_comments(source: &str, states: &[State]) -> String {
    let bytes: Vec<u8> = source
        .bytes()
        .zip(states)
        .map(|(b, s)| if *s == State::Comment { b' ' } else { b })
        .collect();
    // Comment regions start and end at (single-byte-aligned) delimiter
    // boundaries, so whole characters are always blanked together.
    String::from_utf8(bytes).expect("blanking whole comment regions preserves UTF-8")
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

/// Run one opener pattern over the comment-blanked source, keeping
/// matches whose keyword is real code and whose title is in the state the
/// pack's `name_syntax` promises. A rejected match (a decoy inside a
/// comment or string) resumes the search one byte past its start so it
/// cannot shadow a real opener it overlapped.
#[expect(clippy::too_many_arguments, reason = "internal plumbing of one scan")]
fn collect(
    kind: ActualKind,
    pattern: &str,
    pack: &Pack,
    source: &str,
    haystack: &str,
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
    let name_state = match syntax {
        NameSyntax::JsString => State::Str,
        NameSyntax::Raw => State::Code,
    };
    let bytes = source.as_bytes();

    let mut out = Vec::new();
    let mut locs = re.capture_locations();
    let mut at = 0;
    while at <= haystack.len() {
        let Some(m) = re.captures_read_at(&mut locs, haystack, at) else {
            break;
        };
        let Some((kw_start, _)) = locs.get(kw_i) else {
            return Err("(?<kw>...) did not participate in a match".to_string());
        };
        let Some((name_start, name_end)) = locs.get(name_i) else {
            return Err("(?<name>...) did not participate in a match".to_string());
        };
        if states.get(kw_start) != Some(&State::Code) || states.get(name_start) != Some(&name_state)
        {
            at = m.start() + 1;
            continue;
        }
        // The span runs to the matching closer of the first bracket the
        // match itself contains: the call's parens for `it("x", ...)`,
        // the body brace for `mod foo {` — so the pattern must include
        // the definition's opening bracket.
        let open_pos = (kw_start..m.end().min(states.len()))
            .find(|&i| states[i] == State::Code && pairs.iter().any(|(open, _)| *open == bytes[i]))
            .ok_or_else(|| {
                format!(
                    "opener at line {} contains no span bracket (the pattern must include the definition's opening bracket)",
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
