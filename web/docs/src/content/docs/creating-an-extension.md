---
title: Creating an extension
description: Add support for another language or test framework by writing a pack.
---

btt calls extensions **packs**. A pack is a folder that tells btt which test file to use, how to read it, how tree names map to code, and how to scaffold new tests.

Most packs are plain TOML, a template, and sometimes a tree-sitter query. There is no plugin API to implement.

## Pick a parser

| Parser | Use it when |
| --- | --- |
| Lexical | Suites and tests have clear patterns and use balanced brackets |
| Native tree-sitter | The parser should be built into btt itself |
| WASM tree-sitter | You need a full parser without rebuilding btt |

A lexical pack is the quickest place to start. It describes comments, strings, brackets, and the patterns that open suites and tests. If the language has syntax that makes those patterns unreliable, use a tree-sitter grammar instead.

## Files in a pack

```text
my-language/
├── pack.toml
├── templates/
│   └── test.jinja
├── queries/
│   └── tests.scm          # tree-sitter only
└── grammar.wasm           # WASM only
```

Put a project pack in `.btt/packs/<name>/`. Reusable user packs go in `$XDG_CONFIG_HOME/btt/packs/<name>/`, which is usually `~/.config/btt/packs/<name>/`.

Then enable it:

```toml title="btt.toml"
[project]
packs = ["my-language", "rust"]
```

btt tries packs in that order when more than one can handle the same tree.

## Name the pack and its files

This example adds a small Mocha-style pack:

```toml title="pack.toml"
[pack]
name = "mocha-lexical"
version = "0.1.0"
description = "Mocha-style suites through lexical extraction"

[detect]
targets = ["{stem}.spec.mjs"]
```

For `cache.tree`, `{stem}` is `cache`, so this pack looks for `cache.spec.mjs`. btt uses the same pattern when it checks for test files that do not have trees.

## Tell btt how to read the tests

The following patterns find nested `describe` blocks and `it` or `test` calls:

```toml
[grammar]
source = "lexical"

[extract]
name_syntax = "js-string"

[lexical]
line_comment = "//"
block_comment = ["/*", "*/"]
strings = [
  { delim = "\"", escape = "\\" },
  { delim = "'", escape = "\\" },
  { delim = "`", escape = "\\" },
]
nest = [["(", ")"], ["{", "}"]]

[lexical.block]
open = '''(?:^|[^\w$.])(?<kw>(?:describe|suite)(?:\.\w+)?)\s*\(\s*(?<name>"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')\s*[,)]'''

[lexical.test]
open = '''(?:^|[^\w$.])(?<kw>(?:it|test)(?:\.(?:only|skip|todo))?)\s*\(\s*(?<name>"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')\s*[,)]'''
```

Both `open` patterns need named `kw` and `name` captures. They must also include the opening bracket that contains the definition. btt ignores matches inside comments and strings and stops with an error if it cannot account for the brackets in a file.

For a tree-sitter pack, use a grammar and query instead:

```toml
[grammar]
source = "wasm:grammar.wasm"
symbol = "my_language"

[extract]
query = "queries/tests.scm"
```

The query must capture `@block`, `@block.name`, `@test`, and `@test.name`. If tests are marked by an attribute, also capture `@test.marker` and set `test_requires_marker = true`.

## Map tree names to code

Mocha uses the same text in the tree and the test file, except that btt drops `it ` from leaf names:

```toml
[mapping]
root = "block"

[mapping.block]
case = "verbatim"

[mapping.test]
strip_prefix = "it "
case = "verbatim"
```

Use `root = "block"` when the first tree line must match a top-level suite. Use `root = "file"` when it only names the file, as the Rust pack does.

Names can be kept `verbatim` or converted to `snake_case`, `camel_case`, or `pascal_case`. `add_prefix` can add a language convention such as `test_`. `wrappers = ["tests"]` can hide a framework-only container from the comparison.

## Write the scaffold template

Point the manifest at the template and output file:

```toml title="pack.toml"
[scaffold]
template = "templates/test.jinja"
output = "{stem}.spec.mjs"
indent = "  "
```

The template receives `open`, `test`, and `close` events. Each event includes the mapped `name`, original `text`, `depth`, and `indent`.

```jinja title="templates/test.jinja"
{% for ev in events -%}
{% if ev.kind == "open" -%}
{{ ev.indent }}describe("{{ ev.name | js_string }}", () => {
{% elif ev.kind == "test" -%}
{{ ev.indent }}it("{{ ev.name | js_string }}", () => {
{{ ev.indent }}  // TODO: {{ ev.text | line_safe }}
{{ ev.indent }}});
{% elif ev.kind == "close" -%}
{{ ev.indent }}});
{% endif -%}
{% endfor -%}
```

The important test for a template is simple: scaffold a file, then check it with the same pack. There should be no findings.

## Try the pack

```sh
mkdir -p .btt/packs/mocha-lexical/templates
# add pack.toml and templates/test.jinja

btt packs
btt scaffold test/Foo.tree --pack mocha-lexical --stdout
btt scaffold test/Foo.tree --pack mocha-lexical
btt check test/Foo.tree
```

Before sharing a pack, test a matching scaffold, missing and extra tests, sibling order, fake test syntax inside comments and strings, and malformed input.

:::caution
Manifests are strict, and paths cannot escape the pack folder. WASM grammars run without WASI filesystem or network access, but they are still executable parser input. Review and pin them.
:::

The built-in [Rust pack](https://github.com/Maddiaa0/btt/tree/main/packs/rust), [TypeScript pack](https://github.com/Maddiaa0/btt/tree/main/packs/typescript), and [lexical TypeScript pack](https://github.com/Maddiaa0/btt/tree/main/packs-lexical/typescript) are useful examples.
