use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{EngineError, Result};
use crate::hash;
use crate::reconcile::{PlanEntry, ReconcilePlan, ReconcileState};
use crate::scan;

const STREAM_BUFFER_BYTES: usize = 256 * 1024;

/// What to do with a source file whose content already exists at a
/// different destination path (SPEC.md section 3: "Moved | ... | Ask: leave,
/// move into place, or copy"). P3 supports the two non-destructive choices;
/// renaming an existing destination file into place is deferred, since it
/// mutates the destination in a way that overlaps with later collapse work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MovedPolicy {
    #[default]
    Leave,
    Copy,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RunOptions {
    pub on_moved: MovedPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferOutcome {
    Copied,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct TransferResult {
    pub relative_path: PathBuf,
    pub outcome: TransferOutcome,
    pub bytes: Option<u64>,
    pub detail: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct TransferSummary {
    pub copied: usize,
    pub failed: usize,
    pub skipped_verified: usize,
    pub skipped_conflict: usize,
    pub skipped_blocked: usize,
    pub skipped_moved: usize,
    pub bytes_copied: u64,
    pub results: Vec<TransferResult>,
}

/// Copies every `Missing` and `Partial` file from `plan.source_root` to
/// `plan.destination_root`, and `Moved` files if `options.on_moved` says to.
/// `Verified`, `Conflict`, `Blocked`, and `DestinationOnly` entries are never
/// touched. A single file's failure does not stop the rest of the run.
pub fn run(plan: &ReconcilePlan, options: &RunOptions) -> Result<TransferSummary> {
    discard_stale_part_files(&plan.destination_root)?;

    let mut summary = TransferSummary::default();

    for entry in &plan.entries {
        match &entry.state {
            ReconcileState::Verified => summary.skipped_verified += 1,
            ReconcileState::Conflict => summary.skipped_conflict += 1,
            ReconcileState::Blocked { .. } => summary.skipped_blocked += 1,
            ReconcileState::DestinationOnly => {}
            ReconcileState::Moved { .. } => match options.on_moved {
                MovedPolicy::Leave => summary.skipped_moved += 1,
                MovedPolicy::Copy => copy_entry(entry, &plan.destination_root, &mut summary),
            },
            ReconcileState::Missing | ReconcileState::Partial { .. } => {
                copy_entry(entry, &plan.destination_root, &mut summary);
            }
        }
    }

    Ok(summary)
}

fn copy_entry(entry: &PlanEntry, destination_root: &Path, summary: &mut TransferSummary) {
    let Some(source_path) = &entry.source_path else {
        return;
    };
    let destination_path = destination_root.join(&entry.relative_path);

    let result = match copy_file(source_path, &destination_path) {
        Ok(copied) => {
            summary.copied += 1;
            summary.bytes_copied += copied.bytes;
            TransferResult {
                relative_path: entry.relative_path.clone(),
                outcome: TransferOutcome::Copied,
                bytes: Some(copied.bytes),
                detail: None,
            }
        }
        Err(err) => {
            summary.failed += 1;
            TransferResult {
                relative_path: entry.relative_path.clone(),
                outcome: TransferOutcome::Failed,
                bytes: None,
                detail: Some(err.to_string()),
            }
        }
    };
    summary.results.push(result);
}

struct CopiedFile {
    bytes: u64,
}

/// Copy protocol (SPEC.md section 7): write to `<name>.part`, hash while
/// writing, fsync, re-read and compare against the source hash, then
/// atomically rename to the final name. `source` is only ever opened for
/// reading; no code path here writes to it.
fn copy_file(source: &Path, destination: &Path) -> Result<CopiedFile> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|source_err| EngineError::CreateDir {
            path: parent.to_path_buf(),
            source: source_err,
        })?;
    }

    let part_path = part_path_for(destination);

    let mut source_file = File::open(source).map_err(|source_err| EngineError::Open {
        path: source.to_path_buf(),
        source: source_err,
    })?;
    let mut part_file = File::create(&part_path).map_err(|source_err| EngineError::Write {
        path: part_path.clone(),
        source: source_err,
    })?;

    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; STREAM_BUFFER_BYTES];
    let mut bytes = 0u64;
    loop {
        let n = source_file
            .read(&mut buf)
            .map_err(|source_err| EngineError::Read {
                path: source.to_path_buf(),
                source: source_err,
            })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        part_file
            .write_all(&buf[..n])
            .map_err(|source_err| EngineError::Write {
                path: part_path.clone(),
                source: source_err,
            })?;
        bytes += n as u64;
    }
    let source_hash = hasher.finalize();

    part_file
        .sync_all()
        .map_err(|source_err| EngineError::Fsync {
            path: part_path.clone(),
            source: source_err,
        })?;

    // Best-effort: modified time is preserved metadata, not correctness
    // (SPEC.md section 3: timestamps never define a duplicate), so a
    // filesystem that rejects it should not fail the transfer.
    if let Ok(metadata) = source_file.metadata() {
        if let Ok(modified) = metadata.modified() {
            let times = fs::FileTimes::new().set_modified(modified);
            let _ = part_file.set_times(times);
        }
    }
    drop(part_file);
    drop(source_file);

    // Flush before verifying: re-open and re-read what actually landed on
    // disk, rather than trusting the write loop's in-memory hash alone.
    let destination_hash = hash::full_hash(&part_path)?;
    if destination_hash != source_hash {
        let _ = fs::remove_file(&part_path);
        return Err(EngineError::VerifyMismatch {
            path: destination.to_path_buf(),
        });
    }

    fs::rename(&part_path, destination).map_err(|source_err| EngineError::Rename {
        from: part_path.clone(),
        to: destination.to_path_buf(),
        source: source_err,
    })?;

    Ok(CopiedFile { bytes })
}

fn part_path_for(destination: &Path) -> PathBuf {
    let mut os = destination.as_os_str().to_os_string();
    os.push(".part");
    PathBuf::from(os)
}

/// Removes any `.part` file left over from a previous interrupted run
/// (SPEC.md section 7: "discard stray `.part` files at the start of a run").
fn discard_stale_part_files(destination_root: &Path) -> Result<()> {
    for entry in scan::scan_root(destination_root)? {
        if entry.path.extension().and_then(|ext| ext.to_str()) == Some("part") {
            fs::remove_file(&entry.path).map_err(|source| EngineError::Remove {
                path: entry.path.clone(),
                source,
            })?;
        }
    }
    Ok(())
}
