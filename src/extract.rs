//! Extract the actual test structure from a source file using a pack's
//! tree-sitter query.
//!
//! The query uses a small capture vocabulary the core understands:
//!
//! - `@block` / `@block.name` — a nesting construct (a Rust `mod`, a vitest
//!   `describe(...)`) and the node holding its name.
//! - `@test` / `@test.name` — a test definition and the node holding its name.
//! - `@test.marker` — optional; a node (e.g. a `#[test]` attribute) that must
//!   directly precede a `@test` among its siblings for it to count, when the
//!   pack sets `extract.test_requires_marker = true`.
//! - `@unsupported` — a construct the pack recognizes but cannot represent
//!   as a named block or test. Only its source line is retained.
//!
//! Nesting is derived structurally: a captured node's parent is the smallest
//! captured `@block` that contains it. This works for both block-based
//! languages (describe callbacks) and item-based ones (mods) without any
//! language-specific logic in the core.

use crate::error::{Error, Result};
use crate::pack::{self, NameSyntax, Pack};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

/// Whether an extracted node is a nesting block or a test definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActualKind {
    /// A nesting construct (`mod`, `describe`).
    Block,
    /// A test definition (`#[test] fn`, `it(...)`).
    Test,
}

/// One node of the structure actually present in a test file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActualNode {
    /// Block or test.
    pub kind: ActualKind,
    /// The identifier / title as written in the source.
    pub name: String,
    /// 1-based line of the definition.
    pub line: usize,
    /// Nested nodes (always empty for tests).
    pub children: Vec<ActualNode>,
}

/// A recognized source construct that cannot be represented in a tree spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsupported {
    /// 1-based line of the construct.
    pub line: usize,
}

/// Byte span of an extracted test definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestSpan {
    /// 1-based line where the test definition begins.
    pub line: usize,
    /// Inclusive start byte of the test definition.
    pub start: usize,
    /// Exclusive end byte of the test definition.
    pub end: usize,
}

/// Structural nodes and non-structural diagnostics produced in one pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extraction {
    /// Blocks and tests that can be compared with a tree spec.
    pub nodes: Vec<ActualNode>,
    /// Recognized constructs that cannot be represented.
    pub unsupported: Vec<Unsupported>,
    /// Source spans of every extracted test.
    pub test_spans: Vec<TestSpan>,
}

impl ActualNode {
    /// Drop blocks that contain no tests anywhere below them (helper mods,
    /// non-test describes) so they don't show up as noise in diffs.
    #[must_use]
    pub fn prune_empty_blocks(nodes: Vec<ActualNode>) -> Vec<ActualNode> {
        nodes
            .into_iter()
            .filter_map(|mut n| match n.kind {
                ActualKind::Test => Some(n),
                ActualKind::Block => {
                    n.children = Self::prune_empty_blocks(n.children);
                    if n.children.is_empty() { None } else { Some(n) }
                }
            })
            .collect()
    }
}

/// A captured definition, before nesting is derived. Shared with the
/// lexical backend so both build the same forest the same way.
#[derive(Debug)]
pub(crate) struct Capture {
    pub(crate) kind: ActualKind,
    pub(crate) name: String,
    pub(crate) line: usize,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

/// Parse `source` with the pack's grammar and return the top-level actual
/// nodes (already pruned of test-free blocks).
///
/// # Errors
///
/// Fails if the pack's grammar is unavailable, its query does not compile or
/// lacks the required captures, or tree-sitter cannot parse the source.
pub fn extract(pack: &Pack, target: &Path, source: &str) -> Result<Vec<ActualNode>> {
    extract_with_findings(pack, target, source).map(|result| result.nodes)
}

/// Parse `source`, returning both its structure and unsupported constructs.
///
/// # Errors
///
/// Fails under the same conditions as [`extract`].
pub fn extract_with_findings(pack: &Pack, target: &Path, source: &str) -> Result<Extraction> {
    if let pack::Grammar::Lexical(cfg) = pack::grammar_for(pack, target)? {
        return crate::lexical::extract(pack, cfg, source);
    }
    let (tree, language) = parse_source(pack, target, source)?;
    let empty = AliasSet::default();
    let initial_text = empty.substitute(&pack.query);
    let initial = run_query(pack, &language, &tree, source, &initial_text)?;
    let aliases = AliasSet::resolve(&initial.candidates);
    let mut pass = if aliases.is_empty() {
        initial
    } else {
        // Candidate collection is static, and transitive resolution happens
        // entirely in memory. Thus alias support costs at most one extra
        // query compilation per file, regardless of chain length.
        run_query(
            pack,
            &language,
            &tree,
            source,
            &aliases.substitute(&pack.query),
        )?
    };
    Ok(finish_extraction(pack, tree.root_node(), &mut pass))
}

#[derive(Debug, Default)]
pub(crate) struct AliasSet {
    blocks: BTreeSet<String>,
    tests: BTreeSet<String>,
}

#[derive(Debug)]
pub(crate) struct AliasCandidate {
    pub(crate) name: String,
    pub(crate) value: String,
}

impl AliasSet {
    pub(crate) fn substitute(&self, pattern: &str) -> String {
        pattern
            .replace("{{block_aliases}}", &Self::alternation(&self.blocks))
            .replace("{{test_aliases}}", &Self::alternation(&self.tests))
            .replace(
                "{{each_aliases}}",
                &Self::alternation(&self.each_receivers()),
            )
    }

