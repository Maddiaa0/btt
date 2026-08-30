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

fn check(root: &Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_btt"))
        .arg("check")
        .current_dir(root)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
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
}
