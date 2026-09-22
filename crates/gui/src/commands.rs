//! The GUI's entire IPC surface (SPEC.md section 9's five screens). Every
//! command wraps `engine` directly - no logic lives here beyond turning
//! frontend requests into engine calls and engine reports into JSON, mirroring
//! the orchestration `crates/cli/src/main.rs` already does for `migrator run`.

use std::path::PathBuf;

use engine::{
    analyze, apply_collapse_to_plan, find_similar, hash::full_hash, plan as reconcile_plan,
    plan_collapse, run_with_progress, verify as verify_manifest, write_manifest, AnalyzeReport,
    CollapseConfig, CollapseReport, FingerprintCache, Journal, MovedPolicy, PreflightRules,
    ReconcilePlan, RunOptions, SimilarityConfig, SimilarityReport, TransferSummary, VerifyReport,
};
use serde::{Deserialize, Serialize};
use tauri::Emitter;
use tauri_plugin_dialog::DialogExt;

/// Settings shared by the Scan screen and the eventual transfer, so the
/// Review and Version History screens can be computed from one scan and the
/// same settings reused unchanged for the real transfer.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSettings {
    pub source: String,
    pub destination: String,
    pub mode: TransferModeArg,
    #[serde(default)]
    pub on_moved_copy: bool,
    #[serde(default = "default_keep_newest")]
    pub keep_newest: usize,
    #[serde(default)]
    pub archive: bool,
    #[serde(default)]
    pub protected_extensions: Vec<String>,
    #[serde(default = "default_max_path_length")]
    pub max_path_length: usize,
    #[serde(default)]
    pub fat32: bool,
    #[serde(default)]
    pub cache_path: Option<String>,
    #[serde(default)]
    pub journal_path: Option<String>,
    #[serde(default = "default_duplicate_cutoff")]
    pub duplicate_cutoff: f64,
    #[serde(default = "default_high_coverage")]
    pub high_coverage: f64,
}

