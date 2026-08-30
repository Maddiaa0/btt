//! Render a test-file skeleton from a spec tree using a pack's template.
//!
//! The expected structure is flattened into a linear event stream so
//! templates stay simple loops instead of recursive macros:
//!
//! - `open`  — start of a block (`mod x {`, `describe("x", () => {`)
//! - `test`  — a test definition
//! - `close` — end of a block
//!
//! Each event carries `name` (mapped identifier), `text` (original spec
//! text), `depth`, and `indent` (the pack's indent unit repeated `depth`
//! times).

use crate::check::Expected;
use crate::error::{Error, Result};
use crate::extract::{self, ActualKind, SourceNode};
use crate::pack::Pack;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
enum EventKind {
    Open,
    Close,
    Test,
}

#[derive(Debug, Serialize)]
struct Event {
    kind: EventKind,
    name: String,
    text: String,
    depth: usize,
    indent: String,
}

fn flatten(nodes: &[Expected], depth: usize, unit: &str, out: &mut Vec<Event>) {
    for node in nodes {
        let event = |kind: EventKind| Event {
            kind,
            name: node.name.clone(),
            text: node.text.clone(),
            depth,
            indent: unit.repeat(depth),
        };
        match node.kind {
            ActualKind::Test => out.push(event(EventKind::Test)),
            ActualKind::Block => {
                out.push(event(EventKind::Open));
                flatten(&node.children, depth + 1, unit, out);
                out.push(event(EventKind::Close));
            }
        }
    }
}

/// Render the pack's scaffold template for the given expected structure.
///
/// `in_tests_dir` tells the template whether the output lands in a
/// dedicated test directory (a Rust `tests/` integration crate) or next
/// to source — languages whose test-file shape differs by location (Rust
/// wraps colocated tests in `#[cfg(test)] mod tests`) branch on it;
/// other templates ignore it.
///
/// # Errors
///
/// Fails if the pack's template does not compile or render.
pub fn render(
    pack: &Pack,
    expected: &[Expected],
    stem: &str,
    in_tests_dir: bool,
) -> Result<String> {
    let mut events = Vec::new();
    flatten(expected, 0, &pack.manifest.scaffold.indent, &mut events);

    let mut env = minijinja::Environment::new();
    // Escaping filters for interpolating spec text into code. Titles and
    // descriptions are arbitrary text; without these, a quote in a title
    // produces a scaffold that does not compile.
    env.add_filter("js_string", |s: String| {
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            // U+2028/U+2029 are line terminators in JavaScript; escaped,
            // they stay string data on every parser.
            .replace('\u{2028}', "\\u2028")
            .replace('\u{2029}', "\\u2029")
    });
    // JS line comments end at *any* line terminator: spec text quoted in a
    // comment must not be able to end the comment and become code.
    env.add_filter("line_safe", |s: String| {
        s.replace(['\r', '\n'], " ")
            .replace('\u{2028}', "\\u2028")
            .replace('\u{2029}', "\\u2029")
    });
    // Rust string literals that are also format strings (todo!) need brace
    // doubling on top of quote/backslash escaping.
    env.add_filter("rust_string", |s: String| {
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('{', "{{")
            .replace('}', "}}")
    });
    let out = env
        .render_str(
            &pack.template,
            minijinja::context! { events => events, stem => stem, in_tests_dir => in_tests_dir },
        )
        .map_err(|source| Error::Template {
            pack: pack.name().to_string(),
            source: Box::new(source),
        })?;
    // Normalize trailing whitespace: exactly one final newline.
    Ok(format!("{}\n", out.trim_end()))
}