    fn alternation(names: &BTreeSet<String>) -> String {
        if names.is_empty() {
            // A contradictory pair of anchors, valid in both tree-sitter's
            // regex predicates and Rust regex, and impossible for an identifier.
            return r"$^".to_string();
        }
        names
            .iter()
            .map(|name| regex::escape(name))
            .collect::<Vec<_>>()
            .join("|")
    }

    fn each_receivers(&self) -> BTreeSet<String> {
        self.blocks.union(&self.tests).cloned().collect()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.blocks.is_empty() && self.tests.is_empty()
    }

    pub(crate) fn resolve(candidates: &[AliasCandidate]) -> Self {
        let by_name: HashMap<&str, &str> = candidates
            .iter()
            .map(|candidate| (candidate.name.as_str(), candidate.value.as_str()))
            .collect();
        let mut out = Self::default();
        let mut resolved = HashMap::new();
        for candidate in candidates {
            let mut visiting = BTreeSet::new();
            match resolve_name(
                candidate.name.as_str(),
                &by_name,
                &mut resolved,
                &mut visiting,
                MAX_ALIAS_DEPTH,
            ) {
                Some(ActualKind::Block) => {
                    out.blocks.insert(candidate.name.clone());
                }
                Some(ActualKind::Test) => {
                    out.tests.insert(candidate.name.clone());
                }
                None => {}
            }
        }
        out
    }
}

/// Fail closed before source-controlled alias graphs can consume the process
/// stack. Ordinary chains are memoized as candidates are visited; adversarial
/// forward-reference chains deeper than this are simply not treated as aliases.
const MAX_ALIAS_DEPTH: usize = 512;

fn resolve_name<'a>(
    name: &'a str,
    candidates: &HashMap<&'a str, &'a str>,
    resolved: &mut HashMap<&'a str, Option<ActualKind>>,
    visiting: &mut BTreeSet<String>,
    remaining: usize,
) -> Option<ActualKind> {
    if let Some(kind) = resolved.get(name) {
        return *kind;
    }
    if !visiting.insert(name.to_string()) {
        return None;
    }
    if remaining == 0 {
        visiting.remove(name);
        return None;
    }
    let kind = candidates
        .get(name)
        .and_then(|value| resolve_value(value, candidates, resolved, visiting, remaining - 1));
    visiting.remove(name);
    resolved.insert(name, kind);
    kind
}

fn resolve_value<'a>(
    value: &str,
    candidates: &HashMap<&'a str, &'a str>,
    resolved: &mut HashMap<&'a str, Option<ActualKind>>,
    visiting: &mut BTreeSet<String>,
    remaining: usize,
) -> Option<ActualKind> {
    let compact: String = value.chars().filter(|c| !c.is_whitespace()).collect();
    if let Some((_, arms)) = compact.split_once('?') {
        let (left, right) = arms.split_once(':')?;
        if remaining == 0 {
            return None;
        }
        let a = resolve_value(left, candidates, resolved, visiting, remaining - 1)?;
        return (resolve_value(right, candidates, resolved, visiting, remaining - 1)? == a)
            .then_some(a);
    }
    let root = compact.split('.').next()?;
    match root {
        "describe" | "suite" => Some(ActualKind::Block),
        "it" | "test" if compact.ends_with(".describe") => Some(ActualKind::Block),
        "it" | "test" => Some(ActualKind::Test),
        alias => candidates
            .get_key_value(alias)
            .and_then(|(&known, _)| resolve_name(known, candidates, resolved, visiting, remaining)),
    }
}

