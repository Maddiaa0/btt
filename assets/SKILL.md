---
name: btt
description: Branch tree testing (BTT) workflow. Use whenever writing, modifying, or reviewing tests in this project — tests are specified in .tree files first, scaffolded with btt, and kept in sync with `btt check`.
---

# Branch tree testing with btt

This project specifies test suites as **`.tree` files** (the bulloak
given/when/it format, generalized to every language). The tree is the source
of truth for what a test file contains; `btt check` enforces it.

## The workflow — spec first, always

1. **Write or update the `.tree` file first.** Enumerate the branches of the
   behavior (`when` / `given` conditions) and the observable outcomes (`it`
   leaves) before writing any test code.
2. **Scaffold:** `btt scaffold path/to/name.tree` generates the test skeleton
   with correctly named blocks and empty test bodies (`--stdout` to preview,
   `--force` to overwrite).
3. **Implement** the test bodies. Do not rename or restructure the generated
   blocks/tests — names encode the tree.
4. **Verify:** run `btt check` before finishing any task that touched tests.
   Missing tests are errors; extra tests and ordering drift are warnings.
   Fix by updating the tree (if behavior legitimately changed) or the tests —
   never by deleting spec lines just to silence the check.

## .tree format

```text
HashMap
├── when the key is present
│   ├── it returns the value
│   └── when the value was overwritten
│       └── it returns the latest value
└── when the key is absent
    └── it returns none
```

- First line: root — the unit under test. A file may hold several trees
  separated by blank lines.
- `when …` / `given …` nodes are branches (nesting allowed).
- `it …` nodes are leaves (no children) describing one assertion.
- Lines starting with `//` are comments.

## How trees map to code

Configured per language by packs (see `btt packs`; project config in
`btt.toml`):

- **Rust** (`name.tree` ↔ `name.rs`): each branch is a nested
  `mod when_the_key_is_present { use super::*; … }`; each leaf is a
  `#[test] fn returns_the_value()` (the `it ` prefix is dropped,
  snake_case). A surrounding `#[cfg(test)] mod tests { … }` wrapper is
  allowed and ignored by the checker.
- **TypeScript** (`name.tree` ↔ `name.test.ts`): the root is a top-level
  `describe("HashMap", …)`; branches are nested `describe`s with the
  condition text verbatim; leaves are `it("returns the value", …)`.

## Rules of thumb

- New behavior → new branch in the tree, then a new test. Never add a test
  the tree doesn't describe.
- Keep condition text short and observable ("when the input is empty"), and
  leaf text a single outcome ("it returns an error").
- A glance at the `.tree` file should tell a reviewer the full behavior
  surface without reading test code.
