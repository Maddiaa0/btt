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
/// # Errors
///
/// Fails if the pack's template does not compile or render.
pub fn render(pack: &Pack, expected: &[Expected], stem: &str) -> Result<String> {
    let mut events = Vec::new();
    flatten(expected, 0, &pack.manifest.scaffold.indent, &mut events);

    let env = minijinja::Environment::new();
    let out = env
        .render_str(
            &pack.template,
            minijinja::context! { events => events, stem => stem },
        )
        .map_err(|source| Error::Template {
            pack: pack.name().to_string(),
            source: Box::new(source),
        })?;
    // Normalize trailing whitespace: exactly one final newline.
    Ok(format!("{}\n", out.trim_end()))
}
