//! End-to-end tests for the builtin language packs, driven through the
//! public library API. Sources are in-memory; no fixture files needed.

use btt::check::{self, Finding};
use btt::config::{CheckConfig, Level};
use btt::extract::ActualKind;
use btt::{extract, pack, runner, scaffold, tree};
use std::path::Path;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

const TODO_MARKER: &str = "btt:todo";

fn check_source(pack_name: &str, source: &str, cfg: CheckConfig) -> Vec<runner::Reported> {
    let dir = repo_root()
        .join("target/todo-marker-fixtures")
        .join(pack_name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let tree_path = dir.join("map.tree");
    let target = if pack_name == "rust" {
        dir.join("map.rs")
    } else {
        dir.join("map.test.ts")
    };
    std::fs::write(&tree_path, "Map\n└── it works\n").unwrap();
    std::fs::write(&target, source).unwrap();
    let pack = pack::load(pack_name, repo_root()).unwrap();
    runner::check_file(&pack, &tree_path, &target, &cfg).unwrap()
}

const SPEC: &str = "\
HashMap
├── when the key is present
│   └── it returns the value
└── when the key is absent
    └── it returns none
";

fn ts_findings(source: &str) -> Vec<check::Finding> {
    let p = pack::load("typescript", repo_root()).unwrap();
    let trees = tree::parse(SPEC).unwrap();
    let expected = check::expected_from_spec(&trees, &p.manifest.mapping);
    let actual = extract::extract(&p, Path::new("map.test.ts"), source).unwrap();
    let actual = check::unwrap_wrappers(actual, &p.manifest.mapping.wrappers);
    check::diff(&expected, &actual)
}

fn expected_for(pack_name: &str) -> (pack::Pack, Vec<check::Expected>) {
    let p = pack::load(pack_name, repo_root()).unwrap();
    let trees = tree::parse(SPEC).unwrap();
    let expected = check::expected_from_spec(&trees, &p.manifest.mapping);
    (p, expected)
}

mod when_checking_a_matching_typescript_file {
    use super::*;

    #[test]
    fn reports_no_findings() {
        let source = r#"
import { describe, it } from "vitest";
describe("HashMap", () => {
  describe("when the key is present", () => {
    it("returns the value", () => {});
  });
  describe("when the key is absent", () => {
    it.skip("returns none", () => {});
  });
});
"#;
        let findings = ts_findings(source);
        assert!(findings.is_empty(), "{findings:?}");
    }
}

mod when_checking_a_drifted_typescript_file {
    use super::*;

    #[test]
    fn reports_the_missing_and_extra_test() {
        let source = r#"
describe("HashMap", () => {
  describe("when the key is present", () => {
    it("returns the value", () => {});
  });
  describe("when the key is absent", () => {
    it("returns undefined", () => {});
  });
});
"#;
        let findings = ts_findings(source);
        let missing = findings.iter().any(|f| {
            matches!(
                f,
                Finding::Missing {
                    kind: ActualKind::Test,
                    ..
                }
            )
        });
        let extra = findings.iter().any(|f| {
            matches!(
                f,
                Finding::Extra {
                    kind: ActualKind::Test,
                    ..
                }
            )
        });
        assert!(missing && extra, "{findings:?}");
    }
}

mod when_checking_a_playwright_style_file {
    use super::*;

    #[test]
    fn treats_test_describe_as_a_block() {
        let source = r#"
import { test } from "@playwright/test";
test.describe("HashMap", () => {
  test.describe("when the key is present", () => {
    test("returns the value", () => {});
  });
  test.describe("when the key is absent", () => {
    test("returns none", () => {});
  });
});
"#;
        let findings = ts_findings(source);
        assert!(findings.is_empty(), "{findings:?}");
    }
}

mod when_extracting_parameterized_typescript_tests {
    use super::*;

    #[test]
    fn reports_supported_forms_and_ignores_unrelated_each_calls() {
        let source = "test.each(cases)(\"case %s\", fn);\n\
it.each`value | expected`(\"case $value\", fn);\n\
describe.each(cases)(\"group %s\", fn);\n\
myArray.each(fn);\n\
holder.test.each(cases)(\"not supported syntax\", fn);\n";
        let pack = pack::load("typescript", repo_root()).unwrap();
        let result =
            extract::extract_with_findings(&pack, Path::new("map.test.ts"), source).unwrap();
        let lines: Vec<_> = result
            .unsupported
            .iter()
            .map(|finding| finding.line)
            .collect();
        assert_eq!(lines, [1, 2, 3]);
        assert!(result.nodes.is_empty(), "{:?}", result.nodes);
    }

    #[test]
    fn reports_each_through_a_test_alias() {
        let source = "const t = test;\nt.each(cases)(\"case\", fn);\n";
        let pack = pack::load("typescript", repo_root()).unwrap();
        let result =
            extract::extract_with_findings(&pack, Path::new("map.test.ts"), source).unwrap();
        assert_eq!(
            result
                .unsupported
                .iter()
                .map(|finding| finding.line)
                .collect::<Vec<_>>(),
            [2]
        );
    }
}

mod when_resolving_typescript_aliases {
    use super::*;
    use std::fmt::Write;

    #[test]
    fn resolves_direct_conditional_and_transitive_aliases() {
        let source = r#"
const d = flag ? describe : describe.skip;
const d2 = d;
const t = it;
d2("HashMap", () => {
  d("when the key is present", () => { t("returns the value", () => {}); });
  describe("when the key is absent", () => { it("returns none", () => {}); });
});
"#;
        assert!(ts_findings(source).is_empty());
    }

    #[test]
    fn bounds_a_ten_thousand_link_chain() {
        let mut source = String::new();
        for i in 0..10_000 {
            let _ = writeln!(source, "const a{i} = a{};", i + 1);
        }
        source.push_str(
            "const a10000 = describe;\na0(\"HashMap\", () => { it(\"unresolved\", f); });",
        );
        let pack = pack::load("typescript", repo_root()).unwrap();
        let actual = extract::extract(&pack, Path::new("map.test.ts"), &source).unwrap();
        assert!(actual.iter().all(|node| node.kind != ActualKind::Block));
    }

    #[test]
    fn ignores_aliases_outside_module_scope() {
        let source =
            "function bind() { const d = describe; }\nd(\"HashMap\", () => { it(\"ghost\", f); });";
        let pack = pack::load("typescript", repo_root()).unwrap();
        let actual = extract::extract(&pack, Path::new("map.test.ts"), source).unwrap();
        assert!(
            actual.iter().all(|node| node.kind != ActualKind::Block),
            "{actual:?}"
        );
    }
}

mod when_scaffolding_from_a_tree {
    use super::*;

    #[test]
    fn renders_rust_modules_and_test_functions() {
        let (p, expected) = expected_for("rust");
        let out = scaffold::render(&p, &expected, "map", false).unwrap();
        assert!(out.contains("mod when_the_key_is_present {"), "{out}");
        assert!(out.contains("fn returns_the_value()"), "{out}");
        assert!(out.contains("#[test]"), "{out}");
        assert!(out.contains(TODO_MARKER), "{out}");
    }

    #[test]
    fn renders_nested_describe_blocks() {
        let (p, expected) = expected_for("typescript");
        let out = scaffold::render(&p, &expected, "map", false).unwrap();
        assert!(out.contains(r#"describe("HashMap", () => {"#), "{out}");
        assert!(
            out.contains(r#"  describe("when the key is absent", () => {"#),
            "{out}"
        );
        assert!(out.contains(r#"it("returns none", () => {"#), "{out}");
        assert!(out.contains(TODO_MARKER), "{out}");
    }
}

mod when_merging_a_scaffold {
    use super::*;

    fn expected(pack: &pack::Pack, spec: &str) -> Vec<check::Expected> {
        check::expected_from_spec(&tree::parse(spec).unwrap(), &pack.manifest.mapping)
    }

    #[test]
    fn inserts_rust_leaves_and_blocks_without_changing_existing_bytes() {
        let pack = pack::load("rust", repo_root()).unwrap();
        let spec = "Map\n├── when present\n│   ├── it keeps body\n│   └── it adds leaf\n└── when absent\n    └── it adds block leaf\n";
        let source = "mod when_present {\n    use super::*;\n\n    #[test]\n    fn keeps_body() {\n        assert_eq!(2 + 2, 4); // precious body\n    }\n}\n";
        let merged = scaffold::merge(
            &pack,
            &expected(&pack, spec),
            Path::new("tests/map.rs"),
            source,
            "map",
            true,
        )
        .unwrap();
        assert!(merged.contains("        // btt:todo"), "{merged}");
        assert!(
            merged.contains("    #[test]\n    fn adds_leaf()"),
            "{merged}"
        );
        assert!(merged.contains("mod when_absent {"), "{merged}");
        assert!(
            merged.contains("assert_eq!(2 + 2, 4); // precious body"),
            "{merged}"
        );
        let actual = extract::extract(&pack, Path::new("tests/map.rs"), &merged).unwrap();
        let findings = check::diff(&expected(&pack, spec), &actual);
        assert!(findings.is_empty(), "{findings:?}\n{merged}");
    }

    #[test]
    fn inserts_typescript_leaves_in_sibling_order_and_is_idempotent() {
        let pack = pack::load("typescript", repo_root()).unwrap();
        let spec = "Map\n├── it first\n├── it middle\n└── it last\n";
        let source = "describe(\"Map\", () => {\n  it(\"first\", () => { expect(1).toBe(1); });\n  // keep this comment exactly here\n  it(\"last\", () => { expect(3).toBe(3); });\n});\n";
        let once = scaffold::merge(
            &pack,
            &expected(&pack, spec),
            Path::new("map.test.ts"),
            source,
            "map",
            false,
        )
        .unwrap();
        let twice = scaffold::merge(
            &pack,
            &expected(&pack, spec),
            Path::new("map.test.ts"),
            &once,
            "map",
            false,
        )
        .unwrap();
        assert_eq!(once, twice);
        let first = once.find("first").unwrap();
        let middle = once.find("middle").unwrap();
        let last = once.find("last").unwrap();
        assert!(first < middle && middle < last, "{once}");
        assert!(once.contains("  // btt:todo — it middle"), "{once}");
        assert!(
            once.contains("  // keep this comment exactly here"),
            "{once}"
        );
    }

    #[test]
    fn fails_closed_on_ambiguous_duplicates() {
        let pack = pack::load("typescript", repo_root()).unwrap();
        let spec = "Map\n├── it same\n└── it new\n";
        let source = "describe(\"Map\", () => {\n  it(\"same\", () => {});\n  it(\"same\", () => {});\n});\n";
        let error = scaffold::merge(
            &pack,
            &expected(&pack, spec),
            Path::new("map.test.ts"),
            source,
            "map",
            false,
        )
        .unwrap_err();
        assert!(error.to_string().contains("merge manually"), "{error}");
    }

    #[test]
    fn fails_closed_on_syntax_errors() {
        let pack = pack::load("typescript", repo_root()).unwrap();
        let spec = "Map\n├── it existing\n└── it new\n";
        let malformed = "describe(\"Map\", () => {\n  it(\"existing\", () => {});\n";
        let error = scaffold::merge(
            &pack,
            &expected(&pack, spec),
            Path::new("map.test.ts"),
            malformed,
            "map",
            false,
        )
        .unwrap_err();
        assert!(error.to_string().contains("syntax errors"), "{error}");
        assert!(error.to_string().contains("merge manually"), "{error}");
    }

    #[test]
    fn merges_rust_and_typescript_siblings_after_a_non_newline_eof() {
        let rust = pack::load("rust", repo_root()).unwrap();
        let rust_spec = "Map\n├── it existing\n└── it added\n";
        let rust_source = "#[test]\nfn existing() {}";
        let rust_merged = scaffold::merge(
            &rust,
            &expected(&rust, rust_spec),
            Path::new("tests/map.rs"),
            rust_source,
            "map",
            true,
        )
        .unwrap();
        assert!(
            rust_merged.contains("{}\n#[test]\nfn added()"),
            "{rust_merged}"
        );
        let actual = extract::extract(&rust, Path::new("tests/map.rs"), &rust_merged).unwrap();
        assert!(check::diff(&expected(&rust, rust_spec), &actual).is_empty());

        let ts = pack::load("typescript", repo_root()).unwrap();
        let ts_spec = "First\n└── it existing\n\nSecond\n└── it added\n";
        let ts_source = "describe(\"First\", () => { it(\"existing\", () => {}); });";
        let ts_merged = scaffold::merge(
            &ts,
            &expected(&ts, ts_spec),
            Path::new("map.test.ts"),
            ts_source,
            "map",
            false,
        )
        .unwrap();
        assert!(ts_merged.contains(");\ndescribe(\"Second\""), "{ts_merged}");
        let actual = extract::extract(&ts, Path::new("map.test.ts"), &ts_merged).unwrap();
        assert!(check::diff(&expected(&ts, ts_spec), &actual).is_empty());
    }

    #[test]
    fn preserves_crlf_and_refuses_mixed_newlines() {
        let pack = pack::load("typescript", repo_root()).unwrap();
        let spec = "Map\n├── it existing\n└── it added\n";
        let crlf = "describe(\"Map\", () => {\r\n  it(\"existing\", () => {});\r\n});\r\n";
        let merged = scaffold::merge(
            &pack,
            &expected(&pack, spec),
            Path::new("map.test.ts"),
            crlf,
            "map",
            false,
        )
        .unwrap();
        assert!(merged.contains("\r\n  it(\"added\""), "{merged:?}");
        assert!(!merged.replace("\r\n", "").contains('\n'), "{merged:?}");

        let mixed = "describe(\"Map\", () => {\r\n  it(\"existing\", () => {});\n});\r\n";
        let error = scaffold::merge(
            &pack,
            &expected(&pack, spec),
            Path::new("map.test.ts"),
            mixed,
            "map",
            false,
        )
        .unwrap_err();
        assert!(error.to_string().contains("mixed CRLF and LF"), "{error}");
        assert!(error.to_string().contains("merge manually"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn atomically_replaces_while_preserving_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = repo_root().join("target/atomic-merge-fixture");
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("map.rs");
        std::fs::write(&target, "old").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        scaffold::write_atomic(&target, "new contents").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new contents");
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o755
        );
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".btt-"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }
}

mod when_checking_scaffold_markers {
    use super::*;

    fn two_tests() -> String {
        let marker = TODO_MARKER;
        format!(
            "describe(\"Map\", () => {{\n  it(\"works\", () => {{\n    // {marker}\n  }});\n}});\n"
        )
    }

    #[test]
    fn reports_each_marker_at_its_source_line_with_the_default_severity() {
        let findings = check_source("typescript", &two_tests(), CheckConfig::default());
        assert!(matches!(
            findings.as_slice(),
            [runner::Reported {
                finding: Finding::Todo {
                    target_line: 3,
                    test_line: Some(2)
                },
                level: Level::Warn
            }]
        ));
    }

    #[test]
    fn clears_a_finding_when_its_marker_is_removed() {
        let source = two_tests().replace(TODO_MARKER, "body filled");
        assert!(check_source("typescript", &source, CheckConfig::default()).is_empty());
    }

    #[test]
    fn ignores_the_marker_inside_a_string_literal() {
        let source = format!(
            "describe(\"Map\", () => {{ it(\"works\", () => {{ expect(\"{TODO_MARKER}\").toBeTruthy(); }}); }});\n"
        );
        assert!(check_source("typescript", &source, CheckConfig::default()).is_empty());
    }

    #[test]
    fn reports_a_marker_outside_a_test_span() {
        let source = format!(
            "// {TODO_MARKER}\ndescribe(\"Map\", () => {{ it(\"works\", () => {{}}); }});\n"
        );
        let findings = check_source("typescript", &source, CheckConfig::default());
        assert!(matches!(
            findings.as_slice(),
            [runner::Reported {
                finding: Finding::Todo {
                    target_line: 1,
                    test_line: None
                },
                ..
            }]
        ));
    }
}

mod when_scaffolding_hostile_spec_text {
    use super::*;

    #[test]
    fn escapes_quotes_and_format_braces() {
        let spec = "HashMap\n└── it returns \"yes\" for {braced} input\n";
        let trees = tree::parse(spec).unwrap();

        let ts = pack::load("typescript", repo_root()).unwrap();
        let expected = check::expected_from_spec(&trees, &ts.manifest.mapping);
        let out = scaffold::render(&ts, &expected, "map", false).unwrap();
        assert!(
            out.contains(r#"it("returns \"yes\" for {braced} input""#),
            "{out}"
        );

        let rust = pack::load("rust", repo_root()).unwrap();
        let expected = check::expected_from_spec(&trees, &rust.manifest.mapping);
        let out = scaffold::render(&rust, &expected, "map", false).unwrap();
        assert!(
            out.contains(r#"todo!("it returns \"yes\" for {{braced}} input")"#),
            "{out}"
        );
    }
}

mod when_a_scaffolded_file_is_checked {
    use super::*;

    // The scaffold → check round trip is a contract: whatever escaping the
    // template applies, extraction must undo — otherwise scaffolding a
    // hostile title produces a file the tool itself then flags. U+2028 is
    // a JS line terminator; quotes and backslashes exercise escaping.
    #[test]
    fn reports_no_findings_for_hostile_titles() {
        let spec = "HashMap\n└── it returns \"yes\" \\ {ok}\u{2028}more\n";
        let trees = tree::parse(spec).unwrap();
        for (name, target) in [("typescript", "map.test.ts"), ("rust", "map.rs")] {
            let p = pack::load(name, repo_root()).unwrap();
            let expected = check::expected_from_spec(&trees, &p.manifest.mapping);
            let out = scaffold::render(&p, &expected, "map", false).unwrap();
            let actual = extract::extract(&p, Path::new(target), &out).unwrap();
            let actual = check::unwrap_wrappers(actual, &p.manifest.mapping.wrappers);
            let findings = check::diff(&expected, &actual);
            assert!(findings.is_empty(), "{name}: {findings:?}\n---\n{out}");
        }
    }
}

mod when_a_builtin_pack_has_a_wasm_twin {
    use super::*;

    // The packs-wasm/ twins differ only in their [grammar] section; queries
    // and templates must stay byte-identical so a fix to one can't silently
    // miss the other.
    #[test]
    fn keeps_queries_and_templates_identical() {
        for name in ["rust", "typescript"] {
            for rel in ["queries/tests.scm", "templates/test.jinja"] {
                let read = |base: &str| {
                    std::fs::read_to_string(repo_root().join(base).join(name).join(rel)).unwrap()
                };
                assert_eq!(
                    read("packs"),
                    read("packs-wasm"),
                    "packs-wasm/{name}/{rel} drifted from packs/{name}/{rel}"
                );
            }
        }
    }

    #[test]
    fn keeps_release_metadata_identical() {
        for name in ["rust", "typescript"] {
            let native = pack::load_dir(&repo_root().join("packs").join(name)).unwrap();
            let wasm = pack::load_dir(&repo_root().join("packs-wasm").join(name)).unwrap();
            assert_eq!(
                native.manifest.pack.version, wasm.manifest.pack.version,
                "packs-wasm/{name} has a different release version"
            );
            assert_eq!(
                native.manifest.format, wasm.manifest.format,
                "packs-wasm/{name} has a different manifest format"
            );
            assert_eq!(
                native.manifest.compat.btt, wasm.manifest.compat.btt,
                "packs-wasm/{name} has different btt compatibility"
            );
        }
    }
}