/// Add skeletons for expected nodes absent from an existing test file.
/// Existing bytes are only copied; every edit lands between extracted spans.
///
/// # Errors
///
/// Fails closed when extraction fails or a unique, line-aligned insertion
/// point cannot be proven.
pub fn merge(
    pack: &Pack,
    expected: &[Expected],
    target: &Path,
    source: &str,
    stem: &str,
    in_tests_dir: bool,
) -> Result<String> {
    let extraction = extract::extract_with_findings(pack, target, source)?;
    if extraction.has_parse_errors {
        return Err(Error::Merge {
            message: format!("{} contains syntax errors", target.display()),
        });
    }
    if !extraction.unsupported.is_empty() {
        return Err(Error::Merge {
            message: "the file contains unsupported constructs with unknown insertion spans".into(),
        });
    }
    let (actual, root) = merge_root(&extraction.source_nodes, &pack.manifest.mapping.wrappers)?;
    let indent_unit = observed_indent_unit(source, &extraction.source_nodes);
    let mut edits = BTreeMap::<usize, String>::new();
    plan_children(
        pack,
        expected,
        actual,
        root,
        source,
        stem,
        in_tests_dir,
        indent_unit.as_deref(),
        &mut edits,
    )?;
    let mut out =
        String::with_capacity(source.len() + edits.values().map(String::len).sum::<usize>());
    let mut at = 0;
    for (pos, insertion) in edits {
        out.push_str(&source[at..pos]);
        out.push_str(&insertion);
        at = pos;
    }
    out.push_str(&source[at..]);
    Ok(out)
}

#[derive(Clone, Copy)]
struct ParentSpan {
    start: usize,
    end: usize,
}

fn merge_root<'a>(
    nodes: &'a [SourceNode],
    wrappers: &[String],
) -> Result<(&'a [SourceNode], Option<ParentSpan>)> {
    let wrapper_nodes: Vec<_> = nodes
        .iter()
        .filter(|node| node.kind == ActualKind::Block && wrappers.contains(&node.name))
        .collect();
    if wrapper_nodes.len() > 1 {
        return Err(Error::Merge {
            message: "multiple transparent wrapper blocks make the insertion root ambiguous".into(),
        });
    }
    Ok(wrapper_nodes.first().map_or((nodes, None), |node| {
        (
            node.children.as_slice(),
            Some(ParentSpan {
                start: node.start,
                end: node.end,
            }),
        )
    }))
}

#[expect(clippy::too_many_arguments, reason = "recursive merge planner context")]
fn plan_children(
    pack: &Pack,
    expected: &[Expected],
    actual: &[SourceNode],
    parent: Option<ParentSpan>,
    source: &str,
    stem: &str,
    in_tests_dir: bool,
    indent_unit: Option<&str>,
    edits: &mut BTreeMap<usize, String>,
) -> Result<()> {
    let mut previous = None;
    let mut last_index = None;
    for wanted in expected {
        let matches: Vec<_> = actual
            .iter()
            .enumerate()
            .filter(|(_, node)| node.kind == wanted.kind && node.name == wanted.name)
            .collect();
        if matches.len() > 1 {
            return Err(Error::Merge {
                message: format!(
                    "duplicate `{}` definitions make its insertion position ambiguous",
                    wanted.name
                ),
            });
        }
        if let Some((index, found)) = matches.first().copied() {
            if last_index.is_some_and(|last| index <= last) {
                return Err(Error::Merge {
                    message: format!(
                        "existing siblings around `{}` are out of tree order",
                        wanted.name
                    ),
                });
            }
            last_index = Some(index);
            previous = Some(found);
            if wanted.kind == ActualKind::Block {
                plan_children(
                    pack,
                    &wanted.children,
                    &found.children,
                    Some(ParentSpan {
                        start: found.start,
                        end: found.end,
                    }),
                    source,
                    stem,
                    in_tests_dir,
                    indent_unit,
                    edits,
                )?;
            }
        } else {
            let (pos, indent) = insertion_point(previous, actual, parent, source, indent_unit)?;
            let snippet = render_node(pack, wanted, target_indent(&indent), stem, in_tests_dir)?;
            edits.entry(pos).or_default().push_str(&snippet);
        }
    }
    Ok(())
}

fn insertion_point(
    previous: Option<&SourceNode>,
    siblings: &[SourceNode],
    parent: Option<ParentSpan>,
    source: &str,
    indent_unit: Option<&str>,
) -> Result<(usize, String)> {
    if let Some(node) = previous {
        let pos = line_end(source, node.end)?;
        return Ok((pos, line_indent(source, node.start).to_string()));
    }
    if let Some(first) = siblings.first() {
        let pos = line_start(source, first.start);
        return Ok((pos, line_indent(source, first.start).to_string()));
    }
    if let Some(parent) = parent {
        let close = closing_line(source, parent)?;
        let mut indent = line_indent(source, close).to_string();
        ensure_parent_indent(source, parent.start, close)?;
        indent.push_str(indent_unit.ok_or_else(|| Error::Merge {
            message: "an empty block has no surrounding indentation sample".into(),
        })?);
        return Ok((close, indent));
    }
    let pos = source.len();
    if !source.is_empty() && !source.ends_with('\n') {
        return Err(Error::Merge {
            message: "top-level append is not on a fresh line".into(),
        });
    }
    Ok((pos, String::new()))
}

