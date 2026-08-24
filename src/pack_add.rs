//! Thin pack installation: acquire one directory, validate it, and vendor
//! its manifest closure into the current project.

use anyhow::{Context, Result, bail, ensure};
use btt::pack::{self, GrammarSource};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(crate) struct AddedPack {
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) path: PathBuf,
}

/// Add exactly one pack from a local directory or Git repository.
/// `force_git` skips the local-path interpretation of `source`, so a
/// stray directory named like a GitHub shorthand cannot shadow the repo.
pub(crate) fn add(
    source: &str,
    subdir: Option<&Path>,
    project_root: &Path,
    force_git: bool,
) -> Result<AddedPack> {
    let acquired = acquire(source, force_git)?;
    let source_root = acquired
        .root
        .canonicalize()
        .with_context(|| format!("resolving pack source {}", acquired.root.display()))?;
    let pack_dir = select_pack_dir(&source_root, subdir)?;

    // Loading first gives us the manifest closure. Loading the staged copy
    // below remains authoritative for the exact bytes that will be used.
    let source_pack = pack::load_dir(&pack_dir).context("validating source pack")?;
    ensure_wasm_grammar_present(&source_pack)?;
    let name = source_pack.name().to_string();
    ensure!(
        pack::is_safe_name(&name),
        "pack name `{name}` must be one non-hidden path component"
    );

    std::fs::create_dir_all(project_root)
        .with_context(|| format!("creating project root {}", project_root.display()))?;
    let project_root = project_root
        .canonicalize()
        .with_context(|| format!("resolving project root {}", project_root.display()))?;
    let btt_dir = project_root.join(".btt");
    std::fs::create_dir_all(&btt_dir).with_context(|| format!("creating {}", btt_dir.display()))?;
    let btt_dir = btt_dir
        .canonicalize()
        .with_context(|| format!("resolving {}", btt_dir.display()))?;
    ensure!(
        btt_dir.starts_with(&project_root),
        "{} resolves outside the project",
        project_root.join(".btt").display()
    );
    let packs_root = btt_dir.join("packs");
    std::fs::create_dir_all(&packs_root)
        .with_context(|| format!("creating {}", packs_root.display()))?;
    let packs_root = packs_root
        .canonicalize()
        .with_context(|| format!("resolving {}", packs_root.display()))?;
    ensure!(
        packs_root.starts_with(&project_root),
        "{} resolves outside the project",
        btt_dir.join("packs").display()
    );
    let target = packs_root.join(&name);
    ensure_missing(&target)?;
    sweep_stale_staging(&btt_dir);

    let staging = create_unique_dir(&btt_dir, &format!(".pack-add-{name}"))?;
    let staging_guard = DirGuard(Some(staging.clone()));
    for rel in source_pack.manifest.referenced_files() {
        copy_regular_file(&pack_dir, &rel, &staging)?;
    }

    let staged_pack = pack::load_dir(&staging).context("validating copied pack")?;
    ensure!(
        staged_pack.name() == name,
        "pack name changed while it was being copied"
    );
    // The staged manifest bytes were re-read from disk, so its file set is
    // authoritative too — a wasm grammar it declares must have been copied.
    ensure_wasm_grammar_present(&staged_pack)?;
    let version = staged_pack.manifest.pack.version.to_string();
    ensure_missing(&target)?;
    std::fs::rename(&staging, &target)
        .with_context(|| format!("moving validated pack into {}", target.as_path().display()))?;
    staging_guard.keep();

    Ok(AddedPack {
        name,
        version,
        path: target,
    })
}

fn select_pack_dir(source_root: &Path, subdir: Option<&Path>) -> Result<PathBuf> {
    let candidate = match subdir {
        Some(dir) => {
            ensure!(
                is_confined_relative_path(dir),
                "--dir must be a non-empty relative path with no `.` or `..` components"
            );
            source_root.join(dir)
        }
        None => source_root.to_path_buf(),
    };
    let candidate = candidate
        .canonicalize()
        .with_context(|| format!("resolving pack directory {}", candidate.display()))?;
    ensure!(
        candidate.starts_with(source_root),
        "pack directory {} is outside the source",
        candidate.display()
    );
    ensure!(
        candidate.is_dir(),
        "pack directory {} is not a directory",
        candidate.display()
    );
    Ok(candidate)
}

fn is_confined_relative_path(path: &Path) -> bool {
    let mut components = path.components();
    matches!(components.next(), Some(Component::Normal(_)))
        && components.all(|component| matches!(component, Component::Normal(_)))
        && !path.to_string_lossy().contains('\\')
}

