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
#[serde(default)]
pub struct CheckConfig {
    /// Severity of tests/blocks present in the file but absent from the tree.
    pub extra: Level,
    /// Severity of sibling order differing between tree and file.
    pub order: Level,
}

impl Default for CheckConfig {
    fn default() -> Self {
        CheckConfig { extra: Level::Warn, order: Level::Warn }
    }
}

/// A parsed `btt.toml`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ProjectConfig {
    /// The `[project]` section.
    pub project: ProjectSection,
    /// The `[check]` section.
    pub check: CheckConfig,
}

/// The `[project]` section of `btt.toml`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
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
    toml::from_str(&raw).map_err(|source| Error::Toml { path, source: Box::new(source) })
}
