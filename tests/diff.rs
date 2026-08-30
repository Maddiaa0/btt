//! End-to-end coverage for revision-to-revision behavior diffs.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("btt-diff-tests-{}", std::process::id()))
        .join(name);
    _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q"]);
    dir
}

fn git(dir: &Path, args: &[&str]) -> Output {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "BTT Tests")
        .env("GIT_AUTHOR_EMAIL", "btt@example.invalid")
        .env("GIT_COMMITTER_NAME", "BTT Tests")
        .env("GIT_COMMITTER_EMAIL", "btt@example.invalid")
        .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn commit(dir: &Path, message: &str) {
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", message]);
}

fn btt(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_btt"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

fn write(dir: &Path, path: &str, source: &str) {
    let path = dir.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, source).unwrap();
}

fn seeded(name: &str) -> PathBuf {
    let dir = fixture(name);
    write(&dir, "one.tree", "suite\n└── it old\n");
    write(&dir, "deleted.tree", "deleted\n└── it gone\n");
    commit(&dir, "old");
    dir
}

mod when_a_revision_is_compared_with_the_working_tree {
    use super::*;

    #[test]
    fn reports_added_removed_and_conservatively_renamed_behaviors() {
        let dir = fixture("working-structural");
        write(
            &dir,
            "one.tree",
            "suite\n└── when context\n    └── it old\n",
        );
        commit(&dir, "old");
        write(
            &dir,
            "one.tree",
            "suite\n├── when context\n│   └── it renamed\n└── it added\n",
        );
        let output = btt(&dir, &["diff", "HEAD"]);
        assert!(output.status.success());
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(
            stdout.contains("~ suite > when context > it old -> suite > when context > it renamed"),
            "{stdout}"
        );
        assert!(stdout.contains("+ suite > it added"), "{stdout}");
    }

    #[test]
    fn reports_whole_file_additions_and_deletions() {
        let dir = seeded("whole-files");
        std::fs::remove_file(dir.join("deleted.tree")).unwrap();
        write(&dir, "added.tree", "added\n└── it new\n");
        let stdout = String::from_utf8(btt(&dir, &["diff", "HEAD"]).stdout).unwrap();
        assert!(
            stdout.contains("ADDED added.tree\n  + added > it new"),
            "{stdout}"
        );
        assert!(
            stdout.contains("REMOVED deleted.tree\n  - deleted > it gone"),
            "{stdout}"
        );
    }

    #[test]
    fn sees_uncommitted_tree_changes() {
        let dir = seeded("uncommitted");
        write(&dir, "one.tree", "suite\n└── it changed\n");
        assert!(
            !btt(&dir, &["diff", "HEAD", "--format", "json"])
                .stdout
                .is_empty()
        );
    }
}

mod when_two_revisions_are_compared {
    use super::*;

    fn revised(name: &str) -> PathBuf {
        let dir = seeded(name);
        write(&dir, "one.tree", "suite\n└── it new\n");
        commit(&dir, "new");
        dir
    }

    #[test]
    fn emits_the_documented_json_shape() {
        let dir = revised("json");
        let output = btt(&dir, &["diff", "HEAD^..HEAD", "--format", "json"]);
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["summary"]["renamed"], 1);
        assert_eq!(json["results"][0]["status"], "changed");
        assert_eq!(json["results"][0]["renamed"][0]["from"], "suite > it old");
        assert!(json.get("error").is_none());
    }

    #[test]
    fn returns_one_with_exit_code_when_differences_exist() {
        let dir = revised("exit-one");
        assert_eq!(
            btt(&dir, &["diff", "HEAD^..HEAD", "--exit-code"])
                .status
                .code(),
            Some(1)
        );
    }

    #[test]
    fn returns_zero_for_identical_revisions() {
        let dir = revised("identical");
        let output = btt(&dir, &["diff", "HEAD..HEAD", "--exit-code"]);
        assert!(output.status.success());
        assert!(
            String::from_utf8(output.stdout)
                .unwrap()
                .contains("0 tree file(s) changed")
        );
    }
}

mod when_invoked_below_the_repository_root {
    use super::*;

    #[test]
    fn discovers_trees_across_the_repository() {
        let dir = seeded("subdirectory");
        let subdir = dir.join("nested/deep");
        std::fs::create_dir_all(&subdir).unwrap();
        write(&dir, "one.tree", "suite\n└── it changed\n");
        let stdout = String::from_utf8(btt(&subdir, &["diff", "HEAD"]).stdout).unwrap();
        assert!(stdout.contains("CHANGED one.tree"), "{stdout}");
    }
}

mod when_the_comparison_cannot_be_made {
    use super::*;

    #[test]
    fn clearly_reports_an_unknown_revision() {
        let dir = seeded("bad-rev-human");
        let output = btt(&dir, &["diff", "does-not-exist"]);
        assert_eq!(output.status.code(), Some(2));
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("cannot resolve revision does-not-exist")
        );
    }

    #[test]
    fn rejects_a_leading_dash_revision_without_git_usage_output() {
        let dir = seeded("leading-dash-human");
        let output = btt(&dir, &["diff", "--", "-v"]);
        assert_eq!(output.status.code(), Some(2));
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("cannot resolve revision -v"), "{stderr}");
        assert!(!stderr.contains("usage:"), "{stderr}");
        assert_eq!(stderr.lines().count(), 1, "{stderr}");
    }

    #[test]
    fn emits_a_json_envelope_for_a_leading_dash_revision() {
        let dir = seeded("leading-dash-json");
        let output = btt(&dir, &["diff", "--format", "json", "--", "-v"]);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stderr.is_empty());
        let stdout = String::from_utf8(output.stdout).unwrap();
        let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert!(
            json["error"]
                .as_str()
                .unwrap()
                .contains("cannot resolve revision -v")
        );
        assert!(!stdout.contains("usage:"), "{stdout}");
        assert_eq!(stdout.lines().count(), 1);
    }

    #[test]
    fn emits_one_json_error_object_and_exits_two() {
        let dir = seeded("bad-rev-json");
        let output = btt(&dir, &["diff", "does-not-exist", "--format", "json"]);
        assert_eq!(output.status.code(), Some(2));
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(json["error"].as_str().unwrap().contains("does-not-exist"));
        assert_eq!(json["summary"]["tree_files"], 0);
        assert_eq!(String::from_utf8(output.stdout).unwrap().lines().count(), 1);
    }
}
