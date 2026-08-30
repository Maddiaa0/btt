# Authoring an extension pack

btt learns a new language or test convention through a **pack**: a folder of
declarative data — a manifest, an extraction rule, and a scaffold template.
There is no plugin API and (for lexical packs) no compiled code; you write
three small text files, name the pack in `btt.toml`, and btt speaks your
language without a rebuild.

This guide walks from an unsupported test convention to a working pack,
using a real running example: a pack for **Lua** tests written with
[busted](https://lunarmodules.github.io/busted/) (`describe`/`it` blocks in
`*_spec.lua` files). Every command and output shown was run against the
current CLI.

The shipped packs are complete reference implementations — link targets
throughout this guide:

- [`packs/rust`](../packs/rust) — grammar-backed, marker-gated tests,
  identifier names ([`pack.toml`](../packs/rust/pack.toml),
  [`queries/tests.scm`](../packs/rust/queries/tests.scm),
  [`templates/test.jinja`](../packs/rust/templates/test.jinja))
- [`packs/typescript`](../packs/typescript) — grammar-backed, string-literal
  titles, block-rooted mapping
- [`packs-lexical/`](../packs-lexical) — the same two languages defined
  purely lexically, no grammar at all
- [`packs-wasm/`](../packs-wasm) — the same two languages via sandboxed
  WASM grammar modules

Core code, if you want to see exactly what consumes each field:
[`src/pack.rs`](../src/pack.rs) (manifest model, validation, resolution),
[`src/extract.rs`](../src/extract.rs) (tree-sitter extraction),
[`src/lexical.rs`](../src/lexical.rs) (lexical extraction),
[`src/mapping.rs`](../src/mapping.rs) (name rules),
[`src/scaffold.rs`](../src/scaffold.rs) (template rendering),
[`src/runner.rs`](../src/runner.rs) (routing and the uncovered scan).

## 1. Choose the language and grammar source

First identify the test convention you are targeting:

- **File extensions and test-file names.** busted convention: a spec for
  `stack.lua` lives in `stack_spec.lua`. This becomes `[detect].targets`.
- **What a block and a test look like.** busted: `describe("title",
  function() … end)` nests, `it("title", function() … end)` is a leaf.
- **Whether names are identifiers or string titles.** busted titles are
  string literals, like vitest — so titles map verbatim and compare by
  string *value*. Rust-style conventions use identifiers (`fn
  returns_none`) and need a case transform instead.

Then pick how btt will parse the files — the `[grammar]` source. Three
kinds exist, and the choice shapes the rest of the pack:

| source | what it is | when to use |
|---|---|---|
| `builtin:<name>` | a tree-sitter grammar compiled into the btt binary — only `builtin:rust` and `builtin:typescript` exist | a new *convention* for a language btt already parses (e.g. a mocha pack, a different Rust layout) |
| `lexical` | no grammar: a one-screen declarative profile (comment/string syntax, nesting brackets, two opener regexes) | the default for a new language — zero code, fully reviewable, installable in a minute |
| `wasm:<file>` | a full tree-sitter grammar compiled to WASM, shipped inside the pack and run sandboxed | when the lexical profile can't see the syntax you need (indentation nesting, attribute markers, string interpolation) |

Two important boundaries:

- A genuinely **new language** never requires core changes: use `lexical`,
  or `wasm:` for full fidelity. Only *builtin* grammars require support
  compiled into btt.
- `wasm:` packs need a btt built with the `wasm` feature
  (`cargo install --path . --features wasm`); on a default build they fail
  per file with ``wasm grammars need a btt built with the `wasm`
  feature``. Grammar modules are executable code — pin and review them like
  any dependency (see the trust model notes at the top of
  [`src/pack.rs`](../src/pack.rs), and
  [`scripts/fetch-wasm-grammars.sh`](../scripts/fetch-wasm-grammars.sh)
  for tag + sha256 pinning).

The lexical backend is deliberately not a lexer-generator: it models
call-pattern tests (`it("title", …)`) and declaration-pattern tests
(`mod when_x {`, `function test_y(`) in bracket-nested languages. Syntax it
cannot see — Python's indentation nesting, Ruby's `do … end`, JS template
interpolation — is out of scope by design, and files it cannot fully
account for are hard errors, never silent partial extractions. When a
language outgrows the profile, graduate to `wasm:`.

Lua is bracket-nested (a `describe` body sits inside the call's own
parentheses), so the lexical route works — that's what we'll build.

