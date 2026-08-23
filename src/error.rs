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
    #[error("pack `{name}` not found (searched .btt/packs, ~/.btt/packs, and builtins: {})", builtins.join(", "))]
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
    #[error("pack `{pack}`: wasm grammars need a btt built with the `wasm` feature (cargo install btt --features wasm)")]
    WasmUnsupported {
        /// The pack requesting the grammar.
        pack: String,
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
        Error::Io { path: path.into(), source }
    }
}