/// The loader tolerates a missing wasm grammar file (`wasm_grammar: None`,
/// deferred to parse time), so installation must check presence itself:
/// on the source pack for a clear diagnostic, and on the staged copy to
/// guarantee the vendored pack is complete even if the source manifest
/// changed between the closure computation and the copy.
fn ensure_wasm_grammar_present(pack: &pack::Pack) -> Result<()> {
    if let GrammarSource::Wasm(file) = &pack.manifest.grammar.source {
        ensure!(
            pack.wasm_grammar.is_some(),
            "pack `{}` declares grammar `wasm:{}` but the file is missing or unreadable",
            pack.name(),
            file.display()
        );
    }
    Ok(())
}

/// Remove staging directories orphaned by interrupted runs: cleanup is
/// otherwise Drop-based, which a kill signal skips, and the unique names
/// mean no later run would ever reuse the leftovers.
fn sweep_stale_staging(btt_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(btt_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with(".pack-add-") && staging_pid(name).is_some_and(is_dead) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// The pid embedded in a `.pack-add-<name>-<pid>-<seq>` directory name.
fn staging_pid(name: &str) -> Option<u32> {
    let mut parts = name.rsplitn(3, '-');
    let _sequence = parts.next()?;
    parts.next()?.parse().ok()
}

/// Liveness is only checkable via procfs; elsewhere stale directories are
/// left alone rather than risking a concurrent run's live staging.
fn is_dead(pid: u32) -> bool {
    pid != std::process::id()
        && Path::new("/proc").is_dir()
        && !Path::new(&format!("/proc/{pid}")).exists()
}

fn copy_regular_file(source: &Path, rel: &Path, destination: &Path) -> Result<()> {
    let source_file = source.join(rel);
    let metadata = source_file
        .symlink_metadata()
        .with_context(|| format!("reading metadata for {}", source_file.display()))?;
    ensure!(
        metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
        "pack file {} is not a regular file",
        source_file.display()
    );
    let canonical_source = source
        .canonicalize()
        .with_context(|| format!("resolving {}", source.display()))?;
    let canonical_file = source_file
        .canonicalize()
        .with_context(|| format!("resolving {}", source_file.display()))?;
    ensure!(
        canonical_file.starts_with(&canonical_source),
        "pack file {} resolves outside the pack directory",
        source_file.display()
    );

    let bytes = std::fs::read(&canonical_file)
        .with_context(|| format!("reading {}", canonical_file.display()))?;
    let target = destination.join(rel);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&target, bytes).with_context(|| format!("writing {}", target.display()))
}

fn ensure_missing(path: &Path) -> Result<()> {
    match path.symlink_metadata() {
        Ok(_) => bail!(
            "pack destination {} already exists; remove it explicitly before adding again",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("checking {}", path.display())),
    }
}

struct Acquired {
    root: PathBuf,
    _cleanup: Option<DirGuard>,
}

fn acquire(source: &str, force_git: bool) -> Result<Acquired> {
    let local = Path::new(source);
    if !force_git && local.exists() {
        return Ok(Acquired {
            root: local.to_path_buf(),
            _cleanup: None,
        });
    }

    let git_source = parse_git_source(source)?;
    let holder = create_unique_dir(&std::env::temp_dir(), "btt-pack-source")?;
    let checkout = holder.join("repo");
    let cleanup = DirGuard(Some(holder));
    let output = Command::new("git")
        .env("GIT_ALLOW_PROTOCOL", "https:ssh:git:file")
        // Fail fast instead of prompting for credentials on the tty (which
        // a typo'd shorthand triggers: GitHub answers unknown repos with an
        // auth challenge); the captured stderr is relayed below.
        .env("GIT_TERMINAL_PROMPT", "0")
        .arg("clone")
        .arg("--depth")
        .arg("1")
        .arg("--")
        .arg(&git_source)
        .arg(&checkout)
        .output()
        .context("running git clone")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git clone failed: {}", stderr.trim());
    }
    Ok(Acquired {
        root: checkout,
        _cleanup: Some(cleanup),
    })
}

