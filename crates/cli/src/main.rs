use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use engine::{
    analyze, plan, run as transfer_run, AnalyzeReport, EngineError, FingerprintCache, Journal,
    MovedPolicy, PreflightRules, ReconcilePlan, ReconcileState, RunOptions, TransferOutcome,
    TransferSummary,
};

#[derive(Parser)]
#[command(
    name = "migrator",
    version,
    about = "Find duplicates and safely transfer folder trees"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scan one or two roots and report exact duplicates. Nothing is copied
    /// or changed.
    Analyze(AnalyzeArgs),

    /// Compare a source and destination root and classify every file.
    /// Nothing is copied, changed, or deleted.
    Plan(PlanArgs),

    /// Reconcile, then copy every missing or partial file to the
    /// destination. The source is never modified.
    Run(RunArgs),
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum OnMoved {
    /// Leave source-only files whose content already exists elsewhere at
    /// the destination uncopied (default).
    Leave,
    /// Copy them anyway, so the exact source relative path also exists at
    /// the destination.
    Copy,
}

impl From<OnMoved> for MovedPolicy {
    fn from(value: OnMoved) -> Self {
        match value {
            OnMoved::Leave => MovedPolicy::Leave,
            OnMoved::Copy => MovedPolicy::Copy,
        }
    }
}

#[derive(clap::Args)]
struct AnalyzeArgs {
    /// Root directories to scan (one or two)
    #[arg(required = true, num_args = 1..=2)]
    roots: Vec<PathBuf>,

    /// Print the report as JSON instead of text
    #[arg(long)]
    json: bool,

    /// Fingerprint cache file to read and update. If omitted, no cache is
    /// read or written and every file is hashed fresh.
    #[arg(long)]
    cache: Option<PathBuf>,
}

#[derive(clap::Args)]
struct PlanArgs {
    source: PathBuf,
    destination: PathBuf,

    /// Print the plan as JSON instead of text
    #[arg(long)]
    json: bool,

    /// Fingerprint cache file to read and update. If omitted, no cache is
    /// read or written and every file is hashed fresh.
    #[arg(long)]
    cache: Option<PathBuf>,

    /// SQLite journal file to record this plan run in. If omitted, the plan
    /// is not persisted anywhere.
    #[arg(long)]
    journal: Option<PathBuf>,

    /// Longest relative path length allowed at the destination
    #[arg(long, default_value_t = PreflightRules::default().max_path_length)]
    max_path_length: usize,

    /// Reject files over the FAT32 4 GiB limit
    #[arg(long)]
    fat32: bool,
}

#[derive(clap::Args)]
struct RunArgs {
    source: PathBuf,
    destination: PathBuf,

    /// Reconcile and report what would happen, but copy nothing
    #[arg(long)]
    dry_run: bool,

    /// What to do with a source file whose content already exists at a
    /// different destination path
    #[arg(long, value_enum, default_value_t = OnMoved::Leave)]
    on_moved: OnMoved,

    /// Fingerprint cache file to read and update. If omitted, no cache is
    /// read or written and every file is hashed fresh.
    #[arg(long)]
    cache: Option<PathBuf>,

    /// SQLite journal file to record this run in. If omitted, the run is
    /// not persisted anywhere.
    #[arg(long)]
    journal: Option<PathBuf>,

    /// Longest relative path length allowed at the destination
    #[arg(long, default_value_t = PreflightRules::default().max_path_length)]
    max_path_length: usize,

    /// Reject files over the FAT32 4 GiB limit
    #[arg(long)]
    fat32: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Analyze(args) => run_analyze(args),
        Command::Plan(args) => run_plan(args),
        Command::Run(args) => run_run(args),
    }
}

fn load_cache(path: &Option<PathBuf>) -> std::result::Result<FingerprintCache, EngineError> {
    match path {
        Some(p) => FingerprintCache::load(p),
        None => Ok(FingerprintCache::default()),
    }
}

fn build_rules(max_path_length: usize, fat32: bool) -> PreflightRules {
    PreflightRules {
        max_path_length,
        fat32_max_file_size: fat32.then_some(4_294_967_295),
        ..PreflightRules::default()
    }
}

