//! High-level check pipeline shared by the CLI and by btt's own test suite.

use crate::check::{self, Finding};
use crate::config::{CheckConfig, Level};
use crate::error::{Error, Result};
use crate::extract::{self, ActualKind};
use crate::pack::Pack;
use crate::tree;
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Find all `.tree` files under the given paths (files pass through as-is).
///
/// Fails closed: a search path that does not exist or a directory that
/// cannot be read is an error, never an empty result — "no .tree files
/// found" must not be the report for a mistyped path.
///
/// # Errors
///
/// Returns the first walk error (missing path, unreadable directory).
pub fn find_tree_files(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for path in paths {
        if path.is_file() {
            out.push(path.clone());
            continue;
        }
        let walker = WalkDir::new(path).into_iter().filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !(e.file_type().is_dir()
                && (name == ".git" || name == "target" || name == "node_modules"))
        });
        for entry in walker {
            let entry = entry.map_err(|e| walk_error(path, e))?;
            if entry.file_type().is_file() && entry.path().extension().is_some_and(|e| e == "tree")
            {
                out.push(entry.into_path());
            }
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Convert a walk error into a typed I/O error at the most specific path.
fn walk_error(root: &Path, e: walkdir::Error) -> Error {
    let path = e
        .path()
        .map_or_else(|| root.to_path_buf(), Path::to_path_buf);
    let source = e
        .into_io_error()
        .unwrap_or_else(|| std::io::Error::other("filesystem loop"));
    Error::io(path, source)
}

/// A test-bearing source file that no `.tree` spec covers.
#[derive(Debug)]
pub struct Uncovered {
    /// The source file containing tests.
    pub path: PathBuf,
    /// How many tests it contains.
    pub tests: usize,
}

/// Derive the `{stem}` a file name would need for `pattern` to produce it,
/// if it matches at all (`map.test.ts` against `{stem}.test.ts` → `map`).
fn stem_for(pattern: &str, file_name: &str) -> Option<String> {
    let (prefix, suffix) = pattern.split_once("{stem}")?;
    let stem = file_name.strip_prefix(prefix)?.strip_suffix(suffix)?;
    (!stem.is_empty()).then(|| stem.to_string())
}

fn count_actual_tests(nodes: &[extract::ActualNode]) -> usize {
    nodes
        .iter()
        .map(|n| match n.kind {
            ActualKind::Test => 1,
            ActualKind::Block => count_actual_tests(&n.children),
        })
        .sum()
}

/// Outcome of the uncovered scan: files that need specs, plus files the
/// scan could not verify.
#[derive(Debug, Default)]
pub struct UncoveredScan {
    /// Test-bearing files no tree routes to, in path order.
    pub uncovered: Vec<Uncovered>,
    /// Candidates that could not be scanned (unreadable file or directory,
    /// grammar unavailable, …). Unverifiable coverage is a tool failure,
    /// not an absence of findings — strict projects must not pass because
    /// extraction or the directory walk broke.
    pub failed: Vec<(PathBuf, Error)>,
}

/// Find source files under `paths` that contain tests but that no `.tree`
/// spec routes to. This is what keeps partial adoption honest: `check`
/// reports not just "the covered files match" but "these files are not
/// covered at all".
///
/// Coverage is routing-exact: only the file forward routing actually
/// selects for a tree (the *first* existing target pattern) counts as
/// covered — a same-stem sibling matching a later pattern does not. A file
/// is a candidate when its name matches any pack's target pattern in
/// reverse; it is uncovered when the pack's query extracts at least one
/// test from it. Runs on the ambient rayon pool.
#[must_use]
pub fn find_uncovered(packs: &[Pack], paths: &[PathBuf], tree_files: &[PathBuf]) -> UncoveredScan {
    let covered: std::collections::HashSet<PathBuf> = tree_files
        .iter()
        .filter_map(|tree| match resolve_target(tree, packs) {
            Target::Found { path, .. } => Some(path),
            Target::NotFound { .. } => None,
        })
        .collect();

    let mut candidates: Vec<(PathBuf, &Pack)> = Vec::new();
    let mut walk_failures: Vec<(PathBuf, Error)> = Vec::new();
    for path in paths {
        let walker = WalkDir::new(path).into_iter().filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !(e.file_type().is_dir()
                && (name == ".git" || name == "target" || name == "node_modules"))
        });
        for entry in walker {
            let entry = match entry {
                Ok(entry) => entry,
                // A directory that cannot be walked hides an unknown
                // number of candidates: record it as a failure so strict
                // projects fail instead of passing blind.
                Err(e) => {
                    let at = e.path().map_or_else(|| path.clone(), Path::to_path_buf);
                    walk_failures.push((at, walk_error(path, e)));
                    continue;
                }
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let Some(name) = entry.file_name().to_str() else {
                continue;
            };
            let matched = packs.iter().find(|pack| {
                pack.manifest
                    .detect
                    .targets
                    .iter()
                    .any(|pattern| stem_for(pattern, name).is_some())
            });
            if let Some(pack) = matched
                && !covered.contains(entry.path())
            {
                candidates.push((entry.into_path(), pack));
            }
        }
    }
    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    candidates.dedup_by(|a, b| a.0 == b.0);

    let results: Vec<(PathBuf, Result<usize>)> = candidates
        .par_iter()
        .map(|(path, pack)| {
            let count = std::fs::read_to_string(path)
                .map_err(|source| Error::io(path, source))
                .and_then(|source| extract::extract(pack, path, &source))
                .map(|actual| count_actual_tests(&actual));
            (path.clone(), count)
        })
        .collect();

    let mut scan = UncoveredScan::default();
    scan.failed.extend(walk_failures);
    for (path, result) in results {
        match result {
            Ok(tests) if tests > 0 => scan.uncovered.push(Uncovered { path, tests }),
            Ok(_) => {}
            Err(e) => scan.failed.push((path, e)),
        }
    }
    scan
}

