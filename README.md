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
| `btt init [--skill]` | write `btt.toml` (+ Claude skill for agents) |