## 2. Create the pack layout

A pack is one directory, named after the pack:

```text
.btt/packs/lua/
  pack.toml              # manifest — the only required file name
  templates/test.jinja   # scaffold template (any path; named by the manifest)
  queries/tests.scm      # tree-sitter query — grammar-backed packs only
  grammar.wasm           # wasm: packs only
```

Where to put it decides who gets it. Resolution order for a pack named
`lua` (first hit wins — see `load` in [`src/pack.rs`](../src/pack.rs)):

1. `<project>/.btt/packs/lua/` — **project-local**: vendored with the repo,
   versioned, shared with CI and every contributor. Use this for packs a
   project depends on.
2. `$XDG_CONFIG_HOME/btt/packs/lua/` (default `~/.config/btt/packs/lua/`)
   — **user-global**: your personal packs, available in every project.
3. `~/.btt/packs/lua/` — user-global, legacy location.
4. Packs embedded in the binary (`rust`, `typescript`).

A higher entry *shadows* a lower one with the same name, so a project can
override a builtin by vendoring a modified copy under the same name.

Presence is never activation: a pack only runs when `btt.toml` names it in
`[project].packs`. With no configured list, only the builtins load, and btt
prints a note about any pack it is ignoring.

## 3. Write `pack.toml`

The complete manifest for the Lua pack — every section explained below:

```toml
[pack]
name = "lua"
version = "0.1.0"
description = "Lua tests with busted: describe blocks for branches, it for leaves"

[detect]
targets = ["{stem}_spec.lua"]

[grammar]
source = "lexical"

[extract]
name_syntax = "js-string"

[lexical]
line_comment = "--"
strings = [
  { delim = '"', escape = '\' },
  { delim = "'", escape = '\' },
]
nest = [["(", ")"], ["{", "}"]]

[lexical.block]
open = '''(?:^|[^\w.])(?<kw>describe)\s*\(\s*(?<name>"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')\s*,'''

[lexical.test]
open = '''(?:^|[^\w.])(?<kw>it)\s*\(\s*(?<name>"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')\s*,'''

[mapping]
root = "block"

[mapping.block]
case = "verbatim"

[mapping.test]
strip_prefix = "it "
case = "verbatim"

[scaffold]
template = "templates/test.jinja"
output = "{stem}_spec.lua"
indent = "  "
```

Manifests parse **strictly**: an unknown field or section anywhere is a
load error, so typos surface immediately instead of being silently
ignored. Every path a manifest names is confined to the pack directory
(no absolute paths, no `..`, symlinks may not escape).

### `[pack]` — identity

| field | | |
|---|---|---|
| `name` | required | pack name; must match the directory name for resolution to find it |
| `version` | required | shown by `btt packs` |
| `description` | optional | one line, shown by `btt packs` |

### `[detect]` — routing

`targets` is a list of candidate test-file names for a tree file, tried in
order, where `{stem}` is the tree file's stem (`stack.tree` → `stack`).
For `src/stack.tree`, the pattern `{stem}_spec.lua` makes btt look for
`src/stack_spec.lua` — candidates are always siblings of the `.tree` file.
The first pattern (across packs, in `[project].packs` order) that names an
existing file wins.

These patterns are the single source of routing truth in *both*
directions: forward (tree → test file) and reverse — the uncovered scan
matches every file name against them to find test-bearing files that no
tree covers. That is why each pattern must contain exactly one `{stem}`
and no directory separators; anything else is rejected at load with
`target pattern … must contain exactly one {stem} and no directory
separators`.

### `[grammar]` — parsing

| field | | |
|---|---|---|
| `source` | required | `builtin:<name>`, `wasm:<file>`, or `lexical` |
| `symbol` | optional, `wasm:` only | the language symbol the module exports (`tree_sitter_<symbol>`); defaults to the pack name |

Notes per source:

- `builtin:rust`, `builtin:typescript` are the only builtins. The
  typescript grammar switches to its TSX variant for `.tsx`/`.jsx`
  targets automatically. Any other `builtin:` name fails at check time
  with `unknown builtin grammar`.
- `wasm:` names a `tree-sitter build --wasm` module inside the pack
  (e.g. `wasm:grammar.wasm`). Modules run without WASI — no filesystem or
  network — and two active packs may not export the same symbol with
  different grammar bytes.
- `lexical` requires a `[lexical]` section (§4) and forbids
  `extract.query` and `extract.test_requires_marker`.

