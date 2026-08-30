//! The lexical backend's contract is equivalence: for realistic test
//! files, extraction through the blob-free lexical profile must produce
//! exactly what the native tree-sitter grammar produces. A curated corpus
//! pins the known shapes; a seeded differential fuzzer hunts for edge
//! cases in the space of generated test files.

use btt::check;
use btt::extract;
use btt::pack::{self, Pack};
use btt::{scaffold, tree};
use std::path::Path;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn native_pack() -> Pack {
    pack::load_builtin("typescript").unwrap()
}

fn lexical_pack() -> Pack {
    pack::load_dir(&repo_root().join("packs-lexical/typescript")).unwrap()
}

fn rust_native_pack() -> Pack {
    pack::load_builtin("rust").unwrap()
}

fn rust_lexical_pack() -> Pack {
    pack::load_dir(&repo_root().join("packs-lexical/rust")).unwrap()
}

/// Write a lexical pack fixture under `target/lexical-fixtures/<case>/`
/// and load it.
fn fixture_pack(case: &str, manifest: &str) -> btt::Result<Pack> {
    let dir = repo_root().join("target/lexical-fixtures").join(case);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("templates")).unwrap();
    std::fs::write(dir.join("templates/test.jinja"), "").unwrap();
    std::fs::write(dir.join("pack.toml"), manifest).unwrap();
    pack::load_dir(&dir)
}

/// Extract with both backends; panic with the source on any divergence.
fn assert_equivalent(source: &str) {
    let native = extract::extract_with_findings(&native_pack(), Path::new("map.test.ts"), source)
        .unwrap_or_else(|e| panic!("native extraction failed: {e}\n---\n{source}"));
    let lexical = extract::extract_with_findings(&lexical_pack(), Path::new("map.test.ts"), source)
        .unwrap_or_else(|e| panic!("lexical extraction failed: {e}\n---\n{source}"));
    assert_eq!(
        native, lexical,
        "backends diverged\n--- source ---\n{source}\n--- native ---\n{native:#?}\n--- lexical ---\n{lexical:#?}"
    );
}

mod when_extracting_typescript_lexically {
    use super::*;

    #[test]
    fn matches_the_native_extraction() {
        assert_equivalent(
            r#"
import { describe, it, suite, test } from "vitest";
const helper = () => {};
describe("HashMap", () => {
  describe.only("when the key is present", () => {
    it("returns the value", () => {});
    it.skip("skips this one", () => {});
  });
  suite("when the key is absent", () => {
    test.todo("returns none");
  });
});
test.describe("playwright style", () => {
  test("works", async ({ page }) => {});
});
it.describe("also a block", () => {
  it('single quoted "title"', () => {});
  it("escaped \"quotes\" and \\ slashes", () => {});
});
describe/* legal trivia */("comments between tokens", () => {
  it/* here too */("still matches", () => {});
  it("and after the title" /* trailing */, () => {});
});
"#,
        );
    }

    #[test]
    fn reports_parameterized_tests_identically() {
        assert_equivalent(
            r#"
describe("parameterized", () => {
  test.each([[1], [2]])("curried %s", value => {});
  it.each`value | expected
    ${1}  | ${2}
  `("tagged $value", ({ value, expected }) => {});
  describe.each(cases)("nested %s", value => {});
  myArray.each(value => consume(value));
  holder.test.each(cases)("not a recognized receiver", () => {});
  const text = "test.each(cases)(\"decoy\", fn)";
  // test.each(cases)("comment decoy", fn);
});
"#,
        );
    }
}

mod when_decoys_hide_in_comments_and_strings {
    use super::*;

    #[test]
    fn matches_the_native_extraction() {
        assert_equivalent(
            r#"
// it("not a test", () => {});
/* describe("not a block", () => {
     it("still not a test", () => {});
   */
const s = "it(\"nope\", () => {";
const braces = "}}}{{(";
const q = 'describe("inner quotes", () => {})';
describe("real", () => {
  if (true) { helper(); }
  it("counts", () => {
    expect("it(\"fake\", f)").toBe(s);
  });
});
"#,
        );
    }
}

mod when_aliases_bind_describe_and_it {
    use super::*;
    use std::fmt::Write;

