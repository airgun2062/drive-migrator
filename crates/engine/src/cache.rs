use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{EngineError, Result};
use crate::scan::ScanEntry;

/// Filesystem mtimes are compared with this tolerance because FAT and exFAT
/// store local time at coarse precision (SPEC.md section 3).
const MTIME_TOLERANCE_NANOS: i128 = 2_000_000_000;

/// Persisted (path, size, mtime) -> BLAKE3 hash lookup, so unchanged files are
/// not re-hashed on the next run. Never trusted on its own: every entry is
/// checked against the file's current size and mtime before use.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct FingerprintCache {
    entries: HashMap<String, CacheEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    size: u64,
    modified_unix_nanos: i128,
    hash: String,
}

impl FingerprintCache {
    /// Loads the cache from `path`. A missing file is treated as an empty
    /// cache, not an error.
    pub fn load(path: &Path) -> Result<Self> {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|source| EngineError::CacheParse {
                path: path.to_path_buf(),
                source,
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(EngineError::CacheRead {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|source| EngineError::CacheWrite {
                    path: path.to_path_buf(),
                    source,
                })?;
            }
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(EngineError::CacheSerialize)?;
        fs::write(path, bytes).map_err(|source| EngineError::CacheWrite {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Returns the cached hash for `entry` if its size and mtime (within
    /// tolerance) still match what was recorded.
    pub fn lookup(&self, entry: &ScanEntry) -> Option<blake3::Hash> {
        let key = cache_key(&entry.path)?;
        let cached = self.entries.get(&key)?;
        if cached.size != entry.size {
            return None;
        }
        let modified_nanos = unix_nanos(entry.modified)?;
        if (cached.modified_unix_nanos - modified_nanos).abs() > MTIME_TOLERANCE_NANOS {
            return None;
        }
        blake3::Hash::from_hex(&cached.hash).ok()
    }

    /// Records a freshly computed hash for `entry`. Silently skipped for
    /// paths that are not valid UTF-8 or timestamps outside representable
    /// range, since the cache is a pure optimization.
    pub fn record(&mut self, entry: &ScanEntry, hash: blake3::Hash) {
        let Some(key) = cache_key(&entry.path) else {
            return;
        };
        let Some(modified_unix_nanos) = unix_nanos(entry.modified) else {
            return;
        };
        self.entries.insert(
            key,
            CacheEntry {
                size: entry.size,
                modified_unix_nanos,
                hash: hash.to_hex().to_string(),
            },
        );
    }
}

/// Uses the path's lossy UTF-8 form as the cache key. If the conversion is
/// lossy (the path is not valid UTF-8), caching is skipped for that entry
/// rather than risk two different paths colliding on the same key.
fn cache_key(path: &Path) -> Option<String> {
    let lossy = path.to_string_lossy();
    if lossy.contains('\u{FFFD}') {
        return None;
    }
    Some(lossy.into_owned())
}

fn unix_nanos(time: SystemTime) -> Option<i128> {
    match time.duration_since(UNIX_EPOCH) {
        Ok(d) => Some(d.as_nanos() as i128),
        Err(err) => Some(-(err.duration().as_nanos() as i128)),
    }
}
