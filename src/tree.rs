//! Parser for bulloak-style `.tree` specification files.
//!
//! A tree file contains one or more trees. Each tree starts with a root line
//! (the name of the thing under test), followed by branch lines drawn with
//! box characters:
//!
//! ```text
//! HashMap
//! ├── when the key is present
//! │   ├── it returns the value
//! │   └── when the value was overwritten
//! │       └── it returns the latest value
//! └── when the key is absent
//!     └── it returns none
//! ```
//!
//! Nodes starting with `when` or `given` (case-insensitive) are conditions
//! (branches); nodes starting with `it` are actions (leaves). Multiple trees
//! in one file are separated by blank lines.

/// Why a `.tree` file failed to parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ParseError {
    /// A branch line appeared before any root line.
    #[error("line {line}: branch node before any root line")]
    BranchBeforeRoot {
        /// 1-based line number.
        line: usize,
    },

    /// An `it` node was given children.
    #[error("line {line}: `it` nodes cannot have children (parent at line {parent_line})")]
    ActionWithChildren {
        /// 1-based line number of the child.
        line: usize,
        /// 1-based line number of the offending `it` parent.
        parent_line: usize,
    },

    /// A node is indented more than one level past its parent.
    #[error("line {line}: indentation jumps from depth {from} to {to}")]
    DepthJump {
        /// 1-based line number.
        line: usize,
        /// Depth of the deepest open node.
        from: usize,
        /// Depth the line claimed.
        to: usize,
    },

    /// A node does not start with `when`, `given`, or `it`.
    #[error("line {line}: node must start with `when`, `given`, or `it`: {text:?}")]
    UnknownKeyword {
        /// 1-based line number.
        line: usize,
        /// The offending node text.
        text: String,
    },

    /// A line's box-drawing prefix is not well formed.
    #[error("line {line}: malformed tree prefix: {text:?}")]
    MalformedPrefix {
        /// 1-based line number.
        line: usize,
        /// The full offending line.
        text: String,
    },

    /// The file contains no trees at all.
    #[error("no trees found in file")]
    Empty,
}

/// Whether a spec node is a branch or a leaf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// A `when …` / `given …` branch.
    Condition,
    /// An `it …` leaf describing one assertion.
    Action,
}

/// One node of a parsed spec tree.
#[derive(Debug, Clone)]
pub struct SpecNode {
    /// Branch or leaf.
    pub kind: NodeKind,
    /// Full node text, e.g. "when the key is present" or "it returns none".
    pub text: String,
    /// 1-based line number in the `.tree` file.
    pub line: usize,
    /// Child nodes (always empty for actions).
    pub children: Vec<SpecNode>,
}

/// One tree from a `.tree` file.
#[derive(Debug, Clone)]
pub struct SpecTree {
    /// Root line text, e.g. `HashMap`.
    pub root: String,
    /// 1-based line number of the root line.
    pub line: usize,
    /// Top-level nodes under the root.
    pub children: Vec<SpecNode>,
}

/// Parse the contents of a `.tree` file into one or more spec trees.
///
/// # Errors
///
/// Returns a [`ParseError`] describing the offending line if the file is
/// malformed, or [`ParseError::Empty`] if it contains no trees.
pub fn parse(source: &str) -> Result<Vec<SpecTree>, ParseError> {
    let mut trees: Vec<SpecTree> = Vec::new();
    // Stack of (depth, node) for the tree currently being built.
    let mut stack: Vec<(usize, SpecNode)> = Vec::new();

    for (idx, raw) in source.lines().enumerate() {
        let line = idx + 1;
        let text = raw.trim_end();
        if text.trim().is_empty() {
            flush_tree(&mut trees, &mut stack);
            continue;
        }
        if text.trim_start().starts_with("//") {
            continue;
        }

        let (depth, node_text) = parse_line(text, line)?;

        if depth == 0 {
            flush_tree(&mut trees, &mut stack);
            trees.push(SpecTree {
                root: node_text,
                line,
                children: Vec::new(),
            });
            continue;
        }

        if trees.is_empty() {
            return Err(ParseError::BranchBeforeRoot { line });
        }
        let kind = classify(&node_text, line)?;
        let node = SpecNode {
            kind,
            text: node_text,
            line,
            children: Vec::new(),
        };

        // Pop the stack down to this node's parent depth.
        while stack.last().is_some_and(|(d, _)| *d >= depth) {
            if let Some((_, done)) = stack.pop() {
                attach(&mut trees, &mut stack, done);
            }
        }
        match stack.last() {
            Some((d, parent)) => {
                if depth != d + 1 {
                    return Err(ParseError::DepthJump {
                        line,
                        from: *d,
                        to: depth,
                    });
                }
                if parent.kind == NodeKind::Action {
                    return Err(ParseError::ActionWithChildren {
                        line,
                        parent_line: parent.line,
                    });
                }
            }
            None => {
                if depth != 1 {
                    return Err(ParseError::DepthJump {
                        line,
                        from: 0,
                        to: depth,
                    });
                }
            }
        }
        stack.push((depth, node));
    }
    flush_tree(&mut trees, &mut stack);

    if trees.is_empty() {
        return Err(ParseError::Empty);
    }
    Ok(trees)
}

