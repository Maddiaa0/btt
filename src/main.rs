use anyhow::{Context, Result, anyhow, bail};
use btt::check::Finding;
use btt::config::{self, Level};
use btt::extract::ActualKind;
use btt::runner;
use btt::{check, install, pack, scaffold, tree};
use clap::{Args, Parser, Subcommand};
use std::io::{IsTerminal, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "btt", version, about = "Branch tree testing, for any language")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check test files against their .tree specs.
    Check {
        /// Tree files or directories to search (default: project root).
        paths: Vec<PathBuf>,
        /// Number of files to check in parallel (default: one per core).
        #[arg(short, long)]
        jobs: Option<NonZeroUsize>,
    },
    /// Generate a test-file skeleton from a .tree spec.
    Scaffold {
        /// The .tree file to scaffold from.
        tree: PathBuf,
        /// Pack to use (defaults to the project's single configured pack).
        #[arg(short, long)]
        pack: Option<String>,
        /// Output path (defaults to the pack's pattern next to the tree file).
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Overwrite the output file if it exists.
        #[arg(long)]
        force: bool,
        /// Print to stdout instead of writing a file.
        #[arg(long)]
        stdout: bool,
    },
    /// List available language packs (alias for `pack list`).
    Packs,
    /// Install, inspect, and remove language packs.
    Pack {
        #[command(subcommand)]
        cmd: PackCmd,
    },
    /// Create btt.toml (and optionally an agent skill) in this project.
    Init {
        /// Also write .claude/skills/btt/SKILL.md for coding agents.
        #[arg(long)]
        skill: bool,
    },
}

#[derive(Subcommand)]
enum PackCmd {
    /// List builtin and installed packs, with origin and status.
    List,
    /// Show a pack's manifest, files, digests, and install provenance.
    Show {
        /// The pack to inspect.
        name: String,
    },
    /// Install a pack: curated selector by default, or --git / --path.
    Install(InstallArgs),
    /// Remove an installed pack (deletes its directory).
    Rm {
        /// The pack to remove.
        name: String,
        /// Remove from <project>/.btt/packs instead of the user dir.
        #[arg(long)]
        project: bool,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Args)]
struct InstallArgs {
    /// Curated pack name (skips the interactive selector).
    #[arg(conflicts_with_all = ["git", "path"])]
    name: Option<String>,
    /// Install from a git repository URL (cloned shallowly; no code runs).
    #[arg(long, conflicts_with = "path")]
    git: Option<String>,
    /// Branch or tag to clone (with --git).
    #[arg(long, requires = "git")]
    r#ref: Option<String>,
    /// Pack directory inside the repository (with --git).
    #[arg(long, requires = "git")]
    dir: Option<String>,
    /// Install from a local directory.
    #[arg(long)]
    path: Option<PathBuf>,
    /// Install into <project>/.btt/packs (vendored) instead of the user dir.
    #[arg(long)]
    project: bool,
    /// Replace an already-installed pack of the same name.
    #[arg(long)]
    force: bool,
    /// Skip the confirmation prompt (required when stdin is not a tty).
    #[arg(long)]
    yes: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    let cwd = std::env::current_dir().context("getting cwd")?;
    let root = config::find_project_root(&cwd);
    let cfg = config::load(&root)?;

    match cli.command {
        Command::Check { paths, jobs } => cmd_check(&paths, jobs, &root, &cfg),
        Command::Scaffold {
            tree,
            pack,
            output,
            force,
            stdout,
        } => cmd_scaffold(&tree, pack, output, force, stdout, &root, &cfg),
        Command::Packs => Ok(cmd_pack_list(&root, &cfg)),
        Command::Pack { cmd } => match cmd {
            PackCmd::List => Ok(cmd_pack_list(&root, &cfg)),
            PackCmd::Show { name } => cmd_pack_show(&name, &root),
            PackCmd::Install(args) => cmd_pack_install(&args, &root),
            PackCmd::Rm { name, project, yes } => cmd_pack_rm(&name, project, yes, &root),
        },
        Command::Init { skill } => cmd_init(&root, skill),
    }
}

