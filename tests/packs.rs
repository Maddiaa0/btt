//! End-to-end tests for the builtin language packs, driven through the
//! public library API. Sources are in-memory; no fixture files needed.

use btt::check::{self, FindingKind};
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
        let kinds: Vec<FindingKind> = findings.iter().map(|f| f.kind).collect();
        assert!(kinds.contains(&FindingKind::MissingTest), "{findings:?}");
        assert!(kinds.contains(&FindingKind::ExtraTest), "{findings:?}");
    }
}

mod when_scaffolding_from_a_tree {
    use super::*;

    #[test]
    fn renders_rust_modules_and_test_functions() {
        let (p, expected) = expected_for("rust");
        let out = scaffold::render(&p, &expected, "map").unwrap();
        assert!(out.contains("mod when_the_key_is_present {"), "{out}");
        assert!(out.contains("fn returns_the_value()"), "{out}");
        assert!(out.contains("#[test]"), "{out}");
    }

    #[test]
    fn renders_nested_describe_blocks() {
        let (p, expected) = expected_for("typescript");
        let out = scaffold::render(&p, &expected, "map").unwrap();
        assert!(out.contains(r#"describe("HashMap", () => {"#), "{out}");
        assert!(out.contains(r#"  describe("when the key is absent", () => {"#), "{out}");
        assert!(out.contains(r#"it("returns none", () => {"#), "{out}");
    }
}
