//! Extract the actual test structure from a source file using a pack's
//! tree-sitter query.
//!
//! The query uses a small capture vocabulary the core understands:
//!
//! - `@block` / `@block.name` — a nesting construct (a Rust `mod`, a vitest
//!   `describe(...)`) and the node holding its name.
//! - `@test` / `@test.name` — a test definition and the node holding its name.
//! - `@test.marker` — optional; a node (e.g. a `#[test]` attribute) that must
//!   directly precede a `@test` among its siblings for it to count, when the
//!   pack sets `extract.test_requires_marker = true`.
//!
//! Nesting is derived structurally: a captured node's parent is the smallest
//! captured `@block` that contains it. This works for both block-based
//! languages (describe callbacks) and item-based ones (mods) without any
//! language-specific logic in the core.

use crate::pack::{self, Pack};
use anyhow::{bail, Context, Result};
use std::path::Path;
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActualKind {
    Block,
    Test,
}

#[derive(Debug, Clone)]
pub struct ActualNode {
    pub kind: ActualKind,
    /// The identifier / title as written in the source.
    pub name: String,
    /// 1-based line of the definition.
    pub line: usize,
    pub children: Vec<ActualNode>,
}

impl ActualNode {
    fn from_raw(raw: Raw, all: &[Raw]) -> ActualNode {
        let children = all
            .iter()
            .filter(|c| c.parent_idx == Some(raw.idx))
            .cloned()
            .map(|c| ActualNode::from_raw(c, all))
            .collect();
        ActualNode { kind: raw.kind, name: raw.name, line: raw.line, children }
    }

    /// Drop blocks that contain no tests anywhere below them (helper mods,
    /// non-test describes) so they don't show up as noise in diffs.
    pub fn prune_empty_blocks(nodes: Vec<ActualNode>) -> Vec<ActualNode> {
        nodes
            .into_iter()
            .filter_map(|mut n| match n.kind {
                ActualKind::Test => Some(n),
                ActualKind::Block => {
                    n.children = Self::prune_empty_blocks(n.children);
                    if n.children.is_empty() {
                        None
                    } else {
                        Some(n)
                    }
                }
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
struct Raw {
    idx: usize,
    kind: ActualKind,
    name: String,
    line: usize,
    start: usize,
    end: usize,
    parent_idx: Option<usize>,
}

/// Parse `source` with the pack's grammar and return the top-level actual
/// nodes (already pruned of test-free blocks).
pub fn extract(pack: &Pack, target: &Path, source: &str) -> Result<Vec<ActualNode>> {
    let language = pack::language_for(pack, target)?;
    let mut parser = Parser::new();
    parser.set_language(&language).context("setting grammar")?;
    let tree = parser
        .parse(source, None)
        .with_context(|| format!("parsing {}", target.display()))?;

    let query = Query::new(&language, &pack.query)
        .with_context(|| format!("compiling query for pack `{}`", pack.name()))?;

    let idx_of = |cap: &str| query.capture_index_for_name(cap);
    let (Some(block_i), Some(block_name_i), Some(test_i), Some(test_name_i)) = (
        idx_of("block"),
        idx_of("block.name"),
        idx_of("test"),
        idx_of("test.name"),
    ) else {
        bail!(
            "pack `{}`: query must define @block, @block.name, @test, @test.name",
            pack.name()
        );
    };
    let marker_i = idx_of("test.marker");
    if pack.manifest.extract.test_requires_marker && marker_i.is_none() {
        bail!(
            "pack `{}`: test_requires_marker is set but the query has no @test.marker",
            pack.name()
        );
    }

    let mut raws: Vec<Raw> = Vec::new();
    let mut marker_ends: Vec<(usize, usize)> = Vec::new(); // (start, end) byte ranges

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
    while let Some(m) = matches.next() {
        let node_for = |i: u32| m.captures.iter().find(|c| c.index == i).map(|c| c.node);
        if let Some(n) = node_for(block_i)
            && let Some(name_node) = node_for(block_name_i) {
                push_raw(&mut raws, ActualKind::Block, n, name_node, source);
            }
        if let Some(n) = node_for(test_i)
            && let Some(name_node) = node_for(test_name_i) {
                push_raw(&mut raws, ActualKind::Test, n, name_node, source);
            }
        if let Some(i) = marker_i
            && let Some(n) = node_for(i) {
                marker_ends.push((n.start_byte(), n.end_byte()));
            }
    }

    // Deduplicate (a node can appear in multiple query matches).
    raws.sort_by_key(|r| (r.start, std::cmp::Reverse(r.end)));
    raws.dedup_by_key(|r| (r.start, r.end, r.kind.clone()));

    if pack.manifest.extract.test_requires_marker {
        raws.retain(|r| {
            r.kind != ActualKind::Test || has_marker(&tree.root_node(), r, &marker_ends)
        });
    }

    // idx values must match final positions — parent_idx links rely on it.
    for (i, r) in raws.iter_mut().enumerate() {
        r.idx = i;
    }

    // Containment nesting: parent = innermost captured block strictly
    // containing the node. raws is sorted by (start asc, end desc), so a
    // simple stack walk assigns parents in one pass.
    let mut stack: Vec<usize> = Vec::new();
    for i in 0..raws.len() {
        while let Some(&top) = stack.last() {
            let t = &raws[top];
            let contained = raws[i].start >= t.start && raws[i].end <= t.end;
            if contained {
                break;
            }
            stack.pop();
        }
        raws[i].parent_idx = stack.last().copied();
        if raws[i].kind == ActualKind::Block {
            stack.push(i);
        }
    }

    let top: Vec<ActualNode> = raws
        .iter()
        .filter(|r| r.parent_idx.is_none())
        .cloned()
        .map(|r| ActualNode::from_raw(r, &raws))
        .collect();
    Ok(ActualNode::prune_empty_blocks(top))
}

fn push_raw(raws: &mut Vec<Raw>, kind: ActualKind, node: Node, name_node: Node, source: &str) {
    let name = source[name_node.byte_range()].to_string();
    raws.push(Raw {
        idx: 0,
        kind,
        name,
        line: node.start_position().row + 1,
        start: node.start_byte(),
        end: node.end_byte(),
        parent_idx: None,
    });
}

/// A test has a marker if, walking backwards through its previous named
/// siblings, we hit a marker range before hitting any non-marker,
/// non-comment, non-attribute node.
fn has_marker(root: &Node, test: &Raw, marker_ends: &[(usize, usize)]) -> bool {
    let Some(mut node) = root.descendant_for_byte_range(test.start, test.end) else {
        return false;
    };
    // descendant_for_byte_range returns the smallest spanning node; walk up
    // to the outermost node with this exact range (the captured definition),
    // whose siblings are where markers live.
    while let Some(p) = node.parent() {
        if p.start_byte() == test.start && p.end_byte() == test.end {
            node = p;
        } else {
            break;
        }
    }
    let mut prev = node.prev_named_sibling();
    while let Some(p) = prev {
        if marker_ends.contains(&(p.start_byte(), p.end_byte())) {
            return true;
        }
        let kind = p.kind();
        if !kind.contains("comment") && !kind.contains("attribute") {
            return false;
        }
        prev = p.prev_named_sibling();
    }
    false
}
