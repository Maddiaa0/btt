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
use crate::extract::ActualKind;
use crate::pack::Pack;
use serde::Serialize;

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
