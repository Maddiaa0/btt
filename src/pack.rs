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
//!   1. `<project>/.btt/packs/foo/`        (vendored / project-local)
//!   2. `$XDG_CONFIG_HOME/btt/packs/foo/`  (user-global; defaults to
//!      `~/.config/btt/packs/foo/`)
//!   3. `~/.btt/packs/foo/`                (user-global, legacy location)
//!   4. packs embedded in the binary       (rust, typescript)
//!
//! Activation is explicit: packs load when `btt.toml` names them
//! (`packs = [...]`). With no configured list, only the embedded builtins
//! are active ([`load_builtin`]) — a pack sitting in a directory is never
//! executed just for being visible.
//!
//! ## Trust model
//!
//! - **Core** (this binary): configuration, routing, mapping, diffing,
//!   reporting.
//! - **Pack data** (manifest, query, mapping, templates): declarative but
//!   pack-controlled. Manifests parse strictly (unknown fields rejected),
//!   every path they name is confined to the pack directory, and queries
//!   run in the native tree-sitter runtime — they can cost CPU, not reach
//!   the system.
//! - **Executable extension** (`wasm:` grammars): the one part of a pack
//!   that is code — grammar tables plus any external scanner. Instantiated
//!   without WASI, which removes ambient filesystem/network access, but
//!   the tree-sitter host bridge is native code consuming module data, so
//!   this is *not* a hardened boundary for hostile modules. Treat wasm
//!   packs like any other dependency: review them, pin digests, obtain
//!   them from sources you trust (as `scripts/fetch-wasm-grammars.sh`
//!   does). Subprocess isolation for genuinely untrusted packs is future
//!   work.

use crate::error::{Error, Result};
use crate::mapping::Mapping;
use include_dir::{Dir, include_dir};
use semver::{Version, VersionReq};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};

static EMBEDDED_PACKS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/packs");

/// The `pack.toml` schema understood by this btt release.
pub const PACK_FORMAT_VERSION: u32 = 1;

/// Identity section of a pack manifest.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackMeta {
    /// Pack name (matches its directory name).
    pub name: String,
    /// Pack version.
    pub version: Version,
    /// One-line description shown by `btt packs`.
    #[serde(default)]
    pub description: String,
}

/// Compatibility between a pack release and the btt runtime.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Compatibility {
    /// `SemVer` requirement matched against the running btt version.
    pub btt: VersionReq,
}

/// How files are routed to this pack.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Detect {
    /// Candidate test-file names for a tree file, tried in order.
    /// `{stem}` is the tree file's stem: `map.tree` -> `map`.
    /// These patterns are the single source of routing truth — both for
    /// resolving a tree's target and (in reverse) for uncovered detection.
    pub targets: Vec<String>,
}

/// Where a pack's grammar comes from.
#[derive(Debug, Clone)]
pub enum GrammarSource {
    /// A grammar compiled into the core binary (`builtin:<name>`).
    Builtin(String),
    /// A sandboxed WASM grammar shipped with the pack (`wasm:<file>`).
    Wasm(PathBuf),
    /// No grammar: the pack's `[lexical]` profile drives extraction.
    Lexical,
}

impl<'de> Deserialize<'de> for GrammarSource {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        if let Some(name) = s.strip_prefix("builtin:") {
            Ok(GrammarSource::Builtin(name.to_string()))
        } else if let Some(file) = s.strip_prefix("wasm:") {
            Ok(GrammarSource::Wasm(PathBuf::from(file)))
        } else if s == "lexical" {
            Ok(GrammarSource::Lexical)
        } else {
            Err(serde::de::Error::custom(format!(
                "grammar source must be `builtin:<name>`, `wasm:<file>`, or `lexical`, got {s:?}"
            )))
        }
    }
}

/// One string-literal form in a lexical profile.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StringRule {
    /// The delimiter that opens and closes the literal (`"`, `'`, `` ` ``).
    pub delim: String,
    /// The escape prefix inside the literal, if the language has one.
    #[serde(default)]
    pub escape: Option<String>,
}

