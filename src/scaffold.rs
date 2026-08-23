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
use crate::extract::ActualKind;
use crate::pack::Pack;
use anyhow::{Context, Result};
use serde::Serialize;

#[derive(Debug, Serialize)]
struct Event {
    kind: &'static str,
    name: String,
    text: String,
    depth: usize,
    indent: String,
}

fn flatten(nodes: &[Expected], depth: usize, unit: &str, out: &mut Vec<Event>) {
    for node in nodes {
        let indent = unit.repeat(depth);
        match node.kind {
            ActualKind::Test => out.push(Event {
                kind: "test",
                name: node.name.clone(),
                text: node.text.clone(),
                depth,
                indent,
            }),
            ActualKind::Block => {
                out.push(Event {
                    kind: "open",
                    name: node.name.clone(),
                    text: node.text.clone(),
                    depth,
                    indent: indent.clone(),
                });
                flatten(&node.children, depth + 1, unit, out);
                out.push(Event {
                    kind: "close",
                    name: node.name.clone(),
                    text: node.text.clone(),
                    depth,
                    indent,
                });
            }
        }
    }
}

/// Render the pack's scaffold template for the given expected structure.
pub fn render(pack: &Pack, expected: &[Expected], stem: &str) -> Result<String> {
    let mut events = Vec::new();
    flatten(expected, 0, &pack.manifest.scaffold.indent, &mut events);

    let mut env = minijinja::Environment::new();
    env.add_template("scaffold", &pack.template)
        .with_context(|| format!("pack `{}`: invalid scaffold template", pack.name()))?;
    let tmpl = env.get_template("scaffold").unwrap();
    let out = tmpl
        .render(minijinja::context! { events => events, stem => stem })
        .with_context(|| format!("pack `{}`: rendering scaffold", pack.name()))?;
    // Normalize trailing whitespace: exactly one final newline.
    Ok(format!("{}\n", out.trim_end()))
}