fn parse_git_source(source: &str) -> Result<String> {
    let is_url = ["https://", "ssh://", "git://", "file://"]
        .iter()
        .any(|prefix| source.starts_with(prefix));
    let is_scp_style = source
        .split_once(':')
        .is_some_and(|(host, path)| host.contains('@') && !path.is_empty());
    if is_url || is_scp_style {
        return Ok(source.to_string());
    }

    let parts: Vec<_> = source.split('/').collect();
    let github_shorthand = matches!(parts.as_slice(), [owner, repo] if is_repo_component(owner) && is_repo_component(repo));
    if github_shorthand {
        let repo = source.strip_suffix(".git").unwrap_or(source);
        return Ok(format!("https://github.com/{repo}.git"));
    }
    bail!("source `{source}` does not exist; use a local directory, Git URL, or GitHub owner/repo")
}

fn is_repo_component(component: &str) -> bool {
    !component.is_empty()
        && component != "."
        && component != ".."
        && component
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character))
}

fn create_unique_dir(parent: &Path, prefix: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(parent)
        .with_context(|| format!("creating temporary directory parent {}", parent.display()))?;
    for _ in 0..100 {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!("{prefix}-{}-{sequence}", std::process::id()));
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(error).with_context(|| format!("creating {}", candidate.display()));
            }
        }
    }
    bail!("could not allocate a unique temporary directory")
}

struct DirGuard(Option<PathBuf>);

impl DirGuard {
    fn keep(mut self) {
        self.0 = None;
    }
}

impl Drop for DirGuard {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            Self(create_unique_dir(&std::env::temp_dir(), label).unwrap())
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_pack(dir: &Path, name: &str) {
        std::fs::create_dir_all(dir.join("queries")).unwrap();
        std::fs::create_dir_all(dir.join("templates")).unwrap();
        let manifest = format!(
            r#"format = 1

[pack]
name = "{name}"
version = "1.2.3"
description = "test pack"

[compat]
btt = ">=0.2.0"

[detect]
targets = ["{{stem}}.rs"]

[grammar]
source = "builtin:rust"

[extract]
query = "queries/tests.scm"

[mapping]

[scaffold]
template = "templates/test.jinja"
output = "{{stem}}.rs"
"#
        );
        std::fs::write(dir.join("pack.toml"), manifest).unwrap();
        std::fs::write(dir.join("queries/tests.scm"), "(function_item) @test").unwrap();
        std::fs::write(dir.join("templates/test.jinja"), "{{ events }}").unwrap();
    }

