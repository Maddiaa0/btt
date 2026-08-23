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
tests against (`btt init --skill` installs a Claude skill teaching the
tree-first workflow).

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
<repo>/.btt/packs/<name>/    # project-local / vendored — highest priority
~/.btt/packs/<name>/         # user-global
(embedded in the binary)     # rust, typescript
```

Adding a language never means rebuilding the binary: drop a pack folder in
`.btt/packs/` and it loads at runtime. Packs contain no executable code
(manifest + query + templates), so vendoring one is reviewable like any
config change. Grammars currently come compiled into the core
(`builtin:rust`, `builtin:typescript`); sandboxed `wasm:` grammar loading is
the planned extension point for fully self-contained packs.

## Configuration

`btt.toml` at the repo root:

```toml
[project]
packs = ["rust"]        # routing-priority order

[check]
extra = "warn"          # tests in the file but not the tree: error|warn|ignore
order = "warn"          # sibling order drift: error|warn|ignore
```

Missing tests are always errors. `btt check` exits non-zero on errors — wire
it into CI or a pre-commit hook.

## Commands

| command | |
|---|---|
| `btt check [paths]` | diff every `.tree` against its test file |
| `btt scaffold <tree>` | generate a skeleton (`--stdout`, `--force`, `--pack`) |
| `btt packs` | list packs and where they resolve from |
| `btt init [--skill]` | write `btt.toml` (+ Claude skill for agents) |

## Dogfood

This repo checks itself: every module's tests follow `.tree` specs
(`src/*.tree`, `tests/*.tree`), and `tests/selfcheck.rs` runs the full
check pipeline as part of `cargo test`.
