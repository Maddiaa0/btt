use anyhow::{Context, Result, bail};
use btt::check::Finding;
use btt::config::{self, Level};
use btt::extract::ActualKind;
use btt::runner;
use btt::{check, diff, pack, scaffold, tree};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitCode};

mod pack_add;

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
        /// Output format.
        #[arg(long, value_enum, default_value_t)]
        format: OutputFormat,
    },
    /// Compare behaviors in .tree specs between Git revisions.
    Diff {
        /// Revision or revision range (`REV` means `REV..working tree`).
        revisions: String,
        /// Exit 1 when differences exist.
        #[arg(long)]
        exit_code: bool,
        /// Output format.
        #[arg(long, value_enum, default_value_t)]
        format: OutputFormat,
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
        /// Add missing skeletons to an existing file without changing bodies.
        #[arg(long, conflicts_with_all = ["force", "stdout"])]
        merge: bool,
    },
    /// List available language packs and where they come from.
    Packs,
    /// Add a language pack to this project.
    Pack {
        #[command(subcommand)]
        command: PackCommand,
    },
    /// Create btt.toml (and optionally an agent skill) in this project.
    Init {
        /// Also write .claude/skills/btt/SKILL.md for coding agents.
        #[arg(long)]
        skill: bool,
    },
}

#[derive(Clone, Copy, Default, ValueEnum)]
enum OutputFormat {
    #[default]
    Human,
    Json,
}