### `[extract]` — what counts as a test

| field | | |
|---|---|---|
| `query` | required for grammar-backed packs, forbidden for lexical | pack-relative path of the tree-sitter query (§5) |
| `test_requires_marker` | default `false` | when `true`, a `@test` capture only counts if a `@test.marker` capture directly precedes it among its siblings (Rust's `#[test]`); grammar-backed packs only |
| `name_syntax` | default `"raw"` | how captured names decode to titles: `"raw"` (the text *is* the name — identifiers) or `"js-string"` (the capture is a JS-style string literal: quotes stripped, escapes decoded, so titles compare by value, not by whatever escaping the file happens to use) |

### Check severities

The `[check]` table controls whether non-missing findings fail, warn, or are
hidden. Missing expected nodes are always errors.

| field | default | finding |
|---|---|---|
| `extra` | `"warn"` | a test or block exists in the source but not the tree |
| `order` | `"warn"` | sibling order differs between source and tree |
| `uncovered` | `"warn"` | a test-bearing source file has no tree spec |
| `unsupported` | `"error"` | extraction recognizes a construct that cannot be represented, such as `test.each` |
| `todo` | `"warn"` | a line-start comment contains the exact scaffold marker `btt:todo` immediately after its comment leader and optional whitespace |

`test_requires_marker` is for conventions where the test-defining syntax
is ambiguous on its own: in Rust *any* `fn` matches the query, and only
the preceding `#[test]`-like attribute makes it a test. Conventions where
the call itself is unambiguous (`it(...)`) don't need it. The marker walk
skips intervening comments and other attributes.

### `[mapping]` — spec text → expected names

Mapping turns a `.tree` node like "when the key is present" into the name
the test file must contain. See [`src/mapping.rs`](../src/mapping.rs).

**`root`** — how the tree's root line maps onto the file:

- `"file"` (default): the root *is* the file; top-level spec nodes map to
  top-level blocks/tests. Rust: the root line names the module, and
  conditions become top-level `mod`s.
- `"block"`: the root must appear as a top-level block. vitest and busted:
  the root line becomes the outer `describe`.

**`[mapping.block]` / `[mapping.test]`** — one name rule per node
category, each with:

| field | | |
|---|---|---|
| `strip_prefix` | prefix removed (case-insensitively) from the spec text before transforming — typically `"it "` so "it returns none" maps to `returns_none` / `"returns none"` |
| `add_prefix` | prefix prepended after transforming — e.g. `"test_"` for pytest-style `test_returns_none` |
| `case` | `verbatim` (default; trimmed text as-is — string-title runners), or `snake` / `camel` / `pascal` (punctuation dropped, words re-joined — identifier runners) |

For the identifier cases, a name that would start with a digit gets a
leading `_` (no language allows a bare digit-leading identifier); check
and scaffold share every one of these rules, so scaffolded names always
round-trip through check.

**`wrappers`** — block names that are structurally *transparent*: their
children are treated as if they sat at the wrapper's level. This is how
Rust's `#[cfg(test)] mod tests { … }` exists in every file without
appearing in any spec tree (`wrappers = ["tests"]`). Extra blocks that are
**not** listed as wrappers and don't appear in the spec are findings.

### `[scaffold]` — generation

| field | | |
|---|---|---|
| `template` | required | pack-relative path of the MiniJinja template (§6) |
| `output` | required | output file-name pattern, `{stem}` substituted; written next to the tree file unless `--output` overrides |
| `indent` | default four spaces | one indentation unit; the template receives it pre-repeated per depth |

## 4. The lexical profile (`[lexical]`)

A `source = "lexical"` pack declares just enough syntax for btt to tell
code from comments and strings, plus what a block and a test look like.
Extraction ([`src/lexical.rs`](../src/lexical.rs)) masks comments and
strings, matches the two opener regexes, derives each match's span from
its brackets, and nests by span containment.

| field | | |
|---|---|---|
| `line_comment` | optional | line-comment opener (`"--"`, `"//"`, `"#"`); runs to end of line |
| `block_comment` | optional | `["open", "close"]` pair, non-nesting |
| `strings` | optional list | `{ delim = "\"", escape = "\\" }` — delimiter that opens *and* closes, plus the escape prefix if the language has one |
| `nest` | required | bracket pairs defining nesting spans, e.g. `[["(", ")"], ["{", "}"]]`; each must be two distinct single characters |
| `block.open` / `test.open` | required | opener regexes, described next |

Each opener regex is matched against the source with comments blanked to
spaces (so `describe--[[c]]("x")` still matches) and must define:

- **`(?<kw>…)`** on the keyword — the match is rejected unless this lands
  on real code, which is what makes `-- it("decoy", …)` in a comment or a
  string inert.
- **`(?<name>…)`** on the name — for `name_syntax = "js-string"` this must
  capture the whole string literal *including quotes* (and must land on a
  string); for `"raw"` it captures a code identifier.
- **the definition's opening bracket** — the first `nest` bracket inside
  the match determines the definition's span: everything to its matching
  closer. For `it("x", function() … end)` that's the call's `(`, so the
  whole callback nests inside; for `mod foo {` it's the body brace. A
  match containing no bracket is an error naming the line.

Nesting then follows from span containment — a captured definition's
parent is the smallest captured block whose span contains it — and blocks
containing no tests anywhere below are pruned, so helper `describe`s and
utility `mod`s never show up as noise.

**Fail closed is the contract.** A malformed profile fails at pack load,
and a file the scan cannot fully account for — an unterminated string or
block comment, an unbalanced bracket — is a per-file error with a line
number, never a silent partial extraction:

```console
✗ src/stack.tree
    error pack `lua`: lexical extraction: unclosed delimiter, expected `)` (line 25)
```

