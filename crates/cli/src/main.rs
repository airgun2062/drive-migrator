use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use engine::{
    analyze, plan, AnalyzeReport, FingerprintCache, Journal, PreflightRules, ReconcilePlan,
    ReconcileState,
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

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Analyze(args) => run_analyze(args),
        Command::Plan(args) => run_plan(args),
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

    let rules = PreflightRules {
        max_path_length: args.max_path_length,
        fat32_max_file_size: args.fat32.then_some(4_294_967_295),
        ..PreflightRules::default()
    };

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
