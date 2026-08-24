//! Thin pack installation: acquire one directory, validate it, and vendor
//! its manifest closure into the current project.

use anyhow::{Context, Result, bail, ensure};
use btt::pack::{self, GrammarSource};
use std::collections::BTreeSet;
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
pub(crate) fn add(source: &str, subdir: Option<&Path>, project_root: &Path) -> Result<AddedPack> {
    let acquired = acquire(source)?;
    let source_root = acquired
        .root
        .canonicalize()
        .with_context(|| format!("resolving pack source {}", acquired.root.display()))?;
    let pack_dir = select_pack_dir(&source_root, subdir)?;

    // Loading first gives us the manifest closure. Loading the staged copy
    // below remains authoritative for the exact bytes that will be used.
    let source_pack = pack::load_dir(&pack_dir).context("validating source pack")?;
    let name = source_pack.name().to_string();
    ensure!(
        is_safe_pack_name(&name),
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

    let staging = create_unique_dir(&btt_dir, &format!(".pack-add-{name}"))?;
    let staging_guard = DirGuard(Some(staging.clone()));
    for rel in manifest_closure(&source_pack) {
        copy_regular_file(&pack_dir, &rel, &staging)?;
    }

    let staged_pack = pack::load_dir(&staging).context("validating copied pack")?;
    ensure!(
        staged_pack.name() == name,
        "pack name changed while it was being copied"
    );
    let version = staged_pack.manifest.pack.version.clone();
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
        candidate.starts_with(source_root) && candidate.is_dir(),
        "pack directory {} is outside the source",
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

fn is_safe_pack_name(name: &str) -> bool {
    !name.starts_with('.')
        && !name.contains(['/', '\\'])
        && matches!(
            Path::new(name).components().collect::<Vec<_>>().as_slice(),
            [Component::Normal(_)]
        )
}

fn manifest_closure(pack: &pack::Pack) -> BTreeSet<PathBuf> {
    let mut files = BTreeSet::from([PathBuf::from("pack.toml")]);
    if let Some(query) = &pack.manifest.extract.query {
        files.insert(PathBuf::from(query));
    }
    files.insert(PathBuf::from(&pack.manifest.scaffold.template));
    if let GrammarSource::Wasm(grammar) = &pack.manifest.grammar.source {
        files.insert(grammar.clone());
    }
    files
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

fn acquire(source: &str) -> Result<Acquired> {
    let local = Path::new(source);
    if local.exists() {
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
        return Ok(format!("https://github.com/{source}.git"));
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
            r#"[pack]
name = "{name}"
version = "1.2.3"
description = "test pack"

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

            let added = add(source.to_str().unwrap(), None, &project).unwrap();

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
            let first = add(source.to_str().unwrap(), None, &project).unwrap();
            std::fs::write(source.join("queries/tests.scm"), "changed").unwrap();

            let error = add(source.to_str().unwrap(), None, &project).unwrap_err();

            assert!(error.to_string().contains("already exists"), "{error:#}");
            assert_eq!(
                std::fs::read_to_string(first.path.join("queries/tests.scm")).unwrap(),
                "(function_item) @test"
            );
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

            let error = add(source.to_str().unwrap(), None, &project).unwrap_err();

            assert!(error.to_string().contains("one non-hidden"), "{error:#}");
            assert!(!project.join(".btt/escape").exists());
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

            let error = add(source.to_str().unwrap(), None, &project).unwrap_err();

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

            let added = add(&url, None, &project).unwrap();

            assert_eq!(added.name, "from-git");
            assert!(added.path.join("pack.toml").is_file());
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
        fn rejects_an_unknown_source() {
            let error = parse_git_source("not-a-path-or-url").unwrap_err();
            assert!(error.to_string().contains("does not exist"), "{error:#}");
        }
    }
}
