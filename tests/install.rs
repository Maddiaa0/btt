//! Tests for the pack install pipeline: allowlist staging, validation of
//! the staged copy, receipts, atomic commit/swap, curated-index
//! verification, git acquisition, guarded removal, and index freshness.
//! Fixtures live under `target/install-fixtures/<case>/`. The security
//! invariants covered here are enumerated in `docs/pack-install.md`.

use btt::error::Error;
use btt::{install, pack};
use std::path::{Path, PathBuf};

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// A fresh, empty fixture directory for one test case.
fn fixture_root(case: &str) -> PathBuf {
    let dir = repo_root().join("target/install-fixtures").join(case);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const MANIFEST: &str = r#"
[pack]
name = "fixture"
version = "0.0.1"
description = "test fixture pack"

[detect]
targets = ["{stem}.rs"]

[grammar]
source = "builtin:rust"

[extract]
query = "queries/tests.scm"

[scaffold]
template = "templates/test.jinja"
output = "{stem}.rs"
"#;

/// Write a loadable pack into `dir`, with the manifest name replaced.
fn write_pack(dir: &Path, name: &str) {
    std::fs::create_dir_all(dir.join("queries")).unwrap();
    std::fs::create_dir_all(dir.join("templates")).unwrap();
    std::fs::write(dir.join("queries/tests.scm"), "; query\n").unwrap();
    std::fs::write(dir.join("templates/test.jinja"), "// template\n").unwrap();
    std::fs::write(
        dir.join("pack.toml"),
        MANIFEST.replace("name = \"fixture\"", &format!("name = \"{name}\"")),
    )
    .unwrap();
}

/// Every file under `dir`, as sorted `/`-joined relative paths.
fn files_under(dir: &Path) -> Vec<String> {
    let mut files: Vec<String> = walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| {
            e.path()
                .strip_prefix(dir)
                .unwrap()
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect();
    files.sort();
    files
}

fn provenance() -> install::Provenance {
    install::Provenance {
        source: "path".to_string(),
        url: None,
        reference: None,
        commit: None,
    }
}

mod when_staging_a_valid_pack_from_a_path {
    use super::*;

    #[test]
    fn copies_only_the_manifest_closure() {
        let case = fixture_root("stage-allowlist");
        let source = case.join("some-folder");
        write_pack(&source, "fixture");
        std::fs::write(source.join("JUNK.md"), "ride-along").unwrap();
        std::fs::create_dir_all(source.join("queries/nested")).unwrap();
        std::fs::write(source.join("queries/nested/extra.scm"), "junk").unwrap();

        let staged = install::stage(&source, &case.join("packs")).unwrap();
        assert_eq!(
            files_under(staged.dir()),
            vec!["pack.toml", "queries/tests.scm", "templates/test.jinja"]
        );
    }

    #[test]
    fn names_the_staging_dir_after_the_manifest_name() {
        let case = fixture_root("stage-name");
        let source = case.join("folder-named-differently");
        write_pack(&source, "fixture");

        let staged = install::stage(&source, &case.join("packs")).unwrap();
        let dir_name = staged
            .dir()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(dir_name.starts_with("fixture."), "{dir_name}");
        assert!(staged.dir().parent().unwrap().ends_with(".staging"));
    }

    #[test]
    fn validates_the_staged_copy_with_the_loader() {
        let case = fixture_root("stage-validate");
        let source = case.join("src");
        write_pack(&source, "fixture");

        let staged = install::stage(&source, &case.join("packs")).unwrap();
        assert_eq!(staged.pack.name(), "fixture");
        // The loaded pack is the staged copy, not the source.
        assert_eq!(staged.pack.query, "; query\n");
    }
}

#[cfg(unix)]
mod when_the_source_contains_a_symlink {
    use super::*;

    #[test]
    fn refuses_the_install() {
        let case = fixture_root("stage-symlink");
        let source = case.join("src");
        write_pack(&source, "fixture");
        let outside = case.join("outside.jinja");
        std::fs::write(&outside, "leaked").unwrap();
        std::fs::remove_file(source.join("templates/test.jinja")).unwrap();
        std::os::unix::fs::symlink(&outside, source.join("templates/test.jinja")).unwrap();

        let err = install::stage(&source, &case.join("packs")).unwrap_err();
        assert!(matches!(err, Error::InstallUnsafeFile { .. }), "{err}");
        assert!(!case.join("packs/.staging").join("fixture").exists());
    }
}

mod when_a_source_file_exceeds_the_size_cap {
    use super::*;

    #[test]
    fn refuses_the_install() {
        let case = fixture_root("stage-oversize");
        let source = case.join("src");
        write_pack(&source, "fixture");
        let big = "x".repeat(usize::try_from(install::MAX_TEXT_BYTES).unwrap() + 1);
        std::fs::write(source.join("templates/test.jinja"), big).unwrap();

        let err = install::stage(&source, &case.join("packs")).unwrap_err();
        assert!(matches!(err, Error::InstallTooLarge { .. }), "{err}");
    }
}

mod when_the_manifest_name_is_not_a_single_path_component {
    use super::*;

    #[test]
    fn refuses_the_install() {
        let case = fixture_root("stage-bad-name");
        let source = case.join("src");
        write_pack(&source, "a/b");

        let err = install::stage(&source, &case.join("packs")).unwrap_err();
        assert!(matches!(err, Error::UnsafePath { .. }), "{err}");
    }
}

mod when_committing_a_staged_pack {
    use super::*;

    #[test]
    fn installs_under_the_manifest_name() {
        let case = fixture_root("commit-name");
        let source = case.join("oddly-named-source");
        write_pack(&source, "fixture");

        let staged = install::stage(&source, &case.join("packs")).unwrap();
        let installed = install::commit(staged, false).unwrap();
        assert_eq!(installed, case.join("packs/fixture"));
        assert!(pack::load_dir(&installed).is_ok());
    }

    #[test]
    fn writes_a_receipt_with_file_digests() {
        let case = fixture_root("commit-receipt");
        let source = case.join("src");
        write_pack(&source, "fixture");

        let staged = install::stage(&source, &case.join("packs")).unwrap();
        install::write_receipt(&staged, &provenance()).unwrap();
        let installed = install::commit(staged, false).unwrap();

        let receipt = install::read_receipt(&installed).expect("receipt present and parseable");
        assert_eq!(receipt.install.source, "path");
        let manifest_digest = receipt.files.get("pack.toml").expect("pack.toml digested");
        assert!(manifest_digest.starts_with("sha256:"), "{manifest_digest}");
        // Digests match the installed bytes.
        let p = pack::load_dir(&installed).unwrap();
        for f in install::file_digests(&installed, &p.manifest).unwrap() {
            assert_eq!(
                receipt.files.get(&f.rel),
                Some(&format!("sha256:{}", f.sha256)),
                "{}",
                f.rel
            );
        }
    }

    #[test]
    fn removes_the_staging_directory() {
        let case = fixture_root("commit-staging-gone");
        let source = case.join("src");
        write_pack(&source, "fixture");

        let staged = install::stage(&source, &case.join("packs")).unwrap();
        install::commit(staged, false).unwrap();
        assert!(!case.join("packs/.staging").exists());
    }
}

mod when_the_name_is_already_installed {
    use super::*;

    #[test]
    fn refuses_without_force() {
        let case = fixture_root("collision");
        let source = case.join("src");
        write_pack(&source, "fixture");
        let packs = case.join("packs");
        install::commit(install::stage(&source, &packs).unwrap(), false).unwrap();

        let staged = install::stage(&source, &packs).unwrap();
        let err = install::commit(staged, false).unwrap_err();
        assert!(matches!(err, Error::AlreadyInstalled { .. }), "{err}");
        // The refused install cleaned its staging dir up.
        assert!(!packs.join(".staging").exists());
    }

    #[test]
    fn swaps_atomically_with_force() {
        let case = fixture_root("force-swap");
        let source = case.join("src");
        write_pack(&source, "fixture");
        let packs = case.join("packs");
        install::commit(install::stage(&source, &packs).unwrap(), false).unwrap();

        std::fs::write(source.join("templates/test.jinja"), "// v2\n").unwrap();
        let staged = install::stage(&source, &packs).unwrap();
        let installed = install::commit(staged, true).unwrap();

        let template = std::fs::read_to_string(installed.join("templates/test.jinja")).unwrap();
        assert_eq!(template, "// v2\n");
        assert!(!packs.join(".trash").exists());
    }
}

mod when_a_staged_install_is_dropped_without_commit {
    use super::*;

    #[test]
    fn leaves_no_staging_residue() {
        let case = fixture_root("drop-cleanup");
        let source = case.join("src");
        write_pack(&source, "fixture");
        let packs = case.join("packs");

        let staged = install::stage(&source, &packs).unwrap();
        let staging_dir = staged.dir().to_path_buf();
        drop(staged);
        assert!(!staging_dir.exists());
    }
}

mod when_verifying_against_a_curated_index_entry {
    use super::*;
    use std::collections::BTreeMap;

    fn entry_for(staged: &install::Staged) -> install::IndexEntry {
        install::IndexEntry {
            name: staged.name().to_string(),
            kind: "builtin".to_string(),
            description: String::new(),
            dir: "packs-extra/fixture".to_string(),
            files: staged
                .files
                .iter()
                .map(|f| (f.rel.clone(), format!("sha256:{}", f.sha256)))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    #[test]
    fn accepts_matching_digests() {
        let case = fixture_root("verify-ok");
        let source = case.join("src");
        write_pack(&source, "fixture");
        let staged = install::stage(&source, &case.join("packs")).unwrap();

        let entry = entry_for(&staged);
        install::verify_curated(&staged, &entry).unwrap();
    }

    #[test]
    fn rejects_a_tampered_file() {
        let case = fixture_root("verify-tampered");
        let source = case.join("src");
        write_pack(&source, "fixture");
        let staged = install::stage(&source, &case.join("packs")).unwrap();

        let mut entry = entry_for(&staged);
        entry.files.insert(
            "queries/tests.scm".to_string(),
            "sha256:doctored".to_string(),
        );
        let err = install::verify_curated(&staged, &entry).unwrap_err();
        assert!(matches!(err, Error::DigestMismatch { .. }), "{err}");
    }
}

mod when_installing_from_a_git_source {
    use super::*;

    fn git(cwd: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    #[test]
    fn clones_resolves_and_stages_the_pack() {
        let case = fixture_root("git-source");
        let repo = case.join("repo");
        write_pack(&repo.join("mypack"), "mypack");
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "-A"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=btt-test",
                "-c",
                "user.email=btt@test",
                "commit",
                "-q",
                "-m",
                "pack",
            ],
        );

        let url = format!("file://{}", repo.display());
        let checkout = install::fetch_git(&url, None).unwrap();
        assert_eq!(checkout.commit.len(), 40, "{}", checkout.commit);

        let dirs = install::discover(checkout.dir());
        assert_eq!(dirs.len(), 1);
        let staged = install::stage(&dirs[0], &case.join("packs")).unwrap();
        assert_eq!(staged.name(), "mypack");
    }
}

mod when_removing_an_installed_pack {
    use super::*;

    fn installed_fixture(case: &str) -> (PathBuf, PathBuf) {
        let case = fixture_root(case);
        let source = case.join("src");
        write_pack(&source, "fixture");
        let packs = case.join("packs");
        install::commit(install::stage(&source, &packs).unwrap(), false).unwrap();
        (case, packs)
    }

    #[test]
    fn deletes_the_pack_directory() {
        let (_case, packs) = installed_fixture("rm-ok");
        let removed = install::remove(&packs, "fixture").unwrap();
        assert_eq!(removed, packs.join("fixture"));
        assert!(!removed.exists());
    }

    #[test]
    fn refuses_a_name_with_path_separators() {
        let (_case, packs) = installed_fixture("rm-traversal");
        for name in ["../fixture", "a/b", "/abs", ".."] {
            let err = install::remove(&packs, name).unwrap_err();
            assert!(matches!(err, Error::UnsafePath { .. }), "{name}: {err}");
        }
        assert!(packs.join("fixture").exists());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlinked_entry() {
        let (case, packs) = installed_fixture("rm-symlink");
        let elsewhere = case.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, packs.join("linked")).unwrap();

        let err = install::remove(&packs, "linked").unwrap_err();
        assert!(matches!(err, Error::NotRemovable { .. }), "{err}");
        assert!(elsewhere.exists());
    }

    #[test]
    fn refuses_a_missing_pack() {
        let (_case, packs) = installed_fixture("rm-missing");
        let err = install::remove(&packs, "no-such-pack").unwrap_err();
        assert!(matches!(err, Error::NotRemovable { .. }), "{err}");
    }
}

mod when_generating_the_curated_index {
    use super::*;

    #[test]
    fn skips_packs_whose_closure_is_not_fully_tracked() {
        // The wasm packs reference grammar blobs that are fetched from
        // release assets, not tracked by git — they must never be offered
        // through the git-tag install path.
        let (text, skipped) = install::generate_index(repo_root(), "test-tag").unwrap();
        assert!(
            skipped.iter().any(|s| s.contains("packs-wasm/rust")),
            "{skipped:?}"
        );
        let index: install::Index = toml::from_str(&text).unwrap();
        assert!(
            index.packs.iter().all(|p| !p.dir.starts_with("packs-wasm")),
            "wasm packs with untracked blobs must be skipped"
        );
    }

    #[test]
    fn matches_the_committed_index() {
        let committed = std::fs::read_to_string(repo_root().join("packs-index.toml")).unwrap();
        let tag = install::curated_index().unwrap().tag;
        let (generated, _) = install::generate_index(repo_root(), &tag).unwrap();
        assert_eq!(
            generated, committed,
            "packs-index.toml is stale; run scripts/gen-packs-index.sh"
        );
    }
}
