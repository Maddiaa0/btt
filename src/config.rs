//! Project configuration (`btt.toml`) and project-root discovery.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Error,
    Warn,
    Ignore,
}

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ProjectConfig {
    pub project: ProjectSection,
    pub check: CheckConfig,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ProjectSection {
    /// Packs this project uses, in routing-priority order.
    pub packs: Vec<String>,
}

/// Walk up from `start` looking for a `btt.toml` (preferred) or `.git`.
pub fn find_project_root(start: &Path) -> PathBuf {
    let mut fallback = start.to_path_buf();
    for dir in start.ancestors() {
        if dir.join("btt.toml").is_file() {
            return dir.to_path_buf();
        }
        if dir.join(".git").exists() && fallback == *start {
            fallback = dir.to_path_buf();
        }
    }
    fallback
}

/// Load `btt.toml` from the project root, or defaults if absent.
pub fn load(project_root: &Path) -> Result<ProjectConfig> {
    let path = project_root.join("btt.toml");
    if !path.is_file() {
        return Ok(ProjectConfig::default());
    }
    let raw = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
}