    fn run_git(repo: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    mod when_adding_a_local_pack {
        use super::*;

        #[test]
        fn copies_only_the_manifest_closure() {
            let temp = TestDir::new("btt-add-local");
            let source = temp.path().join("source");
            let project = temp.path().join("project");
            write_pack(&source, "demo");
            std::fs::write(source.join("ignored.txt"), "not part of the pack").unwrap();

            let added = add(source.to_str().unwrap(), None, &project, false).unwrap();

            assert_eq!(added.name, "demo");
            assert_eq!(added.version, "1.2.3");
            assert!(added.path.join("pack.toml").is_file());
            assert!(added.path.join("queries/tests.scm").is_file());
            assert!(added.path.join("templates/test.jinja").is_file());
            assert!(!added.path.join("ignored.txt").exists());
        }

        #[test]
        fn refuses_to_replace_an_existing_pack() {
            let temp = TestDir::new("btt-add-existing");
            let source = temp.path().join("source");
            let project = temp.path().join("project");
            write_pack(&source, "demo");
            let first = add(source.to_str().unwrap(), None, &project, false).unwrap();
            std::fs::write(source.join("queries/tests.scm"), "changed").unwrap();

            let error = add(source.to_str().unwrap(), None, &project, false).unwrap_err();

            assert!(error.to_string().contains("already exists"), "{error:#}");
            assert_eq!(
                std::fs::read_to_string(first.path.join("queries/tests.scm")).unwrap(),
                "(function_item) @test"
            );
        }

        #[test]
        fn sweeps_staging_left_by_interrupted_runs() {
            let temp = TestDir::new("btt-add-sweep");
            let source = temp.path().join("source");
            let project = temp.path().join("project");
            write_pack(&source, "demo");
            // u32::MAX cannot be a live pid (Linux pid_max is far smaller).
            let stale = project.join(".btt/.pack-add-old-4294967295-0");
            std::fs::create_dir_all(&stale).unwrap();
            std::fs::write(stale.join("leftover"), "").unwrap();

            add(source.to_str().unwrap(), None, &project, false).unwrap();

            assert!(!stale.exists());
        }
    }

    mod when_the_source_contains_an_unsafe_pack_name {
        use super::*;

        #[test]
        fn refuses_to_write_outside_the_packs_directory() {
            let temp = TestDir::new("btt-add-name");
            let source = temp.path().join("source");
            let project = temp.path().join("project");
            write_pack(&source, "../escape");

            let error = add(source.to_str().unwrap(), None, &project, false).unwrap_err();

            assert!(error.to_string().contains("one non-hidden"), "{error:#}");
            assert!(!project.join(".btt/escape").exists());
        }
    }

    mod when_the_source_declares_a_wasm_grammar {
        use super::*;

        #[test]
        fn refuses_a_pack_whose_grammar_file_is_missing() {
            let temp = TestDir::new("btt-add-wasm-missing");
            let source = temp.path().join("source");
            let project = temp.path().join("project");
            write_pack(&source, "demo");
            let manifest = std::fs::read_to_string(source.join("pack.toml"))
                .unwrap()
                .replace("builtin:rust", "wasm:grammar.wasm");
            std::fs::write(source.join("pack.toml"), manifest).unwrap();

            let error = add(source.to_str().unwrap(), None, &project, false).unwrap_err();

            assert!(
                error.to_string().contains("missing or unreadable"),
                "{error:#}"
            );
            assert!(!project.join(".btt/packs/demo").exists());
        }
    }

    mod when_selecting_a_pack_subdirectory {
        use super::*;

        #[test]
        fn rejects_paths_outside_the_source() {
            let temp = TestDir::new("btt-add-subdir");
            let source = temp.path().join("source");
            let project = temp.path().join("project");
            std::fs::create_dir_all(&source).unwrap();

            let error = add(
                source.to_str().unwrap(),
                Some(Path::new("../outside")),
                &project,
                false,
            )
            .unwrap_err();

            assert!(error.to_string().contains("--dir"), "{error:#}");
        }
    }

    #[cfg(unix)]
    mod when_the_project_packs_directory_escapes {
        use super::*;
        use std::os::unix::fs::symlink;

        #[test]
        fn refuses_to_write_outside_the_project() {
            let temp = TestDir::new("btt-add-project-escape");
            let source = temp.path().join("source");
            let project = temp.path().join("project");
            let outside = temp.path().join("outside");
            write_pack(&source, "demo");
            std::fs::create_dir_all(project.join(".btt")).unwrap();
            std::fs::create_dir_all(&outside).unwrap();
            symlink(&outside, project.join(".btt/packs")).unwrap();

            let error = add(source.to_str().unwrap(), None, &project, false).unwrap_err();

            assert!(
                error.to_string().contains("outside the project"),
                "{error:#}"
            );
            assert!(!outside.join("demo").exists());
        }
    }

    mod when_adding_a_git_repository {
        use super::*;

        #[test]
        fn clones_and_adds_the_pack() {
            let temp = TestDir::new("btt-add-git");
            let source = temp.path().join("source");
            let project = temp.path().join("project");
            std::fs::create_dir_all(&source).unwrap();
            write_pack(&source, "from-git");
            run_git(&source, &["init", "--quiet"]);
            run_git(&source, &["config", "user.email", "test@example.com"]);
            run_git(&source, &["config", "user.name", "btt test"]);
            run_git(&source, &["add", "."]);
            run_git(&source, &["commit", "--quiet", "-m", "fixture"]);
            let url = format!("file://{}", source.display());

            let added = add(&url, None, &project, false).unwrap();

            assert_eq!(added.name, "from-git");
            assert!(added.path.join("pack.toml").is_file());
        }

        #[test]
        fn refuses_a_local_path_when_git_is_forced() {
            let temp = TestDir::new("btt-add-force-git");
            let source = temp.path().join("source");
            let project = temp.path().join("project");
            write_pack(&source, "demo");

            let error = add(source.to_str().unwrap(), None, &project, true).unwrap_err();

            assert!(error.to_string().contains("does not exist"), "{error:#}");
            assert!(!project.join(".btt/packs/demo").exists());
        }
    }

    mod when_parsing_a_git_source {
        use super::*;

        #[test]
        fn expands_a_github_shorthand() {
            assert_eq!(
                parse_git_source("example/btt-packs").unwrap(),
                "https://github.com/example/btt-packs.git"
            );
        }

        #[test]
        fn keeps_an_existing_git_suffix() {
            assert_eq!(
                parse_git_source("example/btt-packs.git").unwrap(),
                "https://github.com/example/btt-packs.git"
            );
        }

        #[test]
        fn rejects_an_unknown_source() {
            let error = parse_git_source("not-a-path-or-url").unwrap_err();
            assert!(error.to_string().contains("does not exist"), "{error:#}");
        }
    }
}