/// Load the project's configured packs; with no `packs = [...]`, only the
/// builtins embedded in this binary. Project and user packs can carry
/// executable grammar code and control what is parsed and how — presence
/// in a directory is never activation, the config must name them.
fn load_packs(root: &Path, cfg: &config::ProjectConfig) -> Result<Vec<pack::Pack>> {
    let packs: Vec<pack::Pack> = if cfg.project.packs.is_empty() {
        warn_inactive_packs(root);
        pack::builtin_names()
            .iter()
            .map(|n| pack::load_builtin(n))
            .collect::<btt::Result<_>>()?
    } else {
        cfg.project
            .packs
            .iter()
            .map(|n| pack::load(n, root))
            .collect::<btt::Result<_>>()?
    };
    // Deterministic pre-flight: wasm symbol collisions are caught here,
    // once, rather than nondeterministically inside parallel workers.
    pack::validate_set(&packs)?;
    Ok(packs)
}

/// An unconfigured run ignores project/user packs; say so instead of
/// silently not finding their tests.
fn warn_inactive_packs(root: &Path) {
    let inactive: Vec<String> = pack::available(root)
        .into_iter()
        .filter(|(_, origin)| *origin != pack::Origin::Builtin)
        .map(|(name, origin)| format!("{name} [{origin}]"))
        .collect();
    if !inactive.is_empty() {
        eprintln!(
            "note: ignoring {} — packs are only active when btt.toml names them (packs = [...])",
            inactive.join(", ")
        );
    }
}

/// The rendered outcome of checking one tree file, assembled off-thread so
/// parallel runs print deterministically, in file order.
struct FileReport {
    lines: Vec<String>,
    errors: usize,
    warnings: usize,
}

