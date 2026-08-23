//! Compare a spec tree against the actual structure extracted from a test
//! file, producing findings.

use crate::extract::{ActualKind, ActualNode};
use crate::mapping::{Mapping, RootMapping};
use crate::tree::{NodeKind, SpecNode, SpecTree};

/// A node of the *expected* structure: the spec tree with mapping rules
/// applied, i.e. what the test file should contain.
#[derive(Debug, Clone)]
pub struct Expected {
    pub kind: ActualKind,
    /// Expected identifier / title in the source file.
    pub name: String,
    /// Original spec text (used by scaffold templates and messages).
    pub text: String,
    pub spec_line: usize,
    pub children: Vec<Expected>,
}

/// Build the expected structure for all trees in a file.
pub fn expected_from_spec(trees: &[SpecTree], mapping: &Mapping) -> Vec<Expected> {
    let mut out = Vec::new();
    for tree in trees {
        let children: Vec<Expected> =
            tree.children.iter().map(|n| expected_node(n, mapping)).collect();
        match mapping.root {
            RootMapping::Block => out.push(Expected {
                kind: ActualKind::Block,
                name: tree.root.trim().to_string(),
                text: tree.root.clone(),
                spec_line: tree.line,
                children,
            }),
            RootMapping::File => out.extend(children),
        }
    }
    out
}

fn expected_node(node: &SpecNode, mapping: &Mapping) -> Expected {
    match node.kind {
        NodeKind::Condition => Expected {
            kind: ActualKind::Block,
            name: mapping.block.apply(&node.text),
            text: node.text.clone(),
            spec_line: node.line,
            children: node.children.iter().map(|c| expected_node(c, mapping)).collect(),
        },
        NodeKind::Action => Expected {
            kind: ActualKind::Test,
            name: mapping.test.apply(&node.text),
            text: node.text.clone(),
            spec_line: node.line,
            children: Vec::new(),
        },
    }
}

