//! Typed errors for the btt library.

use std::path::PathBuf;

/// Any error produced by the btt library.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A `.tree` file failed to parse.
    #[error("{}: {source}", path.display())]
    Parse {
        /// The `.tree` file that failed.
        path: PathBuf,
        /// The underlying parse error (with line information).
        #[source]
        source: crate::tree::ParseError,
    },

    /// A file could not be read or written.
    #[error("{}: {source}", path.display())]
    Io {
        /// The file involved.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// No pack with the requested name exists in any source.
    #[error("pack `{name}` not found (searched .btt/packs, $XDG_CONFIG_HOME/btt/packs, ~/.btt/packs, and builtins: {})", builtins.join(", "))]
    PackNotFound {
        /// The requested pack name.
        name: String,
        /// Names of the packs embedded in this binary.
        builtins: Vec<String>,
    },

    /// A pack manifest or project config failed to parse as TOML.
    #[error("parsing {}: {source}", path.display())]
    Toml {
        /// The TOML file that failed.
        path: PathBuf,
        /// The underlying TOML error.
        #[source]
        source: Box<toml::de::Error>,
    },

    /// A pack manifest referenced a path that could escape its directory.
    #[error("pack `{pack}`: {field} `{value}` must be a relative path with no `..` components")]
    UnsafePath {
        /// The pack with the unsafe reference.
        pack: String,
        /// Which manifest field held it.
        field: &'static str,
        /// The offending value.
        value: String,
    },

    /// A pack manifest is internally inconsistent (cross-field rules).
    #[error("pack `{pack}`: {message}")]
    Manifest {
        /// The pack with the inconsistent manifest.
        pack: String,
        /// What is inconsistent.
        message: String,
    },

    /// A pack uses a manifest format this btt does not understand.
    #[error("pack `{pack}` uses manifest format {format}; this btt supports format {supported}")]
    UnsupportedPackFormat {
        /// The pack with the unsupported manifest.
        pack: String,
        /// The format declared by the pack.
        format: u32,
        /// The format understood by this btt.
        supported: u32,
    },

    /// A pack is not compatible with the running btt version.
    #[error("pack `{pack}` v{version} requires btt {requirement}; running btt {current}")]
    IncompatibleBtt {
        /// The incompatible pack.
        pack: String,
        /// The pack's independently released version.
        version: String,
        /// The btt versions accepted by the pack.
        requirement: String,
        /// The running btt version.
        current: String,
    },

    /// Lexical extraction could not account for the source file.
    #[error("pack `{pack}`: lexical extraction: {message}")]
    Lexical {
        /// The pack whose profile was scanning.
        pack: String,
        /// What the scanner could not account for.
        message: String,
    },

    /// Safe scaffold merging could not determine an unambiguous insertion.
    #[error("cannot safely merge scaffold: {message}; merge manually")]
    Merge {
        /// Why editing between complete extracted spans was not possible.
        message: String,
    },

    /// A target pattern that forward and reverse routing would disagree on.
    #[error(
        "pack `{pack}`: target pattern `{pattern}` must contain exactly one {{stem}} and no directory separators"
    )]
    InvalidTargetPattern {
        /// The pack with the bad pattern.
        pack: String,
        /// The offending pattern.
        pattern: String,
    },

    /// A file referenced by a pack manifest is missing or not UTF-8.
    #[error("pack `{pack}`: missing or invalid file `{file}`")]
    PackFile {
        /// The pack with the broken reference.
        pack: String,
        /// The referenced file, relative to the pack root.
        file: String,
    },

    /// A pack references a builtin grammar this binary does not ship.
    #[error("pack `{pack}`: unknown builtin grammar `{name}`")]
    UnknownGrammar {
        /// The pack with the bad reference.
        pack: String,
        /// The unknown grammar name.
        name: String,
    },

    /// A pack requests a `wasm:` grammar but this binary was built without
    /// the `wasm` feature.
    #[error(
        "pack `{pack}`: wasm grammars need a btt built with the `wasm` feature (cargo install btt-cli --locked --features wasm)"
    )]
    WasmUnsupported {
        /// The pack requesting the grammar.
        pack: String,
    },

    /// Two packs ship different wasm grammar modules under one symbol.
    #[error(
        "packs `{first}` and `{second}` both export `tree_sitter_{symbol}` with different grammars; give one a distinct symbol"
    )]
    GrammarSymbolCollision {
        /// The contested export symbol.
        symbol: String,
        /// The pack that claimed the symbol first (load order).
        first: String,
        /// The pack that collided with it.
        second: String,
    },

    /// Loading or instantiating a pack's WASM grammar failed.
    #[error("pack `{pack}`: wasm grammar error: {message}")]
    WasmGrammar {
        /// The pack owning the grammar.
        pack: String,
        /// The underlying wasm engine error, stringified.
        message: String,
    },

    /// The grammar rejected by the tree-sitter runtime (version mismatch).
    #[error(transparent)]
    Language(#[from] tree_sitter::LanguageError),

    /// A pack's tree-sitter query failed to compile.
    #[error("pack `{pack}`: invalid query: {source}")]
    Query {
        /// The pack owning the query.
        pack: String,
        /// The underlying query error.
        #[source]
        source: Box<tree_sitter::QueryError>,
    },

    /// A pack's query does not define the required captures.
    #[error("pack `{pack}`: query must define @block, @block.name, @test, @test.name")]
    MissingCaptures {
        /// The pack owning the query.
        pack: String,
    },

    /// `test_requires_marker` is set but the query has no `@test.marker`.
    #[error("pack `{pack}`: test_requires_marker is set but the query has no @test.marker")]
    MissingMarkerCapture {
        /// The pack owning the query.
        pack: String,
    },

    /// tree-sitter could not produce a syntax tree for a source file.
    #[error("failed to parse {}", path.display())]
    SourceParse {
        /// The source file that failed.
        path: PathBuf,
    },

    /// A pack's scaffold template failed to compile or render.
    #[error("pack `{pack}`: template error: {source}")]
    Template {
        /// The pack owning the template.
        pack: String,
        /// The underlying template error.
        #[source]
        source: Box<minijinja::Error>,
    },
}

/// Convenience alias used throughout the library.
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }
}