/// A block or test opener pattern in a lexical profile.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Opener {
    /// Regex matched against the source with comments blanked to
    /// whitespace. Must define `(?<kw>...)` on the keyword (must land on
    /// real code) and `(?<name>...)` on the name — a string literal
    /// including its quotes for `name_syntax = "js-string"`, a code
    /// identifier for `"raw"`. The pattern must also include the
    /// definition's opening bracket (the call's `(`, a body's `{`): its
    /// matching closer bounds the definition's span for nesting.
    pub open: String,
}

/// The lexical profile of a `source = "lexical"` pack: just enough syntax
/// to tell code from comments and strings, plus what a block and a test
/// look like. See [`crate::lexical`] for how extraction uses it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lexical {
    /// Line-comment opener (`//`, `#`); runs to end of line.
    #[serde(default)]
    pub line_comment: Option<String>,
    /// Block-comment open/close pair (`["/*", "*/"]`), non-nesting.
    #[serde(default)]
    pub block_comment: Option<(String, String)>,
    /// String-literal forms, tried in order at each position.
    #[serde(default)]
    pub strings: Vec<StringRule>,
    /// Bracket pairs that define nesting spans (e.g. `[["(", ")"], ["{", "}"]]`).
    pub nest: Vec<(String, String)>,
    /// What opens a nesting block.
    pub block: Opener,
    /// What declares a test.
    pub test: Opener,
}

/// Grammar section of a pack manifest.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrammarConfig {
    /// Grammar source.
    pub source: GrammarSource,
    /// Language symbol exported by a WASM grammar (`tree_sitter_<symbol>`).
    /// Defaults to the pack name.
    #[serde(default)]
    pub symbol: Option<String>,
}

/// How the source text of a `@*.name` capture maps to a plain title.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NameSyntax {
    /// The captured text is the name itself (identifiers).
    #[default]
    Raw,
    /// The capture is a JS string literal: quotes are stripped and escape
    /// sequences decoded, so titles compare by value, not by the escaping
    /// a scaffold (or author) happened to use.
    JsString,
}

/// Extraction section of a pack manifest.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Extract {
    /// Path (within the pack) of the tree-sitter query file. Required for
    /// grammar-backed packs; absent for `lexical` packs.
    #[serde(default)]
    pub query: Option<String>,
    /// If true, a `@test` capture only counts as a test when a
    /// `@test.marker` capture (e.g. a `#[test]` attribute) directly
    /// precedes it among its siblings.
    #[serde(default)]
    pub test_requires_marker: bool,
    /// How `@*.name` captures decode to titles.
    #[serde(default)]
    pub name_syntax: NameSyntax,
}

/// Scaffold section of a pack manifest.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Version of the declarative manifest schema, independent of the pack.
    pub format: u32,
    /// Identity.
    pub pack: PackMeta,
    /// Runtime compatibility declared by the pack.
    pub compat: Compatibility,
    /// File routing.
    pub detect: Detect,
    /// Grammar source.
    pub grammar: GrammarConfig,
    /// Extraction query configuration.
    pub extract: Extract,
    /// Node-text-to-identifier rules.
    #[serde(default)]
    pub mapping: Mapping,
    /// Lexical profile (required iff `grammar.source = "lexical"`).
    #[serde(default)]
    pub lexical: Option<Lexical>,
    /// Scaffold template configuration.
    pub scaffold: Scaffold,
}

/// Minimal first pass used to reject a future manifest format before its
/// fields are interpreted as the current strict schema.
#[derive(Deserialize)]
struct ManifestHeader {
    format: u32,
    pack: ManifestPackHeader,
}

#[derive(Deserialize)]
struct ManifestPackHeader {
    name: String,
}

/// Where a loaded pack was resolved from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Embedded in the btt binary.
    Builtin,
    /// A user-global pack directory: `$XDG_CONFIG_HOME/btt/packs/<name>`
    /// (default `~/.config/btt/packs/<name>`) or the legacy
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

/// A pack's wasm grammar module, content-hashed at construction.
///
/// The per-thread language cache keys on `(symbol, hash)` — module
/// *identity*, not name — so a library caller who validates two pack sets
/// separately can never be served one set's grammar under the other set's
/// symbol. Constructing the hash here means it cannot drift from the
/// bytes. (`DefaultHasher` is cache identity under the trusted-pack model,
/// not an adversarial digest.)
#[derive(Debug)]
pub struct WasmGrammar {
    bytes: Vec<u8>,
    hash: u64,
}

