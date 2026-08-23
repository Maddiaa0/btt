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

use crate::error::{Error, Result};
use crate::mapping::Mapping;
use include_dir::{Dir, include_dir};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

static EMBEDDED_PACKS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/packs");

/// Identity section of a pack manifest.
#[derive(Debug, Deserialize)]
pub struct PackMeta {
    /// Pack name (matches its directory name).
    pub name: String,
    /// Pack version.
    pub version: String,
    /// One-line description shown by `btt packs`.
    #[serde(default)]
    pub description: String,
}

/// How files are routed to this pack.
#[derive(Debug, Deserialize)]
pub struct Detect {
    /// File extensions this pack's language uses, e.g. `["rs"]`.
    pub extensions: Vec<String>,
    /// Candidate test-file names for a tree file, tried in order.
    /// `{stem}` is the tree file's stem: `map.tree` -> `map`.
    pub targets: Vec<String>,
}

/// Where a pack's grammar comes from.
#[derive(Debug, Clone)]
pub enum GrammarSource {
    /// A grammar compiled into the core binary (`builtin:<name>`).
    Builtin(String),
    /// A sandboxed WASM grammar shipped with the pack (`wasm:<file>`).
    Wasm(PathBuf),
}

impl<'de> Deserialize<'de> for GrammarSource {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        if let Some(name) = s.strip_prefix("builtin:") {
            Ok(GrammarSource::Builtin(name.to_string()))
        } else if let Some(file) = s.strip_prefix("wasm:") {
            Ok(GrammarSource::Wasm(PathBuf::from(file)))
        } else {
            Err(serde::de::Error::custom(format!(
                "grammar source must be `builtin:<name>` or `wasm:<file>`, got {s:?}"
            )))
        }
    }
}

/// Grammar section of a pack manifest.
#[derive(Debug, Deserialize)]
pub struct GrammarConfig {
    /// Grammar source.
    pub source: GrammarSource,
    /// Language symbol exported by a WASM grammar (`tree_sitter_<symbol>`).
    /// Defaults to the pack name.
    #[serde(default)]
    pub symbol: Option<String>,
}

/// Extraction section of a pack manifest.
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

/// Scaffold section of a pack manifest.
#[derive(Debug, Deserialize)]
pub struct Scaffold {
    /// Path (within the pack) of the scaffold template.
    pub template: String,
    /// Output file name pattern, e.g. `{stem}.test.ts`.
    pub output: String,
    /// Indentation unit used by the template's `indent` values.
    #[serde(default = "default_indent")]
    pub indent: String,
}

fn default_indent() -> String {
    "    ".to_string()
}

/// A parsed `pack.toml`.
#[derive(Debug, Deserialize)]
pub struct Manifest {
    /// Identity.
    pub pack: PackMeta,
    /// File routing.
    pub detect: Detect,
    /// Grammar source.
    pub grammar: GrammarConfig,
    /// Extraction query configuration.
    pub extract: Extract,
    /// Node-text-to-identifier rules.
    #[serde(default)]
    pub mapping: Mapping,
    /// Scaffold template configuration.
    pub scaffold: Scaffold,
}

/// Where a loaded pack was resolved from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Embedded in the btt binary.
    Builtin,
    /// `~/.btt/packs/<name>`.
    User(PathBuf),
    /// `<project>/.btt/packs/<name>`.
    Project(PathBuf),
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Origin::Builtin => f.write_str("builtin"),
            Origin::User(_) => f.write_str("user"),
            Origin::Project(_) => f.write_str("project"),
        }
    }
}

/// A fully loaded pack: manifest plus the file contents it references.
#[derive(Debug)]
pub struct Pack {
    /// The parsed manifest.
    pub manifest: Manifest,
    /// Contents of the extraction query file.
    pub query: String,
    /// Contents of the scaffold template.
    pub template: String,
    /// Bytes of the pack's WASM grammar, when it ships one.
    pub wasm_grammar: Option<Vec<u8>>,
    /// Where this pack was resolved from.
    pub origin: Origin,
}

impl Pack {
    /// The pack's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.manifest.pack.name
    }

    /// Whether this pack's language uses the given file extension.
    #[must_use]
    pub fn matches_extension(&self, ext: &str) -> bool {
        self.manifest.detect.extensions.iter().any(|e| e == ext)
    }
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|source| Error::io(path, source))
}

fn load_from_dir(dir: &Path, origin: Origin) -> Result<Pack> {
    let manifest_path = dir.join("pack.toml");
    let manifest: Manifest = toml::from_str(&read(&manifest_path)?)
        .map_err(|source| Error::Toml { path: manifest_path, source: Box::new(source) })?;
    let query = read(&dir.join(&manifest.extract.query))?;
    let template = read(&dir.join(&manifest.scaffold.template))?;
    let wasm_grammar = match &manifest.grammar.source {
        // An unreadable grammar file is not a load error: it surfaces
        // per-file at parse time (via `grammar_for`), so one broken wasm
        // pack can't abort a whole run.
        GrammarSource::Wasm(file) => std::fs::read(dir.join(file)).ok(),
        GrammarSource::Builtin(_) => None,
    };
    Ok(Pack { manifest, query, template, wasm_grammar, origin })
}

