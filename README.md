# btt — branch tree testing, for any language

A generalization of [bulloak](https://bulloak.dev). Test suites are specified
as `.tree` files (given/when/it branching trees); `btt` scaffolds test
skeletons from them and checks that test files stay in sync — for any
language, via data-only **packs**.

```text
HashMap
├── when the key is present
│   ├── it returns the value
│   └── when the value was overwritten
│       └── it returns the latest value
└── when the key is absent
    └── it returns none
```

```console
$ btt scaffold map.tree          # generate the test skeleton
$ btt check                      # verify every .tree matches its test file
✓ src/tree.tree (tree.rs)
✗ src/map.tree → map.rs
    error missing test  `when_the_key_is_absent > returns_none` (map.tree:7)
```

A glance at the `.tree` files tells you the whole behavior surface of a test
suite without reading test code — and gives coding agents a spec to write
tests against.

## Install

```sh
cargo install --path .   # installs to ~/.cargo/bin/btt
```

Then in each project you want checked: `btt init` writes a starter
`btt.toml`; add `--skill` to also install the agent skill described below.

## Agents

btt is built for the loop where an agent writes the tests: the `.tree` file
is the spec, the agent scaffolds and fills it in, and `btt check` keeps it
honest. Two copy-paste files in this repo — they are **not** interchangeable:

- **[AGENT-SETUP.md](AGENT-SETUP.md)** — a one-time prompt. Paste it into a
  *chat* with your coding agent and it installs btt and initializes your
  project for you. It does not belong in any config file.
- **[SNIPPET.md](SNIPPET.md)** — permanent instructions. Paste it into your
  `CLAUDE.md` / `AGENTS.md` so agents work tree-first. Claude Code users can
  skip it: `btt init --skill` installs a richer skill at
  `.claude/skills/btt/SKILL.md` instead.

## Architecture

One small static binary; languages are **packs** — pure data, no code:

```text
packs/rust/
  pack.toml            # detection globs, naming rules, grammar ref
  queries/tests.scm    # tree-sitter query marking blocks and tests
  templates/test.jinja # scaffold template
```

The core does everything language-agnostic: parse `.tree` specs, run the
pack's tree-sitter query over the test file, derive nesting structurally
(a node's parent is the smallest captured block containing it), apply the
pack's naming rules, and diff. Three mapping strategies cover essentially all
test conventions:

- **nested blocks + flat test names** — Rust `mod`s + `#[test]` fns
- **nested blocks + verbatim titles** — vitest/jest/bun `describe`/`it`
- **flat joined names** — bulloak-style Solidity, Go, pytest (planned)

### Pack resolution (the nvm-ish part)

```text
<repo>/.btt/packs/<name>/            # project-local / vendored — highest priority
$XDG_CONFIG_HOME/btt/packs/<name>/   # user-global (default ~/.config/btt/packs)
~/.btt/packs/<name>/                 # user-global, legacy location
(embedded in the binary)             # rust, typescript
```

Adding a language never means rebuilding the binary: drop a pack folder in
`.btt/packs/` and it loads at runtime. Packs contain no executable code
(manifest + query + templates), so vendoring one is reviewable like any
config change.

### Sandboxed WASM grammars (experimental)

With the `wasm` feature (`cargo install btt --features wasm`), a pack can
ship its own grammar instead of relying on a builtin:

```toml
[grammar]
source = "wasm:grammar.wasm"   # a `tree-sitter build --wasm` artifact
symbol = "rust"                # module exports tree_sitter_<symbol>
```

This is the same architecture Zed uses: the tree-sitter runtime stays
native, while the grammar module — including any external scanner code, the
only arbitrary code a grammar ships — is instantiated in a wasmtime store
with no WASI, so it cannot touch the filesystem or network. The
`packs-wasm/` directory holds wasm twins of the builtin packs (grammars
fetched by `scripts/fetch-wasm-grammars.sh`, pinned by release tag and
sha256), and `tests/wasm.rs` proves they extract structure identical to the
natively compiled grammars. Without the feature, the core stays lean and
wasm packs fail with a clear error.

## Configuration

`btt.toml` at the repo root:

```toml
[project]
packs = ["rust"]        # routing-priority order

[check]
extra = "warn"          # tests in the file but not the tree: error|warn|ignore
order = "warn"          # sibling order drift: error|warn|ignore
uncovered = "warn"      # test-bearing files with no .tree spec: error|warn|ignore
```

Missing tests are always errors. `btt check` exits non-zero on errors — wire
it into CI or a pre-commit hook.

`uncovered` is what makes partial adoption honest: `check` reports not just
"the covered files match" but "these files have tests and no spec at all"
(it reverse-matches each pack's target patterns and extracts, so a repo with
zero `.tree` files reports every test file instead of a hollow pass). Keep
it at `warn` while adopting; in CI, ratchet everything to strict:

```toml
[check]
extra = "error"
order = "error"
uncovered = "error"
```

## Commands

| command | |
|---|---|
| `btt check [paths] [-j N]` | diff every `.tree` against its test file (parallel; `-j` caps threads) |
| `btt scaffold <tree>` | generate a skeleton (`--stdout`, `--force`, `--pack`) |
| `btt packs` | list packs and where they resolve from |
| `btt init [--skill]` | write `btt.toml` (+ Claude skill for agents) |

## Benchmarks

`cargo bench` runs the check pipeline over 40 generated tree/test pairs at
1/4/8 threads (`benches/check.rs`); add `--features wasm` for the
sandboxed-grammar groups (fetch grammars first). Criterion keeps baselines
under `target/criterion` — `cargo bench -- --save-baseline main` before a
change, `-- --baseline main` after, to see the delta.

Read the two group kinds together: the steady-state groups (`check/...`)
show warm wasm parsing indistinguishable from native, but they exclude
grammar compilation; the `-cold` groups build a fresh thread pool per
iteration and expose the per-thread Cranelift compile wasm pays (~50 ms
per grammar per thread with a warm engine). A fully cold *process* also
pays one-time wasmtime engine setup — roughly half a second end-to-end in
release CLI measurements. Neither number alone is the whole story.

## Dogfood

This repo checks itself: every module's tests follow `.tree` specs
(`src/*.tree`, `tests/*.tree`), and `tests/selfcheck.rs` runs the full
check pipeline as part of `cargo test`.