impl WasmGrammar {
    /// Wrap a compiled grammar module, computing its content hash.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        bytes.hash(&mut hasher);
        let hash = hasher.finish();
        Self { bytes, hash }
    }

    /// The module bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The content hash of the module bytes.
    #[must_use]
    pub fn hash(&self) -> u64 {
        self.hash
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
    /// The pack's WASM grammar, when it ships one.
    pub wasm_grammar: Option<WasmGrammar>,
    /// Where this pack was resolved from.
    pub origin: Origin,
}

impl Pack {
    /// The pack's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.manifest.pack.name
    }
}

/// Reject a pack-controlled path value that could escape the directory it
/// is joined onto: absolute paths and any `..`/`.`/prefix component. These
/// values come from `pack.toml`, and packs are only "reviewable data" if
/// the paths they name stay inside the pack (or, for target/output
/// patterns, the tree file's directory).
fn confine(pack: &str, field: &'static str, value: &str) -> Result<()> {
    let safe = !value.is_empty()
        && Path::new(value)
            .components()
            .all(|c| matches!(c, Component::Normal(_)));
    if safe {
        Ok(())
    } else {
        Err(Error::UnsafePath {
            pack: pack.to_string(),
            field,
            value: value.to_string(),
        })
    }
}

/// Validate every path-like manifest field before any of them is used,
/// plus cross-field consistency (a lexical pack needs its profile and
/// nothing grammar-shaped; a grammar pack needs its query).
fn validate(manifest: &Manifest) -> Result<()> {
    let pack = &manifest.pack.name;
    if manifest.format != PACK_FORMAT_VERSION {
        return Err(Error::UnsupportedPackFormat {
            pack: pack.clone(),
            format: manifest.format,
            supported: PACK_FORMAT_VERSION,
        });
    }
    let current =
        Version::parse(env!("CARGO_PKG_VERSION")).expect("Cargo version must be valid SemVer");
    if !manifest.compat.btt.matches(&current) {
        return Err(Error::IncompatibleBtt {
            pack: pack.clone(),
            version: manifest.pack.version.to_string(),
            requirement: manifest.compat.btt.to_string(),
            current: current.to_string(),
        });
    }
    let shape = |message: &str| Error::Manifest {
        pack: pack.clone(),
        message: message.to_string(),
    };
    if matches!(manifest.grammar.source, GrammarSource::Lexical) {
        let Some(lexical) = &manifest.lexical else {
            return Err(shape(
                "grammar source `lexical` requires a [lexical] section",
            ));
        };
        if manifest.extract.query.is_some() {
            return Err(shape("lexical packs must not set extract.query"));
        }
        if manifest.extract.test_requires_marker {
            return Err(shape(
                "extract.test_requires_marker is not supported by lexical packs",
            ));
        }
        // The full profile is checked here so a malformed pack fails its
        // load, not extraction mid-run (or worse: a hang on a degenerate
        // token).
        crate::lexical::validate_profile(lexical).map_err(|message| Error::Lexical {
            pack: pack.clone(),
            message,
        })?;
    } else {
        if manifest.lexical.is_some() {
            return Err(shape("[lexical] requires grammar source `lexical`"));
        }
        if manifest.extract.query.is_none() {
            return Err(shape("grammar-backed packs require extract.query"));
        }
    }
    if let Some(query) = &manifest.extract.query {
        confine(pack, "extract.query", query)?;
    }
    confine(pack, "scaffold.template", &manifest.scaffold.template)?;
    confine(pack, "scaffold.output", &manifest.scaffold.output)?;
    if let GrammarSource::Wasm(file) = &manifest.grammar.source {
        confine(pack, "grammar.source", &file.to_string_lossy())?;
    }
    for target in &manifest.detect.targets {
        confine(pack, "detect.targets", target)?;
        // Forward routing replaces every `{stem}`; reverse (uncovered)
        // matching splits on the first, against the file name only. A
        // pattern the two directions disagree on would corrupt coverage,
        // so only reversible patterns load: exactly one `{stem}`, no
        // directory components.
        let reversible = target.matches("{stem}").count() == 1
            && !target.contains('/')
            && !target.contains('\\');
        if !reversible {
            return Err(Error::InvalidTargetPattern {
                pack: pack.clone(),
                pattern: target.clone(),
            });
        }
    }
    Ok(())
}