/// Splice out transparent wrapper blocks (e.g. Rust's `mod tests`).
pub fn unwrap_wrappers(nodes: Vec<ActualNode>, wrappers: &[String]) -> Vec<ActualNode> {
    let mut out = Vec::new();
    for mut n in nodes {
        n.children = unwrap_wrappers(std::mem::take(&mut n.children), wrappers);
        if n.kind == ActualKind::Block && wrappers.contains(&n.name) {
            out.extend(n.children);
        } else {
            out.push(n);
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingKind {
    MissingBlock,
    MissingTest,
    ExtraBlock,
    ExtraTest,
    OutOfOrder,
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub kind: FindingKind,
    /// Human path to the node, e.g. "when the key is present > returns_none".
    pub path: String,
    /// Line in the .tree file (for missing) — 1-based.
    pub spec_line: Option<usize>,
    /// Line in the target file (for extra / order) — 1-based.
    pub target_line: Option<usize>,
}

/// Diff expected vs actual structure.
pub fn diff(expected: &[Expected], actual: &[ActualNode]) -> Vec<Finding> {
    let mut findings = Vec::new();
    diff_level(expected, actual, "", &mut findings);
    findings
}

fn join(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_string()
    } else {
        format!("{path} > {name}")
    }
}

fn diff_level(expected: &[Expected], actual: &[ActualNode], path: &str, out: &mut Vec<Finding>) {
    let mut used = vec![false; actual.len()];
    let mut matched_positions: Vec<usize> = Vec::new();

    for exp in expected {
        let found = actual.iter().enumerate().find(|(i, a)| {
            !used[*i] && a.kind == exp.kind && a.name.trim() == exp.name
        });
        match found {
            Some((i, a)) => {
                used[i] = true;
                matched_positions.push(i);
                if exp.kind == ActualKind::Block {
                    diff_level(&exp.children, &a.children, &join(path, &exp.name), out);
                }
            }
            None => out.push(Finding {
                kind: match exp.kind {
                    ActualKind::Block => FindingKind::MissingBlock,
                    ActualKind::Test => FindingKind::MissingTest,
                },
                path: join(path, &exp.name),
                spec_line: Some(exp.spec_line),
                target_line: None,
            }),
        }
    }

    if matched_positions.windows(2).any(|w| w[0] > w[1]) {
        out.push(Finding {
            kind: FindingKind::OutOfOrder,
            path: if path.is_empty() { "(top level)".into() } else { path.to_string() },
            spec_line: expected.first().map(|e| e.spec_line),
            target_line: None,
        });
    }

    for (i, a) in actual.iter().enumerate() {
        if !used[i] {
            out.push(Finding {
                kind: match a.kind {
                    ActualKind::Block => FindingKind::ExtraBlock,
                    ActualKind::Test => FindingKind::ExtraTest,
                },
                path: join(path, &a.name),
                spec_line: None,
                target_line: Some(a.line),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::{Case, NameRule};
    use crate::tree;

    fn rust_mapping() -> Mapping {
        Mapping {
            root: RootMapping::File,
            block: NameRule { case: Case::Snake, ..Default::default() },
            test: NameRule {
                strip_prefix: Some("it ".into()),
                case: Case::Snake,
                ..Default::default()
            },
            wrappers: vec!["tests".into()],
        }
    }

    fn block(name: &str, children: Vec<ActualNode>) -> ActualNode {
        ActualNode { kind: ActualKind::Block, name: name.into(), line: 1, children }
    }

    fn test(name: &str) -> ActualNode {
        ActualNode { kind: ActualKind::Test, name: name.into(), line: 1, children: vec![] }
    }

    const SPEC: &str = "\
Adder
├── when both inputs are zero
│   └── it returns zero
└── when one input is negative
    └── it returns the difference
";

    mod when_the_file_matches_the_spec {
        use super::*;

        #[test]
        fn reports_no_findings() {
            let trees = tree::parse(SPEC).unwrap();
            let expected = expected_from_spec(&trees, &rust_mapping());
            let actual = vec![
                block("when_both_inputs_are_zero", vec![test("returns_zero")]),
                block("when_one_input_is_negative", vec![test("returns_the_difference")]),
            ];
            assert!(diff(&expected, &actual).is_empty());
        }

        #[test]
        fn sees_through_wrapper_blocks() {
            let trees = tree::parse(SPEC).unwrap();
            let expected = expected_from_spec(&trees, &rust_mapping());
            let actual = unwrap_wrappers(
                vec![block(
                    "tests",
                    vec![
                        block("when_both_inputs_are_zero", vec![test("returns_zero")]),
                        block("when_one_input_is_negative", vec![test("returns_the_difference")]),
                    ],
                )],
                &rust_mapping().wrappers,
            );
            assert!(diff(&expected, &actual).is_empty());
        }
    }

    mod when_a_test_is_missing {
        use super::*;

        #[test]
        fn reports_it_with_the_spec_line() {
            let trees = tree::parse(SPEC).unwrap();
            let expected = expected_from_spec(&trees, &rust_mapping());
            let actual = vec![
                block("when_both_inputs_are_zero", vec![]),
                block("when_one_input_is_negative", vec![test("returns_the_difference")]),
            ];
            let findings = diff(&expected, &actual);
            assert_eq!(findings.len(), 1);
            assert_eq!(findings[0].kind, FindingKind::MissingTest);
            assert_eq!(findings[0].path, "when_both_inputs_are_zero > returns_zero");
            assert_eq!(findings[0].spec_line, Some(3));
        }
    }

    mod when_the_file_has_extra_tests {
        use super::*;

        #[test]
        fn reports_each_extra_node() {
            let trees = tree::parse(SPEC).unwrap();
            let expected = expected_from_spec(&trees, &rust_mapping());
            let actual = vec![
                block(
                    "when_both_inputs_are_zero",
                    vec![test("returns_zero"), test("does_not_overflow")],
                ),
                block("when_one_input_is_negative", vec![test("returns_the_difference")]),
            ];
            let findings = diff(&expected, &actual);
            assert_eq!(findings.len(), 1);
            assert_eq!(findings[0].kind, FindingKind::ExtraTest);
        }
    }

    mod when_sibling_order_differs {
        use super::*;

        #[test]
        fn reports_an_order_finding() {
            let trees = tree::parse(SPEC).unwrap();
            let expected = expected_from_spec(&trees, &rust_mapping());
            let actual = vec![
                block("when_one_input_is_negative", vec![test("returns_the_difference")]),
                block("when_both_inputs_are_zero", vec![test("returns_zero")]),
            ];
            let findings = diff(&expected, &actual);
            assert_eq!(findings.len(), 1);
            assert_eq!(findings[0].kind, FindingKind::OutOfOrder);
        }
    }
}
