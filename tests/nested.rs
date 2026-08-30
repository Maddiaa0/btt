//! End-to-end: nested `btt.toml` files in a monorepo govern the trees below
//! them, even when `btt check` runs from the repository root.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Build a two-language monorepo: a rust package under the root config and
/// a typescript package with its own `btt.toml`. The typescript test file
/// has one extra test, which errors under the root config (`extra =
/// "error"`) but is ignored under the nested one (`extra = "ignore"`).
fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("btt-nested-test-{}-{name}", std::process::id()));
    _ = std::fs::remove_dir_all(&root);
    let write = |rel: &str, contents: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    };
    let tree = "map\n└── when queried\n    └── it returns the value\n";
    write(
        "btt.toml",
        "[project]\npacks = [\"rust\"]\n\n[check]\nextra = \"error\"\nuncovered = \"error\"\n",
    );
    write("core/map.tree", tree);
    write(
        "core/map.rs",
        "#[cfg(test)]\nmod tests {\n    mod when_queried {\n        #[test]\n        fn returns_the_value() {}\n    }\n}\n",
    );
    write(
        "web/btt.toml",
        "[project]\npacks = [\"typescript\"]\n\n[check]\nextra = \"ignore\"\nuncovered = \"ignore\"\n",
    );
    write("web/map.tree", tree);
    write(
        "web/map.test.ts",
        "describe(\"map\", () => {\n  describe(\"when queried\", () => {\n    it(\"returns the value\", () => {});\n    it(\"is extra\", () => {});\n  });\n});\n",
    );
    write(
        "web/orphan.test.ts",
        "describe(\"orphan\", () => {\n  it(\"is uncovered\", () => {});\n});\n",
    );
    root
}

fn check_args(root: &Path, args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_btt"))
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

fn check(root: &Path) -> (bool, String) {
    let (code, stdout) = check_args(root, &["check"]);
    (code == 0, stdout)
}

fn check_path(root: &Path, path: &str) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_btt"))
        .args(["check", path])
        .current_dir(root)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

mod when_checking_a_monorepo_from_the_root {
    use super::*;

    #[test]
    fn routes_each_subtree_with_its_own_packs() {
        let root = fixture("packs");
        let (_, stdout) = check(&root);
        assert!(stdout.contains("✓ core/map.tree"), "{stdout}");
        assert!(stdout.contains("✓ web/map.tree"), "{stdout}");
    }

    #[test]
    fn applies_the_nested_check_severities() {
        let root = fixture("severities");
        let (ok, stdout) = check(&root);
        assert!(ok, "{stdout}");
        assert!(!stdout.contains("extra"), "{stdout}");
    }

    #[test]
    fn applies_the_nested_uncovered_severity() {
        let root = fixture("uncovered");
        let (ok, stdout) = check(&root);
        assert!(ok, "{stdout}");
        assert!(!stdout.contains("orphan"), "{stdout}");
    }

    #[test]
    fn ignores_a_broken_root_pack_when_checking_only_a_nested_subtree() {
        let root = fixture("broken-root-pack");
        std::fs::write(
            root.join("btt.toml"),
            "[project]\npacks = [\"does-not-exist\"]\n",
        )
        .unwrap();

        let (ok, stdout) = check_path(&root, "web");
        assert!(ok, "{stdout}");
        assert!(stdout.contains("✓ web/map.tree"), "{stdout}");
    }

    #[test]
    fn preserves_the_human_output_byte_for_byte() {
        let root = fixture("human-golden");
        let (code, stdout) = check_args(&root, &["check"]);
        assert_eq!(code, 0);
        assert_eq!(
            stdout,
            "✓ core/map.tree (map.rs)\n✓ web/map.tree (map.test.ts)\n\n2 tree file(s), 1 uncovered, 0 error(s), 0 warning(s)\n"
        );
    }

    #[test]
    fn emits_a_structured_clean_result() {
        let root = fixture("json-clean");
        let (code, stdout) = check_args(&root, &["check", "--format", "json"]);
        assert_eq!(code, 0, "{stdout}");
        let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(json["summary"]["tree_files"], 2);
        assert_eq!(json["summary"]["errors"], 0);
        assert_eq!(json["results"][0]["tree"], "core/map.tree");
        assert_eq!(json["results"][0]["target"], "core/map.rs");
        assert_eq!(json["results"][0]["status"], "pass");
        assert_eq!(json["results"][0]["findings"], serde_json::json!([]));
    }

    #[test]
    fn emits_structured_findings_and_preserves_the_exit_code() {
        let root = fixture("json-findings");
        std::fs::write(
            root.join("web/map.test.ts"),
            "describe(\"map\", () => {\n  test.each([[1]])(\"parameterized\", () => {});\n});\n",
        )
        .unwrap();
        std::fs::write(
            root.join("web/btt.toml"),
            "[project]\npacks = [\"typescript\"]\n\n[check]\nextra = \"ignore\"\nuncovered = \"ignore\"\nunsupported = \"error\"\n",
        )
        .unwrap();

        let (human_code, _) = check_args(&root, &["check", "--format", "human"]);
        let (json_code, stdout) = check_args(&root, &["check", "--format", "json"]);
        assert_eq!(json_code, human_code);
        assert_eq!(json_code, 1, "{stdout}");
        let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(json["summary"]["findings"]["unsupported"]["error"], 1);
        let unsupported = json["results"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|result| result["findings"].as_array().unwrap())
            .find(|finding| finding["kind"] == "unsupported")
            .unwrap();
        assert_eq!(unsupported["severity"], "error");
        assert!(
            unsupported["file"]
                .as_str()
                .unwrap()
                .ends_with("/web/map.test.ts")
        );
        assert_eq!(unsupported["line"], 2);
    }
}