fn cmd_check(
    paths: &[PathBuf],
    jobs: Option<NonZeroUsize>,
    root: &Path,
    cfg: &config::ProjectConfig,
) -> Result<ExitCode> {
    let packs = load_packs(root, cfg)?;
    let search = if paths.is_empty() {
        vec![root.to_path_buf()]
    } else {
        paths.to_vec()
    };
    let tree_files = runner::find_tree_files(&search)?;

    let run = || {
        let outcomes = runner::check_all(&packs, &tree_files, cfg.check);
        let scan = match cfg.check.uncovered {
            Level::Ignore => runner::UncoveredScan::default(),
            Level::Error | Level::Warn => runner::find_uncovered(&packs, &search, &tree_files),
        };
        (outcomes, scan)
    };
    let (outcomes, scan) = match jobs {
        Some(jobs) => rayon::ThreadPoolBuilder::new()
            .num_threads(jobs.get())
            .build()
            .context("building thread pool")?
            .install(run),
        // No -j: the lazily-built global rayon pool (one thread per core).
        None => run(),
    };

    if tree_files.is_empty() && scan.uncovered.is_empty() && scan.failed.is_empty() {
        println!("no .tree files found");
        return Ok(ExitCode::SUCCESS);
    }

    // Config is read only from the invocation root; surface any nested
    // btt.toml that governs a subtree but is not being applied.
    let mut nested_configs = std::collections::BTreeSet::new();
    for tree_path in &tree_files {
        let dir = tree_path.parent().unwrap_or(Path::new("."));
        let dir = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
        if let Some(cfg_dir) = config::nearest_config_dir(&dir, root)
            && cfg_dir != root
        {
            nested_configs.insert(cfg_dir);
        }
    }
    for dir in &nested_configs {
        println!(
            "note: ignoring nested config {} (config is read only from {}); run btt from {} to apply it",
            dir.join("btt.toml").display(),
            root.join("btt.toml").display(),
            dir.display()
        );
    }

    let (mut errors, mut warnings) = (0usize, 0usize);
    for outcome in &outcomes {
        let report = render(outcome, root);
        errors += report.errors;
        warnings += report.warnings;
        for line in &report.lines {
            println!("{line}");
        }
    }
    let scan_report = render_scan(&scan, cfg.check.uncovered, root);
    errors += scan_report.errors;
    warnings += scan_report.warnings;
    for line in &scan_report.lines {
        println!("{line}");
    }
    // Only claim an uncovered count when the scan actually ran.
    let uncovered_part = if cfg.check.uncovered == Level::Ignore {
        String::new()
    } else {
        format!("{} uncovered, ", scan.uncovered.len())
    };
    println!(
        "\n{} tree file(s), {uncovered_part}{errors} error(s), {warnings} warning(s)",
        tree_files.len()
    );
    // Spec drift exits 1; a file that could not be checked at all is a tool
    // failure and exits 2, like every other tool error.
    let failed = !scan.failed.is_empty()
        || outcomes
            .iter()
            .any(|o| matches!(o.result, runner::FileResult::Failed(_)));
    Ok(if failed {
        ExitCode::from(2)
    } else if errors > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// Render the uncovered scan: files needing specs at their configured
/// severity, then scan failures. Unverifiable coverage is a tool failure —
/// strict projects must not go green because extraction broke — so
/// failures are counted in full and printed capped.
fn render_scan(scan: &runner::UncoveredScan, level: Level, root: &Path) -> FileReport {
    let mut report = FileReport {
        lines: Vec::new(),
        errors: 0,
        warnings: 0,
    };
    for u in &scan.uncovered {
        let rel = u.path.strip_prefix(root).unwrap_or(&u.path);
        let sev = if level == Level::Error {
            report.errors += 1;
            "✗"
        } else {
            report.warnings += 1;
            "!"
        };
        report.lines.push(format!(
            "{sev} {} — {} test(s), not covered by any .tree",
            rel.display(),
            u.tests
        ));
    }
    if !scan.uncovered.is_empty() {
        report
            .lines
            .push("    hint: write a .tree next to each file mirroring its tests".to_string());
    }
    report.errors += scan.failed.len();
    for (path, e) in scan.failed.iter().take(5) {
        let rel = path.strip_prefix(root).unwrap_or(path);
        report
            .lines
            .push(format!("✗ {} — coverage scan failed: {e}", rel.display()));
    }
    if scan.failed.len() > 5 {
        report.lines.push(format!(
            "    …and {} more files could not be scanned",
            scan.failed.len() - 5
        ));
    }
    report
}

fn render(outcome: &runner::FileOutcome, root: &Path) -> FileReport {
    let rel = outcome
        .tree_path
        .strip_prefix(root)
        .unwrap_or(&outcome.tree_path);
    let mut report = FileReport {
        lines: Vec::new(),
        errors: 0,
        warnings: 0,
    };
    match &outcome.result {
        runner::FileResult::NoTarget { candidates } => {
            report.errors += 1;
            report
                .lines
                .push(format!("✗ {} — no matching test file", rel.display()));
            for c in candidates.iter().take(4) {
                report.lines.push(format!("    tried {}", c.display()));
            }
            report
                .lines
                .push(format!("    hint: btt scaffold {}", rel.display()));
        }
        // A broken file reports and counts as an error, but never aborts
        // the rest of the run.
        runner::FileResult::Failed(e) => {
            report.errors += 1;
            report.lines.push(format!("✗ {}", rel.display()));
            report.lines.push(format!("    error {e}"));
        }
        runner::FileResult::Checked { target, findings } if findings.is_empty() => {
            let shown = target.file_name().unwrap_or(target.as_os_str());
            report
                .lines
                .push(format!("✓ {} ({})", rel.display(), shown.to_string_lossy()));
        }
        runner::FileResult::Checked { target, findings } => {
            report
                .lines
                .push(format!("✗ {} → {}", rel.display(), target.display()));
            for r in findings {
                let sev = if r.level == Level::Error {
                    report.errors += 1;
                    "error"
                } else {
                    report.warnings += 1;
                    "warn "
                };
                report
                    .lines
                    .push(format!("    {sev} {}", describe(&r.finding, rel, target)));
            }
        }
    }
    report
}

fn describe(finding: &Finding, tree_rel: &Path, target: &Path) -> String {
    let noun = |kind: ActualKind| match kind {
        ActualKind::Block => "block",
        ActualKind::Test => "test ",
    };
    match finding {
        Finding::Missing {
            kind,
            path,
            spec_line,
        } => {
            format!(
                "missing {} `{path}` ({}:{spec_line})",
                noun(*kind),
                tree_rel.display()
            )
        }
        Finding::Extra {
            kind,
            path,
            target_line,
        } => {
            format!(
                "extra   {} `{path}` ({}:{target_line})",
                noun(*kind),
                target.display()
            )
        }
        Finding::OutOfOrder { path } => format!("order differs under `{path}`"),
    }
}

fn cmd_scaffold(
    tree_path: &Path,
    pack_name: Option<String>,
    output: Option<PathBuf>,
    force: bool,
    to_stdout: bool,
    root: &Path,
    cfg: &config::ProjectConfig,
) -> Result<ExitCode> {
    let pack = if let Some(name) = pack_name {
        pack::load(&name, root)?
    } else if cfg.project.packs.is_empty() {
        // Unconfigured: only builtins are candidates, as in `check`.
        let builtins = pack::builtin_names();
        let [only] = builtins.as_slice() else {
            bail!(
                "multiple packs available ({}); pick one with --pack",
                builtins.join(", ")
            );
        };
        pack::load_builtin(only)?
    } else {
        let [only] = cfg.project.packs.as_slice() else {
            bail!(
                "multiple packs configured ({}); pick one with --pack",
                cfg.project.packs.join(", ")
            );
        };
        pack::load(only, root)?
    };

    let spec_src = std::fs::read_to_string(tree_path)
        .with_context(|| format!("reading {}", tree_path.display()))?;
    let trees = tree::parse(&spec_src).map_err(|source| btt::Error::Parse {
        path: tree_path.to_path_buf(),
        source,
    })?;
    let expected = check::expected_from_spec(&trees, &pack.manifest.mapping);
    let stem = tree_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("test");
    let out_path = output.unwrap_or_else(|| {
        tree_path
            .parent()
            .unwrap_or(Path::new("."))
            .join(pack.manifest.scaffold.output.replace("{stem}", stem))
    });
    // The output's location decides the file's shape where a language has
    // more than one (Rust: a `tests/` integration crate is all test code,
    // while anywhere else the template wraps in `#[cfg(test)] mod tests`).
    let in_tests_dir = out_path.components().any(|c| c.as_os_str() == "tests");
    let rendered = scaffold::render(&pack, &expected, stem, in_tests_dir)?;

    if to_stdout {
        print!("{rendered}");
        return Ok(ExitCode::SUCCESS);
    }
    if out_path.exists() && !force {
        bail!(
            "{} already exists (use --force to overwrite, or --stdout to preview)",
            out_path.display()
        );
    }
    std::fs::write(&out_path, rendered)
        .with_context(|| format!("writing {}", out_path.display()))?;
    println!(
        "scaffolded {} ({} tests) from {}",
        out_path.display(),
        runner::count_tests(&expected),
        tree_path.display()
    );
    Ok(ExitCode::SUCCESS)
}

fn grammar_kind(pack: &pack::Pack) -> String {
    match &pack.manifest.grammar.source {
        pack::GrammarSource::Builtin(g) => format!("builtin:{g}"),
        pack::GrammarSource::Wasm(_) => "wasm".to_string(),
        pack::GrammarSource::Lexical => "lexical".to_string(),
    }
}

fn cmd_pack_list(root: &Path, cfg: &config::ProjectConfig) -> ExitCode {
    let builtins = pack::builtin_names();
    let user_dirs = pack::user_pack_dirs();
    for (name, origin) in pack::available(root) {
        match pack::load(&name, root) {
            Ok(p) => {
                let mut tags: Vec<String> = Vec::new();
                if cfg.project.packs.contains(&name) {
                    tags.push("active".to_string());
                }
                // Resolution order hides same-named packs from lower
                // sources; say so instead of leaving it invisible.
                let user_has = user_dirs
                    .iter()
                    .any(|d| d.join(&name).join("pack.toml").is_file());
                match origin {
                    pack::Origin::Project(_) => {
                        if user_has {
                            tags.push("shadows user".to_string());
                        }
                        if builtins.contains(&name) {
                            tags.push("shadows builtin".to_string());
                        }
                    }
                    pack::Origin::User(_) => {
                        if builtins.contains(&name) {
                            tags.push("shadows builtin".to_string());
                        }
                    }
                    pack::Origin::Builtin => {}
                }
                let tags = if tags.is_empty() {
                    String::new()
                } else {
                    format!("  ({})", tags.join(", "))
                };
                println!(
                    "{name}  v{}  [{origin}]  grammar={}{tags}  {}",
                    p.manifest.pack.version,
                    grammar_kind(&p),
                    p.manifest.pack.description
                );
            }
            Err(e) => println!("{name}  [{origin}]  (broken: {e})"),
        }
    }
    ExitCode::SUCCESS
}

fn print_file_table(files: &[install::StagedFile]) {
    println!("files:");
    for f in files {
        println!(
            "  {:<32} {:>9} B  sha256:{}…",
            f.rel,
            f.size,
            &f.sha256[..12.min(f.sha256.len())]
        );
    }
}

fn wasm_blob_warning(pack: &pack::Pack) {
    if let pack::GrammarSource::Wasm(file) = &pack.manifest.grammar.source {
        println!(
            "warning: contains a binary grammar blob (`{}`) — btt cannot review \
             this for you; install only from sources you trust",
            file.display()
        );
    }
}

fn cmd_pack_show(name: &str, root: &Path) -> Result<ExitCode> {
    let p = pack::load(name, root)?;
    println!(
        "{} v{} — {}",
        p.name(),
        p.manifest.pack.version,
        p.manifest.pack.description
    );
    println!("origin: {}", p.origin);
    println!("grammar: {}", grammar_kind(&p));
    println!("targets: {}", p.manifest.detect.targets.join(", "));
    match &p.origin {
        pack::Origin::Builtin => println!("files: embedded in the btt binary"),
        pack::Origin::User(dir) | pack::Origin::Project(dir) => {
            println!("directory: {}", dir.display());
            let files = install::file_digests(dir, &p.manifest)?;
            print_file_table(&files);
            wasm_blob_warning(&p);
            if let Some(receipt) = install::read_receipt(dir) {
                println!(
                    "installed: {} by {} (source: {})",
                    receipt.install.date, receipt.install.installed_by, receipt.install.source
                );
                if let Some(url) = &receipt.install.url {
                    println!("from: {url}");
                }
                if let Some(reference) = &receipt.install.reference {
                    println!("ref: {reference}");
                }
                if let Some(commit) = &receipt.install.commit {
                    println!("commit: {commit}");
                }
                let changed: Vec<&str> = files
                    .iter()
                    .filter(|f| receipt.files.get(&f.rel) != Some(&format!("sha256:{}", f.sha256)))
                    .map(|f| f.rel.as_str())
                    .collect();
                if !changed.is_empty() {
                    println!("modified since install: {}", changed.join(", "));
                }
            } else {
                println!("no install receipt (vendored or copied by hand)");
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Prompt for yes/no on stdin. Callers must have checked the terminal.
fn confirm(prompt: &str, default_yes: bool) -> Result<bool> {
    let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
    print!("{prompt} {hint} ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(match line.trim().to_lowercase().as_str() {
        "" => default_yes,
        "y" | "yes" => true,
        _ => false,
    })
}

/// Prompt for a 1-based selection out of `count` items.
fn select_number(count: usize) -> Result<usize> {
    print!("install which? [1-{count}] ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let n: usize = line.trim().parse().context("not a number")?;
    if n == 0 || n > count {
        bail!("selection out of range");
    }
    Ok(n - 1)
}

/// Resolve one pack directory inside a source that may hold several.
fn choose_pack_dir(base: &Path) -> Result<PathBuf> {
    let mut dirs = install::discover(base);
    match dirs.len() {
        0 => Err(btt::Error::NoPackInSource {
            path: base.to_path_buf(),
        }
        .into()),
        1 => Ok(dirs.remove(0)),
        _ => {
            println!("packs found in the source:");
            for (i, d) in dirs.iter().enumerate() {
                let rel = d.strip_prefix(base).unwrap_or(d);
                println!("  {}. {}", i + 1, rel.display());
            }
            if !std::io::stdin().is_terminal() {
                bail!("multiple packs in source; point --path or --dir at one of them");
            }
            let i = select_number(dirs.len())?;
            Ok(dirs.remove(i))
        }
    }
}

fn print_review(staged: &install::Staged, prov: &install::Provenance, curated: bool) {
    let m = &staged.pack.manifest;
    println!();
    println!(
        "pack {} v{} — {}",
        m.pack.name, m.pack.version, m.pack.description
    );
    println!("grammar: {}", grammar_kind(&staged.pack));
    println!("targets: {}", m.detect.targets.join(", "));
    let source = match prov.source.as_str() {
        "curated" => format!(
            "curated — {} @ {} (verified against digests in this btt build)",
            prov.url.as_deref().unwrap_or("?"),
            prov.reference.as_deref().unwrap_or("?")
        ),
        "git" => format!(
            "{} @ {}",
            prov.url.as_deref().unwrap_or("?"),
            prov.commit.as_deref().unwrap_or("?")
        ),
        _ => "local path".to_string(),
    };
    println!("source: {source}");
    print_file_table(&staged.files);
    wasm_blob_warning(&staged.pack);
    if !curated {
        println!();
        println!("review the pack contents:");
        print_pack_text(staged);
    }
}

/// Print the full text of every reviewable staged file — a lexical or
/// query/template pack is one screen of text; the wasm blob (if any) is
/// deliberately excluded and covered by the blob warning instead.
fn print_pack_text(staged: &install::Staged) {
    let wasm_rel = match &staged.pack.manifest.grammar.source {
        pack::GrammarSource::Wasm(f) => Some(f.to_string_lossy().into_owned()),
        pack::GrammarSource::Builtin(_) | pack::GrammarSource::Lexical => None,
    };
    for f in &staged.files {
        if Some(&f.rel) == wasm_rel.as_ref() {
            continue;
        }
        println!(
            "--- {} {}",
            f.rel,
            "-".repeat(60_usize.saturating_sub(f.rel.len()))
        );
        match std::fs::read_to_string(staged.dir().join(&f.rel)) {
            Ok(text) => print!("{text}"),
            Err(e) => println!("(unreadable: {e})"),
        }
        println!();
    }
}

/// An acquired install source: the pack directory to stage from, its
/// provenance, the curated index entry when applicable, and the temp
/// checkout (if any) kept alive — and cleaned up on drop — until staging
/// has copied out of it.
struct Acquired {
    source_dir: PathBuf,
    prov: install::Provenance,
    curated: Option<install::IndexEntry>,
    _checkout: Option<install::Checkout>,
}

fn acquire_path(path: &Path) -> Result<Acquired> {
    let source_dir = if path.join("pack.toml").is_file() {
        path.to_path_buf()
    } else {
        choose_pack_dir(path)?
    };
    Ok(Acquired {
        source_dir,
        prov: install::Provenance {
            source: "path".to_string(),
            url: None,
            reference: None,
            commit: None,
        },
        curated: None,
        _checkout: None,
    })
}

fn acquire_git(url: &str, reference: Option<&str>, subdir: Option<&str>) -> Result<Acquired> {
    let co = install::fetch_git(url, reference)?;
    let base = match subdir {
        Some(sub) => {
            let joined = co.dir().join(sub);
            let canon = joined
                .canonicalize()
                .with_context(|| format!("--dir {sub}: not found in the repository"))?;
            if !canon.starts_with(co.dir().canonicalize()?) {
                bail!("--dir must name a directory inside the repository");
            }
            joined
        }
        None => co.dir().to_path_buf(),
    };
    let source_dir = if base.join("pack.toml").is_file() {
        base
    } else {
        choose_pack_dir(&base)?
    };
    Ok(Acquired {
        source_dir,
        prov: install::Provenance {
            source: "git".to_string(),
            url: Some(url.to_string()),
            reference: reference.map(ToString::to_string),
            commit: Some(co.commit.clone()),
        },
        curated: None,
        _checkout: Some(co),
    })
}

/// Acquire from the curated index; `Ok(None)` when this build offers no
/// curated packs (a message has been printed).
fn acquire_curated(name: Option<&str>) -> Result<Option<Acquired>> {
    let index = install::curated_index()?;
    if index.packs.is_empty() {
        println!(
            "this btt build ships no curated packs yet; \
             install with --git <url> or --path <dir>"
        );
        return Ok(None);
    }
    let mut packs = index.packs;
    let entry = if let Some(n) = name {
        let i = packs.iter().position(|p| p.name == n).ok_or_else(|| {
            anyhow!(
                "`{n}` is not in the curated index (available: {})",
                packs
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
        packs.swap_remove(i)
    } else {
        println!("curated packs (tag {}):", index.tag);
        for (i, p) in packs.iter().enumerate() {
            println!("  {}. {} [{}] — {}", i + 1, p.name, p.kind, p.description);
        }
        if !std::io::stdin().is_terminal() {
            bail!("stdin is not a terminal; pass a pack name to install non-interactively");
        }
        packs.swap_remove(select_number(packs.len())?)
    };
    let url = env!("CARGO_PKG_REPOSITORY");
    println!("fetching {url} at tag {} …", index.tag);
    let co = install::fetch_git(url, Some(&index.tag))?;
    Ok(Some(Acquired {
        source_dir: co.dir().join(&entry.dir),
        prov: install::Provenance {
            source: "curated".to_string(),
            url: Some(url.to_string()),
            reference: Some(index.tag.clone()),
            commit: Some(co.commit.clone()),
        },
        curated: Some(entry),
        _checkout: Some(co),
    }))
}

fn cmd_pack_install(args: &InstallArgs, root: &Path) -> Result<ExitCode> {
    let dest = if args.project {
        root.join(".btt/packs")
    } else {
        pack::user_install_root()
            .context("cannot resolve the user pack directory (no home directory)")?
    };

    let acquired = if let Some(path) = &args.path {
        acquire_path(path)?
    } else if let Some(url) = &args.git {
        acquire_git(url, args.r#ref.as_deref(), args.dir.as_deref())?
    } else {
        match acquire_curated(args.name.as_deref())? {
            Some(a) => a,
            None => return Ok(ExitCode::SUCCESS),
        }
    };

    let staged = install::stage(&acquired.source_dir, &dest)?;
    if let Some(entry) = &acquired.curated {
        install::verify_curated(&staged, entry)?;
    }
    let curated = acquired.curated.is_some();
    let prov = acquired.prov.clone();
    let name = staged.name().to_string();
    let version = staged.pack.manifest.pack.version.clone();

    print_review(&staged, &prov, curated);
    if pack::builtin_names().contains(&name) {
        println!(
            "warning: `{name}` is also a builtin pack; projects naming it in \
             btt.toml will use this installed copy instead of the builtin"
        );
    }

    let proceed = if args.yes {
        true
    } else if std::io::stdin().is_terminal() {
        // Curated content is pre-verified; third-party defaults to no.
        confirm(&format!("install `{name}`?"), curated)?
    } else {
        bail!("stdin is not a terminal; pass --yes to confirm the install non-interactively");
    };
    if !proceed {
        println!("aborted; nothing installed");
        return Ok(ExitCode::FAILURE);
    }

    install::write_receipt(&staged, &prov)?;
    let installed = install::commit(staged, args.force)?;
    println!("installed {name} v{version} to {}", installed.display());
    println!("activate it by adding \"{name}\" to packs = [...] in btt.toml");
    Ok(ExitCode::SUCCESS)
}

fn cmd_pack_rm(name: &str, project: bool, yes: bool, root: &Path) -> Result<ExitCode> {
    if !pack::is_valid_name(name) {
        bail!("invalid pack name `{name}`");
    }
    let roots = if project {
        vec![root.join(".btt/packs")]
    } else {
        pack::user_pack_dirs()
    };
    // Pick the first root holding a real removable directory of this name.
    // Skipping symlinks/non-dirs means a stray symlink in a higher-priority
    // root can't shadow (and block removal of) a genuine pack lower down.
    let Some(packs_root) = roots.iter().find(|r| {
        r.join(name)
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_dir())
    }) else {
        if pack::builtin_names().iter().any(|b| b == name) {
            bail!("`{name}` is built into the btt binary and cannot be removed");
        }
        bail!(
            "pack `{name}` is not installed in {}",
            roots
                .iter()
                .map(|r| r.display().to_string())
                .collect::<Vec<_>>()
                .join(" or ")
        );
    };
    let target = packs_root.join(name);
    let version = pack::load_dir(&target)
        .map(|p| format!(" v{}", p.manifest.pack.version))
        .unwrap_or_default();
    println!("will remove {}{version}", target.display());
    let proceed = if yes {
        true
    } else if std::io::stdin().is_terminal() {
        confirm("remove?", false)?
    } else {
        bail!("stdin is not a terminal; pass --yes to confirm the removal non-interactively");
    };
    if !proceed {
        println!("aborted; nothing removed");
        return Ok(ExitCode::FAILURE);
    }
    let removed = install::remove(packs_root, name)?;
    println!("removed {}", removed.display());
    if let Ok(p) = pack::load(name, root) {
        println!("note: `{name}` still resolves from the {} source", p.origin);
    }
    Ok(ExitCode::SUCCESS)
}

const DEFAULT_CONFIG: &str = r#"# btt — branch tree testing (https://github.com/Maddiaa0/btt)

[project]
# Packs this project uses, in routing-priority order. Only packs named here
# are active (with no list, only builtins load). A named pack resolves from
# .btt/packs/, then user dirs ($XDG_CONFIG_HOME/btt/packs, legacy
# ~/.btt/packs), then the builtins.
packs = ["rust"]

[check]
# Severity of tests present in the file but absent from the tree: error | warn | ignore
extra = "warn"
# Severity of sibling order differing between tree and file: error | warn | ignore
order = "warn"
# Severity of test-bearing files with no .tree spec: error | warn | ignore
# (warn while adopting; set to "error" in CI once every file has a tree)
uncovered = "warn"
"#;

fn cmd_init(root: &Path, skill: bool) -> Result<ExitCode> {
    let cfg_path = root.join("btt.toml");
    if cfg_path.exists() {
        println!("btt.toml already exists, leaving it untouched");
    } else {
        std::fs::write(&cfg_path, DEFAULT_CONFIG)?;
        println!("wrote btt.toml — edit [project].packs for your languages");
    }
    if skill {
        let skill_dir = root.join(".claude/skills/btt");
        std::fs::create_dir_all(&skill_dir)?;
        let skill_path = skill_dir.join("SKILL.md");
        if skill_path.exists() {
            println!(
                "{} already exists, leaving it untouched",
                skill_path.display()
            );
        } else {
            std::fs::write(&skill_path, include_str!("../assets/SKILL.md"))?;
            println!("wrote {}", skill_path.display());
        }
    } else {
        println!(
            "tip: `btt init --skill` writes a Claude skill teaching agents the tree-first workflow"
        );
    }
    Ok(ExitCode::SUCCESS)
}
