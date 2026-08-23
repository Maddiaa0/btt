//! Pack installation: acquisition, staging, validation, receipts, the
//! curated index, and removal. Design: `docs/pack-install.md`.
//!
//! The pipeline every source funnels through:
//!
//! 1. **Acquire** — a local directory (`--path`), a shallow git clone
//!    (`--git`), or a clone of the official repo at the tag pinned in the
//!    embedded curated index. Acquisition never executes pack-provided
//!    code and never parses archive formats.
//! 2. **Stage** ([`stage`]) — copy *only* the manifest closure
//!    (`pack.toml` + the files it references) into a staging directory
//!    under the destination packs root. Symlinks and special files are
//!    refused, sizes are capped, and every file is content-hashed as it
//!    is copied.
//! 3. **Validate** — the staged copy is loaded with [`pack::load_dir`],
//!    the same strict loader `btt check` uses. Validating the copy (not
//!    the source) closes the gap between check and use.
//! 4. **Receipt** ([`write_receipt`]) — provenance (source, url, commit)
//!    and per-file digests, written into the staged directory.
//! 5. **Commit** ([`commit`]) — an atomic rename into
//!    `<packs-root>/<manifest name>`; replacing an existing pack is a
//!    revert-safe swap through a trash directory.
//!
//! Installation never activates a pack: packs only run when `btt.toml`
//! names them.

use crate::error::{Error, Result};
use crate::pack::{self, GrammarSource, Manifest};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Size cap for text files in a pack (manifest, query, template).
pub const MAX_TEXT_BYTES: u64 = 256 * 1024;
/// Size cap for a compiled wasm grammar module.
pub const MAX_WASM_BYTES: u64 = 8 * 1024 * 1024;

/// The curated pack index compiled into this binary. Regenerate with
/// `scripts/gen-packs-index.sh`; freshness is enforced by
/// `tests/install.rs`.
const CURATED_INDEX: &str = include_str!("../packs-index.toml");

static SEQ: AtomicU64 = AtomicU64::new(0);

/// A process-unique suffix for staging/trash directory names.
fn unique_suffix() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

/// One staged file: its pack-relative path, size, and content digest.
#[derive(Debug, Clone)]
pub struct StagedFile {
    /// Path relative to the pack root, as named by the manifest.
    pub rel: String,
    /// Size in bytes.
    pub size: u64,
    /// Lowercase hex sha256 of the contents.
    pub sha256: String,
}

/// A pack staged under the destination packs root, validated and ready to
/// [`commit`]. Dropping an uncommitted `Staged` removes its staging
/// directory, so an aborted install leaves no residue.
#[derive(Debug)]
pub struct Staged {
    dir: PathBuf,
    packs_root: PathBuf,
    committed: bool,
    /// The staged pack, loaded from the staging directory by the same
    /// loader `btt check` uses.
    pub pack: pack::Pack,
    /// Digests of every staged file.
    pub files: Vec<StagedFile>,
}

impl Staged {
    /// The pack's name (and therefore its final directory name).
    #[must_use]
    pub fn name(&self) -> &str {
        self.pack.name()
    }

    /// The staging directory the files currently live in.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_dir_all(&self.dir);
            // Drops `.staging` itself when this was its last entry.
            let _ = std::fs::remove_dir(self.packs_root.join(".staging"));
        }
    }
}

/// The files a manifest closes over, with the size cap each must obey.
/// This is the complete list an install copies — nothing else in the
/// source directory ever reaches the packs root.
fn closure(manifest: &Manifest) -> Vec<(PathBuf, u64)> {
    let mut files = vec![(PathBuf::from("pack.toml"), MAX_TEXT_BYTES)];
    // Lexical packs have no query file: their whole profile lives in the
    // manifest itself.
    if let Some(query) = &manifest.extract.query {
        files.push((PathBuf::from(query), MAX_TEXT_BYTES));
    }
    files.push((PathBuf::from(&manifest.scaffold.template), MAX_TEXT_BYTES));
    if let GrammarSource::Wasm(file) = &manifest.grammar.source {
        files.push((file.clone(), MAX_WASM_BYTES));
    }
    files
}