struct QueryPass {
    captures: Vec<Capture>,
    markers: Vec<(usize, usize)>,
    unsupported: Vec<Unsupported>,
    candidates: Vec<AliasCandidate>,
}

fn run_query(
    pack: &Pack,
    language: &tree_sitter::Language,
    tree: &tree_sitter::Tree,
    source: &str,
    query_text: &str,
) -> Result<QueryPass> {
    let query = Query::new(language, query_text).map_err(|source| Error::Query {
        pack: pack.name().to_string(),
        source: Box::new(source),
    })?;

    let idx_of = |cap: &str| query.capture_index_for_name(cap);
    let (Some(block_i), Some(block_name_i), Some(test_i), Some(test_name_i)) = (
        idx_of("block"),
        idx_of("block.name"),
        idx_of("test"),
        idx_of("test.name"),
    ) else {
        return Err(Error::MissingCaptures {
            pack: pack.name().to_string(),
        });
    };
    let marker_i = idx_of("test.marker");
    let unsupported_i = idx_of("unsupported");
    let alias_name_i = idx_of("alias.name");
    let alias_value_i = idx_of("alias.value");
    if pack.manifest.extract.test_requires_marker && marker_i.is_none() {
        return Err(Error::MissingMarkerCapture {
            pack: pack.name().to_string(),
        });
    }

    let mut captures: Vec<Capture> = Vec::new();
    let mut marker_ranges: Vec<(usize, usize)> = Vec::new(); // (start, end) bytes
    let mut unsupported = Vec::new();
    let mut candidates = Vec::new();
    let syntax = pack.manifest.extract.name_syntax;

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
    while let Some(m) = matches.next() {
        let node_for = |i: u32| m.captures.iter().find(|c| c.index == i).map(|c| c.node);
        if let Some(n) = node_for(block_i)
            && let Some(name) = node_for(block_name_i)
        {
            captures.push(capture(ActualKind::Block, n, name, source, syntax));
        }
        if let Some(n) = node_for(test_i)
            && let Some(name) = node_for(test_name_i)
        {
            captures.push(capture(ActualKind::Test, n, name, source, syntax));
        }
        if let Some(i) = marker_i
            && let Some(n) = node_for(i)
        {
            marker_ranges.push((n.start_byte(), n.end_byte()));
        }
        if let Some(i) = unsupported_i
            && let Some(n) = node_for(i)
        {
            unsupported.push(Unsupported {
                line: n.start_position().row + 1,
            });
        }
        if let (Some(name_i), Some(value_i)) = (alias_name_i, alias_value_i)
            && let (Some(name), Some(value)) = (node_for(name_i), node_for(value_i))
            && is_module_alias(name)
        {
            candidates.push(AliasCandidate {
                name: source[name.byte_range()].to_string(),
                value: source[value.byte_range()].to_string(),
            });
        }
    }

    Ok(QueryPass {
        captures,
        markers: marker_ranges,
        unsupported,
        candidates,
    })
}

fn is_module_alias(name: Node<'_>) -> bool {
    let Some(declaration) = name.parent().and_then(|declarator| declarator.parent()) else {
        return false;
    };
    if !matches!(
        declaration.kind(),
        "lexical_declaration" | "variable_declaration"
    ) {
        return false;
    }
    declaration.parent().is_some_and(|parent| {
        parent.kind() == "program"
            || (parent.kind() == "export_statement"
                && parent
                    .parent()
                    .is_some_and(|grandparent| grandparent.kind() == "program"))
    })
}

fn finish_extraction(pack: &Pack, root: Node<'_>, pass: &mut QueryPass) -> Extraction {
    let captures = &mut pass.captures;

    // Sort into pre-order (parents before children) and deduplicate: a node
    // can appear in multiple query matches.
    captures.sort_by_key(|c| (c.start, std::cmp::Reverse(c.end)));
    captures.dedup_by_key(|c| (c.start, c.end, c.kind));

    if pack.manifest.extract.test_requires_marker {
        captures.retain(|c| c.kind != ActualKind::Test || has_marker(root, c, &pass.markers));
    }

    let test_spans = captures
        .iter()
        .filter(|capture| capture.kind == ActualKind::Test)
        .map(|capture| TestSpan {
            line: capture.line,
            start: capture.start,
            end: capture.end,
        })
        .collect();

    // In pre-order, a block's descendants are exactly the following captures
    // whose start lies before the block's end, so one cursor pass builds the
    // whole forest.
    let mut i = 0;
    let top = build_nodes(captures, &mut i, usize::MAX);
    pass.unsupported.sort_by_key(|finding| finding.line);
    Extraction {
        nodes: ActualNode::prune_empty_blocks(top),
        unsupported: std::mem::take(&mut pass.unsupported),
        test_spans,
    }
}