fn flush_tree(trees: &mut [SpecTree], stack: &mut Vec<(usize, SpecNode)>) {
    while let Some((_, node)) = stack.pop() {
        attach(trees, stack, node);
    }
}

fn attach(trees: &mut [SpecTree], stack: &mut [(usize, SpecNode)], node: SpecNode) {
    match stack.last_mut() {
        Some((_, parent)) => parent.children.push(node),
        None => trees
            .last_mut()
            .expect("attach called with no current tree")
            .children
            .push(node),
    }
}

fn classify(text: &str, line: usize) -> Result<NodeKind, ParseError> {
    let lower = text.to_lowercase();
    if lower.starts_with("when ") || lower.starts_with("given ") {
        Ok(NodeKind::Condition)
    } else if lower == "it" || lower.starts_with("it ") {
        Ok(NodeKind::Action)
    } else {
        Err(ParseError::UnknownKeyword {
            line,
            text: text.to_string(),
        })
    }
}

/// Split a line into (depth, node text). Depth 0 is a root line.
///
/// Each nesting level is a 4-column prefix group: `│   `, `    `, `├── `, or
/// `└── `. The final group before the text must be a connector (`├──`/`└──`).
fn parse_line(line: &str, lineno: usize) -> Result<(usize, String), ParseError> {
    let mut rest = line;
    let mut depth = 0usize;
    loop {
        if let Some(r) = strip_group(rest, "├──").or_else(|| strip_group(rest, "└──")) {
            return Ok((depth + 1, r.trim().to_string()));
        }
        if let Some(r) = strip_group(rest, "│").or_else(|| rest.strip_prefix("    ")) {
            depth += 1;
            rest = r;
            continue;
        }
        if depth == 0 {
            return Ok((0, rest.trim().to_string()));
        }
        return Err(ParseError::MalformedPrefix {
            line: lineno,
            text: line.to_string(),
        });
    }
}

/// Strip a prefix marker plus enough spaces to fill its 4-column group.
fn strip_group<'a>(s: &'a str, marker: &str) -> Option<&'a str> {
    let rest = s.strip_prefix(marker)?;
    let pad = 4usize.saturating_sub(marker.chars().count());
    // Allow the group to be cut short at end-of-line, otherwise require padding.
    if rest.is_empty() {
        return Some(rest);
    }
    let mut r = rest;
    for _ in 0..pad {
        r = r.strip_prefix(' ')?;
    }
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASIC: &str = "\
HashMap
├── when the key is present
│   ├── it returns the value
│   └── when the value was overwritten
│       └── it returns the latest value
└── when the key is absent
    └── it returns none
";

    mod when_parsing_a_basic_tree {
        use super::*;

        #[test]
        fn resolves_the_root_name() {
            let trees = parse(BASIC).unwrap();
            assert_eq!(trees.len(), 1);
            assert_eq!(trees[0].root, "HashMap");
        }

        #[test]
        fn nests_children_by_indentation() {
            let trees = parse(BASIC).unwrap();
            let root = &trees[0];
            assert_eq!(root.children.len(), 2);
            let present = &root.children[0];
            assert_eq!(present.kind, NodeKind::Condition);
            assert_eq!(present.children.len(), 2);
            assert_eq!(
                present.children[1].children[0].text,
                "it returns the latest value"
            );
        }

        #[test]
        fn records_line_numbers() {
            let trees = parse(BASIC).unwrap();
            assert_eq!(trees[0].children[1].line, 6);
        }
    }

    mod when_the_file_has_multiple_trees {
        use super::*;

        #[test]
        fn splits_them_on_blank_lines() {
            let src = "A\n└── it works\n\nB\n└── it also works\n";
            let trees = parse(src).unwrap();
            assert_eq!(trees.len(), 2);
            assert_eq!(trees[1].root, "B");
            assert_eq!(trees[1].children[0].text, "it also works");
        }
    }

    mod when_the_tree_is_malformed {
        use super::*;

        #[test]
        fn rejects_children_under_it_nodes() {
            let src = "A\n└── it works\n    └── it nested\n";
            assert!(matches!(
                parse(src),
                Err(ParseError::ActionWithChildren {
                    line: 3,
                    parent_line: 2
                })
            ));
        }

        #[test]
        fn rejects_unknown_node_keywords() {
            let src = "A\n└── sometimes it works\n";
            assert!(matches!(
                parse(src),
                Err(ParseError::UnknownKeyword { line: 2, .. })
            ));
        }

        #[test]
        fn rejects_depth_jumps() {
            let src = "A\n└── when x\n│       └── it works\n";
            assert!(matches!(parse(src), Err(ParseError::DepthJump { .. })));
        }

        #[test]
        fn rejects_empty_files() {
            assert!(matches!(parse("\n\n"), Err(ParseError::Empty)));
        }
    }
}