/// Read one file from an install source, enforcing the copy rules: the
/// final component must be a regular file (not a symlink), its resolved
/// path must stay inside the source directory, and its size must clear
/// `cap`.
///
/// The read goes through the canonical, checked path (not the original
/// name) and is length-bounded, so neither a final-component swapped to a
/// symlink after the stat nor a file that grows after the stat can defeat
/// the checks.
fn read_source_file(source: &Path, rel: &Path, cap: u64) -> Result<Vec<u8>> {
    let path = source.join(rel);
    let meta = path.symlink_metadata().map_err(|e| Error::io(&path, e))?;
    if meta.file_type().is_symlink() || !meta.file_type().is_file() {
        return Err(Error::InstallUnsafeFile { path });
    }
    // A symlinked *directory* between source root and file evades the
    // final-component check; the canonical path closes that route, and is
    // also what we then open — so the bytes read are the bytes checked.
    let canon = path.canonicalize().map_err(|e| Error::io(&path, e))?;
    let source_canon = source.canonicalize().map_err(|e| Error::io(source, e))?;
    if !canon.starts_with(&source_canon) {
        return Err(Error::InstallUnsafeFile { path });
    }
    let file = File::open(&canon).map_err(|e| Error::io(&canon, e))?;
    // Re-stat through the open handle: a regular file, within cap.
    let fmeta = file.metadata().map_err(|e| Error::io(&canon, e))?;
    if !fmeta.is_file() || fmeta.len() > cap {
        return if fmeta.is_file() {
            Err(Error::InstallTooLarge {
                path,
                size: fmeta.len(),
                limit: cap,
            })
        } else {
            Err(Error::InstallUnsafeFile { path })
        };
    }
    // Read at most cap+1 bytes so a race that grows the file past the cap
    // after the stat is still caught rather than copied wholesale.
    let mut buf = Vec::new();
    file.take(cap + 1)
        .read_to_end(&mut buf)
        .map_err(|e| Error::io(&canon, e))?;
    if buf.len() as u64 > cap {
        return Err(Error::InstallTooLarge {
            path,
            size: buf.len() as u64,
            limit: cap,
        });
    }
    Ok(buf)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// Parse and pre-validate the manifest of an install source. Validation
/// here is what makes joining the closure paths safe; the authoritative
/// check still runs on the staged copy.
fn source_manifest(source: &Path) -> Result<Manifest> {
    let bytes = read_source_file(source, Path::new("pack.toml"), MAX_TEXT_BYTES)?;
    let manifest_path = source.join("pack.toml");
    let text = String::from_utf8(bytes).map_err(|_| Error::PackFile {
        pack: source.display().to_string(),
        file: "pack.toml".to_string(),
    })?;
    let manifest: Manifest = toml::from_str(&text).map_err(|e| Error::Toml {
        path: manifest_path,
        source: Box::new(e),
    })?;
    pack::validate(&manifest)?;
    if !pack::is_valid_name(&manifest.pack.name) {
        return Err(Error::UnsafePath {
            pack: manifest.pack.name.clone(),
            field: "pack.name",
            value: manifest.pack.name.clone(),
        });
    }
    Ok(manifest)
}

/// Stage a pack from `source` under `packs_root`: allowlist-copy the
/// manifest closure into `<packs_root>/.staging/…`, then validate the
/// copy with [`pack::load_dir`]. Nothing outside the staging directory is
/// written; the returned [`Staged`] cleans up after itself unless
/// [`commit`]ed.
///
/// # Errors
///
/// Any copy-rule violation (symlink, escape, size cap), a manifest the
/// loader rejects, or I/O failure.
pub fn stage(source: &Path, packs_root: &Path) -> Result<Staged> {
    let manifest = source_manifest(source)?;
    let name = manifest.pack.name.clone();

    let staging_root = packs_root.join(".staging");
    std::fs::create_dir_all(&staging_root).map_err(|e| Error::io(&staging_root, e))?;
    // Sweep leftovers of interrupted installs of this same pack. Match the
    // exact `<name>.<pid>-<seq>` grammar, not a bare `<name>.` prefix: a
    // prefix match would also delete the live staging dir of a concurrent
    // install of a different, dot-containing pack (`foo` vs `foo.bar`).
    if let Ok(entries) = std::fs::read_dir(&staging_root) {
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            if let Some(suffix) = file_name
                .to_string_lossy()
                .strip_prefix(&format!("{name}."))
                && is_staging_suffix(suffix)
            {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }

    let dir = staging_root.join(format!("{name}.{}", unique_suffix()));
    std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
    // Until `Staged` exists to own cleanup, this guard removes the
    // staging dir on any early return.
    let mut guard = CleanupGuard(Some(dir.clone()));

    let mut files = Vec::new();
    for (rel, cap) in closure(&manifest) {
        let bytes = read_source_file(source, &rel, cap)?;
        let dest = dir.join(&rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        std::fs::write(&dest, &bytes).map_err(|e| Error::io(&dest, e))?;
        files.push(StagedFile {
            rel: rel.to_string_lossy().into_owned(),
            size: bytes.len() as u64,
            sha256: sha256_hex(&bytes),
        });
    }

    // The authoritative validation: the copy that will be installed.
    let pack = pack::load_dir(&dir)?;
    guard.0 = None;
    Ok(Staged {
        dir,
        packs_root: packs_root.to_path_buf(),
        committed: false,
        pack,
        files,
    })
}

/// True when `suffix` is a `<pid>-<seq>` staging tag: two non-empty runs
/// of ASCII digits joined by a single `-`. Used to sweep only real
/// staging leftovers, never a differently-named pack's dir.
fn is_staging_suffix(suffix: &str) -> bool {
    matches!(
        suffix.split_once('-'),
        Some((pid, seq))
            if !pid.is_empty()
                && !seq.is_empty()
                && pid.bytes().all(|b| b.is_ascii_digit())
                && seq.bytes().all(|b| b.is_ascii_digit())
    )
}

struct CleanupGuard(Option<PathBuf>);

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        if let Some(dir) = &self.0 {
            let _ = std::fs::remove_dir_all(dir);
            // Drop the `.staging` parent too when this was its last entry,
            // so a failed install leaves no empty scaffolding behind.
            if let Some(parent) = dir.parent() {
                let _ = std::fs::remove_dir(parent);
            }
        }
    }
}

/// Atomically move a staged pack into `<packs_root>/<name>`. Replacing an
/// existing pack (`force`) swaps through a trash directory and restores
/// the original if the swap fails partway.
///
/// # Errors
///
/// [`Error::AlreadyInstalled`] without `force`, or I/O failure (after
/// which the destination holds either the old pack or the new one, never
/// a partial state).
pub fn commit(mut staged: Staged, force: bool) -> Result<PathBuf> {
    let target = staged.packs_root.join(staged.name());
    let exists = target.symlink_metadata().is_ok();
    if exists && !force {
        return Err(Error::AlreadyInstalled {
            name: staged.name().to_string(),
            path: target,
        });
    }
    if exists {
        let trash_root = staged.packs_root.join(".trash");
        std::fs::create_dir_all(&trash_root).map_err(|e| Error::io(&trash_root, e))?;
        let trash = trash_root.join(format!("{}.{}", staged.name(), unique_suffix()));
        std::fs::rename(&target, &trash).map_err(|e| Error::io(&target, e))?;
        if let Err(e) = std::fs::rename(&staged.dir, &target) {
            // Roll the old pack back so a failed swap loses nothing.
            let _ = std::fs::rename(&trash, &target);
            return Err(Error::io(&target, e));
        }
        let _ = std::fs::remove_dir_all(&trash);
        let _ = std::fs::remove_dir(&trash_root);
    } else {
        std::fs::rename(&staged.dir, &target).map_err(|e| Error::io(&target, e))?;
    }
    staged.committed = true;
    let _ = std::fs::remove_dir(staged.packs_root.join(".staging"));
    Ok(target)
}

/// Where an installed pack came from, recorded in its receipt.
#[derive(Debug, Clone)]
pub struct Provenance {
    /// `"curated"`, `"git"`, or `"path"`.
    pub source: String,
    /// Remote URL, for git-backed sources.
    pub url: Option<String>,
    /// The ref as requested (branch or tag), when one was given.
    pub reference: Option<String>,
    /// The resolved commit hash, for git-backed sources.
    pub commit: Option<String>,
}

/// The `[install]` table of a receipt.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ReceiptInstall {
    /// `"curated"`, `"git"`, or `"path"`.
    pub source: String,
    /// Remote URL, for git-backed sources.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The ref as requested (branch or tag).
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// The resolved commit hash.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// The btt that performed the install.
    pub installed_by: String,
    /// Install time, RFC 3339 UTC.
    pub date: String,
}

