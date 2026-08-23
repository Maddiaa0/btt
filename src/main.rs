use anyhow::{Context, Result, bail};
use btt::check::Finding;
use btt::config::{self, Level};
use btt::extract::ActualKind;
use btt::runner;
use btt::{check, pack, scaffold, tree};
use clap::{Parser, Subcommand};
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
    /// List available language packs and where they come from.
    Packs,
    /// Create btt.toml (and optionally an agent skill) in this project.
    Init {
        /// Also write .claude/skills/btt/SKILL.md for coding agents.
        #[arg(long)]
        skill: bool,
    },
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

    match cli.command {
        Command::Check { paths, jobs } => cmd_check(&paths, jobs, &root),
        Command::Scaffold {
            tree,
            pack,
            output,
            force,
            stdout,
        } => cmd_scaffold(&tree, pack, output, force, stdout, &root),
        Command::Packs => Ok(cmd_packs(&root)),
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

/// Config and packs governing one subtree (the reach of one `btt.toml`).
struct Subtree {
    cfg: config::ProjectConfig,
    packs: Vec<pack::Pack>,
}

/// Find every config root in the requested search area. Tree-bearing roots
/// are included explicitly so a single-file invocation still resolves its
/// ancestor config; walking also finds config-only subtrees whose uncovered
/// source files need their own packs and severity.
fn subtree_roots(search: &[PathBuf], tree_files: &[PathBuf], root: &Path) -> Result<Vec<PathBuf>> {
    let mut roots = std::collections::BTreeSet::from([root.to_path_buf()]);
    roots.extend(
        tree_files
            .iter()
            .map(|tree| config::governing_root(tree, root)),
    );

    for path in search {
        let config_start = if path.is_dir() {
            path.as_path()
        } else {
            path.parent().unwrap_or(Path::new("."))
        };
        let config_start = std::path::absolute(config_start)
            .with_context(|| format!("resolving {}", config_start.display()))?;
        if let Some(config_root) = config::nearest_config_dir(&config_start, root) {
            roots.insert(config_root);
        }

        let walker = walkdir::WalkDir::new(path)
            .into_iter()
            .filter_entry(|entry| {
                let name = entry.file_name().to_string_lossy();
                !(entry.file_type().is_dir()
                    && (name == ".git" || name == "target" || name == "node_modules"))
            });
        for entry in walker {
            let entry = entry.with_context(|| format!("walking {}", path.display()))?;
            if entry.file_type().is_file() && entry.file_name() == "btt.toml" {
                let Some(parent) = entry.path().parent() else {
                    continue;
                };
                let absolute = std::path::absolute(parent)
                    .with_context(|| format!("resolving {}", parent.display()))?;
                if absolute.starts_with(root) {
                    roots.insert(absolute);
                }
            }
        }
    }
    Ok(roots.into_iter().collect())
}

fn cmd_check(paths: &[PathBuf], jobs: Option<NonZeroUsize>, root: &Path) -> Result<ExitCode> {
    let search = if paths.is_empty() {
        vec![root.to_path_buf()]
    } else {
        paths.to_vec()
    };
    let tree_files = runner::find_tree_files(&search)?;

    let mut subtrees = std::collections::BTreeMap::new();
    for sub_root in subtree_roots(&search, &tree_files, root)? {
        let cfg = config::load(&sub_root)?;
        let packs = load_packs(&sub_root, &cfg)?;
        subtrees.insert(sub_root, Subtree { cfg, packs });
    }

    let mut grouped_trees: std::collections::BTreeMap<PathBuf, Vec<PathBuf>> =
        std::collections::BTreeMap::new();
    for tree in &tree_files {
        grouped_trees
            .entry(config::governing_root(tree, root))
            .or_default()
            .push(tree.clone());
    }

    let run = || {
        let mut outcomes = Vec::new();
        for (sub_root, trees) in &grouped_trees {
            let subtree = &subtrees[sub_root];
            outcomes.extend(runner::check_all(&subtree.packs, trees, subtree.cfg.check));
        }
        outcomes.sort_by(|a, b| a.tree_path.cmp(&b.tree_path));

        let mut scans = Vec::new();
        for (sub_root, subtree) in &subtrees {
            let level = subtree.cfg.check.uncovered;
            if level == Level::Ignore {
                continue;
            }
            let mut scan = runner::find_uncovered(&subtree.packs, &search, &tree_files);
            scan.uncovered
                .retain(|item| config::governing_root(&item.path, root) == *sub_root);
            scan.failed
                .retain(|(path, _)| config::governing_root(path, root) == *sub_root);
            scans.push((level, scan));
        }
        (outcomes, scans)
    };
    let (outcomes, scans) = match jobs {
        Some(jobs) => rayon::ThreadPoolBuilder::new()
            .num_threads(jobs.get())
            .build()
            .context("building thread pool")?
            .install(run),
        // No -j: the lazily-built global rayon pool (one thread per core).
        None => run(),
    };

    if tree_files.is_empty()
        && scans
            .iter()
            .all(|(_, scan)| scan.uncovered.is_empty() && scan.failed.is_empty())
    {
        println!("no .tree files found");
        return Ok(ExitCode::SUCCESS);
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
    let mut uncovered = 0usize;
    for (level, scan) in &scans {
        uncovered += scan.uncovered.len();
        let scan_report = render_scan(scan, *level, root);
        errors += scan_report.errors;
        warnings += scan_report.warnings;
        for line in &scan_report.lines {
            println!("{line}");
        }
    }
    // Only claim an uncovered count when the scan actually ran.
    let uncovered_part = if scans.is_empty() {
        String::new()
    } else {
        format!("{uncovered} uncovered, ")
    };
    println!(
        "\n{} tree file(s), {uncovered_part}{errors} error(s), {warnings} warning(s)",
        tree_files.len()
    );
    // Spec drift exits 1; a file that could not be checked at all is a tool
    // failure and exits 2, like every other tool error.
    let failed = scans.iter().any(|(_, scan)| !scan.failed.is_empty())
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
) -> Result<ExitCode> {
    let sub_root = config::governing_root(tree_path, root);
    let cfg = config::load(&sub_root)?;
    let pack = if let Some(name) = pack_name {
        pack::load(&name, &sub_root)?
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
        pack::load(only, &sub_root)?
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

fn cmd_packs(root: &Path) -> ExitCode {
    for (name, origin) in pack::available(root) {
        match pack::load(&name, root) {
            Ok(p) => println!(
                "{name}  v{}  [{origin}]  {}",
                p.manifest.pack.version, p.manifest.pack.description
            ),
            Err(e) => println!("{name}  [{origin}]  (broken: {e})"),
        }
    }
    ExitCode::SUCCESS
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