    #[test]
    fn matches_the_native_extraction() {
        assert_equivalent(
            r#"
const d = flag ? describe : describe.skip;
const d2 = d;
const t = it;
d2("suite", () => { d("nested", () => { t("works", f); }); });
"#,
        );
    }

    #[test]
    fn reports_aliased_each_identically() {
        assert_equivalent("const t = test;\nt.each(cases)(\"case\", fn);\n");
    }

    #[test]
    fn resolves_module_var_and_exported_aliases_identically() {
        let source =
            "var d = describe;\nexport const t = test;\nd(\"suite\", () => { t(\"works\", f); });";
        assert_equivalent(source);
        let actual = extract::extract(&lexical_pack(), Path::new("map.test.ts"), source).unwrap();
        assert_eq!(actual.first().map(|node| node.name.as_str()), Some("suite"));
    }

    #[test]
    fn ignores_for_header_aliases_identically() {
        let source =
            "for (const d = describe; ready; step()) {}\nd(\"ghost\", () => { it(\"nope\", f); });";
        assert_equivalent(source);
        let actual = extract::extract(&lexical_pack(), Path::new("map.test.ts"), source).unwrap();
        assert!(
            actual
                .iter()
                .all(|node| node.kind != extract::ActualKind::Block)
        );
    }

    #[test]
    fn bounds_a_ten_thousand_link_chain_identically() {
        let mut source = String::new();
        for i in 0..10_000 {
            let _ = writeln!(source, "const a{i} = a{};", i + 1);
        }
        source.push_str("const a10000 = describe;\na0(\"suite\", () => { it(\"works\", f); });");
        assert_equivalent(&source);
        let actual = extract::extract(&lexical_pack(), Path::new("map.test.ts"), &source).unwrap();
        assert!(
            actual
                .iter()
                .all(|node| node.kind != extract::ActualKind::Block)
        );
    }

    #[test]
    fn drops_source_derived_patterns_between_files() {
        let pack = lexical_pack();
        for i in 0..100 {
            let source = format!(
                "const alias{i} = describe;\nalias{i}(\"suite {i}\", () => {{ it(\"works\", f); }});"
            );
            let actual = extract::extract(&pack, Path::new("map.test.ts"), &source).unwrap();
            let expected = format!("suite {i}");
            assert_eq!(
                actual.first().map(|node| node.name.as_str()),
                Some(expected.as_str())
            );
        }
    }

    #[test]
    fn ignores_non_module_aliases_identically() {
        assert_equivalent(
            "function bind() { const d = describe; }\nd(\"ghost\", () => { it(\"nope\", f); });",
        );
    }
}

mod when_fuzzing_random_test_files {
    use super::*;

    /// Deterministic LCG so failures reproduce; print the seed and source
    /// on divergence.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }

        fn below(&mut self, n: usize) -> usize {
            usize::try_from(self.next() % n as u64).unwrap()
        }

        fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
            &items[self.below(items.len())]
        }
    }

    const TITLE_POOL: &[char] = &[
        'a', 'B', '3', ' ', '_', '"', '\'', '\\', '{', '}', '(', ')', '.', '$', 'é', '\u{2028}',
    ];

    fn title(rng: &mut Rng) -> String {
        (0..rng.below(12)).map(|_| *rng.pick(TITLE_POOL)).collect()
    }

    /// Emit a title as a JS string literal, randomly single- or
    /// double-quoted (double uses the same escaping the scaffold's
    /// `js_string` filter applies).
    fn literal(rng: &mut Rng, t: &str) -> String {
        if rng.below(4) == 0 {
            let escaped = t
                .replace('\\', "\\\\")
                .replace('\'', "\\'")
                .replace('\u{2028}', "\\u2028");
            format!("'{escaped}'")
        } else {
            let escaped = t
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\u{2028}', "\\u2028");
            format!("\"{escaped}\"")
        }
    }

    /// Comment trivia legally allowed between an opener's tokens.
    fn trivia(rng: &mut Rng) -> &'static str {
        rng.pick(&["", "", "", "/* trivia */"])
    }

    struct Receivers {
        tests: Vec<String>,
        blocks: Vec<String>,
        each: Vec<String>,
    }

    fn gen_items(rng: &mut Rng, depth: usize, receivers: &Receivers, out: &mut String) {
        use std::fmt::Write;
        let indent = "  ".repeat(depth + 1);
        for _ in 0..=rng.below(3) {
            match rng.below(if depth < 3 { 10 } else { 8 }) {
                0 => {
                    let m = rng.pick(&["", ".only", ".skip", ".todo"]);
                    let (tr, name) = (trivia(rng), title(rng));
                    let receiver = rng.pick(&receivers.tests);
                    let _ = writeln!(
                        out,
                        "{indent}{receiver}{m}{tr}({}, () => {{ expect(1).toBe(1); }});",
                        literal(rng, &name)
                    );
                }
                1 => {
                    let (tr, name) = (trivia(rng), title(rng));
                    let receiver = rng.pick(&receivers.tests);
                    let _ = writeln!(
                        out,
                        "{indent}{receiver}{tr}({}, () => {{}});",
                        literal(rng, &name)
                    );
                }
                2 => {
                    let name = title(rng);
                    let _ = writeln!(out, "{indent}// it({}, () => {{ decoy", literal(rng, &name));
                }
                3 => {
                    let _ = writeln!(
                        out,
                        "{indent}const s{} = \"describe(\\\"decoy\\\", () => {{{{\";",
                        rng.below(1000)
                    );
                }
                4 => {
                    let receiver = rng.pick(&receivers.each).clone();
                    let name = title(rng);
                    if rng.below(2) == 0 {
                        let _ = writeln!(
                            out,
                            "{indent}{receiver}.each([[1], [2]])({}, () => {{}});",
                            literal(rng, &name)
                        );
                    } else {
                        let _ = writeln!(
                            out,
                            "{indent}{receiver}.each`value | expected\n1 | 1`({}, () => {{}});",
                            literal(rng, &name)
                        );
                    }
                }
                5 => {
                    let alias = format!("local_alias_{}", rng.below(1000));
                    let target = rng.pick(&["it", "test", "describe"]);
                    let _ = writeln!(
                        out,
                        "{indent}function bind_{alias}() {{ const {alias} = {target}; {alias}(\"ignored\", () => {{}}); }}"
                    );
                }
                6 => {
                    let alias = format!("loop_alias_{}", rng.below(1000));
                    let target = rng.pick(&["it", "test", "describe"]);
                    let _ = writeln!(
                        out,
                        "{indent}for (const {alias} = {target}; false;) {{ {alias}(\"ignored\", () => {{}}); }}"
                    );
                }
                _ => {
                    let f = rng.pick(&receivers.blocks);
                    let (tr, name) = (trivia(rng), title(rng));
                    let _ = writeln!(out, "{indent}{f}{tr}({}, () => {{", literal(rng, &name));
                    gen_items(rng, depth + 1, receivers, out);
                    let _ = writeln!(out, "{indent}}});");
                }
            }
        }
    }

    fn gen_file(rng: &mut Rng) -> String {
        use std::fmt::Write;
        let mut out = String::from("import { describe, it } from \"vitest\";\n");
        let mut receivers = Receivers {
            tests: vec!["it".to_owned(), "test".to_owned()],
            blocks: ["describe", "suite", "describe.only", "test.describe"]
                .map(str::to_owned)
                .to_vec(),
            each: vec!["it".to_owned(), "test".to_owned(), "describe".to_owned()],
        };
        for group in 0..rng.below(3) {
            let base = rng.pick(&["it", "test", "describe"]);
            let chain_len = 1 + rng.below(3);
            let mut value = (*base).to_owned();
            for link in 0..chain_len {
                let alias = format!("alias_{group}_{link}");
                let export = if rng.below(4) == 0 { "export " } else { "" };
                let binding = rng.pick(&["const", "let", "var"]);
                let _ = writeln!(out, "{export}{binding} {alias} = {value};");
                value = alias;
            }
            receivers.each.push(value.clone());
            if matches!(*base, "it" | "test") {
                receivers.tests.push(value);
            } else {
                receivers.blocks.push(value);
            }
        }
        for _ in 0..=rng.below(2) {
            let name = title(rng);
            let _ = writeln!(out, "describe({}, () => {{", literal(rng, &name));
            gen_items(rng, 0, &receivers, &mut out);
            out.push_str("});\n");
        }
        out
    }

    #[test]
    fn emits_aliases_and_parameterized_tests() {
        let mut alias_files = 0;
        let mut curried_each_files = 0;
        let mut tagged_each_files = 0;
        let mut aliased_each_files = 0;
        for seed in 0..64u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let source = gen_file(&mut rng);
            alias_files += usize::from(source.lines().any(|line| {
                !line.starts_with(' ')
                    && line
                        .split_ascii_whitespace()
                        .any(|word| word.starts_with("alias_"))
            }));
            curried_each_files += usize::from(source.contains(".each([["));
            tagged_each_files += usize::from(source.contains(".each`"));
            aliased_each_files += usize::from(
                source
                    .lines()
                    .any(|line| line.trim_start().starts_with("alias_") && line.contains(".each")),
            );
        }
        assert!(alias_files > 0, "generator emitted no alias declarations");
        assert!(
            curried_each_files > 0,
            "generator emitted no curried .each calls"
        );
        assert!(
            tagged_each_files > 0,
            "generator emitted no tagged .each calls"
        );
        assert!(
            aliased_each_files > 0,
            "generator emitted no aliased .each calls"
        );
    }

    #[test]
    fn never_diverges_from_the_native_extraction() {
        // 120 seeds keeps the richer files near the previous runtime; the cost is the native
        // (per-extract query compilation), not the lexical scan.
        let (native_p, lexical_p) = (native_pack(), lexical_pack());
        for seed in 0..120u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let source = gen_file(&mut rng);
            let native =
                extract::extract_with_findings(&native_p, Path::new("map.test.ts"), &source)
                    .unwrap_or_else(|e| panic!("seed {seed}: native failed: {e}\n---\n{source}"));
            let lexical =
                extract::extract_with_findings(&lexical_p, Path::new("map.test.ts"), &source)
                    .unwrap_or_else(|e| panic!("seed {seed}: lexical failed: {e}\n---\n{source}"));
            assert_eq!(
                native, lexical,
                "seed {seed} diverged\n--- source ---\n{source}\n--- native ---\n{native:#?}\n--- lexical ---\n{lexical:#?}"
            );
        }
    }
}