/// A pack's install receipt: provenance plus per-file digests. Written at
/// install time; read back by `btt pack show`/`list`. This is what makes
/// a future `pack update`, audits, and CI pinning possible.
#[derive(Debug, Serialize, Deserialize)]
pub struct Receipt {
    /// Provenance of the install.
    pub install: ReceiptInstall,
    /// `sha256:<hex>` per pack-relative file path.
    pub files: BTreeMap<String, String>,
}

/// File name of the receipt inside an installed pack directory. The
/// loader never reads it (loaders read only manifest-referenced files).
pub const RECEIPT_FILE: &str = "receipt.toml";

/// Write the install receipt into the staged directory, so the commit
/// rename carries it into place atomically with the pack.
///
/// # Errors
///
/// Serialization or I/O failure.
pub fn write_receipt(staged: &Staged, prov: &Provenance) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let receipt = Receipt {
        install: ReceiptInstall {
            source: prov.source.clone(),
            url: prov.url.clone(),
            reference: prov.reference.clone(),
            commit: prov.commit.clone(),
            installed_by: format!("btt {}", env!("CARGO_PKG_VERSION")),
            date: rfc3339_utc(now),
        },
        files: staged
            .files
            .iter()
            .map(|f| (f.rel.clone(), format!("sha256:{}", f.sha256)))
            .collect(),
    };
    let text = toml::to_string_pretty(&receipt).map_err(|e| Error::Receipt {
        pack: staged.name().to_string(),
        message: e.to_string(),
    })?;
    let path = staged.dir.join(RECEIPT_FILE);
    std::fs::write(&path, text).map_err(|e| Error::io(&path, e))
}

