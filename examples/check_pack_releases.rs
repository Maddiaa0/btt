//! CI checks for independently versioned pack releases.
//!
//! `--base <git-ref>` requires every changed manifest-closure to carry a
//! strictly newer pack version than the base revision. `--tag <tag>`
//! validates an immutable `pack/<name>/v<semver>` release tag.

use anyhow::{Context, Result, bail};
use semver::Version;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

const PACK_ROOTS: [&str; 3] = ["packs", "packs-lexical", "packs-wasm"];

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match (args.next().as_deref(), args.next(), args.next()) {
        (Some("--base"), Some(base), None) => check_changes(&base),
        (Some("--tag"), Some(tag), None) => check_tag(&tag),
        _ => bail!(
            "usage: cargo run --example check_pack_releases -- \
             (--base <git-ref> | --tag pack/<name>/v<semver>)"
        ),
    }
}

fn git(args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout).context("git output was not UTF-8")
}

fn git_at(reference: &str, path: &Path) -> Result<Option<String>> {
    let object = format!("{reference}:{}", path.display());
    let output = Command::new("git")
        .args(["show", &object])
        .output()
        .with_context(|| format!("running git show {object}"))?;
    if output.status.success() {
        return String::from_utf8(output.stdout)
            .context("pack manifest in git was not UTF-8")
            .map(Some);
    }
    Ok(None)
}

fn parse_manifest(text: &str, label: &str) -> Result<toml::Value> {
    toml::from_str(text).with_context(|| format!("parsing {label}"))
}

fn string_field<'a>(value: &'a toml::Value, table: &str, field: &str) -> Result<&'a str> {
    value
        .get(table)
        .and_then(|v| v.get(field))
        .and_then(toml::Value::as_str)
        .with_context(|| format!("missing string `{table}.{field}`"))
}

fn version(value: &toml::Value) -> Result<Version> {
    let raw = string_field(value, "pack", "version")?;
    Version::parse(raw).with_context(|| format!("invalid pack version `{raw}`"))
}

fn closure(value: &toml::Value) -> Result<BTreeSet<PathBuf>> {
    let mut files = BTreeSet::from([PathBuf::from("pack.toml")]);
    if let Some(query) = value
        .get("extract")
        .and_then(|v| v.get("query"))
        .and_then(toml::Value::as_str)
    {
        files.insert(PathBuf::from(query));
    }
    files.insert(PathBuf::from(string_field(value, "scaffold", "template")?));
    if let Some(wasm) = string_field(value, "grammar", "source")?.strip_prefix("wasm:") {
        files.insert(PathBuf::from(wasm));
    }
    Ok(files)
}

fn pack_dir(path: &Path) -> Option<PathBuf> {
    let mut parts = path.components();
    let root = parts.next()?.as_os_str().to_str()?;
    if !PACK_ROOTS.contains(&root) {
        return None;
    }
    let name = parts.next()?.as_os_str();
    Some(Path::new(root).join(name))
}

fn check_changes(base: &str) -> Result<()> {
    git(&["rev-parse", "--verify", &format!("{base}^{{commit}}")])
        .with_context(|| format!("base revision `{base}` is unavailable; CI must fetch history"))?;
    let mut diff_args = vec!["diff", "--name-only", base, "--"];
    diff_args.extend(PACK_ROOTS);
    let changed = git(&diff_args)?;
    let mut by_pack: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for file in changed.lines().map(PathBuf::from) {
        if let Some(dir) = pack_dir(&file) {
            by_pack.entry(dir).or_default().push(file);
        }
    }

    for (dir, changed_files) in by_pack {
        let manifest_path = dir.join("pack.toml");
        let current_text = std::fs::read_to_string(&manifest_path).with_context(|| {
            format!(
                "pack `{}` was removed; releases must remain available under an immutable tag",
                dir.display()
            )
        })?;
        let current = parse_manifest(&current_text, &manifest_path.display().to_string())?;
        let Some(base_text) = git_at(base, &manifest_path)? else {
            btt::pack::load_dir(&dir)
                .with_context(|| format!("validating new pack in {}", dir.display()))?;
            let initial = version(&current)?;
            if initial != Version::new(0, 1, 0) {
                bail!(
                    "new pack {} must begin at v0.1.0, not v{}",
                    string_field(&current, "pack", "name")?,
                    initial
                );
            }
            println!(
                "new pack {} v{}",
                string_field(&current, "pack", "name")?,
                initial
            );
            continue;
        };
        let old = parse_manifest(&base_text, &format!("{base}:{}", manifest_path.display()))?;
        let relevant: BTreeSet<PathBuf> =
            closure(&old)?.union(&closure(&current)?).cloned().collect();
        let closure_changed = changed_files.iter().any(|file| {
            file.strip_prefix(&dir)
                .is_ok_and(|relative| relevant.contains(relative))
        });
        if !closure_changed {
            continue;
        }

        btt::pack::load_dir(&dir)
            .with_context(|| format!("validating changed pack in {}", dir.display()))?;

        let old_version = version(&old)?;
        let new_version = version(&current)?;
        if new_version <= old_version {
            bail!(
                "{} changed without a pack version bump (still v{}); update `pack.version` using the policy in docs/pack-releases.md",
                dir.display(),
                old_version
            );
        }
        println!("{}: v{} -> v{}", dir.display(), old_version, new_version);
    }
    Ok(())
}

fn pack_dirs() -> Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    for root in PACK_ROOTS {
        for entry in std::fs::read_dir(root).with_context(|| format!("reading {root}"))? {
            let path = entry?.path();
            if path.join("pack.toml").is_file() {
                dirs.push(path);
            }
        }
    }
    dirs.sort();
    Ok(dirs)
}

fn check_tag(tag: &str) -> Result<()> {
    let selector = tag
        .strip_prefix("pack/")
        .with_context(|| format!("pack release tag `{tag}` must start with `pack/`"))?;
    let (name, raw_version) = selector
        .rsplit_once("/v")
        .with_context(|| format!("pack release tag `{tag}` must end with `/v<semver>`"))?;
    if name.is_empty() || name.contains('/') {
        bail!("pack release tag `{tag}` contains an invalid pack name");
    }
    let tag_version = Version::parse(raw_version)
        .with_context(|| format!("pack release tag `{tag}` has an invalid version"))?;

    let mut matched = Vec::new();
    for dir in pack_dirs()? {
        let manifest_path = dir.join("pack.toml");
        let text = std::fs::read_to_string(&manifest_path)?;
        let manifest = parse_manifest(&text, &manifest_path.display().to_string())?;
        if string_field(&manifest, "pack", "name")? == name {
            btt::pack::load_dir(&dir)
                .with_context(|| format!("validating release pack in {}", dir.display()))?;
            matched.push((dir, version(&manifest)?));
        }
    }
    let [(dir, manifest_version)] = matched.as_slice() else {
        bail!(
            "release tag `{tag}` must select exactly one pack manifest; found {}",
            matched.len()
        );
    };
    if manifest_version != &tag_version {
        bail!(
            "release tag `{tag}` disagrees with {} (`pack.version` is v{})",
            dir.display(),
            manifest_version
        );
    }
    println!("validated {name} v{tag_version} from {}", dir.display());
    Ok(())
}