Be honest about what your profile cannot see, in a comment at the top of
the manifest, the way [`packs-lexical/rust/pack.toml`](../packs-lexical/rust/pack.toml)
does. The Lua pack, for instance, does not model `--[[ … ]]` block
comments or `[[ … ]]` long strings: `--[[` starts with the line-comment
opener `--`, so only its first line masks, and a bracket inside the rest
would fail the scan (closed, with a line number). Spec files that hit a
profile's limits belong on a `wasm:` grammar instead.

There is no `test_requires_marker` for lexical packs — a marker like
`#[test]` belongs *inside* the test opener regex. See
[`packs-lexical/rust/pack.toml`](../packs-lexical/rust/pack.toml) for how
the Rust twin does exactly that.

## 5. The tree-sitter query (grammar-backed packs)

For `builtin:` and `wasm:` sources, `extract.query` names a tree-sitter
query using a small capture vocabulary the core understands
([`src/extract.rs`](../src/extract.rs)):

| capture | required | on |
|---|---|---|
| `@block` | yes | a whole nesting construct (a Rust `mod_item`, a `describe(...)` call) |
| `@block.name` | yes | the node holding the block's name, within the same match |
| `@test` | yes | a whole test definition |
| `@test.name` | yes | the node holding the test's name |
| `@test.marker` | only with `test_requires_marker` | a node (e.g. an `attribute_item` for `#[test]`) that must directly precede a `@test` among its siblings |
| `@unsupported` | no | a recognized construct that cannot be represented; only its source line is reported |

Nesting is derived structurally, not from the query: a captured node's
parent is the smallest captured `@block` that contains it. This one rule
covers block-based languages (describe callbacks) and item-based ones
(mods) with no language logic in the core. Blocks with no tests below
them are pruned; helper captures (any other `@name`, conventionally
`@_something`) are ignored by the core and exist for predicates.

From the builtin Rust pack ([full query](../packs/rust/queries/tests.scm)):

```scheme
; Blocks: any module. Modules containing no tests are pruned by the core.
(mod_item
  name: (identifier) @block.name) @block

; Markers: #[test], #[tokio::test], #[test_case(...)], #[rstest] etc.
(attribute_item
  (attribute
    [(identifier) @_attr
     (scoped_identifier) @_attr]
    (#match? @_attr "test"))) @test.marker

; Tests: any function — only counted when preceded by a marker
; (extract.test_requires_marker = true).
(function_item
  name: (identifier) @test.name) @test
```

Tips for writing yours:

- **Inspect the syntax tree first.** `tree-sitter parse sample_spec.lua`
  (with the grammar's CLI setup) or the
  [tree-sitter playground](https://tree-sitter.github.io/tree-sitter/7-playground.html)
  shows the node kinds and field names to match on.
- **Capture exactly the name.** `@block.name` / `@test.name` text is used
  byte-for-byte (then decoded per `name_syntax`), so the captured node
  must contain only the identifier or the string literal — not a whole
  signature. With `name_syntax = "js-string"`, capture the whole `(string)`
  node including quotes and let the core decode it, as the
  [typescript query](../packs/typescript/queries/tests.scm) does.
- **Anchor argument positions.** The typescript pack uses
  `(arguments . (string) @block.name)` — the leading `.` anchors to the
  *first* argument, so `describe(cond ? "a" : "b", …)` doesn't match.
- **A node captured in several matches is fine** — captures are
  deduplicated by span before nesting is built.

Missing any required capture fails with `query must define @block,
@block.name, @test, @test.name`; setting `test_requires_marker` without a
`@test.marker` capture fails with its own message.

## 6. The scaffold template

Every generated test body should contain a line whose trimmed content starts
with a supported line-comment leader (`//`, `#`, `--`, or `;`), optional
whitespace, and the exact marker `btt:todo`. `btt check` reports that line until
the body is filled and the marker removed; occurrences in string literals or
later in a prose comment are ignored. A marker comment outside every extracted
test span is still reported as a file-level finding. A line-start comment that
quotes the marker intentionally still triggers; split the string or reword the
comment when testing or discussing the contract in source. Commented-out
scaffolds also remain findings. Fresh scaffolds therefore warn by default;
projects can set `todo = "error"` under `[check]` once unfinished bodies must
fail CI.

`btt scaffold` flattens the expected tree into a linear event stream so
templates stay simple loops instead of recursive macros
([`src/scaffold.rs`](../src/scaffold.rs)). The MiniJinja context:

- **`events`** — a list, in document order, of:
  - `kind` — `"open"` (start of a block), `"test"`, `"close"` (end of a
    block; `close` carries the same fields as its `open`)
  - `name` — the mapped identifier/title (what check expects to find)
  - `text` — the original spec text, e.g. `it returns nil on pop`
  - `depth` — 0-based nesting depth
  - `indent` — the pack's `scaffold.indent` unit repeated `depth` times,
    precomputed so templates just write `{{ ev.indent }}`
- **`stem`** — the tree file's stem (`stack`)
- **`in_tests_dir`** — `true` when the output path has a `tests/`
  component. Languages whose test-file shape differs by location branch on
  it (the [Rust template](../packs/rust/templates/test.jinja) wraps in
  `#[cfg(test)] mod tests { … }` only outside `tests/`); other templates
  ignore it.

Three escaping filters are available for interpolating spec text into
code — titles are arbitrary text, and an unescaped quote would generate a
scaffold that doesn't compile:

| filter | use for |
|---|---|
| `js_string` | text inside `"…"` in JS-like syntax (escapes `\`, `"`, and U+2028/U+2029) |
| `line_safe` | text in a `//`-style line comment (newlines and JS line terminators neutralized) |
| `rust_string` | text inside a Rust format-string literal like `todo!("…")` (also doubles `{` `}`) |

The Lua template, `templates/test.jinja`:

```jinja
{% for ev in events -%}
{% if ev.kind == "open" -%}
{{ ev.indent }}describe("{{ ev.name | js_string }}", function()
{% elif ev.kind == "close" -%}
{{ ev.indent }}end)

{% elif ev.kind == "test" -%}
{{ ev.indent }}it("{{ ev.name | js_string }}", function()
{{ ev.indent }}  -- btt:todo — {{ ev.text | line_safe }}
{{ ev.indent }}end)

{% endif -%}
{% endfor -%}
```

Rendered output is normalized to end with exactly one newline. The one
hard requirement: **the template must emit names exactly as the mapping
produces them**, because `btt check` extracts them back. Scaffold, then
check, and fix the template until the round trip is clean — that's the
whole verification loop, next.

## 7. Enable and verify

Activate the pack in `btt.toml`:

```toml
[project]
packs = ["lua"]
```

Confirm discovery and origin — a `[project]` origin means your directory
won the resolution race (a builtin it shadows would silently lose here,
which is also how you *check* shadowing):

```console
$ btt packs
lua  v0.1.0  [project]  Lua tests with busted: describe blocks for branches, it for leaves
rust  v0.1.0  [builtin]  Rust tests: nested `mod` blocks for branches, #[test] fns for leaves
typescript  v0.1.0  [builtin]  TypeScript/JavaScript tests: describe blocks for branches, it/test for leaves (vitest, jest, bun)
```

Write a spec, `src/stack.tree`:

```text
Stack
├── when the stack is empty
│   ├── it reports a size of zero
│   └── it returns nil on pop
└── when an item was pushed
    ├── it reports a size of one
    └── it pops the pushed item
```

Exercise generation (`--pack` is only needed when several packs are
configured; `--stdout` previews without writing):

```console
$ btt scaffold src/stack.tree --pack lua --stdout
describe("Stack", function()
  describe("when the stack is empty", function()
    it("reports a size of zero", function()
      -- btt:todo — it reports a size of zero
    end)

    it("returns nil on pop", function()
      -- btt:todo — it returns nil on pop
    end)

  end)
  …
```

Note the shape: `root = "block"` made the root line the outer `describe`,
`strip_prefix = "it "` removed the leaf keyword, and `verbatim` kept
titles as strings. Write it for real and check the round trip:

```console
$ btt scaffold src/stack.tree
scaffolded src/stack_spec.lua (4 tests) from src/stack.tree
$ btt check
✗ src/stack.tree → src/stack_spec.lua
    warn  todo: test body never filled in — scaffold marker still present (src/stack_spec.lua:4)
    …

1 tree file(s), 0 uncovered, 0 error(s), 4 warning(s)
```

Now break it on purpose — rename one test title in `stack_spec.lua` from
`"returns nil on pop"` to `"errors on pop"` — so you can see how a
mismatch surfaces before you trust the pack:

```console
$ btt check src
✗ src/stack.tree → src/stack_spec.lua
    error missing test  `Stack > when the stack is empty > returns nil on pop` (src/stack.tree:4)
    warn  extra   test  `Stack > when the stack is empty > errors on pop` (src/stack_spec.lua:7)

1 tree file(s), 0 uncovered, 1 error(s), 1 warning(s)
```

A missing/extra *pair* like this is check working as intended. The same
pair for a test that **is** in both places means extraction and mapping
disagree — a broken capture (extraction saw the wrong node text) or a
mapping rule producing a different name than your template writes. The
`missing` line shows what mapping expects; compare it against
`btt scaffold --stdout`, which shows the same names, and against what your
query/opener actually captures.

Finally, revert the rename and confirm `btt check` exits 0 again. A pack
is done when scaffold → check round-trips cleanly *and* an intentional
mismatch is reported where you expect it.

## 8. Package, share, troubleshoot

**Sharing.** A data-only pack is just its directory — vendor it by
committing `.btt/packs/<name>/` (activation still requires the
`btt.toml` entry, so cloning a repo never silently activates its packs),
or distribute it for users to drop into `~/.config/btt/packs/<name>/`.
Project packs shadow user packs, which shadow builtins with the same
name; `btt packs` always shows which origin won.

**Common failures**, with the messages you'll actually see:

| symptom | cause |
|---|---|
| ``pack `lua` not found (searched .btt/packs, $XDG_CONFIG_HOME/btt/packs, ~/.btt/packs, and builtins: rust, typescript)`` | the configured name matches no pack directory — check spelling and that the directory contains `pack.toml` |
| `note: ignoring lua [project] — packs are only active when btt.toml names them` | the pack exists but `[project].packs` doesn't name it |
| ``unknown field `…`, expected one of …`` (TOML error) | typo in the manifest; manifests reject unknown fields strictly |
| `query must define @block, @block.name, @test, @test.name` | a required capture is missing or misspelled in `tests.scm` |
| `test_requires_marker is set but the query has no @test.marker` | marker enforcement enabled without a marker capture |
| ``unknown builtin grammar `…` `` | `builtin:` name other than `rust`/`typescript` — ship a `wasm:` grammar or a lexical profile instead |
| ``wasm grammars need a btt built with the `wasm` feature`` | rebuild/install btt with `--features wasm` |
| `[lexical.block] open must define a (?<kw>...) group` (or `name`) | opener regex missing a required named group |
| `opener at line N contains no span bracket` | the opener matched but includes no `nest` bracket — extend the pattern through the definition's opening bracket |
| `lexical extraction: unterminated string` / `unclosed delimiter, expected …` (with a line) | fail-closed: the file uses syntax the profile doesn't model — fix the profile's comment/string rules, or move to a grammar |
| `✗ … no matching test file`, listing candidates | routing picked no existing file; remember candidates are siblings of the `.tree`, and the *first* existing pattern across packs in `[project].packs` order wins — reorder packs or tighten `targets` if the wrong pack claims a stem |
| missing + extra pair for a test present in both spec and file | mapping/extraction mismatch — see §7 |

Tool errors (broken pack, unparseable file) exit 2 and never abort the
rest of a run; spec drift exits 1; clean exits 0.