/// Parse a source file with the pack's grammar, returning the syntax tree
/// and the language (needed to compile the pack's query).
fn parse_source(
    pack: &Pack,
    target: &Path,
    source: &str,
) -> Result<(tree_sitter::Tree, tree_sitter::Language)> {
    match pack::grammar_for(pack, target)? {
        pack::Grammar::Native(language) => {
            let mut parser = Parser::new();
            parser.set_language(&language)?;
            let tree = parser
                .parse(source, None)
                .ok_or_else(|| Error::SourceParse {
                    path: target.to_path_buf(),
                })?;
            Ok((tree, language))
        }
        pack::Grammar::Wasm {
            symbol,
            bytes,
            hash,
        } => wasm::parse(pack, symbol, bytes, hash, target, source),
        // Dispatched before parse_source is reached (`extract`).
        pack::Grammar::Lexical(_) => unreachable!("lexical packs do not parse"),
    }
}

/// Sandboxed grammar support (Zed-style): grammars are instantiated in a
/// wasmtime store with no WASI, so they can only compute over the bytes we
/// feed them. Each thread keeps one parser whose store persists across
/// files, plus a language cache — so a grammar is compiled once per thread,
/// and every subsequent file on that thread pays only `set_language` +
/// parse. The wasmtime engine is shared process-wide.
#[cfg(feature = "wasm")]
mod wasm {
    use super::{Error, Pack, Parser, Result};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::OnceLock;
    use tree_sitter::wasmtime::Engine;
    use tree_sitter::{Language, Tree, WasmStore};

    struct ThreadState {
        parser: Parser,
        /// Compiled languages keyed by (export symbol, module content
        /// hash) — module *identity*, not name. Library callers validate
        /// pack sets independently, and nothing stops a later set from
        /// reusing an earlier set's symbol for a different module; content
        /// keying makes a cache hit correct for any caller in any order.
        /// (`pack::validate_set` remains as the CLI's friendly pre-flight
        /// diagnostic, not what makes this safe.)
        languages: HashMap<(String, u64), Language>,
    }

    thread_local! {
        static STATE: RefCell<ThreadState> = RefCell::new(ThreadState {
            parser: Parser::new(),
            languages: HashMap::new(),
        });
    }

    fn engine() -> &'static Engine {
        static ENGINE: OnceLock<Engine> = OnceLock::new();
        ENGINE.get_or_init(Engine::default)
    }

    pub fn parse(
        pack: &Pack,
        symbol: &str,
        bytes: &[u8],
        hash: u64,
        target: &Path,
        source: &str,
    ) -> Result<(Tree, Language)> {
        let err = |e: String| Error::WasmGrammar {
            pack: pack.name().to_string(),
            message: e,
        };
        let key = (symbol.to_string(), hash);
        STATE.with_borrow_mut(|state| {
            if !state.languages.contains_key(&key) {
                // Loading needs the store back from the parser (or a
                // fresh one on this thread's first wasm parse).
                let mut store = match state.parser.take_wasm_store() {
                    Some(store) => store,
                    None => WasmStore::new(engine()).map_err(|e| err(e.to_string()))?,
                };
                let loaded = store.load_language(symbol, bytes);
                // The store goes back into the parser even when loading
                // fails; dropping it here would orphan every language
                // already compiled on this thread.
                state
                    .parser
                    .set_wasm_store(store)
                    .map_err(|e| err(e.to_string()))?;
                let language = loaded.map_err(|e| err(e.to_string()))?;
                state.languages.insert(key.clone(), language);
            }
            let language = state.languages[&key].clone();
            state.parser.set_language(&language)?;
            let tree = state
                .parser
                .parse(source, None)
                .ok_or_else(|| Error::SourceParse {
                    path: target.to_path_buf(),
                })?;
            Ok((tree, language))
        })
    }
}

/// Stub with `wasm::parse`'s signature, so `parse_source` needs no
/// feature-conditional code: without the `wasm` feature, wasm packs fail
/// with an actionable per-file error.
#[cfg(not(feature = "wasm"))]
mod wasm {
    use super::{Error, Pack, Result};
    use std::path::Path;