/// Read a pack's install receipt, if present and parseable. Best-effort:
/// display code tolerates missing or malformed receipts.
#[must_use]
pub fn read_receipt(pack_dir: &Path) -> Option<Receipt> {
    let text = std::fs::read_to_string(pack_dir.join(RECEIPT_FILE)).ok()?;
    toml::from_str(&text).ok()
}

/// Compute the closure digests of an installed pack directory, applying
/// the same per-file copy rules as staging. Used by `pack show` and the
/// review gate.
///
/// # Errors
///
/// A closure file that is missing, unsafe, or oversized.
pub fn file_digests(pack_dir: &Path, manifest: &Manifest) -> Result<Vec<StagedFile>> {
    closure(manifest)
        .into_iter()
        .map(|(rel, cap)| {
            let bytes = read_source_file(pack_dir, &rel, cap)?;
            Ok(StagedFile {
                rel: rel.to_string_lossy().into_owned(),
                size: bytes.len() as u64,
                sha256: sha256_hex(&bytes),
            })
        })
        .collect()
}

/// Remove an installed pack directory. Guards: the name must be a single
/// normal path component, and the entry must be a real directory (not a
/// symlink) physically inside `packs_root`.
///
/// # Errors
///
/// [`Error::UnsafePath`] for a bad name, [`Error::NotRemovable`] for a
/// missing/symlinked/non-directory entry, or I/O failure.
pub fn remove(packs_root: &Path, name: &str) -> Result<PathBuf> {
    if !pack::is_valid_name(name) {
        return Err(Error::UnsafePath {
            pack: name.to_string(),
            field: "pack name",
            value: name.to_string(),
        });
    }
    let path = packs_root.join(name);
    let Ok(meta) = path.symlink_metadata() else {
        return Err(Error::NotRemovable {
            path,
            reason: "not installed here",
        });
    };
    if meta.file_type().is_symlink() {
        return Err(Error::NotRemovable {
            path,
            reason: "it is a symlink, not an installed pack directory",
        });
    }
    if !meta.file_type().is_dir() {
        return Err(Error::NotRemovable {
            path,
            reason: "not a directory",
        });
    }
    let canon = path.canonicalize().map_err(|e| Error::io(&path, e))?;
    let root_canon = packs_root
        .canonicalize()
        .map_err(|e| Error::io(packs_root, e))?;
    if canon.parent() != Some(root_canon.as_path()) {
        return Err(Error::NotRemovable {
            path,
            reason: "it resolves outside the packs directory",
        });
    }
    std::fs::remove_dir_all(&path).map_err(|e| Error::io(&path, e))?;
    Ok(path)
}

