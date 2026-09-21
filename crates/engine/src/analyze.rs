use std::collections::HashMap;
use std::path::PathBuf;

use rayon::prelude::*;
use serde::Serialize;

use crate::cache::FingerprintCache;
use crate::error::Result;
use crate::hash;
use crate::scan::{self, ScanEntry};

#[derive(Debug, Clone, Serialize)]
pub struct DuplicateGroup {
    pub hash: String,
    pub size: u64,
    pub files: Vec<PathBuf>,
}

#[derive(Debug, Default, Serialize)]
pub struct AnalyzeReport {
    pub roots: Vec<PathBuf>,
    pub files_scanned: usize,
    pub bytes_scanned: u64,
    pub duplicate_groups: Vec<DuplicateGroup>,
    pub duplicate_files: usize,
    pub reclaimable_bytes: u64,
}

/// Scans `roots`, groups files by size, then confirms exact duplicates with
/// BLAKE3 (T1 in SPEC.md section 4). Nothing is copied or changed; this is
/// analysis only.
pub fn analyze(roots: &[PathBuf], cache: &mut FingerprintCache) -> Result<AnalyzeReport> {
    let entries = scan::scan_roots(roots)?;
    let files_scanned = entries.len();
    let bytes_scanned = entries.iter().map(|e| e.size).sum();

    let mut by_size: HashMap<u64, Vec<ScanEntry>> = HashMap::new();
    for entry in entries {
        by_size.entry(entry.size).or_default().push(entry);
    }
    // Files with a unique size cannot be duplicates of anything else scanned.
    let size_groups: Vec<Vec<ScanEntry>> = by_size.into_values().filter(|g| g.len() > 1).collect();

    let hashed_groups: Vec<Vec<(ScanEntry, blake3::Hash, bool)>> = size_groups
        .into_par_iter()
        .map(|group| hash_size_group(&group, cache))
        .collect::<Result<Vec<_>>>()?;

    for group in &hashed_groups {
        for (entry, hash, is_new) in group {
            if *is_new {
                cache.record(entry, *hash);
            }
        }
    }

    let mut by_hash: HashMap<String, (u64, Vec<PathBuf>)> = HashMap::new();
    for group in &hashed_groups {
        for (entry, hash, _) in group {
            let key = hash.to_hex().to_string();
            let bucket = by_hash
                .entry(key)
                .or_insert_with(|| (entry.size, Vec::new()));
            bucket.1.push(entry.path.clone());
        }
    }

    let mut duplicate_groups: Vec<DuplicateGroup> = by_hash
        .into_iter()
        .filter(|(_, (_, files))| files.len() > 1)
        .map(|(hash, (size, mut files))| {
            files.sort();
            DuplicateGroup { hash, size, files }
        })
        .collect();
    duplicate_groups.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.hash.cmp(&b.hash)));

    let duplicate_files: usize = duplicate_groups.iter().map(|g| g.files.len()).sum();
    let reclaimable_bytes: u64 = duplicate_groups
        .iter()
        .map(|g| g.size * (g.files.len() as u64 - 1))
        .sum();

    Ok(AnalyzeReport {
        roots: roots.to_vec(),
        files_scanned,
        bytes_scanned,
        duplicate_groups,
        duplicate_files,
        reclaimable_bytes,
    })
}

/// Resolves the confirmed hash for every entry in a same-size group, using
/// the cache where possible and otherwise the sample-then-full BLAKE3
/// pipeline. Returns `(entry, hash, is_newly_computed)` for members that
/// survive the sample-hash pre-filter; entries proven unique by sampling
/// alone are dropped, since they cannot be duplicates.
fn hash_size_group(
    group: &[ScanEntry],
    cache: &FingerprintCache,
) -> Result<Vec<(ScanEntry, blake3::Hash, bool)>> {
    enum Resolved {
        Cached(ScanEntry, blake3::Hash),
        Sampled(ScanEntry, blake3::Hash),
    }

    let mut pending = Vec::with_capacity(group.len());
    for entry in group {
        if let Some(cached_hash) = cache.lookup(entry) {
            pending.push(Resolved::Cached(entry.clone(), cached_hash));
        } else {
            let sample = hash::sample_hash(&entry.path, entry.size)?;
            pending.push(Resolved::Sampled(entry.clone(), sample));
        }
    }

    let mut sample_counts: HashMap<blake3::Hash, usize> = HashMap::new();
    for item in &pending {
        if let Resolved::Sampled(_, sample) = item {
            *sample_counts.entry(*sample).or_default() += 1;
        }
    }

    let mut out = Vec::with_capacity(group.len());
    for item in pending {
        match item {
            Resolved::Cached(entry, hash) => out.push((entry, hash, false)),
            Resolved::Sampled(entry, sample) => {
                if sample_counts[&sample] < 2 {
                    // No other file in this size group shares its sample
                    // hash, so it cannot be a duplicate; skip the full hash.
                    continue;
                }
                let full = hash::full_hash(&entry.path)?;
                out.push((entry, full, true));
            }
        }
    }
    Ok(out)
}