    pub fn parse(
        pack: &Pack,
        _symbol: &str,
        _bytes: &[u8],
        _hash: u64,
        _target: &Path,
        _source: &str,
    ) -> Result<(tree_sitter::Tree, tree_sitter::Language)> {
        Err(Error::WasmUnsupported {
            pack: pack.name().to_string(),
        })
    }
}

pub(crate) fn build_nodes(
    captures: &[Capture],
    i: &mut usize,
    parent_end: usize,
) -> Vec<ActualNode> {
    let mut out = Vec::new();
    while *i < captures.len() && captures[*i].start < parent_end {
        let c = &captures[*i];
        *i += 1;
        let children = match c.kind {
            ActualKind::Block => build_nodes(captures, i, c.end),
            ActualKind::Test => Vec::new(),
        };
        out.push(ActualNode {
            kind: c.kind,
            name: c.name.clone(),
            line: c.line,
            children,
        });
    }
    out
}

fn capture(
    kind: ActualKind,
    node: Node,
    name_node: Node,
    source: &str,
    syntax: NameSyntax,
) -> Capture {
    let name = decode_name(syntax, &source[name_node.byte_range()]);
    Capture {
        kind,
        name,
        line: node.start_position().row + 1,
        start: node.start_byte(),
        end: node.end_byte(),
    }
}

/// Decode a captured name's source text to a plain title, per the pack's
/// declared syntax. Shared with the lexical backend.
pub(crate) fn decode_name(syntax: NameSyntax, raw: &str) -> String {
    match syntax {
        NameSyntax::Raw => raw.to_string(),
        NameSyntax::JsString => decode_js_string(raw),
    }
}

/// Decode a JS string literal's source text (`"a \"b\""`) to its value.
/// Titles are compared against `.tree` text and templates escape when
/// scaffolding, so extraction must see the string's *value* — otherwise a
/// scaffolded quote round-trips into a spurious mismatch.
fn decode_js_string(text: &str) -> String {
    let inner = match text.chars().next() {
        Some(q @ ('"' | '\'' | '`')) if text.len() >= 2 && text.ends_with(q) => {
            &text[1..text.len() - 1]
        }
        _ => text,
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some('b') => out.push('\u{0008}'),
            Some('f') => out.push('\u{000c}'),
            Some('v') => out.push('\u{000b}'),
            Some('x') => push_js_hex(&mut out, &mut chars),
            Some('u') => push_js_unicode(&mut out, &mut chars),
            // JS: `\q` is just `q` — this covers \" \' \` \\ too.
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// Decode the `NN` of a `\xNN` escape; malformed escapes stay literal.
fn push_js_hex(out: &mut String, chars: &mut std::str::Chars) {
    if let Some(hex) = chars.as_str().get(..2)
        && let Ok(code) = u8::from_str_radix(hex, 16)
    {
        out.push(char::from(code));
        chars.nth(1);
    } else {
        out.push_str("\\x");
    }
}

/// Decode a `\u{...}` or `\uNNNN` escape; malformed escapes stay literal.
fn push_js_unicode(out: &mut String, chars: &mut std::str::Chars) {
    let rest = chars.as_str();
    if let Some(body) = rest.strip_prefix('{') {
        if let Some((hex, _)) = body.split_once('}')
            && let Some(c) = u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
        {
            out.push(c);
            chars.nth(hex.len() + 1); // consume `{`, the digits, and `}`
            return;
        }
    } else if let Some(hex) = rest.get(..4)
        && let Some(c) = u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
    {
        out.push(c);
        chars.nth(3);
        return;
    }
    out.push_str("\\u");
}

/// A test has a marker if, walking backwards through its previous named
/// siblings, we hit a marker range before hitting any non-marker,
/// non-comment, non-attribute node.
fn has_marker(root: Node, test: &Capture, marker_ranges: &[(usize, usize)]) -> bool {
    let Some(mut node) = root.descendant_for_byte_range(test.start, test.end) else {
        return false;
    };
    // descendant_for_byte_range returns the smallest spanning node; walk up
    // to the outermost node with this exact range (the captured definition),
    // whose siblings are where markers live.
    while let Some(p) = node.parent() {
        if p.start_byte() == test.start && p.end_byte() == test.end {
            node = p;
        } else {
            break;
        }
    }
    let mut prev = node.prev_named_sibling();
    while let Some(p) = prev {
        if marker_ranges.contains(&(p.start_byte(), p.end_byte())) {
            return true;
        }
        let kind = p.kind();
        if !kind.contains("comment") && !kind.contains("attribute") {
            return false;
        }
        prev = p.prev_named_sibling();
    }
    false
}