mod when_extracting_rust_lexically {
    use super::*;

    // The rust profile encodes the #[test] marker in the opener regex
    // (the native pack uses test_requires_marker + a sibling walk); both
    // must agree on which fns count. Covers the colocated shape
    // (#[cfg(test)] mod tests wrapper) and a tests/-style flat file.
    #[test]
    fn matches_the_native_extraction() {
        for source in [
            r#"
pub fn lookup() {}

#[cfg(test)]
mod tests {
    use super::*;
    fn helper() {}

    mod when_the_key_is_present {
        use super::*;

        #[test]
        fn returns_the_value() {
            assert_eq!(lookup("{key}"), "value");
        }

        mod when_the_value_was_overwritten {
            #[tokio::test]
            async fn returns_the_latest_value() {
                todo!("it returns the latest {{value}}");
            }
        }
    }

    mod when_the_key_is_absent {
        #[test]
        #[ignore]
        fn returns_none() {}
    }

    mod helpers_without_tests {
        fn not_a_test() {}
    }
}
"#,
            r#"
// #[test] fn decoy_in_comment() {}
/* mod decoy_block { #[test] fn nope() {} */
const DECOY: &str = "mod fake { #[test] fn also_fake() {{{ }";

mod when_running_flat {
    #[test]
    fn works() {}
}

#[test]
fn top_level_test() {}
"#,
        ] {
            let native = extract::extract(&rust_native_pack(), Path::new("map.rs"), source)
                .unwrap_or_else(|e| panic!("native failed: {e}\n---\n{source}"));
            let lexical = extract::extract(&rust_lexical_pack(), Path::new("map.rs"), source)
                .unwrap_or_else(|e| panic!("lexical failed: {e}\n---\n{source}"));
            assert_eq!(
                native, lexical,
                "backends diverged\n--- source ---\n{source}\n--- native ---\n{native:#?}\n--- lexical ---\n{lexical:#?}"
            );
        }
    }
}

