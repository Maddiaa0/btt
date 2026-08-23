//! High-level check pipeline shared by the CLI and by btt's own test suite.

use crate::check::{self, Finding, FindingKind};
use crate::config::{CheckConfig, Level};
use crate::extract;
use crate::pack::Pack;
use crate::tree;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Find all `.tree` files under the given paths (files pass through as-is).
pub fn find_tree_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for path in paths {
        if path.is_file() {
            out.push(path.clone());
            continue;
        }
        for entry in WalkDir::new(path)
            .into_iter()
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                !(e.file_type().is_dir()
                    && (name == ".git" || name == "target" || name == "node_modules"))
            })
            .flatten()
        {
            if entry.file_type().is_file()
                && entry.path().extension().is_some_and(|e| e == "tree")
            {
                out.push(entry.into_path());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

pub enum Target<'a> {
    Found { pack: &'a Pack, path: PathBuf },
    /// No candidate file exists; carries the candidates that were tried.
    NotFound { candidates: Vec<PathBuf> },
}

/// Locate the test file a `.tree` file describes: for each pack in priority
/// order, try its target patterns next to the tree file.
pub fn resolve_target<'a>(tree_path: &Path, packs: &'a [Pack]) -> Target<'a> {
    let dir = tree_path.parent().unwrap_or(Path::new("."));
    let stem = tree_path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
    let mut candidates = Vec::new();
    for pack in packs {
        for pattern in &pack.manifest.detect.targets {
            let candidate = dir.join(pattern.replace("{stem}", stem));
            if candidate.is_file() {
                return Target::Found { pack, path: candidate };
            }
            candidates.push(candidate);
        }
    }
    Target::NotFound { candidates }
}

/// A finding with its configured severity resolved.
pub struct Reported {
    pub finding: Finding,
    pub level: Level,
}

/// Check one tree file against its target. Returns findings whose configured
/// level is not `ignore`.
pub fn check_file(
    pack: &Pack,
    tree_path: &Path,
    target: &Path,
    cfg: &CheckConfig,
) -> Result<Vec<Reported>> {
    let spec_src = std::fs::read_to_string(tree_path)
        .with_context(|| format!("reading {}", tree_path.display()))?;
    let trees = tree::parse(&spec_src)
        .with_context(|| format!("parsing {}", tree_path.display()))?;
    let source = std::fs::read_to_string(target)
        .with_context(|| format!("reading {}", target.display()))?;

    let mapping = &pack.manifest.mapping;
    let expected = check::expected_from_spec(&trees, mapping);
    let actual = extract::extract(pack, target, &source)?;
    let actual = check::unwrap_wrappers(actual, &mapping.wrappers);

    let findings = check::diff(&expected, &actual);
    Ok(findings
        .into_iter()
        .filter_map(|finding| {
            let level = match finding.kind {
                FindingKind::MissingBlock | FindingKind::MissingTest => Level::Error,
                FindingKind::ExtraBlock | FindingKind::ExtraTest => cfg.extra,
                FindingKind::OutOfOrder => cfg.order,
            };
            match level {
                Level::Ignore => None,
                level => Some(Reported { finding, level }),
            }
        })
        .collect())
}

/// Count the number of expected tests in a spec (for summary output).
pub fn count_tests(expected: &[check::Expected]) -> usize {
    expected
        .iter()
        .map(|e| match e.kind {
            crate::extract::ActualKind::Test => 1,
            crate::extract::ActualKind::Block => count_tests(&e.children),
        })
        .sum()
}
