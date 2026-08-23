use anyhow::{bail, Context, Result};
use btt::config::{self, Level};
use btt::runner::{self, Target};
use btt::{check, pack, scaffold, tree};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

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

fn main() {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => std::process::exit(code),
        Err(err) => {
            eprintln!("error: {err:#}");
            std::process::exit(2);
        }
    }
}

fn run(cli: Cli) -> Result<i32> {
    let cwd = std::env::current_dir().context("getting cwd")?;
    let root = config::find_project_root(&cwd);
    let cfg = config::load(&root)?;

    match cli.command {
        Command::Check { paths } => cmd_check(paths, &root, &cfg),
        Command::Scaffold { tree, pack, output, force, stdout } => {
            cmd_scaffold(tree, pack, output, force, stdout, &root, &cfg)
        }
        Command::Packs => cmd_packs(&root),
        Command::Init { skill } => cmd_init(&root, skill),
    }
}

/// Load the project's configured packs, or every available pack when the
/// project doesn't pin any.
fn load_packs(root: &std::path::Path, cfg: &config::ProjectConfig) -> Result<Vec<pack::Pack>> {
    let names: Vec<String> = if cfg.project.packs.is_empty() {
        pack::available(root).into_iter().map(|(n, _)| n).collect()
    } else {
        cfg.project.packs.clone()
    };
    names.iter().map(|n| pack::load(n, root)).collect()
}

fn cmd_check(paths: Vec<PathBuf>, root: &std::path::Path, cfg: &config::ProjectConfig) -> Result<i32> {
    let packs = load_packs(root, cfg)?;
    let search = if paths.is_empty() { vec![root.to_path_buf()] } else { paths };
    let tree_files = runner::find_tree_files(&search);
    if tree_files.is_empty() {
        println!("no .tree files found");
        return Ok(0);
    }

    let mut errors = 0usize;
    let mut warnings = 0usize;
    for tree_path in &tree_files {
        let rel = tree_path.strip_prefix(root).unwrap_or(tree_path);
        match runner::resolve_target(tree_path, &packs) {
            Target::NotFound { candidates } => {
                errors += 1;
                println!("✗ {} — no matching test file", rel.display());
                for c in candidates.iter().take(4) {
                    println!("    tried {}", c.display());
                }
                println!("    hint: btt scaffold {}", rel.display());
            }
            Target::Found { pack, path } => {
                let reported = runner::check_file(pack, tree_path, &path, &cfg.check)?;
                if reported.is_empty() {
                    println!("✓ {} ({})", rel.display(), path.file_name().unwrap().to_string_lossy());
                    continue;
                }
                println!("✗ {} → {}", rel.display(), path.display());
                for r in &reported {
                    let (label, loc) = describe(&r.finding, rel, &path);
                    let sev = match r.level {
                        Level::Error => "error",
                        Level::Warn => "warn ",
                        Level::Ignore => unreachable!(),
                    };
                    match r.level {
                        Level::Error => errors += 1,
                        _ => warnings += 1,
                    }
                    println!("    {sev} {label} {loc}");
                }
            }
        }
    }
    println!(
        "\n{} tree file(s), {} error(s), {} warning(s)",
        tree_files.len(),
        errors,
        warnings
    );
    Ok(if errors > 0 { 1 } else { 0 })
}

fn describe(
    finding: &check::Finding,
    tree_rel: &std::path::Path,
    target: &std::path::Path,
) -> (String, String) {
    use check::FindingKind::*;
    let label = match finding.kind {
        MissingBlock => format!("missing block `{}`", finding.path),
        MissingTest => format!("missing test  `{}`", finding.path),
        ExtraBlock => format!("extra block   `{}`", finding.path),
        ExtraTest => format!("extra test    `{}`", finding.path),
        OutOfOrder => format!("order differs under `{}`", finding.path),
    };
    let loc = match (finding.spec_line, finding.target_line) {
        (Some(l), _) => format!("({}:{})", tree_rel.display(), l),
        (_, Some(l)) => format!("({}:{})", target.display(), l),
        _ => String::new(),
    };
    (label, loc)
}

fn cmd_scaffold(
    tree_path: PathBuf,
    pack_name: Option<String>,
    output: Option<PathBuf>,
    force: bool,
    to_stdout: bool,
    root: &std::path::Path,
    cfg: &config::ProjectConfig,
) -> Result<i32> {
    let pack = match pack_name {
        Some(name) => pack::load(&name, root)?,
        None => {
            let mut packs = cfg.project.packs.clone();
            if packs.is_empty() {
                packs = pack::available(root).into_iter().map(|(n, _)| n).collect();
            }
            if packs.len() == 1 {
                pack::load(&packs[0], root)?
            } else {
                bail!(
                    "multiple packs available ({}); pick one with --pack",
                    packs.join(", ")
                );
            }
        }
    };

    let spec_src = std::fs::read_to_string(&tree_path)
        .with_context(|| format!("reading {}", tree_path.display()))?;
    let trees = tree::parse(&spec_src)?;
    let expected = check::expected_from_spec(&trees, &pack.manifest.mapping);
    let stem = tree_path.file_stem().and_then(|s| s.to_str()).unwrap_or("test");
    let rendered = scaffold::render(&pack, &expected, stem)?;

    if to_stdout {
        print!("{rendered}");
        return Ok(0);
    }
    let out_path = output.unwrap_or_else(|| {
        tree_path
            .parent()
            .unwrap_or(std::path::Path::new("."))
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
    Ok(0)
}

fn cmd_packs(root: &std::path::Path) -> Result<i32> {
    for (name, origin) in pack::available(root) {
        match pack::load(&name, root) {
            Ok(p) => println!(
                "{name}  v{}  [{origin}]  {}",
                p.manifest.pack.version, p.manifest.pack.description
            ),
            Err(e) => println!("{name}  [{origin}]  (broken: {e})"),
        }
    }
    Ok(0)
}

const DEFAULT_CONFIG: &str = r#"# btt — branch tree testing (https://github.com/Maddiaa0/btt)

[project]
# Packs this project uses, in routing-priority order.
# Project-local packs in .btt/packs/ override user (~/.btt/packs) and builtin ones.
packs = ["rust"]

[check]
# Severity of tests present in the file but absent from the tree: error | warn | ignore
extra = "warn"
# Severity of sibling order differing between tree and file: error | warn | ignore
order = "warn"
"#;

fn cmd_init(root: &std::path::Path, skill: bool) -> Result<i32> {
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
        std::fs::write(&skill_path, include_str!("../assets/SKILL.md"))?;
        println!("wrote {}", skill_path.display());
    } else {
        println!("tip: `btt init --skill` writes a Claude skill teaching agents the tree-first workflow");
    }
    Ok(0)
}
