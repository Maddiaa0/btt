# Overview

What branch tree testing is, how btt works, and why a small test outline is useful when people or agents write code.

**btt keeps a short, readable outline next to each test file.** The outline says which cases should exist. btt can turn it into a test skeleton and check that the finished tests still match it.

```text
HashMap
├── when the key is present
│   ├── it returns the value
│   └── when the value was overwritten
│       └── it returns the latest value
└── when the key is absent
    └── it returns none
```

That file is small enough to read in a pull request. It shows the cases and how they relate without making you work through setup code and assertions first.

## What btt does

The usual workflow is:

1. Write or update the `.tree` file.
2. Run `btt scaffold path/to/file.tree` to create the test skeleton.
3. Fill in the test bodies.
4. Run `btt check` to compare the tree with the test code.

If a test from the tree is missing, the check fails. Extra tests, tests in a different order, and test files without trees can be warnings or errors, depending on `btt.toml`.

btt checks test **structure**. It cannot tell whether an assertion is correct or whether you chose the right cases. That part still needs judgement.

## The tree format

The first line names the thing being tested. The rest uses three plain-English prefixes:

- `given …` describes state prepared before the action.
- `when …` describes an action, input, or decision.
- `it …` describes one expected result.

Branches can be nested. `it …` lines are leaves and cannot have children.

The format comes from the [Branching Tree Technique](https://prberg.com/presentations/solidity-summit-2023/), a lightweight way to plan Solidity tests that was inspired by Gherkin. [Bulloak](https://www.bulloak.dev/) added scaffolding and checking for Solidity. btt keeps that workflow and moves the language-specific parts into packs.

## Why use a tree?

### It is quicker to review

Test code is good at proving a case, but not always good at showing the whole plan. The tree puts the plan in one place. Missing opposites tend to stand out: success without failure, present without absent, authorized without unauthorized.

### It does not go stale quietly

A separate test plan is easy to forget. `btt check` ties this one to the code. If somebody adds, removes, renames, or moves a test without updating the tree, btt reports it.

### It gives agents a useful checkpoint

An agent can write a lot of code before you have seen its plan. Asking it to show the tree first gives you something small to review before the implementation grows around it.

This does not make the agent's work correct by itself. It simply lets you check the list of cases early, then use btt to make sure those promised tests were actually written.

## How btt is put together

btt is a Rust CLI. Its core handles tree parsing, file discovery, scaffolding, comparison, and reporting. Language packs describe the source code around it.

**Architecture at a glance**

Inputs:

- `.tree files`: the tests that should exist
- `btt.toml`: active packs and check levels
- Language pack: file routing, name mapping, source extraction, and scaffold templates

The btt core parses the tree, finds the test file, and builds the expected test structure. `btt scaffold` renders a test skeleton; `btt check` compares the test code and reports differences.

A pack contains four main pieces:

- **Detection:** which test file belongs to a `.tree` file.
- **Extraction:** how to find suites and tests in source code.
- **Mapping:** how tree text becomes a function name or suite title.
- **Scaffolding:** the template used to generate new tests.

Rust and TypeScript packs are built in. Other packs can live in a project or in your user config directory. A pack only becomes active when it is listed in `btt.toml`.

### Ways to read source code

- **Native tree-sitter** is used by the built-in Rust and TypeScript packs.
- **Lexical packs** describe comments, strings, brackets, suites, and tests in a small TOML file.
- **WASM tree-sitter packs** include a full parser when a lexical pack is not enough.

WASM grammars run without WASI filesystem or network access, but you should still review and pin them as you would any executable dependency.

See [Creating an extension](/creating-an-extension/) for the pack format.

## Docs for tools and agents

The same docs are available without the site layout:

- [`llms.txt`](/llms.txt) is a short index.
- [`llms-full.txt`](/llms-full.txt) contains every page.
- Each page also has a Markdown version, such as [`overview.md`](/overview.md).

## Further reading

- [Paul Berg's Solidity Summit 2023 presentation](https://prberg.com/presentations/solidity-summit-2023/)
- [Branching Tree Technique in the Solidity Testing Handbook](https://www.soliditytestingbook.com/branching-tree-technique)
- [Bulloak](https://www.bulloak.dev/)