/// Validate a set of packs that will run together: two packs must not ship
/// *different* wasm grammar modules under the same export symbol.
///
/// This runs once, before any parallel work, so the outcome is
/// deterministic — a per-thread check would report the collision or not
/// depending on which worker touched which pack first.
///
/// This is a diagnostic, not a safety mechanism: the per-thread language
/// cache keys on module content ([`WasmGrammar`]), so a colliding setup
/// that skips this check still parses each pack with its own grammar. The
/// pre-flight exists to name the confusing setup clearly instead.
///
/// # Errors
///
/// Returns [`Error::GrammarSymbolCollision`] naming both packs.
pub fn validate_set(packs: &[Pack]) -> Result<()> {
    let mut seen: BTreeMap<&str, (&Pack, &WasmGrammar)> = BTreeMap::new();
    for pack in packs {
        if !matches!(pack.manifest.grammar.source, GrammarSource::Wasm(_)) {
            continue;
        }
        let Some(grammar) = pack.wasm_grammar.as_ref() else {
            continue; // missing grammar file: surfaces per file at parse time
        };
        let symbol = pack
            .manifest
            .grammar
            .symbol
            .as_deref()
            .unwrap_or(pack.name());
        match seen.get(symbol) {
            Some((first, first_grammar)) if first_grammar.bytes() != grammar.bytes() => {
                return Err(Error::GrammarSymbolCollision {
                    symbol: symbol.to_string(),
                    first: first.name().to_string(),
                    second: pack.name().to_string(),
                });
            }
            Some(_) => {}
            None => {
                seen.insert(symbol, (pack, grammar));
            }
        }
    }
    Ok(())
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|source| Error::io(path, source))
}

fn parse_manifest(raw: &str, path: &Path) -> Result<Manifest> {
    let toml_error = |source| Error::Toml {
        path: path.to_path_buf(),
        source: Box::new(source),
    };
    let header: ManifestHeader = toml::from_str(raw).map_err(toml_error)?;
    if header.format != PACK_FORMAT_VERSION {
        return Err(Error::UnsupportedPackFormat {
            pack: header.pack.name,
            format: header.format,
            supported: PACK_FORMAT_VERSION,
        });
    }
    toml::from_str(raw).map_err(toml_error)
}

/// Resolve a pack-relative file to its real path, requiring it to still be
/// a regular file inside the pack directory after following symlinks.
/// `confine` checks the textual path; this closes the symlink hole.
fn resolve_inside(dir: &Path, rel: &Path, pack: &str, field: &'static str) -> Result<PathBuf> {
    let path = dir.join(rel);
    let canon = path
        .canonicalize()
        .map_err(|source| Error::io(&path, source))?;
    let dir_canon = dir
        .canonicalize()
        .map_err(|source| Error::io(dir, source))?;
    if canon.starts_with(&dir_canon) && canon.is_file() {
        Ok(canon)
    } else {
        Err(Error::UnsafePath {
            pack: pack.to_string(),
            field,
            value: rel.display().to_string(),
        })
    }
}