fn render_node(
    pack: &Pack,
    node: &Expected,
    indent: &str,
    stem: &str,
    in_tests_dir: bool,
) -> Result<String> {
    let rendered = render(pack, std::slice::from_ref(node), stem, in_tests_dir)?;
    let extraction = extract::extract_with_findings(
        pack,
        Path::new(&pack.manifest.scaffold.output.replace("{stem}", stem)),
        &rendered,
    )?;
    let (nodes, _) = merge_root(&extraction.source_nodes, &pack.manifest.mapping.wrappers)?;
    let found = nodes
        .iter()
        .find(|candidate| candidate.kind == node.kind && candidate.name == node.name)
        .ok_or_else(|| Error::Merge {
            message: format!("the scaffold template did not produce `{}`", node.name),
        })?;
    let mut start = line_start(&rendered, found.start);
    if node.kind == ActualKind::Test && pack.manifest.extract.test_requires_marker && start > 0 {
        start = line_start(&rendered, start - 1);
    }
    let end = line_end(&rendered, found.end)?;
    let raw = &rendered[start..end];
    let rendered_indent = line_indent(&rendered, found.start);
    let mut out = String::new();
    for line in raw.split_inclusive('\n') {
        if line.trim().is_empty() {
            out.push_str(line);
        } else {
            out.push_str(indent);
            out.push_str(line.strip_prefix(rendered_indent).unwrap_or(line));
        }
    }
    Ok(out)
}

fn line_start(source: &str, pos: usize) -> usize {
    source[..pos].rfind('\n').map_or(0, |at| at + 1)
}

fn line_end(source: &str, pos: usize) -> Result<usize> {
    let rest = &source[pos..];
    let newline = rest.find('\n').map_or(source.len(), |at| pos + at + 1);
    let tail = &source[pos..newline];
    if !tail
        .trim_matches(|c: char| c.is_whitespace() || c == ';')
        .is_empty()
    {
        return Err(Error::Merge {
            message: "a definition does not end at a clean line boundary".into(),
        });
    }
    Ok(newline)
}

fn line_indent(source: &str, pos: usize) -> &str {
    let start = line_start(source, pos);
    let prefix = &source[start..pos];
    let end = prefix
        .find(|c: char| !c.is_whitespace())
        .unwrap_or(prefix.len());
    &prefix[..end]
}

fn closing_line(source: &str, parent: ParentSpan) -> Result<usize> {
    let last = source[parent.start..parent.end].trim_end().len() + parent.start;
    let close = line_start(source, last.saturating_sub(1));
    let closing = source[close..last].trim_start();
    if close <= parent.start || !(closing.starts_with('}') || closing.starts_with(')')) {
        return Err(Error::Merge {
            message: "an empty parent block has no clean closing line".into(),
        });
    }
    Ok(close)
}

fn ensure_parent_indent(source: &str, parent_start: usize, close: usize) -> Result<()> {
    let parent_indent = line_indent(source, parent_start);
    let close_indent = line_indent(source, close);
    if parent_indent != close_indent {
        return Err(Error::Merge {
            message: "parent delimiters use inconsistent indentation".into(),
        });
    }
    // No child exists to sample, so the pack unit is the only safe increment;
    // the parent indentation itself is still derived from this file.
    Ok(())
}

fn observed_indent_unit(source: &str, nodes: &[SourceNode]) -> Option<String> {
    for parent in nodes.iter().filter(|node| node.kind == ActualKind::Block) {
        let parent_indent = line_indent(source, parent.start);
        if let Some(child) = parent.children.first() {
            let child_indent = line_indent(source, child.start);
            if let Some(unit) = child_indent
                .strip_prefix(parent_indent)
                .filter(|unit| !unit.is_empty())
            {
                return Some(unit.to_string());
            }
        }
        if let Some(unit) = observed_indent_unit(source, &parent.children) {
            return Some(unit);
        }
    }
    None
}

fn target_indent(indent: &str) -> &str {
    indent
}