/// Load a pack from an explicit directory, bypassing the resolution order.
///
/// # Errors
///
/// Fails if the directory does not contain a valid pack.
pub fn load_dir(dir: &Path) -> Result<Pack> {
    load_from_dir(dir, Origin::Project(dir.to_path_buf()))
}

fn load_embedded(dir: &Dir<'static>) -> Result<Pack> {
    let pack_name = dir
        .path()
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let file = |p: &str| -> Result<&'static str> {
        dir.get_file(dir.path().join(p))
            .and_then(include_dir::File::contents_utf8)
            .ok_or_else(|| Error::PackFile { pack: pack_name.clone(), file: p.to_string() })
    };
    let manifest: Manifest = toml::from_str(file("pack.toml")?).map_err(|source| Error::Toml {
        path: PathBuf::from(format!("<builtin:{pack_name}>/pack.toml")),
        source: Box::new(source),
    })?;
    let query = file(&manifest.extract.query)?.to_string();
    let template = file(&manifest.scaffold.template)?.to_string();
    let wasm_grammar = match &manifest.grammar.source {
        GrammarSource::Wasm(file) => {
            dir.get_file(dir.path().join(file)).map(|f| f.contents().to_vec())
        }
        GrammarSource::Builtin(_) => None,
    };
    Ok(Pack { manifest, query, template, wasm_grammar, origin: Origin::Builtin })
}

/// Load a pack by name, honoring the resolution order.
///
/// # Errors
///
/// Returns [`Error::PackNotFound`] if no source provides the pack, or an
/// error describing the broken file if a pack exists but fails to load.
pub fn load(name: &str, project_root: &Path) -> Result<Pack> {
    let local = project_root.join(".btt/packs").join(name);
    if local.join("pack.toml").is_file() {
        return load_from_dir(&local, Origin::Project(local.clone()));
    }
    if let Some(global) = home_dir().map(|h| h.join(".btt/packs").join(name))
        && global.join("pack.toml").is_file()
    {
        return load_from_dir(&global, Origin::User(global.clone()));
    }
    if let Some(dir) = EMBEDDED_PACKS.get_dir(name) {
        return load_embedded(dir);
    }
    Err(Error::PackNotFound { name: name.to_string(), builtins: builtin_names() })
}

/// Names of all packs visible from a project, with their winning origin,
/// sorted by name.
#[must_use]
pub fn available(project_root: &Path) -> Vec<(String, Origin)> {
    type OriginCtor = fn(PathBuf) -> Origin;
    let mut map: BTreeMap<String, Origin> = BTreeMap::new();
    let dirs: [(Option<PathBuf>, OriginCtor); 2] = [
        (Some(project_root.join(".btt/packs")), Origin::Project),
        (home_dir().map(|h| h.join(".btt/packs")), Origin::User),
    ];
    for (dir, make_origin) in dirs {
        let Some(dir) = dir else { continue };
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.join("pack.toml").is_file()
                && let Some(name) = path.file_name().and_then(|n| n.to_str())
            {
                map.entry(name.to_string()).or_insert_with(|| make_origin(path.clone()));
            }
        }
    }
    for name in builtin_names() {
        map.entry(name).or_insert(Origin::Builtin);
    }
    map.into_iter().collect()
}

/// Names of the packs embedded in this binary.
#[must_use]
pub fn builtin_names() -> Vec<String> {
    EMBEDDED_PACKS
        .dirs()
        .filter_map(|d| d.path().file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect()
}

fn home_dir() -> Option<PathBuf> {
    std::env::home_dir()
}

/// A pack's grammar, resolved for a specific target file.
#[derive(Debug)]
pub enum Grammar<'a> {
    /// A grammar compiled into this binary.
    Native(tree_sitter::Language),
    /// A sandboxed WASM grammar shipped by the pack.
    Wasm {
        /// The language symbol the module exports (`tree_sitter_<symbol>`).
        symbol: &'a str,
        /// The compiled grammar module.
        bytes: &'a [u8],
    },
}

/// Resolve the grammar for a pack + target file.
///
/// # Errors
///
/// Fails if the pack references an unknown builtin grammar or ships a
/// `wasm:` grammar whose file was not loaded.
pub fn grammar_for<'a>(pack: &'a Pack, target: &Path) -> Result<Grammar<'a>> {
    match &pack.manifest.grammar.source {
        GrammarSource::Wasm(file) => {
            let bytes = pack.wasm_grammar.as_deref().ok_or_else(|| Error::PackFile {
                pack: pack.name().to_string(),
                file: file.display().to_string(),
            })?;
            let symbol = pack.manifest.grammar.symbol.as_deref().unwrap_or(pack.name());
            Ok(Grammar::Wasm { symbol, bytes })
        }
        GrammarSource::Builtin(name) => match name.as_str() {
            "rust" => Ok(Grammar::Native(tree_sitter_rust::LANGUAGE.into())),
            "typescript" => {
                let ext = target.extension().and_then(|e| e.to_str()).unwrap_or_default();
                if ext == "tsx" || ext == "jsx" {
                    Ok(Grammar::Native(tree_sitter_typescript::LANGUAGE_TSX.into()))
                } else {
                    Ok(Grammar::Native(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()))
                }
            }
            other => Err(Error::UnknownGrammar {
                pack: pack.name().to_string(),
                name: other.to_string(),
            }),
        },
    }
}
