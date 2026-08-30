//! Project configuration (`btt.toml`) and project-root discovery.

use crate::error::{Error, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Severity assigned to a category of finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Fail the check.
    Error,
    /// Report, but do not fail the check.
    Warn,
    /// Do not report at all.
    Ignore,
}

/// Severities for the configurable finding categories.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CheckConfig {
    /// Severity of tests/blocks present in the file but absent from the tree.
    pub extra: Level,
    /// Severity of sibling order differing between tree and file.
    pub order: Level,
    /// Severity of test-bearing source files that no `.tree` spec covers.
    pub uncovered: Level,
}

impl Default for CheckConfig {
    fn default() -> Self {
        CheckConfig {
            extra: Level::Warn,
            order: Level::Warn,
            uncovered: Level::Warn,
        }
    }
}

/// A parsed `btt.toml`.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectConfig {
    /// The `[project]` section.
    pub project: ProjectSection,
    /// The `[check]` section.
    pub check: CheckConfig,
}

/// The `[project]` section of `btt.toml`.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectSection {
    /// Packs this project uses, in routing-priority order.
    pub packs: Vec<String>,
}

/// Walk up from `start` looking for a `btt.toml` (preferred) or `.git`.
#[must_use]
pub fn find_project_root(start: &Path) -> PathBuf {
    start
        .ancestors()
        .find(|dir| dir.join("btt.toml").is_file())
        .or_else(|| start.ancestors().find(|dir| dir.join(".git").exists()))
        .unwrap_or(start)
        .to_path_buf()
}

/// Find the nearest directory at or above `dir` — without escaping `root` —
/// that contains a `btt.toml`.
///
/// `dir` must share `root`'s form (both absolute, typically) for the
/// containment check to work.
#[must_use]
pub fn nearest_config_dir(dir: &Path, root: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .take_while(|d| d.starts_with(root))
        .find(|d| d.join("btt.toml").is_file())
        .map(Path::to_path_buf)
}

/// The directory whose `btt.toml` governs `tree_path`: the nearest config
/// at or above it within `root`, or `root` itself when none exists.
#[must_use]
pub fn governing_root(tree_path: &Path, root: &Path) -> PathBuf {
    let dir = tree_path.parent().unwrap_or(Path::new("."));
    let dir = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    nearest_config_dir(&dir, root).unwrap_or_else(|| root.to_path_buf())
}

/// Load `btt.toml` from the project root, or defaults if absent.
///
/// # Errors
///
/// Fails if `btt.toml` exists but cannot be read or parsed.
pub fn load(project_root: &Path) -> Result<ProjectConfig> {
    let path = project_root.join("btt.toml");
    if !path.is_file() {
        return Ok(ProjectConfig::default());
    }
    let raw = std::fs::read_to_string(&path).map_err(|source| Error::io(&path, source))?;
    toml::from_str(&raw).map_err(|source| Error::Toml {
        path,
        source: Box::new(source),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh scratch directory unique to this test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("btt-config-tests-{}", std::process::id()))
            .join(name);
        _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch_config(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("btt.toml"), "").unwrap();
    }

    mod when_finding_the_nearest_config {
        use super::*;

        #[test]
        fn finds_a_config_beside_the_tree_file() {
            let root = scratch("beside");
            touch_config(&root.join("web"));
            assert_eq!(
                nearest_config_dir(&root.join("web"), &root),
                Some(root.join("web"))
            );
        }

        #[test]
        fn walks_up_to_an_ancestor_within_the_root() {
            let root = scratch("ancestor");
            touch_config(&root.join("web"));
            std::fs::create_dir_all(root.join("web/src/deep")).unwrap();
            assert_eq!(
                nearest_config_dir(&root.join("web/src/deep"), &root),
                Some(root.join("web"))
            );
        }

        #[test]
        fn stops_at_the_invocation_root() {
            let base = scratch("stops");
            touch_config(&base);
            let root = base.join("repo");
            std::fs::create_dir_all(root.join("src")).unwrap();
            assert_eq!(nearest_config_dir(&root.join("src"), &root), None);
        }

        #[test]
        fn returns_none_without_any_config() {
            let root = scratch("none");
            std::fs::create_dir_all(root.join("src")).unwrap();
            assert_eq!(nearest_config_dir(&root.join("src"), &root), None);
        }
    }

    mod when_resolving_the_governing_root {
        use super::*;

        #[test]
        fn uses_the_nearest_config_for_a_tree_file() {
            let root = scratch("governs");
            touch_config(&root.join("web"));
            assert_eq!(
                governing_root(&root.join("web/map.tree"), &root),
                root.join("web")
            );
        }

        #[test]
        fn falls_back_to_the_invocation_root() {
            let root = scratch("fallback");
            std::fs::create_dir_all(root.join("src")).unwrap();
            assert_eq!(governing_root(&root.join("src/map.tree"), &root), root);
        }
    }
}
