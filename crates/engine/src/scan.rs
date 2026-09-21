use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::error::{EngineError, Result};

/// One file found under a scanned root. Directories are not represented.
#[derive(Debug, Clone)]
pub struct ScanEntry {
    pub path: PathBuf,
    pub size: u64,
    pub modified: SystemTime,
}

/// Walks `root` and returns every regular file found under it. Symlinks are
/// not followed, so a link cycle cannot cause an infinite walk.
///
/// A root that does not exist yet scans as empty rather than erroring: a
/// destination root not yet created is the normal case for a first-ever
/// reconcile plan (SPEC.md section 3).
pub fn scan_root(root: &Path) -> Result<Vec<ScanEntry>> {
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();

    for result in walkdir::WalkDir::new(root).follow_links(false) {
        let dir_entry = result.map_err(|err| {
            let path = err
                .path()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| root.to_path_buf());
            let source = err
                .into_io_error()
                .unwrap_or_else(|| io_other("directory walk failed"));
            EngineError::Walk { path, source }
        })?;

        if !dir_entry.file_type().is_file() {
            continue;
        }

        let metadata = dir_entry.metadata().map_err(|err| {
            let source = err
                .into_io_error()
                .unwrap_or_else(|| io_other("failed to stat entry"));
            EngineError::Metadata {
                path: dir_entry.path().to_path_buf(),
                source,
            }
        })?;

        let size = metadata.len();
        let modified = metadata
            .modified()
            .map_err(|source| EngineError::Metadata {
                path: dir_entry.path().to_path_buf(),
                source,
            })?;

        entries.push(ScanEntry {
            path: dir_entry.into_path(),
            size,
            modified,
        });
    }

    Ok(entries)
}

/// Scans every root and returns the combined file list.
pub fn scan_roots(roots: &[PathBuf]) -> Result<Vec<ScanEntry>> {
    let mut all = Vec::new();
    for root in roots {
        all.extend(scan_root(root)?);
    }
    Ok(all)
}

fn io_other(message: &str) -> std::io::Error {
    std::io::Error::other(message.to_string())
}
