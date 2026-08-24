<p align="center">
  <img src="assets/hero.svg" alt="A forest formed from branching btt commands and test vocabulary, above the btt wordmark and an example checker specification" width="920">
</p>

# btt — branch tree testing, for any language

`.tree` files specify your test suites; `btt` scaffolds the test skeletons
and checks that the tests never drift from the spec. A generalization of
[bulloak](https://bulloak.dev) to any language.

## What a btt test is

Each test file gets a `.tree` spec next to it — a branching given/when/it
tree describing every behavior:

```text
HashMap
├── when the key is present
│   ├── it returns the value
│   └── when the value was overwritten
│       └── it returns the latest value
└── when the key is absent
    └── it returns none
```

`btt scaffold` turns it into a skeleton in your language — here Rust, where
branches become nested `mod`s and leaves become `#[test]` fns:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    mod when_the_key_is_present {
        use super::*;

        #[test]
        fn returns_the_value() {
            todo!("it returns the value");
        }

        mod when_the_value_was_overwritten {
            use super::*;

            #[test]
            fn returns_the_latest_value() {
                todo!("it returns the latest value");
            }
        }
    }

    mod when_the_key_is_absent {
        use super::*;

        #[test]
        fn returns_none() {
            todo!("it returns none");
        }
    }
}
```

(Scaffold shape is location-aware: under `tests/`, where the whole file is
test code, the skeleton is flat — no `#[cfg(test)]` wrapper.)

You fill in the bodies; `btt check` fails whenever tree and tests disagree:

```console
$ btt check
✓ src/tree.tree (tree.rs)
✗ src/map.tree → map.rs
    error missing test  `when_the_key_is_absent > returns_none` (map.tree:7)
```

## Why

- **Trees are skimmable.** A `.tree` file is a module's whole behavior
  surface in a dozen lines. Skim the trees in a repo and you understand the
  system without reading any test code.
- **Agents can show you the tree.** When an agent builds something, ask it
  to print the `.tree` in chat — you review the shape of the implementation,
  branch by branch, without opening the test files.
- **The spec can't rot.** `btt check` fails CI the moment tests and tree
  diverge, so trees stay a truthful map instead of stale documentation.

## Quickstart

```console
$ cargo install --path .        # puts btt on your PATH
$ cd your-project
$ btt init                      # writes btt.toml (--skill: agent skill too)
$ $EDITOR src/map.tree          # write the spec
$ btt scaffold src/map.tree     # generate the test skeleton
$ btt check                     # keep them in sync — wire into CI
```

## Working with agents

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

## Any language: packs

Languages are **packs** — folders of declarative data, no plugin API. Drop
one in `.btt/packs/`, name it in `btt.toml`'s `packs = [...]`, and btt
speaks your language without a rebuild. Packs come in three kinds:

- **Native** — Rust and TypeScript grammars ship embedded in the binary.
- **WASM** (`--features wasm`) — a pack brings its own full tree-sitter
  grammar as a `tree-sitter build --wasm` module, run sandboxed: no WASI,
  so no filesystem or network access. Treat grammar modules like
  dependencies — pin and review them.
- **Lexical** — no grammar at all: a short declarative profile (comment and
  string syntax, nesting brackets, two regexes for what a block and a test
  look like). The whole definition is plain text on one screen — your agent
  can write and install one in a second for whatever language you're using.
  It fails closed on anything it can't fully account for; when a language
  outgrows it, graduate to a WASM pack.

Want to add your language? The step-by-step authoring guide —
[docs/extension-packs.md](docs/extension-packs.md) — walks from an
unsupported test convention to a working, verified pack, and documents
every manifest field, query capture, and template input along the way.

Packs carry independent `SemVer` versions, a manifest-format version, and a
`btt` compatibility requirement. Releases use immutable per-pack tags; the
policy and maintainer workflow are documented in
[docs/pack-releases.md](docs/pack-releases.md).

Add one from a local directory or Git repository, then name it in
`btt.toml`. Packs in a monorepo use `--dir` to select their directory:

```console
$ btt pack add owner/repo --dir packs/python
```

This vendors only the files named by the pack manifest into
`.btt/packs/<name>`; Git and your project history provide distribution,
review, and versioning. Only add packs from sources you trust: queries
and lexical profiles are data, but a wasm grammar is code — the no-WASI
sandbox removes ambient filesystem and network access, not every risk
from a deliberately hostile module. Review what lands in `.btt/packs/`
like any other dependency.

## Configuration

`btt.toml` at the repo root:

```toml
[project]
packs = ["rust"]        # routing-priority order; empty/absent = builtins only

[check]
extra = "warn"          # tests in the file but not the tree: error|warn|ignore
order = "warn"          # sibling order drift: error|warn|ignore
uncovered = "warn"      # test-bearing files with no .tree spec: error|warn|ignore
```

Missing tests are always errors, and `btt check` exits non-zero on errors.
`uncovered` also reports test files that have no spec at all — keep it at
`warn` while adopting, then ratchet everything to `error` in CI.

## Commands

| command | |
|---|---|
| `btt check [paths] [-j N]` | diff every `.tree` against its test file (parallel; `-j` caps threads) |
| `btt scaffold <tree>` | generate a skeleton (`--stdout`, `--force`, `--pack`) |
| `btt packs` | list packs and where they resolve from |
| `btt pack add <source> [--dir <path>] [--git]` | vendor one local or Git-hosted pack into this project (`--git`: never treat the source as a local path) |
| `btt init [--skill]` | write `btt.toml` (+ Claude skill for agents) |

## Writing a lexical pack

A pack is a folder: `pack.toml` plus a scaffold template. For a lexical
pack the manifest is the entire language definition — this working
Solidity (Foundry) one fits on a screen:

```toml
format = 1

[pack]
name = "solidity"
version = "0.1.0"
description = "Foundry tests via lexical scanning"

[compat]
btt = ">=0.2.0"

[detect]
targets = ["{stem}.t.sol"]

[grammar]
source = "lexical"

[extract]

[lexical]
line_comment = "//"
block_comment = ["/*", "*/"]
strings = [{ delim = '"', escape = '\' }, { delim = "'", escape = '\' }]
nest = [["(", ")"], ["{", "}"]]

# Openers capture the keyword and name, and include the opening bracket
# whose matching closer bounds the definition's span.
[lexical.block]
open = '''(?:^|[^\w])(?<kw>contract)\s+(?<name>\w+)[^{;]*\{'''

[lexical.test]
open = '''(?:^|[^\w])(?<kw>function)\s+(?<name>test\w*)\s*\('''

[mapping]
root = "block"          # the tree's root line is a top-level contract

[mapping.block]
case = "pascal"

[mapping.test]
strip_prefix = "it "
add_prefix = "test_"
case = "pascal"

[scaffold]
template = "templates/test.jinja"
output = "{stem}.t.sol"
```

With this pack active, a `map.tree` rooted at `MapTest` with the leaf
"it returns the value" checks against
`contract MapTest { function test_ReturnsTheValue() ... }` in
`map.t.sol`. The in-repo packs are the reference examples:
`packs-lexical/rust` and `packs-lexical/typescript` (lexical),
`packs-wasm/` (WASM), `packs/` (builtin).

## Limitations

Lexical packs see comments, strings, and brackets — not grammar.
Languages whose structure isn't bracket-shaped are out of their scope:
Python's indentation nesting, Ruby's `do…end`, and syntax that hides
brackets from a scanner (JS regex literals, template-string
interpolation). Extraction fails closed — a file the profile can't fully
account for is reported, never silently mis-read — and a language that
outgrows its profile graduates to a WASM grammar pack.