fn default_keep_newest() -> usize {
    CollapseConfig::default().keep_newest
}
fn default_max_path_length() -> usize {
    PreflightRules::default().max_path_length
}
fn default_duplicate_cutoff() -> f64 {
    SimilarityConfig::default().duplicate_cutoff
}
fn default_high_coverage() -> f64 {
    SimilarityConfig::default().high_coverage
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferModeArg {
    Copy,
    Collapse,
}

fn similarity_config(settings: &ScanSettings) -> SimilarityConfig {
    SimilarityConfig {
        duplicate_cutoff: settings.duplicate_cutoff,
        high_coverage: settings.high_coverage,
        ..SimilarityConfig::default()
    }
}

fn collapse_config(settings: &ScanSettings) -> CollapseConfig {
    CollapseConfig {
        keep_newest: settings.keep_newest,
        archive: settings.archive,
        protected_extensions: settings
            .protected_extensions
            .iter()
            .map(|e| e.to_ascii_lowercase())
            .collect(),
        similarity: similarity_config(settings),
    }
}

fn preflight_rules(settings: &ScanSettings) -> PreflightRules {
    PreflightRules {
        max_path_length: settings.max_path_length,
        fat32_max_file_size: settings.fat32.then_some(4_294_967_295),
        ..PreflightRules::default()
    }
}

fn load_cache(path: &Option<String>) -> Result<FingerprintCache, String> {
    match path {
        Some(p) => FingerprintCache::load(&PathBuf::from(p)).map_err(|e| e.to_string()),
        None => Ok(FingerprintCache::default()),
    }
}

fn save_cache(cache: &FingerprintCache, path: &Option<String>) {
    if let Some(p) = path {
        // Best-effort, same as the CLI: a cache write failure should not
        // fail the whole operation.
        let _ = cache.save(&PathBuf::from(p));
    }
}

/// Everything the Scan/Review/Version History screens need from one round
/// trip: the reconcile plan (state counts, per-file status), the exact-
/// duplicate groups, the near-duplicate/version relationships, and - only in
/// Collapse mode - what would be kept, collapsed, or archived.
#[derive(Debug, Serialize)]
pub struct ScanBundle {
    pub plan: ReconcilePlan,
    pub analyze: AnalyzeReport,
    pub similarity: SimilarityReport,
    pub collapse: Option<CollapseReport>,
}

#[tauri::command]
pub fn pick_folder(app: tauri::AppHandle) -> Option<String> {
    app.dialog()
        .file()
        .blocking_pick_folder()
        .and_then(|fp| fp.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn scan(settings: ScanSettings) -> Result<ScanBundle, String> {
    let source = PathBuf::from(&settings.source);
    let destination = PathBuf::from(&settings.destination);
    let mut cache = load_cache(&settings.cache_path)?;
    let rules = preflight_rules(&settings);

    let plan_report =
        reconcile_plan(&source, &destination, &mut cache, &rules).map_err(|e| e.to_string())?;
    let analyze_report =
        analyze(std::slice::from_ref(&source), &mut cache).map_err(|e| e.to_string())?;
    let similarity_report =
        find_similar(std::slice::from_ref(&source), &similarity_config(&settings))
            .map_err(|e| e.to_string())?;

    let collapse_report = if settings.mode == TransferModeArg::Collapse {
        let config = collapse_config(&settings);
        Some(
            plan_collapse(std::slice::from_ref(&source), &mut cache, &config)
                .map_err(|e| e.to_string())?,
        )
    } else {
        None
    };

    save_cache(&cache, &settings.cache_path);

    Ok(ScanBundle {
        plan: plan_report,
        analyze: analyze_report,
        similarity: similarity_report,
        collapse: collapse_report,
    })
}

/// One entry of the live transfer-progress stream, emitted to the frontend
/// as the `transfer-progress` event after each file the transfer touches.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TransferProgressEvent {
    relative_path: String,
    outcome: &'static str,
    bytes: Option<u64>,
    files_done: usize,
}

#[tauri::command]
pub fn transfer(
    settings: ScanSettings,
    dry_run: bool,
    window: tauri::Window,
) -> Result<TransferSummary, String> {
    let source = PathBuf::from(&settings.source);
    let destination = PathBuf::from(&settings.destination);
    let mut cache = load_cache(&settings.cache_path)?;
    let rules = preflight_rules(&settings);

    let mut plan_report =
        reconcile_plan(&source, &destination, &mut cache, &rules).map_err(|e| e.to_string())?;

    let collapse_report = if settings.mode == TransferModeArg::Collapse {
        let config = collapse_config(&settings);
        Some(
            plan_collapse(std::slice::from_ref(&source), &mut cache, &config)
                .map_err(|e| e.to_string())?,
        )
    } else {
        None
    };

    save_cache(&cache, &settings.cache_path);

    // Same ordering as the CLI: record the full, unfiltered plan to the
    // journal first, then apply collapse filtering to what actually
    // transfers - so the journal keeps a complete audit trail even when
    // collapse mode narrows the copy set.
    let mut journal = match &settings.journal_path {
        Some(path) => Journal::open(&PathBuf::from(path)).ok(),
        None => None,
    };
    let run_id = journal
        .as_mut()
        .and_then(|j| j.record_plan(&plan_report).ok());

    if let Some(collapse) = &collapse_report {
        apply_collapse_to_plan(&mut plan_report, collapse);
    }

    if dry_run {
        return Ok(TransferSummary::default());
    }

    let options = RunOptions {
        on_moved: if settings.on_moved_copy {
            MovedPolicy::Copy
        } else {
            MovedPolicy::Leave
        },
    };

    let mut files_done = 0usize;
    let summary = run_with_progress(&plan_report, &options, |result| {
        files_done += 1;
        let event = TransferProgressEvent {
            relative_path: result.relative_path.to_string_lossy().into_owned(),
            outcome: match result.outcome {
                engine::TransferOutcome::Copied => "copied",
                engine::TransferOutcome::Failed => "failed",
            },
            bytes: result.bytes,
            files_done,
        };
        let _ = window.emit("transfer-progress", &event);
    })
    .map_err(|e| e.to_string())?;

    if let (Some(journal), Some(run_id)) = (journal.as_mut(), run_id) {
        let _ = journal.record_transfer_results(run_id, &summary.results);
    }

    Ok(summary)
}

#[tauri::command]
pub fn write_manifest_cmd(
    destination: String,
    source: Option<String>,
) -> Result<engine::ManifestWriteResult, String> {
    write_manifest(
        &PathBuf::from(destination),
        source.as_deref().map(PathBuf::from).as_deref(),
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn verify_cmd(destination: String) -> Result<VerifyReport, String> {
    verify_manifest(&PathBuf::from(destination)).map_err(|e| e.to_string())
}

/// Version History's "drop a file to find its family" lookup: hashes the
/// dropped file so the frontend can match it against an already-fetched
/// `ScanBundle`'s duplicate groups (by hash) and version families (by path).
#[tauri::command]
pub fn hash_file(path: String) -> Result<String, String> {
    full_hash(&PathBuf::from(path))
        .map(|h| h.to_hex().to_string())
        .map_err(|e| e.to_string())
}
