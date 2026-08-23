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
    let cfg = config::load(&root)?;

    match cli.command {
        Command::Check { paths, jobs } => cmd_check(&paths, jobs, &root, &cfg),
        Command::Scaffold { tree, pack, output, force, stdout } => {
            cmd_scaffold(&tree, pack, output, force, stdout, &root, &cfg)
        }
        Command::Packs => Ok(cmd_packs(&root)),
        Command::Init { skill } => cmd_init(&root, skill),
    }
}

/// Load the project's configured packs, or every available pack when the
/// project doesn't pin any.
fn load_packs(root: &Path, cfg: &config::ProjectConfig) -> Result<Vec<pack::Pack>> {
    let names: Vec<String> = if cfg.project.packs.is_empty() {
        pack::available(root).into_iter().map(|(name, _)| name).collect()
    } else {
        cfg.project.packs.clone()
    };
    Ok(names.iter().map(|n| pack::load(n, root)).collect::<btt::Result<_>>()?)
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
    let search = if paths.is_empty() { vec![root.to_path_buf()] } else { paths.to_vec() };
    let tree_files = runner::find_tree_files(&search);

    let run = || {
        let outcomes = runner::check_all(&packs, &tree_files, cfg.check);
        let uncovered = match cfg.check.uncovered {
            Level::Ignore => Vec::new(),
            Level::Error | Level::Warn => runner::find_uncovered(&packs, &search),
        };
        (outcomes, uncovered)
    };
    let (outcomes, uncovered) = match jobs {
        Some(jobs) => rayon::ThreadPoolBuilder::new()
            .num_threads(jobs.get())
            .build()
            .context("building thread pool")?
            .install(run),
        // No -j: the lazily-built global rayon pool (one thread per core).
        None => run(),
    };

    if tree_files.is_empty() && uncovered.is_empty() {
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
    for u in &uncovered {
        let rel = u.path.strip_prefix(root).unwrap_or(&u.path);
        let sev = if cfg.check.uncovered == Level::Error {
            errors += 1;
            "✗"
        } else {
            warnings += 1;
            "!"
        };
        println!("{sev} {} — {} test(s), not covered by any .tree", rel.display(), u.tests);
    }
    if !uncovered.is_empty() {
        println!("    hint: write a .tree next to each file mirroring its tests");
    }
    // Only claim an uncovered count when the scan actually ran.
    let uncovered_part = if cfg.check.uncovered == Level::Ignore {
        String::new()
    } else {
        format!("{} uncovered, ", uncovered.len())
    };
    println!(
        "\n{} tree file(s), {uncovered_part}{errors} error(s), {warnings} warning(s)",
        tree_files.len()
    );
    // Spec drift exits 1; a file that could not be checked at all is a tool
    // failure and exits 2, like every other tool error.
    let failed = outcomes.iter().any(|o| matches!(o.result, runner::FileResult::Failed(_)));
    Ok(if failed {
        ExitCode::from(2)
    } else if errors > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn render(outcome: &runner::FileOutcome, root: &Path) -> FileReport {
    let rel = outcome.tree_path.strip_prefix(root).unwrap_or(&outcome.tree_path);
    let mut report = FileReport { lines: Vec::new(), errors: 0, warnings: 0 };
    match &outcome.result {
        runner::FileResult::NoTarget { candidates } => {
            report.errors += 1;
            report.lines.push(format!("✗ {} — no matching test file", rel.display()));
            for c in candidates.iter().take(4) {
                report.lines.push(format!("    tried {}", c.display()));
            }
            report.lines.push(format!("    hint: btt scaffold {}", rel.display()));
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
            report.lines.push(format!("✓ {} ({})", rel.display(), shown.to_string_lossy()));
        }
        runner::FileResult::Checked { target, findings } => {
            report.lines.push(format!("✗ {} → {}", rel.display(), target.display()));
            for r in findings {
                let sev = if r.level == Level::Error {
                    report.errors += 1;
                    "error"
                } else {
                    report.warnings += 1;
                    "warn "
                };
                report.lines.push(format!("    {sev} {}", describe(&r.finding, rel, target)));
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
        Finding::Missing { kind, path, spec_line } => {
            format!("missing {} `{path}` ({}:{spec_line})", noun(*kind), tree_rel.display())
        }
        Finding::Extra { kind, path, target_line } => {
            format!("extra   {} `{path}` ({}:{target_line})", noun(*kind), target.display())
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
    } else {
        let mut packs = cfg.project.packs.clone();
        if packs.is_empty() {
            packs = pack::available(root).into_iter().map(|(name, _)| name).collect();
        }
        let [only] = packs.as_slice() else {
            bail!("multiple packs available ({}); pick one with --pack", packs.join(", "));
        };
        pack::load(only, root)?
    };

    let spec_src = std::fs::read_to_string(tree_path)
        .with_context(|| format!("reading {}", tree_path.display()))?;
    let trees = tree::parse(&spec_src)
        .map_err(|source| btt::Error::Parse { path: tree_path.to_path_buf(), source })?;
    let expected = check::expected_from_spec(&trees, &pack.manifest.mapping);
    let stem = tree_path.file_stem().and_then(|s| s.to_str()).unwrap_or("test");
    let rendered = scaffold::render(&pack, &expected, stem)?;

    if to_stdout {
        print!("{rendered}");
        return Ok(ExitCode::SUCCESS);
    }
    let out_path = output.unwrap_or_else(|| {
        tree_path
            .parent()
            .unwrap_or(Path::new("."))
            .join(pack.manifest.scaffold.output.replace("{stem}", stem))
    });
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
# Packs this project uses, in routing-priority order.
# Project-local packs in .btt/packs/ override user ($XDG_CONFIG_HOME/btt/packs,
# then legacy ~/.btt/packs) and builtin ones.
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
            println!("{} already exists, leaving it untouched", skill_path.display());
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