fn load_from_dir(dir: &Path, origin: Origin) -> Result<Pack> {
    // The manifest gets the same symlink confinement as the files it
    // names; its pack name is unknown until parsed, so errors carry the
    // directory name instead.
    let label = dir.file_name().map_or_else(
        || dir.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let manifest_path = resolve_inside(dir, Path::new("pack.toml"), &label, "pack.toml")?;
    let manifest = parse_manifest(&read(&manifest_path)?, &manifest_path)?;
    validate(&manifest)?;
    let name = manifest.pack.name.clone();
    let query = match &manifest.extract.query {
        Some(q) => read(&resolve_inside(dir, Path::new(q), &name, "extract.query")?)?,
        None => String::new(),
    };
    let template = read(&resolve_inside(
        dir,
        Path::new(&manifest.scaffold.template),
        &name,
        "scaffold.template",
    )?)?;
    let wasm_grammar = match &manifest.grammar.source {
        GrammarSource::Wasm(file) => match resolve_inside(dir, file, &name, "grammar.source") {
            Ok(real) => std::fs::read(real).ok().map(WasmGrammar::new),
            // An escaping symlink is a hard error; a merely missing or
            // unreadable grammar surfaces per file at parse time, so one
            // broken wasm pack can't abort a whole run.
            Err(e @ Error::UnsafePath { .. }) => return Err(e),
            Err(_) => None,
        },
        GrammarSource::Builtin(_) | GrammarSource::Lexical => None,
    };
    Ok(Pack {
        manifest,
        query,
        template,
        wasm_grammar,
        origin,
    })
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
            .ok_or_else(|| Error::PackFile {
                pack: pack_name.clone(),
                file: p.to_string(),
            })
    };
    let manifest_path = PathBuf::from(format!("<builtin:{pack_name}>/pack.toml"));
    let manifest = parse_manifest(file("pack.toml")?, &manifest_path)?;
    validate(&manifest)?;
    let query = match &manifest.extract.query {
        Some(q) => file(q)?.to_string(),
        None => String::new(),
    };
    let template = file(&manifest.scaffold.template)?.to_string();
    let wasm_grammar = match &manifest.grammar.source {
        GrammarSource::Wasm(file) => dir
            .get_file(dir.path().join(file))
            .map(|f| WasmGrammar::new(f.contents().to_vec())),
        GrammarSource::Builtin(_) | GrammarSource::Lexical => None,
    };
    Ok(Pack {
        manifest,
        query,
        template,
        wasm_grammar,
        origin: Origin::Builtin,
    })
}

/// Load a pack by name, honoring the resolution order.
///
/// # Errors
///
/// Returns [`Error::PackNotFound`] if no source provides the pack, or an
/// error describing the broken file if a pack exists but fails to load.
pub fn load(name: &str, project_root: &Path) -> Result<Pack> {
    // The name is joined into filesystem paths below: a single normal
    // path component only, so `load("../../x")` cannot walk anywhere.
    let mut components = Path::new(name).components();
    if !matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(_)), None)
    ) {
        return Err(Error::UnsafePath {
            pack: name.to_string(),
            field: "pack name",
            value: name.to_string(),
        });
    }
    let local = project_root.join(".btt/packs").join(name);
    if local.join("pack.toml").is_file() {
        return load_from_dir(&local, Origin::Project(local.clone()));
    }
    for user_dir in user_pack_dirs() {
        let global = user_dir.join(name);
        if global.join("pack.toml").is_file() {
            return load_from_dir(&global, Origin::User(global.clone()));
        }
    }
    if let Some(dir) = EMBEDDED_PACKS.get_dir(name) {
        return load_embedded(dir);
    }
    Err(Error::PackNotFound {
        name: name.to_string(),
        builtins: builtin_names(),
    })
}

/// Load an embedded pack directly, bypassing project/user resolution.
///
/// This is the unconfigured default: with no `packs = [...]` in
/// `btt.toml`, only the binary's own packs run — a repo cannot smuggle a
/// pack into an unconfigured run by reusing a builtin's name in
/// `.btt/packs/`. Shadowing a builtin is opted into by naming it.
///
/// # Errors
///
/// Returns [`Error::PackNotFound`] if the binary embeds no such pack.
pub fn load_builtin(name: &str) -> Result<Pack> {
    match EMBEDDED_PACKS.get_dir(name) {
        Some(dir) => load_embedded(dir),
        None => Err(Error::PackNotFound {
            name: name.to_string(),
            builtins: builtin_names(),
        }),
    }
}