mod when_fuzzing_random_rust_files {
    use super::*;
    use std::fmt::Write;

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }

        fn below(&mut self, n: usize) -> usize {
            usize::try_from(self.next() % n as u64).unwrap()
        }

        fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
            &items[self.below(items.len())]
        }
    }

    const WORDS: &[&str] = &[
        "when", "the", "key", "value", "present", "absent", "returns", "latest", "none", "over",
    ];

    fn ident(rng: &mut Rng) -> String {
        let n = 1 + rng.below(3);
        let parts: Vec<&str> = (0..n).map(|_| *rng.pick(WORDS)).collect();
        format!("{}_{}", parts.join("_"), rng.below(100))
    }

    fn gen_items(rng: &mut Rng, depth: usize, out: &mut String) {
        let indent = "    ".repeat(depth + 1);
        for _ in 0..=rng.below(3) {
            match rng.below(if depth < 3 { 6 } else { 4 }) {
                0 => {
                    let marker = rng.pick(&["#[test]", "#[tokio::test]"]);
                    let extra = rng.pick(&["", "#[ignore]\n"]);
                    let extra = extra.replace('\n', &format!("\n{indent}"));
                    let _ = writeln!(
                        out,
                        "{indent}{marker}\n{indent}{extra}fn {}() {{\n{indent}    todo!(\"it {{braced}} \\\"quoted\\\"\");\n{indent}}}",
                        ident(rng)
                    );
                }
                1 => {
                    let _ = writeln!(out, "{indent}fn {}() {{}}", ident(rng));
                }
                2 => {
                    let _ = writeln!(out, "{indent}// #[test] fn {}() {{ decoy", ident(rng));
                }
                3 => {
                    let _ = writeln!(
                        out,
                        "{indent}const S{}: &str = \"mod fake {{ #[test] fn f() {{{{( \";",
                        rng.below(1000)
                    );
                }
                _ => {
                    let _ = writeln!(
                        out,
                        "{indent}mod {} {{\n{indent}    use super::*;",
                        ident(rng)
                    );
                    gen_items(rng, depth + 1, out);
                    let _ = writeln!(out, "{indent}}}");
                }
            }
        }
    }

    fn gen_file(rng: &mut Rng) -> String {
        let mut out = String::from("pub fn code() {}\n\n");
        if rng.below(2) == 0 {
            out.push_str("#[cfg(test)]\nmod tests {\n    use super::*;\n");
            gen_items(rng, 0, &mut out);
            out.push_str("}\n");
        } else {
            let mut inner = String::new();
            gen_items(rng, 0, &mut inner);
            // Flat, tests/-style file: strip one indent level.
            for line in inner.lines() {
                let _ = writeln!(out, "{}", line.strip_prefix("    ").unwrap_or(line));
            }
        }
        out
    }

    #[test]
    fn never_diverges_from_the_native_extraction() {
        let (native_p, lexical_p) = (rust_native_pack(), rust_lexical_pack());
        for seed in 0..100u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let source = gen_file(&mut rng);
            let native = extract::extract(&native_p, Path::new("map.rs"), &source)
                .unwrap_or_else(|e| panic!("seed {seed}: native failed: {e}\n---\n{source}"));
            let lexical = extract::extract(&lexical_p, Path::new("map.rs"), &source)
                .unwrap_or_else(|e| panic!("seed {seed}: lexical failed: {e}\n---\n{source}"));
            assert_eq!(
                native, lexical,
                "seed {seed} diverged\n--- source ---\n{source}\n--- native ---\n{native:#?}\n--- lexical ---\n{lexical:#?}"
            );
        }
    }
}

mod when_a_raw_name_profile_extracts_declarations {
    use super::*;

