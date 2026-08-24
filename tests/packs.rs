//! End-to-end tests for the builtin language packs, driven through the
//! public library API. Sources are in-memory; no fixture files needed.

use btt::check::{self, Finding};
use btt::extract::ActualKind;
use btt::{extract, pack, scaffold, tree};
use std::path::Path;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
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

mod when_scaffolding_from_a_tree {
    use super::*;

    #[test]
    fn renders_rust_modules_and_test_functions() {
        let (p, expected) = expected_for("rust");
        let out = scaffold::render(&p, &expected, "map", false).unwrap();
        assert!(out.contains("mod when_the_key_is_present {"), "{out}");
        assert!(out.contains("fn returns_the_value()"), "{out}");
        assert!(out.contains("#[test]"), "{out}");
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
