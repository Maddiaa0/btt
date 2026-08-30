//! Structural behavior differences between parsed tree specifications.

use crate::tree::{NodeKind, SpecNode, SpecTree};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// A behavior path and its structural location.
#[derive(Clone, Debug)]
struct Behavior {
    path: String,
    parent: String,
    position: usize,
    kind: NodeKind,
}

/// One conservative behavior rename.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Rename {
    /// Path in the old tree.
    pub from: String,
    /// Path in the new tree.
    pub to: String,
}

/// Structural changes within one tree file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Paths only on the new side.
    pub added: Vec<String>,
    /// Paths only on the old side.
    pub removed: Vec<String>,
    /// Unambiguous edited sibling labels.
    pub renamed: Vec<Rename>,
}

impl Changes {
    /// Whether this file has any behavior changes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.renamed.is_empty()
    }
}

/// Compare two parsed tree files by their full structural paths.
#[must_use]
pub fn compare(old: &[SpecTree], new: &[SpecTree]) -> Changes {
    let old = flatten(old);
    let new = flatten(new);
    let old_paths: BTreeSet<_> = old.iter().map(|item| item.path.as_str()).collect();
    let new_paths: BTreeSet<_> = new.iter().map(|item| item.path.as_str()).collect();
    let mut removed: Vec<_> = old
        .iter()
        .filter(|item| !new_paths.contains(item.path.as_str()))
        .cloned()
        .collect();
    let mut added: Vec<_> = new
        .iter()
        .filter(|item| !old_paths.contains(item.path.as_str()))
        .cloned()
        .collect();

    let old_groups = group_indices(&removed);
    let new_groups = group_indices(&added);
    let mut renamed = Vec::new();
    let mut used_old = BTreeSet::new();
    let mut used_new = BTreeSet::new();
    for (parent, old_indices) in old_groups {
        let Some(new_indices) = new_groups.get(&parent) else {
            continue;
        };
        if old_indices.len() == 1 && new_indices.len() == 1 {
            let old_index = old_indices[0];
            let new_index = new_indices[0];
            let before = &removed[old_index];
            let after = &added[new_index];
            if before.position == after.position && before.kind == after.kind {
                renamed.push(Rename {
                    from: before.path.clone(),
                    to: after.path.clone(),
                });
                used_old.insert(old_index);
                used_new.insert(new_index);
            }
        }
    }
    removed = removed
        .into_iter()
        .enumerate()
        .filter(|(index, _)| !used_old.contains(index))
        .map(|(_, item)| item)
        .collect();
    added = added
        .into_iter()
        .enumerate()
        .filter(|(index, _)| !used_new.contains(index))
        .map(|(_, item)| item)
        .collect();
    Changes {
        added: added.into_iter().map(|item| item.path).collect(),
        removed: removed.into_iter().map(|item| item.path).collect(),
        renamed,
    }
}

fn group_indices(items: &[Behavior]) -> BTreeMap<String, Vec<usize>> {
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, item) in items.iter().enumerate() {
        groups.entry(item.parent.clone()).or_default().push(index);
    }
    groups
}

fn flatten(trees: &[SpecTree]) -> Vec<Behavior> {
    let mut out = Vec::new();
    for tree in trees {
        for (position, node) in tree.children.iter().enumerate() {
            flatten_node(node, &tree.root, position, &mut out);
        }
    }
    out
}

fn flatten_node(node: &SpecNode, parent: &str, position: usize, out: &mut Vec<Behavior>) {
    let path = format!("{parent} > {}", node.text);
    out.push(Behavior {
        path: path.clone(),
        parent: parent.to_string(),
        position,
        kind: node.kind,
    });
    for (child_position, child) in node.children.iter().enumerate() {
        flatten_node(child, &path, child_position, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree;

    mod when_tree_contents_differ {
        use super::*;

        #[test]
        fn computes_a_structural_full_path_set_diff() {
            let old = tree::parse("suite\n└── it old\n").unwrap();
            let new = tree::parse("suite\n├── it old\n└── it added\n").unwrap();
            let changes = compare(&old, &new);
            assert_eq!(changes.added, ["suite > it added"]);
            assert!(changes.removed.is_empty());
        }

        #[test]
        fn recognizes_only_unambiguous_same_position_sibling_renames() {
            let old = tree::parse("suite\n└── it old\n").unwrap();
            let new = tree::parse("suite\n└── it new\n").unwrap();
            let changes = compare(&old, &new);
            assert_eq!(
                changes.renamed,
                [Rename {
                    from: "suite > it old".into(),
                    to: "suite > it new".into()
                }]
            );

            let ambiguous_old = tree::parse("suite\n├── it one\n└── it two\n").unwrap();
            let ambiguous_new = tree::parse("suite\n├── it three\n└── it four\n").unwrap();
            assert!(compare(&ambiguous_old, &ambiguous_new).renamed.is_empty());
        }
    }

    mod when_tree_files_differ {
        use super::*;

        #[test]
        fn keeps_additions_and_removals_separated_by_file() {
            let parsed = tree::parse("suite\n└── it behavior\n").unwrap();
            let added = compare(&[], &parsed);
            let removed = compare(&parsed, &[]);
            assert_eq!(added.added, ["suite > it behavior"]);
            assert!(added.removed.is_empty());
            assert_eq!(removed.removed, ["suite > it behavior"]);
            assert!(removed.added.is_empty());
        }
    }
}