    // `name_syntax = "raw"` covers declaration-pattern tests: the name is
    // a code identifier, not a string literal, and the span bracket (the
    // parameter list or body brace) follows the name. This is the shape
    // Solidity/Foundry, Go, and pytest conventions take.
    const SOLIDITY_MANIFEST: &str = r#"
format = 1

[pack]
name = "solidity-lex"
version = "0.0.1"

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
strings = [{ delim = '"', escape = '\' }]
nest = [["(", ")"], ["{", "}"]]

[lexical.block]
open = '''(?:^|[^\w$])(?<kw>contract)\s+(?<name>\w+)[^{]*\{'''

[lexical.test]
open = '''(?:^|[^\w$])(?<kw>function)\s+(?<name>test\w*)\s*\('''

[scaffold]
template = "templates/test.jinja"
output = "{stem}.t.sol"
"#;

    #[test]
    fn nests_tests_by_bracket_spans() {
        let p = super::fixture_pack("solidity", SOLIDITY_MANIFEST).unwrap();
        let source = r#"
// SPDX-License-Identifier: MIT
contract MapTest is Test {
    function setUp() public {}
    function test_present() public {
        assert(lookup("key") != 0);
    }
    function test_absent() public {}
}
"#;
        let actual = extract::extract(&p, Path::new("map.t.sol"), source).unwrap();
        assert_eq!(actual.len(), 1, "{actual:#?}");
        assert_eq!(actual[0].name, "MapTest");
        let tests: Vec<&str> = actual[0].children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(tests, ["test_present", "test_absent"], "{actual:#?}");
    }
}

mod when_a_scaffolded_file_is_checked_lexically {
    use super::*;

    // Same round-trip contract the grammar-backed packs honor: whatever
    // the template escapes, lexical extraction must decode back.
    #[test]
    fn reports_no_findings_for_hostile_titles() {
        let spec = "HashMap\n└── it returns \"yes\" \\ {ok}\u{2028}more\n";
        let trees = tree::parse(spec).unwrap();
        let p = lexical_pack();
        let expected = check::expected_from_spec(&trees, &p.manifest.mapping);
        let out = scaffold::render(&p, &expected, "map", false).unwrap();
        let actual = extract::extract(&p, Path::new("map.test.ts"), &out).unwrap();
        let actual = check::unwrap_wrappers(actual, &p.manifest.mapping.wrappers);
        let findings = check::diff(&expected, &actual);
        assert!(findings.is_empty(), "{findings:?}\n---\n{out}");
    }
}

mod when_a_lexical_profile_is_malformed {
    use super::*;

    // A malformed profile fails the pack load with a clear error — it
    // must never reach extraction, where a degenerate token could hang
    // the scan or an absent capture group could panic it.
    #[test]
    fn refuses_to_load() {
        let base = std::fs::read_to_string(repo_root().join("packs-lexical/typescript/pack.toml"))
            .unwrap();
        for (label, from, to) in [
            (
                "empty delim",
                "{ delim = '\"', escape = '\\' }",
                "{ delim = '', escape = '\\' }",
            ),
            (
                "empty nest",
                r#"nest = [["(", ")"], ["{", "}"]]"#,
                "nest = []",
            ),
            (
                "multi-char bracket",
                r#"nest = [["(", ")"], ["{", "}"]]"#,
                r#"nest = [["((", "))"]]"#,
            ),
            ("missing name group", "(?<name>", "(?<title>"),
            ("invalid regex", "(?<kw>", "(?<kw>["),
        ] {
            let mutated = base.replace(from, to);
            assert_ne!(mutated, base, "{label}: mutation did not apply");
            let err = super::fixture_pack("malformed", &mutated).unwrap_err();
            assert!(matches!(err, btt::Error::Lexical { .. }), "{label}: {err}");
        }
    }
}

mod when_the_scanner_cannot_account_for_the_source {
    use super::*;

    // Syntax the profile cannot balance or terminate must be a loud tool
    // error, never a silent partial extraction that mis-nests or drops
    // tests (an unterminated string used to mask to EOF and "succeed"
    // with whatever preceded it).
    #[test]
    fn fails_closed() {
        for source in [
            "describe(\"unclosed\", () => {\n",
            "const stray = 1; }\n",
            "it(\"kept\", () => {});\nconst broken = \"unterminated\n",
            "it(\"kept\", () => {});\n/* never closed\n",
        ] {
            let err = extract::extract(&lexical_pack(), Path::new("map.test.ts"), source)
                .expect_err(source);
            assert!(matches!(err, btt::Error::Lexical { .. }), "{source}: {err}");
        }
    }
}
