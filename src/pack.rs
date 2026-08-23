//! Language pack model and loading.
//!
//! A pack is a directory containing:
//! ```text
//! pack.toml            # manifest: detection, grammar, mapping, scaffold config
//! queries/tests.scm    # tree-sitter query extracting blocks and tests
//! templates/test.jinja # scaffold template
//! ```
//!
//! Resolution order for a pack named `foo`:
//!   1. `<project>/.btt/packs/foo/`   (vendored / project-local)
//!   2. `~/.btt/packs/foo/`           (user-global, installed via `btt ext`)
//!   3. packs embedded in the binary  (rust, typescript)
//!
//! Packs are data-only: a manifest, a tree-sitter query, and templates. They
//! never contain executable code, so vendoring one is as reviewable as any
//! other config change.

use crate::mapping::Mapping;
use anyhow::{bail, Context, Result};
use include_dir::{include_dir, Dir};
use serde::Deserialize;
use std::path::{Path, PathBuf};

static EMBEDDED_PACKS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/packs");

#[derive(Debug, Deserialize)]
pub struct PackMeta {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct Detect {
    /// File extensions this pack's language uses, e.g. ["rs"].
    pub extensions: Vec<String>,
    /// Candidate test-file names for a tree file, tried in order.
    /// `{stem}` is the tree file's stem: `map.tree` -> `map`.
    pub targets: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct GrammarConfig {
    /// Grammar source. `builtin:<name>` uses a grammar compiled into the
    /// core binary. (A `wasm:<file>` source is the planned extension point
    /// for packs shipping their own sandboxed grammar.)
    pub source: String,
}

#[derive(Debug, Deserialize)]
pub struct Extract {
    /// Path (within the pack) of the tree-sitter query file.
    pub query: String,
    /// If true, a `@test` capture only counts as a test when a
    /// `@test.marker` capture (e.g. a `#[test]` attribute) directly
    /// precedes it among its siblings.
    #[serde(default)]
    pub test_requires_marker: bool,
}

#[derive(Debug, Deserialize)]
pub struct Scaffold {
    /// Path (within the pack) of the scaffold template.
    pub template: String,
    /// Output file name pattern, e.g. "{stem}.test.ts".
    pub output: String,
    /// Indentation unit used by the template's `indent` values.
    #[serde(default = "default_indent")]
    pub indent: String,
}

fn default_indent() -> String {
    "    ".to_string()
}

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub pack: PackMeta,
    pub detect: Detect,
    pub grammar: GrammarConfig,
    pub extract: Extract,
    #[serde(default)]
    pub mapping: Mapping,
    pub scaffold: Scaffold,
}

/// A fully loaded pack: manifest plus the file contents it references.
pub struct Pack {
    pub manifest: Manifest,
    pub query: String,
    pub template: String,
    /// Where this pack came from, for `btt packs` output.
    pub origin: String,
}

impl Pack {
    pub fn name(&self) -> &str {
        &self.manifest.pack.name
    }

    pub fn matches_extension(&self, ext: &str) -> bool {
        self.manifest.detect.extensions.iter().any(|e| e == ext)
    }
}

fn load_from_dir(dir: &Path, origin: &str) -> Result<Pack> {
    let manifest_path = dir.join("pack.toml");
    let raw = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let manifest: Manifest =
        toml::from_str(&raw).with_context(|| format!("parsing {}", manifest_path.display()))?;
    let query = std::fs::read_to_string(dir.join(&manifest.extract.query))
        .with_context(|| format!("reading query {}", manifest.extract.query))?;
    let template = std::fs::read_to_string(dir.join(&manifest.scaffold.template))
        .with_context(|| format!("reading template {}", manifest.scaffold.template))?;
    Ok(Pack { manifest, query, template, origin: origin.to_string() })
}

fn load_embedded(dir: &Dir<'_>) -> Result<Pack> {
    let file = |p: &str| -> Result<String> {
        let f = dir
            .get_file(format!("{}/{}", dir.path().display(), p))
            .or_else(|| dir.get_file(p))
            .with_context(|| format!("embedded pack missing {p}"))?;
        Ok(f.contents_utf8().context("embedded file not utf8")?.to_string())
    };
    let manifest: Manifest = toml::from_str(&file("pack.toml")?)
        .with_context(|| format!("parsing embedded pack {}", dir.path().display()))?;
    let query = file(&manifest.extract.query)?;
    let template = file(&manifest.scaffold.template)?;
    Ok(Pack { manifest, query, template, origin: "builtin".to_string() })
}

/// Load a pack by name, honoring the resolution order.
pub fn load(name: &str, project_root: &Path) -> Result<Pack> {
    let local = project_root.join(".btt/packs").join(name);
    if local.join("pack.toml").is_file() {
        return load_from_dir(&local, &local.display().to_string());
    }
    if let Some(home) = home_dir() {
        let global = home.join(".btt/packs").join(name);
        if global.join("pack.toml").is_file() {
            return load_from_dir(&global, &global.display().to_string());
        }
    }
    if let Some(dir) = EMBEDDED_PACKS.get_dir(name) {
        return load_embedded(dir);
    }
    bail!(
        "pack `{name}` not found (looked in .btt/packs, ~/.btt/packs, and builtins: {})",
        builtin_names().join(", ")
    );
}

/// Names of all packs visible from a project, deduplicated by precedence.
pub fn available(project_root: &Path) -> Vec<(String, String)> {
    let mut seen: Vec<(String, String)> = Vec::new();
    let mut push = |name: String, origin: String| {
        if !seen.iter().any(|(n, _)| *n == name) {
            seen.push((name, origin));
        }
    };
    for (dir, label) in [
        (project_root.join(".btt/packs"), "project"),
        (home_dir().map(|h| h.join(".btt/packs")).unwrap_or_default(), "user"),
    ] {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                if e.path().join("pack.toml").is_file() {
                    push(e.file_name().to_string_lossy().to_string(), label.to_string());
                }
            }
        }
    }
    for name in builtin_names() {
        push(name, "builtin".to_string());
    }
    seen.sort();
    seen
}

pub fn builtin_names() -> Vec<String> {
    EMBEDDED_PACKS
        .dirs()
        .map(|d| d.path().file_name().unwrap().to_string_lossy().to_string())
        .collect()
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Resolve the tree-sitter language for a pack + target file.
pub fn language_for(pack: &Pack, target: &Path) -> Result<tree_sitter::Language> {
    let Some(builtin) = pack.manifest.grammar.source.strip_prefix("builtin:") else {
        bail!(
            "pack `{}`: unsupported grammar source `{}` (only builtin:* for now)",
            pack.name(),
            pack.manifest.grammar.source
        );
    };
    let ext = target.extension().and_then(|e| e.to_str()).unwrap_or("");
    match builtin {
        "rust" => Ok(tree_sitter_rust::LANGUAGE.into()),
        "typescript" => {
            if ext == "tsx" || ext == "jsx" {
                Ok(tree_sitter_typescript::LANGUAGE_TSX.into())
            } else {
                Ok(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
            }
        }
        other => bail!("unknown builtin grammar `{other}`"),
    }
}
