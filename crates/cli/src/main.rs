use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use engine::{analyze, AnalyzeReport, FingerprintCache};

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

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Analyze(args) => run_analyze(args),
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
