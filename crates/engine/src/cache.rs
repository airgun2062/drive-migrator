use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::applog::Log;
use crate::error::{EngineError, Result};
use crate::scan::ScanEntry;

/// Filesystem mtimes are compared with this tolerance because FAT and exFAT
/// store local time at coarse precision (SPEC.md section 3).
const MTIME_TOLERANCE_NANOS: i128 = 2_000_000_000;

/// Persisted (path, size, mtime) -> hash lookup, so unchanged files are not
/// re-hashed on the next run. Never trusted on its own: every entry is
/// checked against the file's current size and mtime before use.
///
/// Stores both the cheap sample hash (first/last `SAMPLE_CHUNK_BYTES`) and
/// the full BLAKE3 hash independently, since a file can be resolved at
/// either stage: one proven unique by its sample alone never needs a full
/// hash at all, but without caching the sample too, a rescan would have to
/// recompute it from scratch every time even though nothing about the file
/// changed - only the (usually far more expensive) full hash was ever
/// worth persisting before this.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct FingerprintCache {
    entries: HashMap<String, CacheEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CacheEntry {
    size: u64,
    modified_unix_nanos: i128,
    #[serde(default)]
    sample_hash: Option<String>,
    #[serde(default)]
    full_hash: Option<String>,
}

impl FingerprintCache {
    /// Loads the cache from `path`. Never fails the caller: a missing file
    /// starts empty, and a file that can't be read or parsed (corrupted, an
    /// older/incompatible schema, ...) also starts empty rather than
    /// aborting - the cache is a pure optimization, so losing it costs a
    /// slower run, not a broken one. Every case is logged, since silently
    /// discarding a cache that failed to load would otherwise look like
    /// "the cache just isn't helping" with no visible reason why.
    pub fn load(path: &Path) -> Self {
        match fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<Self>(&bytes) {
                Ok(cache) => {
                    Log::info(
                        "cache",
                        &format!(
                            "loaded {} - {} entries",
                            path.display(),
                            cache.entries.len()
                        ),
                    );
                    cache
                }
                Err(err) => {
                    Log::error(
                        "cache",
                        &format!(
                            "failed to parse cache {} - starting fresh: {err}",
                            path.display()
                        ),
                    );
                    Self::default()
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                Log::info(
                    "cache",
                    &format!("no cache at {} yet - starting empty", path.display()),
                );
                Self::default()
            }
            Err(err) => {
                Log::error(
                    "cache",
                    &format!(
                        "failed to read cache {} - starting fresh: {err}",
                        path.display()
                    ),
                );
                Self::default()
            }
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
        fs::write(path, bytes).map_err(|source| {
            Log::error(
                "cache",
                &format!("failed to write cache {}: {source}", path.display()),
            );
            EngineError::CacheWrite {
                path: path.to_path_buf(),
                source,
            }
        })?;
        Log::info(
            "cache",
            &format!("saved {} - {} entries", path.display(), self.entries.len()),
        );
        Ok(())
    }

    /// Returns the cached full hash for `entry` if its size and mtime
    /// (within tolerance) still match what was recorded.
    pub fn lookup(&self, entry: &ScanEntry) -> Option<blake3::Hash> {
        self.matching_entry(entry)?
            .full_hash
            .as_deref()
            .and_then(|h| blake3::Hash::from_hex(h).ok())
    }

    /// Returns the cached sample hash for `entry` if its size and mtime
    /// (within tolerance) still match what was recorded - lets a rescan
    /// skip re-sampling a file that was already resolved (either proven
    /// unique by its sample alone, or carried through to a full hash) last
    /// time, not only files that made it all the way to a full hash.
    pub fn lookup_sample(&self, entry: &ScanEntry) -> Option<blake3::Hash> {
        self.matching_entry(entry)?
            .sample_hash
            .as_deref()
            .and_then(|h| blake3::Hash::from_hex(h).ok())
    }

    fn matching_entry(&self, entry: &ScanEntry) -> Option<&CacheEntry> {
        let key = cache_key(&entry.path)?;
        let cached = self.entries.get(&key)?;
        if cached.size != entry.size {
            return None;
        }
        let modified_nanos = unix_nanos(entry.modified)?;
        if (cached.modified_unix_nanos - modified_nanos).abs() > MTIME_TOLERANCE_NANOS {
            return None;
        }
        Some(cached)
    }

    /// Records a freshly computed full hash for `entry`, preserving its
    /// sample hash if the existing entry (if any) still matches this
    /// file's current size/mtime. Silently skipped for paths that are not
    /// valid UTF-8 or timestamps outside representable range, since the
    /// cache is a pure optimization.
    pub fn record(&mut self, entry: &ScanEntry, hash: blake3::Hash) {
        self.upsert(entry, |cached| {
            cached.full_hash = Some(hash.to_hex().to_string())
        });
    }

    /// Records a freshly computed sample hash for `entry`, preserving any
    /// already-cached full hash the same way `record` preserves the sample.
    pub fn record_sample(&mut self, entry: &ScanEntry, hash: blake3::Hash) {
        self.upsert(entry, |cached| {
            cached.sample_hash = Some(hash.to_hex().to_string())
        });
    }

    fn upsert(&mut self, entry: &ScanEntry, apply: impl FnOnce(&mut CacheEntry)) {
        let Some(key) = cache_key(&entry.path) else {
            return;
        };
        let Some(modified_unix_nanos) = unix_nanos(entry.modified) else {
            return;
        };

        let mut cached = match self.entries.get(&key) {
            // Same file (by size/mtime) already has an entry - update it in
            // place so its other hash field survives.
            Some(existing)
                if existing.size == entry.size
                    && existing.modified_unix_nanos == modified_unix_nanos =>
            {
                existing.clone()
            }
            // New file, or the existing entry is for a since-changed
            // version of it - start a fresh entry rather than carrying
            // over a hash that no longer describes this content.
            _ => CacheEntry {
                size: entry.size,
                modified_unix_nanos,
                sample_hash: None,
                full_hash: None,
            },
        };
        apply(&mut cached);
        self.entries.insert(key, cached);
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