/// Find pack directories (those holding a `pack.toml`) under `dir`,
/// sorted. Hidden directories (`.git`, `.staging`, …) are not descended
/// into. Bounded depth: a pack at most three levels down.
#[must_use]
pub fn discover(dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = walkdir::WalkDir::new(dir)
        .max_depth(4)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().is_file() && e.file_name() == "pack.toml")
        .filter_map(|e| e.path().parent().map(Path::to_path_buf))
        .collect();
    dirs.sort();
    dirs
}

/// A temporary shallow git checkout, removed on drop.
#[derive(Debug)]
pub struct Checkout {
    dir: PathBuf,
    /// The commit the checkout resolved to.
    pub commit: String,
}

impl Checkout {
    /// The checkout's directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Checkout {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn git(args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("git")
        // Restrict transports to the safe, non-executing set. Git's `ext::`
        // (and other remote-helper) transports run arbitrary shell commands
        // *at clone time* — before any review — and are permitted by default
        // for command-line URLs on many git versions. Pinning the allowlist
        // makes "installing never runs pack code" hold regardless of the
        // user's git version or config. `file` is needed for local tests.
        .env("GIT_ALLOW_PROTOCOL", "https:ssh:file:git")
        .args(args)
        .output()
        .map_err(|e| Error::Git {
            args: args.join(" "),
            detail: e.to_string(),
        })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(Error::Git {
            args: args.join(" "),
            detail: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

/// Shallow-clone `url` (optionally at a branch or tag) into a temp
/// directory and resolve the commit it landed on. `git clone` executes no
/// repository-provided code.
///
/// # Errors
///
/// [`Error::Git`] when the clone or rev-parse fails.
pub fn fetch_git(url: &str, reference: Option<&str>) -> Result<Checkout> {
    let dir = std::env::temp_dir().join(format!("btt-clone-{}", unique_suffix()));
    let dir_str = dir.to_string_lossy().into_owned();
    let mut args = vec!["clone", "--quiet", "--depth", "1"];
    if let Some(reference) = reference {
        args.extend(["--branch", reference]);
    }
    args.extend(["--", url, &dir_str]);
    if let Err(e) = git(&args) {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e);
    }
    match git(&["-C", &dir_str, "rev-parse", "HEAD"]) {
        Ok(commit) => Ok(Checkout { dir, commit }),
        Err(e) => {
            // The clone succeeded but resolving HEAD failed: clean up the
            // checkout rather than leaking it (no `Checkout` exists yet to
            // own the cleanup on drop).
            let _ = std::fs::remove_dir_all(&dir);
            Err(e)
        }
    }
}

/// One pack offered by the curated index.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexEntry {
    /// Pack name (equals its manifest name and install directory).
    pub name: String,
    /// Grammar kind, e.g. `wasm` or `builtin`.
    pub kind: String,
    /// One-line description, from the manifest.
    pub description: String,
    /// Directory of the pack inside the repo at the pinned tag.
    pub dir: String,
    /// `sha256:<hex>` per pack-relative file path — the complete closure.
    pub files: BTreeMap<String, String>,
}

/// The curated pack index embedded in this binary: an immutable release
/// tag plus per-file digests for every offered pack. Installs fetch at
/// the tag and verify every byte against these digests.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Index {
    /// The immutable git tag the digests were computed at.
    pub tag: String,
    /// Offered packs.
    #[serde(default, rename = "pack", skip_serializing_if = "Vec::is_empty")]
    pub packs: Vec<IndexEntry>,
}

/// Parse the curated index compiled into this binary.
///
/// # Errors
///
/// [`Error::Toml`] if the embedded index is malformed (a build defect,
/// caught by the freshness test).
pub fn curated_index() -> Result<Index> {
    toml::from_str(CURATED_INDEX).map_err(|e| Error::Toml {
        path: PathBuf::from("<embedded packs-index.toml>"),
        source: Box::new(e),
    })
}