fn run_analyze(args: AnalyzeArgs) -> ExitCode {
    let mut cache = match &args.cache {
        Some(path) => match FingerprintCache::load(path) {
            Ok(cache) => cache,
            Err(err) => {
                eprintln!("error: {err}");
                return ExitCode::FAILURE;
            }
        },
        None => FingerprintCache::default(),
    };

    let report = match analyze(&args.roots, &mut cache) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };

    if let Some(path) = &args.cache {
        if let Err(err) = cache.save(path) {
            eprintln!("warning: failed to save fingerprint cache: {err}");
        }
    }

    if args.json {
        match serde_json::to_string_pretty(&report) {
            Ok(json) => println!("{json}"),
            Err(err) => {
                eprintln!("error: failed to serialize report: {err}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        print_text_report(&report);
    }

    ExitCode::SUCCESS
}

fn print_text_report(report: &AnalyzeReport) {
    println!(
        "Scanned {} files ({}) across {} root(s)",
        report.files_scanned,
        format_bytes(report.bytes_scanned),
        report.roots.len()
    );
    println!(
        "Found {} duplicate group(s), {} duplicate file(s), {} reclaimable",
        report.duplicate_groups.len(),
        report.duplicate_files,
        format_bytes(report.reclaimable_bytes)
    );

    for group in &report.duplicate_groups {
        println!(
            "\nblake3:{} ({}, {} copies)",
            &group.hash[..16],
            format_bytes(group.size),
            group.files.len()
        );
        for file in &group.files {
            println!("  {}", file.display());
        }
    }
}

fn run_plan(args: PlanArgs) -> ExitCode {
    let mut cache = match load_cache(&args.cache) {
        Ok(cache) => cache,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };

    let rules = build_rules(args.max_path_length, args.fat32);

    let report = match plan(&args.source, &args.destination, &mut cache, &rules) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };

    if let Some(path) = &args.cache {
        if let Err(err) = cache.save(path) {
            eprintln!("warning: failed to save fingerprint cache: {err}");
        }
    }

    if let Some(path) = &args.journal {
        match Journal::open(path) {
            Ok(mut journal) => {
                if let Err(err) = journal.record_plan(&report) {
                    eprintln!("warning: failed to record plan in journal: {err}");
                }
            }
            Err(err) => eprintln!("warning: failed to open journal: {err}"),
        }
    }

    if args.json {
        match serde_json::to_string_pretty(&report) {
            Ok(json) => println!("{json}"),
            Err(err) => {
                eprintln!("error: failed to serialize report: {err}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        print_plan_text_report(&report);
    }

    ExitCode::SUCCESS
}

fn print_plan_text_report(report: &ReconcilePlan) {
    println!(
        "Reconcile plan: {} -> {}",
        report.source_root.display(),
        report.destination_root.display()
    );
    println!(
        "verified {}  partial {}  missing {}  moved {}  conflict {}  blocked {}  destination-only {}",
        report.summary.verified,
        report.summary.partial,
        report.summary.missing,
        report.summary.moved,
        report.summary.conflict,
        report.summary.blocked,
        report.summary.destination_only
    );

    for entry in &report.entries {
        if matches!(
            entry.state,
            ReconcileState::Verified | ReconcileState::DestinationOnly
        ) {
            continue;
        }
        println!(
            "[{}] {}",
            state_label(&entry.state),
            entry.relative_path.display()
        );
    }
}

fn run_run(args: RunArgs) -> ExitCode {
    let mut cache = match load_cache(&args.cache) {
        Ok(cache) => cache,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };

    let rules = build_rules(args.max_path_length, args.fat32);

    let report = match plan(&args.source, &args.destination, &mut cache, &rules) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };

    if let Some(path) = &args.cache {
        if let Err(err) = cache.save(path) {
            eprintln!("warning: failed to save fingerprint cache: {err}");
        }
    }

    let mut journal = match &args.journal {
        Some(path) => match Journal::open(path) {
            Ok(journal) => Some(journal),
            Err(err) => {
                eprintln!("warning: failed to open journal: {err}");
                None
            }
        },
        None => None,
    };
    let run_id = journal.as_mut().and_then(|j| match j.record_plan(&report) {
        Ok(id) => Some(id),
        Err(err) => {
            eprintln!("warning: failed to record plan in journal: {err}");
            None
        }
    });

    if args.dry_run {
        println!("Dry run: no files were copied.");
        print_plan_text_report(&report);
        return ExitCode::SUCCESS;
    }

    let options = RunOptions {
        on_moved: args.on_moved.into(),
    };
    let summary = match transfer_run(&report, &options) {
        Ok(summary) => summary,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };

    if let (Some(journal), Some(run_id)) = (journal.as_mut(), run_id) {
        if let Err(err) = journal.record_transfer_results(run_id, &summary.results) {
            eprintln!("warning: failed to record transfer results in journal: {err}");
        }
    }

    print_run_summary(&summary);

    if summary.failed > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn print_run_summary(summary: &TransferSummary) {
    println!(
        "Copied {} file(s) ({}), {} failed",
        summary.copied,
        format_bytes(summary.bytes_copied),
        summary.failed
    );
    println!(
        "Skipped: {} verified, {} conflict, {} blocked, {} moved",
        summary.skipped_verified,
        summary.skipped_conflict,
        summary.skipped_blocked,
        summary.skipped_moved
    );

    for result in &summary.results {
        if result.outcome == TransferOutcome::Failed {
            println!(
                "[FAILED] {} ({})",
                result.relative_path.display(),
                result.detail.as_deref().unwrap_or("unknown error")
            );
        }
    }
}

fn state_label(state: &ReconcileState) -> &'static str {
    match state {
        ReconcileState::Verified => "VERIFIED",
        ReconcileState::Partial { .. } => "PARTIAL",
        ReconcileState::Missing => "MISSING",
        ReconcileState::Moved { .. } => "MOVED",
        ReconcileState::Conflict => "CONFLICT",
        ReconcileState::Blocked { .. } => "BLOCKED",
        ReconcileState::DestinationOnly => "DESTINATION-ONLY",
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}