/// Names of all packs visible from a project, with their winning origin,
/// sorted by name.
#[must_use]
pub fn available(project_root: &Path) -> Vec<(String, Origin)> {
    type OriginCtor = fn(PathBuf) -> Origin;
    let mut map: BTreeMap<String, Origin> = BTreeMap::new();
    let mut dirs: Vec<(PathBuf, OriginCtor)> = vec![(
        project_root.join(".btt/packs"),
        Origin::Project as OriginCtor,
    )];
    dirs.extend(
        user_pack_dirs()
            .into_iter()
            .map(|d| (d, Origin::User as OriginCtor)),
    );
    for (dir, make_origin) in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.join("pack.toml").is_file()
                && let Some(name) = path.file_name().and_then(|n| n.to_str())
            {
                map.entry(name.to_string())
                    .or_insert_with(|| make_origin(path.clone()));
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
        .filter_map(|d| {
            d.path()
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .collect()
}

/// User-global pack directories, in priority order.
fn user_pack_dirs() -> Vec<PathBuf> {
    user_pack_dirs_from(
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        std::env::home_dir(),
    )
}

/// Pure resolution of the user-global pack directories:
///   1. `$XDG_CONFIG_HOME/btt/packs`, defaulting to `~/.config/btt/packs`
///      when `XDG_CONFIG_HOME` is unset or (per the XDG Base Directory
///      spec) not an absolute path.
///   2. `~/.btt/packs` — the pre-XDG location, kept for existing installs.
fn user_pack_dirs_from(xdg_config_home: Option<PathBuf>, home: Option<PathBuf>) -> Vec<PathBuf> {
    let config = xdg_config_home
        .filter(|p| p.is_absolute())
        .or_else(|| home.as_deref().map(|h| h.join(".config")));
    let mut dirs = Vec::new();
    if let Some(config) = config {
        dirs.push(config.join("btt/packs"));
    }
    if let Some(home) = home {
        dirs.push(home.join(".btt/packs"));
    }
    dirs
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
        /// Content hash of `bytes` — the cache identity of the module.
        hash: u64,
    },
    /// No grammar: extraction runs the pack's lexical profile.
    Lexical(&'a Lexical),
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
            let grammar = pack.wasm_grammar.as_ref().ok_or_else(|| Error::PackFile {
                pack: pack.name().to_string(),
                file: file.display().to_string(),
            })?;
            let symbol = pack
                .manifest
                .grammar
                .symbol
                .as_deref()
                .unwrap_or(pack.name());
            Ok(Grammar::Wasm {
                symbol,
                bytes: grammar.bytes(),
                hash: grammar.hash(),
            })
        }
        GrammarSource::Lexical => {
            let lexical = pack
                .manifest
                .lexical
                .as_ref()
                .ok_or_else(|| Error::Manifest {
                    pack: pack.name().to_string(),
                    message: "grammar source `lexical` requires a [lexical] section".to_string(),
                })?;
            Ok(Grammar::Lexical(lexical))
        }
        GrammarSource::Builtin(name) => match name.as_str() {
            "rust" => Ok(Grammar::Native(tree_sitter_rust::LANGUAGE.into())),
            "typescript" => {
                let ext = target
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or_default();
                if ext == "tsx" || ext == "jsx" {
                    Ok(Grammar::Native(tree_sitter_typescript::LANGUAGE_TSX.into()))
                } else {
                    Ok(Grammar::Native(
                        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                    ))
                }
            }
            other => Err(Error::UnknownGrammar {
                pack: pack.name().to_string(),
                name: other.to_string(),
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod when_resolving_user_pack_dirs {
        use super::*;

        #[test]
        fn prefers_xdg_config_home_then_legacy() {
            let dirs =
                user_pack_dirs_from(Some(PathBuf::from("/xdg")), Some(PathBuf::from("/home/me")));
            assert_eq!(
                dirs,
                vec![
                    PathBuf::from("/xdg/btt/packs"),
                    PathBuf::from("/home/me/.btt/packs")
                ]
            );
        }

        #[test]
        fn defaults_to_dot_config_when_xdg_is_unset() {
            let dirs = user_pack_dirs_from(None, Some(PathBuf::from("/home/me")));
            assert_eq!(
                dirs,
                vec![
                    PathBuf::from("/home/me/.config/btt/packs"),
                    PathBuf::from("/home/me/.btt/packs")
                ]
            );
        }

        #[test]
        fn ignores_a_relative_xdg_config_home() {
            let dirs =
                user_pack_dirs_from(Some(PathBuf::from("rel")), Some(PathBuf::from("/home/me")));
            assert_eq!(
                dirs,
                vec![
                    PathBuf::from("/home/me/.config/btt/packs"),
                    PathBuf::from("/home/me/.btt/packs")
                ]
            );
        }

        #[test]
        fn uses_only_xdg_when_home_is_unknown() {
            let dirs = user_pack_dirs_from(Some(PathBuf::from("/xdg")), None);
            assert_eq!(dirs, vec![PathBuf::from("/xdg/btt/packs")]);
        }
    }
}