/// Verify a staged pack byte-for-byte against its curated index entry:
/// the staged file set must equal the index file set, digest by digest.
///
/// # Errors
///
/// [`Error::DigestMismatch`] naming the first offending file.
pub fn verify_curated(staged: &Staged, entry: &IndexEntry) -> Result<()> {
    let staged_digests: BTreeMap<&str, String> = staged
        .files
        .iter()
        .map(|f| (f.rel.as_str(), format!("sha256:{}", f.sha256)))
        .collect();
    for (file, want) in &entry.files {
        match staged_digests.get(file.as_str()) {
            Some(got) if got == want => {}
            _ => {
                return Err(Error::DigestMismatch {
                    name: entry.name.clone(),
                    file: file.clone(),
                });
            }
        }
    }
    if let Some(extra) = staged_digests
        .keys()
        .find(|k| !entry.files.contains_key(**k))
    {
        return Err(Error::DigestMismatch {
            name: entry.name.clone(),
            file: (*extra).to_string(),
        });
    }
    Ok(())
}

/// Regenerate the curated index from a btt repo working tree: every
/// non-builtin pack whose entire manifest closure is tracked by git is
/// included with working-tree digests; incomplete packs are skipped with
/// a reason. Returns the index file text and the skip list.
///
/// The digests are only valid for installs once `tag` points at the tree
/// they were computed from — the release process must tag the commit this
/// ran against (freshness at HEAD is enforced by `tests/install.rs`).
///
/// # Errors
///
/// Git or I/O failure, or two candidate packs claiming one name.
pub fn generate_index(repo_root: &Path, tag: &str) -> Result<(String, Vec<String>)> {
    let root_str = repo_root.to_string_lossy().into_owned();
    let tracked: std::collections::BTreeSet<PathBuf> = git(&["-C", &root_str, "ls-files", "-z"])?
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect();

    let mut skipped = Vec::new();
    let mut packs: Vec<IndexEntry> = Vec::new();
    // Candidates: tracked `<dir>/<name>/pack.toml` outside `packs/`
    // (builtins ship in the binary and are not installable).
    let mut candidates: Vec<&PathBuf> = tracked
        .iter()
        .filter(|p| {
            p.file_name().is_some_and(|f| f == "pack.toml")
                && p.components().count() == 3
                && !p.starts_with("packs")
        })
        .collect();
    candidates.sort();

    for manifest_rel in candidates {
        let pack_rel = manifest_rel.parent().unwrap_or(Path::new(""));
        let pack_dir = repo_root.join(pack_rel);
        let label = pack_rel.display().to_string();
        let manifest = match source_manifest(&pack_dir) {
            Ok(m) => m,
            Err(e) => {
                skipped.push(format!("{label}: {e}"));
                continue;
            }
        };
        let missing: Vec<String> = closure(&manifest)
            .iter()
            .filter(|(rel, _)| !tracked.contains(&pack_rel.join(rel)))
            .map(|(rel, _)| rel.display().to_string())
            .collect();
        if !missing.is_empty() {
            skipped.push(format!(
                "{label}: closure not fully tracked by git ({})",
                missing.join(", ")
            ));
            continue;
        }
        let files = match file_digests(&pack_dir, &manifest) {
            Ok(digests) => digests
                .into_iter()
                .map(|f| (f.rel, format!("sha256:{}", f.sha256)))
                .collect(),
            // A tracked-but-unreadable closure file skips this one pack
            // (loudly), rather than aborting the whole index generation.
            Err(e) => {
                skipped.push(format!("{label}: {e}"));
                continue;
            }
        };
        if let Some(dup) = packs.iter().find(|p| p.name == manifest.pack.name) {
            return Err(Error::DigestMismatch {
                name: manifest.pack.name.clone(),
                file: format!("duplicate curated name (also {})", dup.dir),
            });
        }
        packs.push(IndexEntry {
            name: manifest.pack.name.clone(),
            kind: match manifest.grammar.source {
                GrammarSource::Builtin(_) => "builtin".to_string(),
                GrammarSource::Wasm(_) => "wasm".to_string(),
                GrammarSource::Lexical => "lexical".to_string(),
            },
            description: manifest.pack.description.clone(),
            dir: label,
            files,
        });
    }

    let index = Index {
        tag: tag.to_string(),
        packs,
    };
    let body = toml::to_string_pretty(&index).map_err(|e| Error::Receipt {
        pack: "packs-index".to_string(),
        message: e.to_string(),
    })?;
    let text = format!(
        "# Curated pack index, embedded into the btt binary at build time.\n\
         # Generated by scripts/gen-packs-index.sh — do not edit by hand.\n\
         # Installs fetch the official repo at `tag` and verify every file\n\
         # against these digests; the release process must tag the tree this\n\
         # was generated from.\n\n{body}"
    );
    Ok((text, skipped))
}

