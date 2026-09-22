use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use engine::{
    analyze, find_similar, plan, run as transfer_run, verify as verify_manifest, write_manifest,
    AnalyzeReport, EngineError, FingerprintCache, Journal, MovedPolicy, PreflightRules,
    ReconcilePlan, ReconcileState, Relationship, RunOptions, SimilarityConfig, SimilarityReport,
    TransferOutcome, TransferSummary, VerifyReport,
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

    /// Write (or refresh) the tamper-evident manifest for a destination:
    /// MIGRATION_MANIFEST.json, its checksum, a backup copy, and the SQLite
    /// record of every file's SHA-256.
    WriteManifest(WriteManifestArgs),

    /// Re-scan a destination and report what changed since its manifest was
    /// written: moved, renamed, changed, or deleted files.
    Verify(VerifyArgs),

    /// Find near-duplicate and contains/diverged relationships among
    /// text-like files (txt, md, html, json, csv, tsv, source code).
    /// Thresholds are uncalibrated defaults; see SPEC.md section 4.
    Similar(SimilarArgs),
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

#[derive(clap::Args)]
struct WriteManifestArgs {
    destination: PathBuf,

    /// Source root to record in the manifest, for reference only
    #[arg(long)]
    source: Option<PathBuf>,

    /// Print the result as JSON instead of text
    #[arg(long)]
    json: bool,
}

#[derive(clap::Args)]
struct VerifyArgs {
    destination: PathBuf,

    /// Print the report as JSON instead of text
    #[arg(long)]
    json: bool,
}

#[derive(clap::Args)]
struct SimilarArgs {
    /// Root directories to scan (one or two)
    #[arg(required = true, num_args = 1..=2)]
    roots: Vec<PathBuf>,

    /// Print the report as JSON instead of text
    #[arg(long)]
    json: bool,

    /// Minimum estimated Jaccard similarity for a pair to be reported
    #[arg(long, default_value_t = SimilarityConfig::default().duplicate_cutoff)]
    duplicate_cutoff: f64,

    /// Coverage threshold used to call a direction "high" when classifying
    /// a pair's relationship
    #[arg(long, default_value_t = SimilarityConfig::default().high_coverage)]
    high_coverage: f64,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Analyze(args) => run_analyze(args),
        Command::Plan(args) => run_plan(args),
        Command::Run(args) => run_run(args),
        Command::WriteManifest(args) => run_write_manifest(args),
        Command::Verify(args) => run_verify(args),
        Command::Similar(args) => run_similar(args),
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

fn run_write_manifest(args: WriteManifestArgs) -> ExitCode {
    let result = match write_manifest(&args.destination, args.source.as_deref()) {
        Ok(result) => result,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };

    if args.json {
        match serde_json::to_string_pretty(&result) {
            Ok(json) => println!("{json}"),
            Err(err) => {
                eprintln!("error: failed to serialize result: {err}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        println!(
            "Wrote manifest for {} files ({}), {} duplicate group(s)",
            result.file_count,
            format_bytes(result.total_bytes),
            result.duplicate_group_count
        );
        println!(
            "{}: sha256:{}",
            result.manifest_path.display(),
            result.sha256
        );
        if !result.app_local_hash_recorded {
            println!("warning: could not determine an app data directory; the app-local copy of the hash was not recorded");
        }
    }

    ExitCode::SUCCESS
}

fn run_verify(args: VerifyArgs) -> ExitCode {
    let report = match verify_manifest(&args.destination) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };

    if args.json {
        match serde_json::to_string_pretty(&report) {
            Ok(json) => println!("{json}"),
            Err(err) => {
                eprintln!("error: failed to serialize report: {err}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        print_verify_report(&report);
    }

    if !report.integrity.trusted || !report.changed.is_empty() || !report.deleted.is_empty() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn print_verify_report(report: &VerifyReport) {
    println!("Verify: {}", report.destination_root.display());
    println!(
        "manifest integrity: {}",
        if report.integrity.trusted {
            "trusted"
        } else {
            "NOT TRUSTED"
        }
    );
    println!(
        "unchanged {}  changed {}  moved {}  deleted {}  unrecorded {}",
        report.unchanged,
        report.changed.len(),
        report.moved.len(),
        report.deleted.len(),
        report.unrecorded.len()
    );

    for c in &report.changed {
        println!("[CHANGED] {}", c.relative_path.display());
    }
    for m in &report.moved {
        println!(
            "[MOVED] {} -> {}",
            m.recorded_path.display(),
            m.current_path.display()
        );
    }
    for d in &report.deleted {
        println!("[DELETED] {}", d.display());
    }
    for u in &report.unrecorded {
        println!("[UNRECORDED] {}", u.display());
    }
}

fn run_similar(args: SimilarArgs) -> ExitCode {
    let config = SimilarityConfig {
        duplicate_cutoff: args.duplicate_cutoff,
        high_coverage: args.high_coverage,
        ..SimilarityConfig::default()
    };

    let report = match find_similar(&args.roots, &config) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };

    if args.json {
        match serde_json::to_string_pretty(&report) {
            Ok(json) => println!("{json}"),
            Err(err) => {
                eprintln!("error: failed to serialize report: {err}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        print_similarity_report(&report);
    }

    ExitCode::SUCCESS
}

fn print_similarity_report(report: &SimilarityReport) {
    println!(
        "Considered {} file(s), skipped {} unsupported format(s)",
        report.files_considered, report.files_skipped_unsupported
    );
    println!(
        "Thresholds are uncalibrated defaults (SPEC.md section 4) - review before acting on them."
    );
    println!("Found {} similar pair(s)", report.pairs.len());

    for pair in &report.pairs {
        println!(
            "\n[{}] jaccard~{:.2}  {} -> {}: {:.2}  {} -> {}: {:.2}",
            relationship_label(pair.relationship),
            pair.jaccard_estimate,
            pair.a.display(),
            pair.b.display(),
            pair.coverage_a_to_b,
            pair.b.display(),
            pair.a.display(),
            pair.coverage_b_to_a
        );
    }
}

fn relationship_label(relationship: Relationship) -> &'static str {
    match relationship {
        Relationship::NearDuplicate => "NEAR-DUPLICATE",
        Relationship::Diverged => "DIVERGED",
        Relationship::AIsMoreComplete => "A-IS-MORE-COMPLETE",
        Relationship::BIsMoreComplete => "B-IS-MORE-COMPLETE",
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