/// Result of routing a `.tree` file to its test file.
pub enum Target<'a> {
    /// A test file exists; check against it with this pack.
    Found {
        /// The pack whose target pattern matched.
        pack: &'a Pack,
        /// The matched test file.
        path: PathBuf,
    },
    /// No candidate file exists; carries the candidates that were tried.
    NotFound {
        /// Every candidate path that was tried, in order.
        candidates: Vec<PathBuf>,
    },
}

/// Locate the test file a `.tree` file describes: for each pack in priority
/// order, try its target patterns next to the tree file.
#[must_use]
pub fn resolve_target<'a>(tree_path: &Path, packs: &'a [Pack]) -> Target<'a> {
    let dir = tree_path.parent().unwrap_or(Path::new("."));
    let stem = tree_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let mut candidates = Vec::new();
    for pack in packs {
        for pattern in &pack.manifest.detect.targets {
            let candidate = dir.join(pattern.replace("{stem}", stem));
            if candidate.is_file() {
                return Target::Found {
                    pack,
                    path: candidate,
                };
            }
            candidates.push(candidate);
        }
    }
    Target::NotFound { candidates }
}

/// A finding with its configured severity resolved.
#[derive(Debug)]
pub struct Reported {
    /// The underlying finding.
    pub finding: Finding,
    /// Its severity under the project's check config (never [`Level::Ignore`]).
    pub level: Level,
}

/// Check one tree file against its target. Returns findings whose configured
/// level is not `ignore`.
///
/// # Errors
///
/// Fails if either file cannot be read, the spec does not parse, or the
/// pack cannot extract structure from the target.
pub fn check_file(
    pack: &Pack,
    tree_path: &Path,
    target: &Path,
    cfg: &CheckConfig,
) -> Result<Vec<Reported>> {
    let spec_src =
        std::fs::read_to_string(tree_path).map_err(|source| Error::io(tree_path, source))?;
    let trees = tree::parse(&spec_src).map_err(|source| Error::Parse {
        path: tree_path.to_path_buf(),
        source,
    })?;
    let source = std::fs::read_to_string(target).map_err(|source| Error::io(target, source))?;

    let mapping = &pack.manifest.mapping;
    let expected = check::expected_from_spec(&trees, mapping);
    let actual = extract::extract(pack, target, &source)?;
    let actual = check::unwrap_wrappers(actual, &mapping.wrappers);

    let findings = check::diff(&expected, &actual);
    Ok(findings
        .into_iter()
        .filter_map(|finding| {
            let level = match finding {
                Finding::Missing { .. } => Level::Error,
                Finding::Extra { .. } => cfg.extra,
                Finding::OutOfOrder { .. } => cfg.order,
            };
            match level {
                Level::Ignore => None,
                level => Some(Reported { finding, level }),
            }
        })
        .collect())
}

/// What happened when one tree file was checked.
#[derive(Debug)]
pub enum FileResult {
    /// A target was found and checked; `findings` may be empty (a pass).
    Checked {
        /// The test file that was checked.
        target: PathBuf,
        /// Findings at their configured severities.
        findings: Vec<Reported>,
    },
    /// No candidate test file exists for this tree.
    NoTarget {
        /// Every candidate path that was tried, in order.
        candidates: Vec<PathBuf>,
    },
    /// The file could not be checked (unreadable, spec parse error, …).
    Failed(Error),
}

/// Outcome of checking a single tree file within a run.
#[derive(Debug)]
pub struct FileOutcome {
    /// The `.tree` file this outcome belongs to.
    pub tree_path: PathBuf,
    /// What happened.
    pub result: FileResult,
}

/// Check many tree files, in parallel on the ambient rayon pool.
///
/// Files are independent: one broken file becomes a [`FileResult::Failed`]
/// without affecting the rest. Outcomes are returned in input order. Callers
/// control parallelism by running this inside `rayon::ThreadPool::install`
/// (as `btt check -j` does); otherwise the global pool is used.
#[must_use]
pub fn check_all(packs: &[Pack], tree_files: &[PathBuf], cfg: CheckConfig) -> Vec<FileOutcome> {
    tree_files
        .par_iter()
        .map(|tree_path| {
            let result = match resolve_target(tree_path, packs) {
                Target::NotFound { candidates } => FileResult::NoTarget { candidates },
                Target::Found { pack, path } => match check_file(pack, tree_path, &path, &cfg) {
                    Ok(findings) => FileResult::Checked {
                        target: path,
                        findings,
                    },
                    Err(e) => FileResult::Failed(e),
                },
            };
            FileOutcome {
                tree_path: tree_path.clone(),
                result,
            }
        })
        .collect()
}

/// Count the number of expected tests in a spec (for summary output).
#[must_use]
pub fn count_tests(expected: &[check::Expected]) -> usize {
    expected
        .iter()
        .map(|e| match e.kind {
            ActualKind::Test => 1,
            ActualKind::Block => count_tests(&e.children),
        })
        .sum()
}