/// Render a receipt timestamp: seconds since the unix epoch as RFC 3339
/// UTC. Hand-rolled (days-to-civil-date) to avoid a date dependency.
fn rfc3339_utc(secs: u64) -> String {
    let secs = i64::try_from(secs).unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (hour, minute, second) = (rem / 3600, (rem / 60) % 60, rem % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    mod when_formatting_receipt_timestamps {
        use super::*;

        #[test]
        fn renders_the_unix_epoch() {
            assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        }

        #[test]
        fn renders_a_modern_date_with_time_of_day() {
            // 2026-08-23 12:34:56 UTC
            assert_eq!(rfc3339_utc(1_787_488_496), "2026-08-23T12:34:56Z");
        }

        #[test]
        fn renders_a_leap_day() {
            // 2024-02-29 00:00:00 UTC
            assert_eq!(rfc3339_utc(1_709_164_800), "2024-02-29T00:00:00Z");
        }
    }

    mod when_computing_the_manifest_closure {
        use super::*;

        fn manifest(grammar: &str) -> Manifest {
            toml::from_str(&format!(
                r#"
                format = 1

                [pack]
                name = "fixture"
                version = "0.0.1"
                [compat]
                btt = ">=0.2.0"
                [detect]
                targets = ["{{stem}}.rs"]
                [grammar]
                source = "{grammar}"
                [extract]
                query = "queries/tests.scm"
                [scaffold]
                template = "templates/test.jinja"
                output = "{{stem}}.rs"
                "#
            ))
            .unwrap()
        }

        #[test]
        fn lists_the_manifest_query_and_template() {
            let files: Vec<PathBuf> = closure(&manifest("builtin:rust"))
                .into_iter()
                .map(|(rel, _)| rel)
                .collect();
            assert_eq!(
                files,
                vec![
                    PathBuf::from("pack.toml"),
                    PathBuf::from("queries/tests.scm"),
                    PathBuf::from("templates/test.jinja"),
                ]
            );
        }

        #[test]
        fn includes_a_wasm_grammar_file() {
            let files = closure(&manifest("wasm:grammar.wasm"));
            assert_eq!(files.last().unwrap().0, PathBuf::from("grammar.wasm"));
            assert_eq!(files.last().unwrap().1, MAX_WASM_BYTES);
        }

        #[test]
        fn omits_the_query_for_a_lexical_pack() {
            let manifest: Manifest = toml::from_str(
                r#"
                format = 1

                [pack]
                name = "fixture"
                version = "0.0.1"
                [compat]
                btt = ">=0.2.0"
                [detect]
                targets = ["{stem}.rs"]
                [grammar]
                source = "lexical"
                [extract]
                [scaffold]
                template = "templates/test.jinja"
                output = "{stem}.rs"
                [lexical]
                nest = [["(", ")"]]
                [lexical.block]
                open = "x"
                [lexical.test]
                open = "y"
                "#,
            )
            .unwrap();
            let files: Vec<PathBuf> = closure(&manifest).into_iter().map(|(rel, _)| rel).collect();
            assert_eq!(
                files,
                vec![
                    PathBuf::from("pack.toml"),
                    PathBuf::from("templates/test.jinja"),
                ]
            );
        }
    }

    mod when_recognising_staging_suffixes {
        use super::*;

        #[test]
        fn accepts_a_pid_seq_tag() {
            assert!(is_staging_suffix("12345-0"));
            assert!(is_staging_suffix("1-999"));
        }

        #[test]
        fn rejects_a_dotted_pack_name_tail() {
            // `bar.7-1` is the tail of a concurrent `foo.bar` staging dir
            // seen through the `foo.` prefix — must not be swept.
            assert!(!is_staging_suffix("bar.7-1"));
            assert!(!is_staging_suffix("7"));
            assert!(!is_staging_suffix("-1"));
            assert!(!is_staging_suffix("1-"));
        }
    }
}
