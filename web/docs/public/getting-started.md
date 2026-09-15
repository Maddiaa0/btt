# Getting started

Add btt to a project, write one tree, scaffold the tests, and check the result.

Here is the shortest route from an existing Rust project to a checked test tree. TypeScript uses the same commands and generates different test syntax.

## Install and initialize

```sh
cargo install --git https://github.com/Maddiaa0/btt
cd your-project
btt init
```

`btt init` writes a `btt.toml` with the Rust pack enabled. For TypeScript, change it to:

```toml
[project]
packs = ["typescript"]

[check]
extra = "warn"
order = "warn"
uncovered = "warn"
```

`btt packs` shows which packs are available and where btt found them.

## Write a tree

Create `src/map.tree` next to `src/map.rs`:

```text
HashMap
├── when the key is present
│   ├── it returns the value
│   └── when the value was overwritten
│       └── it returns the latest value
└── when the key is absent
    └── it returns none
```

Keep branch names short. Each `it …` line should name one result that the test can observe.

## Preview the skeleton

```sh
btt scaffold src/map.tree --stdout
```

If it looks right, write the file:

```sh
btt scaffold src/map.tree
```

For Rust, branches become nested modules and leaves become `#[test]` functions. Files under `tests/` do not get an extra `#[cfg(test)] mod tests` wrapper.

> **Note**
>
> Scaffolding will not replace an existing test file unless you pass `--force`. Check the path before using it.

## Write the tests

Replace the generated `todo!` calls with real setup and assertions:

```rust
#[test]
fn returns_none() {
    let map = HashMap::<String, String>::new();
    assert_eq!(map.get("missing"), None);
}
```

Keep the generated names and nesting. If the plan changes, update the tree first and then change the test structure to match.

## Run the check

```sh
btt check
```

A mismatch includes the path through the tree and the line where the missing case was declared:

```text
✓ src/map.tree (map.rs)
✗ src/cache.tree → cache.rs
    error missing test  `when_empty > returns_none` (cache.tree:4)
```

While working, you can check one file or directory:

```sh
btt check src/map.tree
btt check src/
btt check -j 4
```

Missing tests always fail. `btt.toml` controls the other findings:

- `extra`: a test exists in code but not in the tree.
- `order`: sibling tests are in a different order.
- `uncovered`: a test file has no `.tree` file.

For an existing project, keep `uncovered = "warn"` while you add trees. Change it to `"error"` once the whole test suite is covered, then add `btt check` to CI.

## Using a coding agent

Run this if you want Claude Code to learn the tree-first workflow:

```sh
btt init --skill
```

Other agents can read the same rules from a repository instruction file. See [Installing the skill + usage](/installing-the-skill/) for the exact setup.

## Commands

| Command | What it does |
| --- | --- |
| `btt init [--skill]` | Writes project config and optionally the Claude skill |
| `btt scaffold <tree>` | Generates a test skeleton |
| `btt scaffold <tree> --stdout` | Prints the skeleton without writing it |
| `btt check [paths] [-j N]` | Compares trees with test code |
| `btt packs` | Lists available packs |