#[derive(Subcommand)]
enum PackCommand {
    /// Add one pack from a local directory or Git repository.
    Add {
        /// Local directory, Git URL, or GitHub owner/repo.
        source: String,
        /// Pack directory within the source repository.
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Treat the source as Git even if a matching local path exists.
        #[arg(long)]
        git: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json = cli.json_requested();
    let diff_json = cli.diff_json_requested();
    match run(cli) {
        Ok(code) => code,
        Err(err) => {
            if json {
                let serialized = if diff_json {
                    serde_json::to_string(&DiffJsonReport::error(format!("{err:#}")))
                } else {
                    serde_json::to_string(&JsonReport::error(format!("{err:#}")))
                };
                println!(
                    "{}",
                    serialized
                        .unwrap_or_else(|_| r#"{"error":"failed to serialize error"}"#.to_string())
                );
            } else {
                eprintln!("error: {err:#}");
            }
            ExitCode::from(2)
        }
    }
}

impl Cli {
    fn json_requested(&self) -> bool {
        matches!(
            self.command,
            Command::Check {
                format: OutputFormat::Json,
                ..
            } | Command::Diff {
                format: OutputFormat::Json,
                ..
            }
        )
    }

    fn diff_json_requested(&self) -> bool {
        matches!(
            self.command,
            Command::Diff {
                format: OutputFormat::Json,
                ..
            }
        )
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    let cwd = std::env::current_dir().context("getting cwd")?;
    let root = config::find_project_root(&cwd);

    match cli.command {
        Command::Check {
            paths,
            jobs,
            format,
        } => cmd_check(&paths, jobs, format, &root),
        Command::Diff {
            revisions,
            exit_code,
            format,
        } => cmd_diff(&revisions, exit_code, format, &cwd),
        Command::Scaffold {
            tree,
            pack,
            output,
            force,
            stdout,
            merge,
        } => cmd_scaffold(&tree, pack, output, force, stdout, merge, &root),
        Command::Packs => Ok(cmd_packs(&root)),
        Command::Pack { command } => match command {
            PackCommand::Add { source, dir, git } => {
                cmd_pack_add(&source, dir.as_deref(), git, &root)
            }
        },
        Command::Init { skill } => cmd_init(&root, skill),
    }
}

#[derive(Default, Serialize)]
struct DiffJsonSummary {
    tree_files: usize,
    added: usize,
    removed: usize,
    renamed: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum DiffStatus {
    Added,
    Removed,
    Changed,
}

#[derive(Serialize)]
struct DiffJsonResult {
    tree: String,
    status: DiffStatus,
    added: Vec<String>,
    removed: Vec<String>,
    renamed: Vec<diff::Rename>,
}

#[derive(Serialize)]
struct DiffJsonReport {
    summary: DiffJsonSummary,
    results: Vec<DiffJsonResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl DiffJsonReport {
    fn error(error: String) -> Self {
        Self {
            summary: DiffJsonSummary::default(),
            results: Vec::new(),
            error: Some(error),
        }
    }
}

#[derive(Clone, Copy)]
enum DiffSide<'a> {
    Revision(&'a str),
    Working,
}

fn cmd_diff(
    revisions: &str,
    exit_code: bool,
    format: OutputFormat,
    cwd: &Path,
) -> Result<ExitCode> {
    let repo = git_repo_root(cwd)?;
    let (old, new) = match revisions.split_once("..") {
        Some((old, new)) if !old.is_empty() && !new.is_empty() && !new.contains("..") => {
            (DiffSide::Revision(old), DiffSide::Revision(new))
        }
        Some(_) => bail!("revision range must be REV..REV"),
        None => (DiffSide::Revision(revisions), DiffSide::Working),
    };
    if revisions.is_empty() {
        bail!("revision must not be empty");
    }
    let old_files = read_diff_side(&repo, old)?;
    let new_files = read_diff_side(&repo, new)?;
    let paths: std::collections::BTreeSet<_> =
        old_files.keys().chain(new_files.keys()).cloned().collect();
    let mut report = DiffJsonReport {
        summary: DiffJsonSummary::default(),
        results: Vec::new(),
        error: None,
    };
    for path in paths {
        let (status, changes) = match (old_files.get(&path), new_files.get(&path)) {
            (Some(old), Some(new)) => {
                let old =
                    tree::parse(old).with_context(|| format!("parsing {path} on old side"))?;
                let new =
                    tree::parse(new).with_context(|| format!("parsing {path} on new side"))?;
                let changes = diff::compare(&old, &new);
                if changes.is_empty() {
                    continue;
                }
                (DiffStatus::Changed, changes)
            }
            (None, Some(new)) => (
                DiffStatus::Added,
                diff::Changes {
                    added: behavior_paths(new, &path, "new")?,
                    ..diff::Changes::default()
                },
            ),
            (Some(old), None) => (
                DiffStatus::Removed,
                diff::Changes {
                    removed: behavior_paths(old, &path, "old")?,
                    ..diff::Changes::default()
                },
            ),
            (None, None) => unreachable!(),
        };
        report.summary.added += changes.added.len();
        report.summary.removed += changes.removed.len();
        report.summary.renamed += changes.renamed.len();
        report.results.push(DiffJsonResult {
            tree: path,
            status,
            added: changes.added,
            removed: changes.removed,
            renamed: changes.renamed,
        });
    }
    report.summary.tree_files = report.results.len();
    let different = !report.results.is_empty();
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string(&report)?),
        OutputFormat::Human => render_diff_human(&report),
    }
    Ok(if exit_code && different {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn behavior_paths(source: &str, path: &str, side: &str) -> Result<Vec<String>> {
    let trees = tree::parse(source).with_context(|| format!("parsing {path} on {side} side"))?;
    Ok(diff::compare(&[], &trees).added)
}

fn render_diff_human(report: &DiffJsonReport) {
    for result in &report.results {
        let label = match result.status {
            DiffStatus::Added => "ADDED",
            DiffStatus::Removed => "REMOVED",
            DiffStatus::Changed => "CHANGED",
        };
        println!("{label} {}", result.tree);
        for rename in &result.renamed {
            println!("  ~ {} -> {}", rename.from, rename.to);
        }
        for path in &result.removed {
            println!("  - {path}");
        }
        for path in &result.added {
            println!("  + {path}");
        }
    }
    println!(
        "\n{} tree file(s) changed, {} added, {} removed, {} renamed",
        report.summary.tree_files,
        report.summary.added,
        report.summary.removed,
        report.summary.renamed
    );
}

fn git_repo_root(cwd: &Path) -> Result<PathBuf> {
    let output = ProcessCommand::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .output()
        .context("running git rev-parse")?;
    if !output.status.success() {
        bail!(
            "not a git repository: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(PathBuf::from(String::from_utf8(output.stdout)?.trim()))
}

fn read_diff_side(repo: &Path, side: DiffSide<'_>) -> Result<BTreeMap<String, String>> {
    match side {
        DiffSide::Working => {
            let files = runner::find_tree_files(&[repo.to_path_buf()])?;
            files
                .into_iter()
                .map(|path| {
                    let relative = path
                        .strip_prefix(repo)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .replace('\\', "/");
                    let source = std::fs::read_to_string(&path)
                        .with_context(|| format!("reading {}", path.display()))?;
                    Ok((relative, source))
                })
                .collect()
        }
        DiffSide::Revision(revision) => read_revision(repo, revision),
    }
}

fn read_revision(repo: &Path, revision: &str) -> Result<BTreeMap<String, String>> {
    let object_id = resolve_revision(repo, revision)?;
    let output = ProcessCommand::new("git")
        .args(["ls-tree", "-r", "--name-only", &object_id, "--"])
        .current_dir(repo)
        .output()
        .with_context(|| format!("listing revision {revision}"))?;
    if !output.status.success() {
        bail!(
            "cannot read revision {revision}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let names = String::from_utf8(output.stdout)?;
    let mut files = BTreeMap::new();
    for path in names.lines().filter(|path| is_discovered_tree(path)) {
        let object = format!("{object_id}:{path}");
        let shown = ProcessCommand::new("git")
            .args(["show", &object, "--"])
            .current_dir(repo)
            .output()
            .with_context(|| format!("reading {path} at {revision}"))?;
        if !shown.status.success() {
            bail!(
                "cannot read {path} at {revision}: {}",
                String::from_utf8_lossy(&shown.stderr).trim()
            );
        }
        files.insert(path.to_string(), String::from_utf8(shown.stdout)?);
    }
    Ok(files)
}

fn resolve_revision(repo: &Path, revision: &str) -> Result<String> {
    let commit = format!("{revision}^{{commit}}");
    let output = ProcessCommand::new("git")
        .args(["rev-parse", "--verify", "--end-of-options", &commit])
        .current_dir(repo)
        .output()
        .with_context(|| format!("resolving revision {revision}"))?;
    if !output.status.success() {
        bail!(
            "cannot resolve revision {revision}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn is_discovered_tree(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|ext| ext == "tree")
        && !path
            .split('/')
            .any(|part| matches!(part, ".git" | "target" | "node_modules"))
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
enum FindingKind {
    Missing,
    Extra,
    OutOfOrder,
    Unsupported,
    Todo,
    Uncovered,
    NoTarget,
    CheckFailed,
    ScanFailed,
}

#[derive(Serialize)]
struct JsonFinding {
    kind: FindingKind,
    severity: Level,
    message: String,
    tree_path: Option<String>,
    file: String,
    line: Option<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum JsonStatus {
    Pass,
    Fail,
}

#[derive(Serialize)]
struct JsonResult {
    tree: Option<String>,
    target: Option<String>,
    status: JsonStatus,
    findings: Vec<JsonFinding>,
}

#[derive(Default, Serialize)]
struct JsonSummary {
    tree_files: usize,
    uncovered: usize,
    errors: usize,
    warnings: usize,
    findings: BTreeMap<FindingKind, BTreeMap<Level, usize>>,
}

#[derive(Serialize)]
struct JsonReport {
    summary: JsonSummary,
    results: Vec<JsonResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl JsonReport {
    fn new(tree_files: usize) -> Self {
        Self {
            summary: JsonSummary {
                tree_files,
                ..JsonSummary::default()
            },
            results: Vec::new(),
            error: None,
        }
    }

    fn error(error: String) -> Self {
        Self {
            error: Some(error),
            ..Self::new(0)
        }
    }

    fn push(&mut self, result: JsonResult) {
        for finding in &result.findings {
            *self
                .summary
                .findings
                .entry(finding.kind)
                .or_default()
                .entry(finding.severity)
                .or_default() += 1;
        }
        self.summary.errors = self.summary.count_serialized_severity("error");
        self.summary.warnings = self.summary.count_serialized_severity("warn");
        self.results.push(result);
    }
}

impl JsonSummary {
    fn count_serialized_severity(&self, wanted: &str) -> usize {
        self.findings
            .values()
            .flat_map(BTreeMap::iter)
            .filter(|(severity, _)| {
                serde_json::to_value(severity)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .as_deref()
                    == Some(wanted)
            })
            .map(|(_, count)| count)
            .sum()
    }
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
    let mut roots = std::collections::BTreeSet::new();
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
        roots.insert(
            config::nearest_config_dir(&config_start, root).unwrap_or_else(|| root.to_path_buf()),
        );

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

fn cmd_check(
    paths: &[PathBuf],
    jobs: Option<NonZeroUsize>,
    format: OutputFormat,
    root: &Path,
) -> Result<ExitCode> {
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
            let uncovered_level = subtree.cfg.check.uncovered;
            let unsupported_level = subtree.cfg.check.unsupported;
            if uncovered_level == Level::Ignore && unsupported_level == Level::Ignore {
                continue;
            }
            let mut scan = runner::find_uncovered(&subtree.packs, &search, &tree_files);
            scan.uncovered
                .retain(|item| config::governing_root(&item.path, root) == *sub_root);
            scan.unsupported
                .retain(|item| config::governing_root(&item.path, root) == *sub_root);
            scan.failed
                .retain(|(path, _)| config::governing_root(path, root) == *sub_root);
            scans.push((uncovered_level, unsupported_level, scan));
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
        && scans.iter().all(|(_, _, scan)| {
            scan.uncovered.is_empty() && scan.unsupported.is_empty() && scan.failed.is_empty()
        })
    {
        match format {
            OutputFormat::Human => println!("no .tree files found"),
            OutputFormat::Json => println!("{}", serde_json::to_string(&JsonReport::new(0))?),
        }
        return Ok(ExitCode::SUCCESS);
    }

    if matches!(format, OutputFormat::Json) {
        return render_json(&tree_files, &outcomes, &scans, root);
    }
    Ok(render_human(&tree_files, &outcomes, &scans, root))
}

fn render_human(
    tree_files: &[PathBuf],
    outcomes: &[runner::FileOutcome],
    scans: &[(Level, Level, runner::UncoveredScan)],
    root: &Path,
) -> ExitCode {
    let (mut errors, mut warnings) = (0usize, 0usize);
    for outcome in outcomes {
        let report = render(outcome, root);
        errors += report.errors;
        warnings += report.warnings;
        for line in &report.lines {
            println!("{line}");
        }
    }
    let mut uncovered = 0usize;
    for (uncovered_level, unsupported_level, scan) in scans {
        uncovered += scan.uncovered.len();
        let scan_report = render_scan(scan, *uncovered_level, *unsupported_level, root);
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
    let failed = scans.iter().any(|(_, _, scan)| !scan.failed.is_empty())
        || outcomes
            .iter()
            .any(|o| matches!(o.result, runner::FileResult::Failed(_)));
    if failed {
        ExitCode::from(2)
    } else if errors > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn shown_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn finding_json(
    reported: &runner::Reported,
    tree: &Path,
    target: &Path,
    root: &Path,
) -> JsonFinding {
    let (kind, message, tree_path, file, line) = match &reported.finding {
        Finding::Missing {
            path, spec_line, ..
        } => (
            FindingKind::Missing,
            describe(&reported.finding, tree, target),
            Some(path.clone()),
            tree.display().to_string(),
            Some(*spec_line),
        ),
        Finding::Extra {
            path, target_line, ..
        } => (
            FindingKind::Extra,
            describe(&reported.finding, tree, target),
            Some(path.clone()),
            shown_path(target, root),
            Some(*target_line),
        ),
        Finding::OutOfOrder { path } => (
            FindingKind::OutOfOrder,
            describe(&reported.finding, tree, target),
            Some(path.clone()),
            shown_path(target, root),
            None,
        ),
        Finding::Unsupported { target_line } => (
            FindingKind::Unsupported,
            describe(&reported.finding, tree, target),
            None,
            shown_path(target, root),
            Some(*target_line),
        ),
        Finding::Todo { target_line, .. } => (
            FindingKind::Todo,
            describe(&reported.finding, tree, target),
            None,
            shown_path(target, root),
            Some(*target_line),
        ),
    };
    JsonFinding {
        kind,
        severity: reported.level,
        message,
        tree_path,
        file,
        line,
    }
}

fn render_json(
    tree_files: &[PathBuf],
    outcomes: &[runner::FileOutcome],
    scans: &[(Level, Level, runner::UncoveredScan)],
    root: &Path,
) -> Result<ExitCode> {
    let mut report = JsonReport::new(tree_files.len());
    let mut tool_failed = false;
    for outcome in outcomes {
        tool_failed |= add_json_outcome(&mut report, outcome, root);
    }
    for (uncovered_level, unsupported_level, scan) in scans {
        tool_failed |= add_json_scan(
            &mut report,
            scan,
            *uncovered_level,
            *unsupported_level,
            root,
        );
    }
    let exit = if tool_failed {
        ExitCode::from(2)
    } else if report.summary.errors > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    };
    println!("{}", serde_json::to_string(&report)?);
    Ok(exit)
}

fn add_json_outcome(report: &mut JsonReport, outcome: &runner::FileOutcome, root: &Path) -> bool {
    let tree = shown_path(&outcome.tree_path, root);
    let (target, findings, tool_failed) = match &outcome.result {
        runner::FileResult::Checked { target, findings } => (
            Some(shown_path(target, root)),
            findings
                .iter()
                .map(|finding| finding_json(finding, Path::new(&tree), target, root))
                .collect(),
            false,
        ),
        runner::FileResult::NoTarget { candidates } => {
            let mut message = "no matching test file".to_string();
            for candidate in candidates.iter().take(4) {
                _ = write!(message, "\ntried {}", candidate.display());
            }
            _ = write!(message, "\nhint: btt scaffold {tree}");
            (
                None,
                vec![JsonFinding {
                    kind: FindingKind::NoTarget,
                    severity: Level::Error,
                    message,
                    tree_path: None,
                    file: tree.clone(),
                    line: None,
                }],
                false,
            )
        }
        runner::FileResult::Failed(error) => (
            None,
            vec![JsonFinding {
                kind: FindingKind::CheckFailed,
                severity: Level::Error,
                message: error.to_string(),
                tree_path: None,
                file: tree.clone(),
                line: None,
            }],
            true,
        ),
    };
    report.push(JsonResult {
        tree: Some(tree),
        target,
        status: if findings.is_empty() {
            JsonStatus::Pass
        } else {
            JsonStatus::Fail
        },
        findings,
    });
    tool_failed
}

fn add_json_scan(
    report: &mut JsonReport,
    scan: &runner::UncoveredScan,
    uncovered_level: Level,
    unsupported_level: Level,
    root: &Path,
) -> bool {
    report.summary.uncovered += scan.uncovered.len();
    if uncovered_level != Level::Ignore {
        for uncovered in &scan.uncovered {
            report.push(JsonResult { tree: None, target: None, status: JsonStatus::Fail, findings: vec![JsonFinding {
                kind: FindingKind::Uncovered, severity: uncovered_level,
                message: format!("{} test(s), not covered by any .tree\nhint: write a .tree next to each file mirroring its tests", uncovered.tests),
                tree_path: None, file: shown_path(&uncovered.path, root), line: None,
            }] });
        }
    }
    if unsupported_level != Level::Ignore {
        for unsupported in &scan.unsupported {
            report.push(JsonResult { tree: None, target: None, status: JsonStatus::Fail, findings: vec![JsonFinding {
                kind: FindingKind::Unsupported, severity: unsupported_level,
                message: "unsupported: parameterized test (test.each) is not representable — expand into explicit leaves (see AGENT-SETUP)".to_string(),
                tree_path: None, file: shown_path(&unsupported.path, root), line: Some(unsupported.line),
            }] });
        }
    }
    for (path, error) in &scan.failed {
        report.push(JsonResult {
            tree: None,
            target: None,
            status: JsonStatus::Fail,
            findings: vec![JsonFinding {
                kind: FindingKind::ScanFailed,
                severity: Level::Error,
                message: format!("coverage scan failed: {error}"),
                tree_path: None,
                file: shown_path(path, root),
                line: None,
            }],
        });
    }
    !scan.failed.is_empty()
}

/// Render the uncovered scan: files needing specs at their configured
/// severity, then scan failures. Unverifiable coverage is a tool failure —
/// strict projects must not go green because extraction broke — so
/// failures are counted in full and printed capped.
fn render_scan(
    scan: &runner::UncoveredScan,
    uncovered_level: Level,
    unsupported_level: Level,
    root: &Path,
) -> FileReport {
    let mut report = FileReport {
        lines: Vec::new(),
        errors: 0,
        warnings: 0,
    };
    if uncovered_level != Level::Ignore {
        for u in &scan.uncovered {
            let rel = u.path.strip_prefix(root).unwrap_or(&u.path);
            let sev = if uncovered_level == Level::Error {
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
    }
    if unsupported_level != Level::Ignore {
        for finding in &scan.unsupported {
            let rel = finding.path.strip_prefix(root).unwrap_or(&finding.path);
            let sev = if unsupported_level == Level::Error {
                report.errors += 1;
                "error"
            } else {
                report.warnings += 1;
                "warn "
            };
            report.lines.push(format!(
                "    {sev} unsupported: parameterized test (test.each) is not representable — expand into explicit leaves (see AGENT-SETUP) ({}:{})",
                rel.display(),
                finding.line
            ));
        }
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
        Finding::Unsupported { target_line } => format!(
            "unsupported: parameterized test (test.each) is not representable — expand into explicit leaves (see AGENT-SETUP) ({}:{target_line})",
            target.display()
        ),
        Finding::Todo { target_line, .. } => format!(
            "todo: test body never filled in — scaffold marker still present ({}:{target_line})",
            target.display()
        ),
    }
}

fn cmd_scaffold(
    tree_path: &Path,
    pack_name: Option<String>,
    output: Option<PathBuf>,
    force: bool,
    to_stdout: bool,
    merge: bool,
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
    let rendered = if merge {
        let existing = std::fs::read_to_string(&out_path)
            .with_context(|| format!("reading {} for merge", out_path.display()))?;
        scaffold::merge(&pack, &expected, &out_path, &existing, stem, in_tests_dir)?
    } else {
        scaffold::render(&pack, &expected, stem, in_tests_dir)?
    };

    if to_stdout {
        print!("{rendered}");
        return Ok(ExitCode::SUCCESS);
    }
    if out_path.exists() && !force && !merge {
        bail!(
            "{} already exists (use --force to overwrite, or --stdout to preview)",
            out_path.display()
        );
    }
    if merge {
        scaffold::write_atomic(&out_path, &rendered)?;
    } else {
        std::fs::write(&out_path, rendered)
            .with_context(|| format!("writing {}", out_path.display()))?;
    }
    println!(
        "{} {} ({} tests) from {}",
        if merge { "merged" } else { "scaffolded" },
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

fn cmd_pack_add(source: &str, dir: Option<&Path>, git: bool, root: &Path) -> Result<ExitCode> {
    let added = pack_add::add(source, dir, root, git)?;
    println!(
        "added {} v{} to {}",
        added.name,
        added.version,
        added.path.display()
    );
    println!(
        "activate it by adding {:?} to [project].packs in btt.toml",
        added.name
    );
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
# Severity of recognized constructs btt cannot represent: error | warn | ignore
unsupported = "error"
# Severity of scaffold markers left in test bodies: error | warn | ignore
todo = "warn"
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
