use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rayon::prelude::*;
use serde::Serialize;
use unicode_normalization::UnicodeNormalization;

use crate::cache::FingerprintCache;
use crate::error::{EngineError, Result};
use crate::hash;
use crate::policy::{self, PreflightIssue, PreflightRules};
use crate::scan::{self, ScanEntry};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum PartialReason {
    SizeMismatch {
        source_size: u64,
        destination_size: u64,
    },
}

/// A source file's status relative to the destination (SPEC.md section 3).
/// `Blocked` and `DestinationOnly` are the two states not centered on a
/// source file: the former still is (a source file that cannot be placed),
/// the latter is purely about a destination file with no source match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ReconcileState {
    Verified,
    Partial { reason: PartialReason },
    Missing,
    Moved { destination_relative: PathBuf },
    Conflict,
    Blocked { issues: Vec<PreflightIssue> },
    DestinationOnly,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanEntry {
    pub relative_path: PathBuf,
    pub source_path: Option<PathBuf>,
    pub destination_path: Option<PathBuf>,
    pub size: Option<u64>,
    pub state: ReconcileState,
}

#[derive(Debug, Default, Serialize)]
pub struct ReconcileSummary {
    pub verified: usize,
    pub partial: usize,
    pub missing: usize,
    pub moved: usize,
    pub conflict: usize,
    pub blocked: usize,
    pub destination_only: usize,
}

#[derive(Debug, Serialize)]
pub struct ReconcilePlan {
    pub source_root: PathBuf,
    pub destination_root: PathBuf,
    pub entries: Vec<PlanEntry>,
    pub summary: ReconcileSummary,
}

/// Compares `source_root` against `destination_root` and classifies every
/// file. This never copies or deletes anything (SPEC.md section 1, principle
/// 1: the source is never modified).
pub fn plan(
    source_root: &Path,
    destination_root: &Path,
    cache: &mut FingerprintCache,
    rules: &PreflightRules,
) -> Result<ReconcilePlan> {
    if roots_are_nested(source_root, destination_root) {
        return Err(EngineError::NestedRoots {
            source_root: source_root.to_path_buf(),
            destination_root: destination_root.to_path_buf(),
        });
    }

    let source_entries = relative_entries(source_root)?;
    let destination_entries = relative_entries(destination_root)?;

    let source_by_key = group_by_key(source_entries);
    let destination_by_key = group_by_key(destination_entries);

    let mut keys: HashSet<String> = HashSet::new();
    keys.extend(source_by_key.keys().cloned());
    keys.extend(destination_by_key.keys().cloned());
    let mut keys: Vec<String> = keys.into_iter().collect();
    keys.sort();

    let mut entries_out = Vec::new();
    let mut claimed_destination_keys: HashSet<String> = HashSet::new();
    let mut deferred_sources: Vec<RelativeEntry> = Vec::new();
    let mut unmatched: Vec<TaggedEntry> = Vec::new();

    for key in keys {
        let sources = source_by_key.get(&key);
        let destinations = destination_by_key.get(&key);

        let Some(source_list) = sources else {
            if let Some(destination_list) = destinations {
                for d in destination_list {
                    unmatched.push(TaggedEntry {
                        origin: Origin::Destination,
                        entry: d.clone(),
                    });
                }
            }
            continue;
        };

        if source_list.len() > 1 {
            for s in source_list {
                let with = source_list
                    .iter()
                    .filter(|other| other.absolute != s.absolute)
                    .map(|other| other.relative.clone())
                    .collect();
                entries_out.push(PlanEntry {
                    relative_path: s.relative.clone(),
                    source_path: Some(s.absolute.clone()),
                    destination_path: None,
                    size: Some(s.size),
                    state: ReconcileState::Blocked {
                        issues: vec![PreflightIssue::PathCollision { with }],
                    },
                });
            }
            if let Some(destination_list) = destinations {
                for d in destination_list {
                    unmatched.push(TaggedEntry {
                        origin: Origin::Destination,
                        entry: d.clone(),
                    });
                }
            }
            continue;
        }

        let s = &source_list[0];

        let Some(destination_list) = destinations else {
            let issues = policy::check_file(&s.relative, s.size, rules);
            if issues.is_empty() {
                deferred_sources.push(s.clone());
                unmatched.push(TaggedEntry {
                    origin: Origin::Source,
                    entry: s.clone(),
                });
            } else {
                entries_out.push(PlanEntry {
                    relative_path: s.relative.clone(),
                    source_path: Some(s.absolute.clone()),
                    destination_path: None,
                    size: Some(s.size),
                    state: ReconcileState::Blocked { issues },
                });
            }
            continue;
        };

        claimed_destination_keys.insert(key.clone());
        let d = &destination_list[0];
        for extra in destination_list.iter().skip(1) {
            unmatched.push(TaggedEntry {
                origin: Origin::Destination,
                entry: extra.clone(),
            });
        }

        if s.size != d.size {
            entries_out.push(PlanEntry {
                relative_path: s.relative.clone(),
                source_path: Some(s.absolute.clone()),
                destination_path: Some(d.absolute.clone()),
                size: Some(s.size),
                state: ReconcileState::Partial {
                    reason: PartialReason::SizeMismatch {
                        source_size: s.size,
                        destination_size: d.size,
                    },
                },
            });
            continue;
        }

        let source_hash = resolve_full_hash(s, cache)?;
        let destination_hash = resolve_full_hash(d, cache)?;
        let state = if source_hash == destination_hash {
            ReconcileState::Verified
        } else {
            ReconcileState::Conflict
        };
        entries_out.push(PlanEntry {
            relative_path: s.relative.clone(),
            source_path: Some(s.absolute.clone()),
            destination_path: Some(d.absolute.clone()),
            size: Some(s.size),
            state,
        });
    }

    let HashIndexResult {
        index,
        entry_hashes,
        cache_updates,
    } = build_hash_index(unmatched.clone(), cache)?;
    for (entry, hash) in cache_updates {
        cache.record(&entry.as_scan_entry(), hash);
    }

    for s in deferred_sources {
        let state = entry_hashes
            .get(&s.absolute)
            .and_then(|hex| index.get(hex))
            .and_then(|members| members.iter().find(|m| m.origin == Origin::Destination))
            .map(|target| {
                claimed_destination_keys.insert(target.entry.key.clone());
                ReconcileState::Moved {
                    destination_relative: target.entry.relative.clone(),
                }
            })
            .unwrap_or(ReconcileState::Missing);

        entries_out.push(PlanEntry {
            relative_path: s.relative.clone(),
            source_path: Some(s.absolute.clone()),
            destination_path: None,
            size: Some(s.size),
            state,
        });
    }

    for tagged in unmatched.iter().filter(|t| t.origin == Origin::Destination) {
        if claimed_destination_keys.contains(&tagged.entry.key) {
            continue;
        }
        entries_out.push(PlanEntry {
            relative_path: tagged.entry.relative.clone(),
            source_path: None,
            destination_path: Some(tagged.entry.absolute.clone()),
            size: Some(tagged.entry.size),
            state: ReconcileState::DestinationOnly,
        });
    }

    entries_out.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

    let mut summary = ReconcileSummary::default();
    for entry in &entries_out {
        match &entry.state {
            ReconcileState::Verified => summary.verified += 1,
            ReconcileState::Partial { .. } => summary.partial += 1,
            ReconcileState::Missing => summary.missing += 1,
            ReconcileState::Moved { .. } => summary.moved += 1,
            ReconcileState::Conflict => summary.conflict += 1,
            ReconcileState::Blocked { .. } => summary.blocked += 1,
            ReconcileState::DestinationOnly => summary.destination_only += 1,
        }
    }

    Ok(ReconcilePlan {
        source_root: source_root.to_path_buf(),
        destination_root: destination_root.to_path_buf(),
        entries: entries_out,
        summary,
    })
}

#[derive(Debug, Clone)]
struct RelativeEntry {
    relative: PathBuf,
    key: String,
    absolute: PathBuf,
    size: u64,
    modified: SystemTime,
}

impl RelativeEntry {
    fn as_scan_entry(&self) -> ScanEntry {
        ScanEntry {
            path: self.absolute.clone(),
            size: self.size,
            modified: self.modified,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    Source,
    Destination,
}

#[derive(Debug, Clone)]
struct TaggedEntry {
    origin: Origin,
    entry: RelativeEntry,
}

/// Best-effort check for one root sitting inside the other (SPEC.md section
/// 3: "guard against a destination nested inside the source, or the
/// reverse"). Uses `std::path::absolute` rather than `canonicalize`: the
/// destination commonly does not exist yet (canonicalize would fail), and on
/// Windows canonicalize prefixes an existing path with `\\?\`, which then no
/// longer compares equal to a non-canonicalized path for the same location.
/// `absolute` normalizes both consistently without touching the filesystem,
/// at the cost of not resolving symlinks.
fn roots_are_nested(a: &Path, b: &Path) -> bool {
    let a = std::path::absolute(a).unwrap_or_else(|_| a.to_path_buf());
    let b = std::path::absolute(b).unwrap_or_else(|_| b.to_path_buf());
    a == b || a.starts_with(&b) || b.starts_with(&a)
}

fn relative_entries(root: &Path) -> Result<Vec<RelativeEntry>> {
    scan::scan_root(root)?
        .into_iter()
        .map(|entry| {
            let relative = entry
                .path
                .strip_prefix(root)
                .map_err(|_| EngineError::PathNotUnderRoot {
                    root: root.to_path_buf(),
                    path: entry.path.clone(),
                })?
                .to_path_buf();
            let key = normalize_key(&relative);
            Ok(RelativeEntry {
                relative,
                key,
                absolute: entry.path,
                size: entry.size,
                modified: entry.modified,
            })
        })
        .collect()
}

fn group_by_key(entries: Vec<RelativeEntry>) -> HashMap<String, Vec<RelativeEntry>> {
    let mut map: HashMap<String, Vec<RelativeEntry>> = HashMap::new();
    for entry in entries {
        map.entry(entry.key.clone()).or_default().push(entry);
    }
    map
}

/// Builds a comparison key for a relative path: forward slashes, Unicode
/// NFC, then lowercased. Two paths a user would consider "the same name"
/// (NFC vs NFD, or differing only in case) produce the same key, so a
/// destination that cannot tell them apart is caught as a collision. The raw
/// path is always kept separately for writing (SPEC.md section 3).
fn normalize_key(relative: &Path) -> String {
    let forward = relative.to_string_lossy().replace('\\', "/");
    let nfc: String = forward.nfc().collect();
    nfc.to_lowercase()
}

fn resolve_full_hash(entry: &RelativeEntry, cache: &mut FingerprintCache) -> Result<blake3::Hash> {
    let scan_entry = entry.as_scan_entry();
    if let Some(hash) = cache.lookup(&scan_entry) {
        return Ok(hash);
    }
    let hash = hash::full_hash(&entry.absolute)?;
    cache.record(&scan_entry, hash);
    Ok(hash)
}

struct HashIndexResult {
    index: HashMap<String, Vec<TaggedEntry>>,
    entry_hashes: HashMap<PathBuf, String>,
    cache_updates: Vec<(RelativeEntry, blake3::Hash)>,
}

/// Builds a content-hash index over entries that could not be matched by
/// path, so a source-only file can be recognized as `Moved` if its content
/// already exists at a different destination path. Entries whose size (or
/// sample hash) is unique within this set cannot match anything and are
/// simply absent from the returned index.
fn build_hash_index(tagged: Vec<TaggedEntry>, cache: &FingerprintCache) -> Result<HashIndexResult> {
    let mut by_size: HashMap<u64, Vec<TaggedEntry>> = HashMap::new();
    for t in tagged {
        by_size.entry(t.entry.size).or_default().push(t);
    }
    let groups: Vec<Vec<TaggedEntry>> = by_size.into_values().filter(|g| g.len() > 1).collect();

    let hashed: Vec<Vec<(TaggedEntry, blake3::Hash, bool)>> = groups
        .into_par_iter()
        .map(|group| hash_tagged_group(group, cache))
        .collect::<Result<Vec<_>>>()?;

    let mut index: HashMap<String, Vec<TaggedEntry>> = HashMap::new();
    let mut entry_hashes: HashMap<PathBuf, String> = HashMap::new();
    let mut cache_updates = Vec::new();

    for group in hashed {
        for (tagged_entry, hash, is_new) in group {
            let hex = hash.to_hex().to_string();
            if is_new {
                cache_updates.push((tagged_entry.entry.clone(), hash));
            }
            entry_hashes.insert(tagged_entry.entry.absolute.clone(), hex.clone());
            index.entry(hex).or_default().push(tagged_entry);
        }
    }

    Ok(HashIndexResult {
        index,
        entry_hashes,
        cache_updates,
    })
}

fn hash_tagged_group(
    group: Vec<TaggedEntry>,
    cache: &FingerprintCache,
) -> Result<Vec<(TaggedEntry, blake3::Hash, bool)>> {
    enum Resolved {
        Cached(TaggedEntry, blake3::Hash),
        Sampled(TaggedEntry, blake3::Hash),
    }

    let mut pending = Vec::with_capacity(group.len());
    for tagged in group {
        let scan_entry = tagged.entry.as_scan_entry();
        if let Some(hash) = cache.lookup(&scan_entry) {
            pending.push(Resolved::Cached(tagged, hash));
        } else {
            let sample = hash::sample_hash(&tagged.entry.absolute, tagged.entry.size)?;
            pending.push(Resolved::Sampled(tagged, sample));
        }
    }

    let mut sample_counts: HashMap<blake3::Hash, usize> = HashMap::new();
    for item in &pending {
        if let Resolved::Sampled(_, sample) = item {
            *sample_counts.entry(*sample).or_default() += 1;
        }
    }

    let mut out = Vec::with_capacity(pending.len());
    for item in pending {
        match item {
            Resolved::Cached(tagged, hash) => out.push((tagged, hash, false)),
            Resolved::Sampled(tagged, sample) => {
                if sample_counts[&sample] < 2 {
                    continue;
                }
                let full = hash::full_hash(&tagged.entry.absolute)?;
                out.push((tagged, full, true));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(relative: &str) -> RelativeEntry {
        let relative = PathBuf::from(relative);
        let key = normalize_key(&relative);
        RelativeEntry {
            absolute: PathBuf::from("/tmp").join(&relative),
            relative,
            key,
            size: 0,
            modified: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn normalize_key_folds_case() {
        assert_eq!(
            normalize_key(Path::new("Report.txt")),
            normalize_key(Path::new("report.TXT"))
        );
    }

    #[test]
    fn normalize_key_folds_nfc_and_nfd() {
        let nfc = "\u{00e9}.txt"; // "é.txt", precomposed
        let nfd = "e\u{0301}.txt"; // "é.txt", e + combining acute accent
        assert_eq!(normalize_key(Path::new(nfc)), normalize_key(Path::new(nfd)));
    }

    #[test]
    fn normalize_key_treats_backslash_and_forward_slash_the_same() {
        assert_eq!(
            normalize_key(Path::new("a/b.txt")),
            normalize_key(&PathBuf::from("a\\b.txt"))
        );
    }

    #[test]
    fn normalize_key_distinguishes_different_names() {
        assert_ne!(
            normalize_key(Path::new("a.txt")),
            normalize_key(Path::new("b.txt"))
        );
    }

    #[test]
    fn roots_are_nested_detects_a_destination_inside_the_source_even_when_it_does_not_exist_yet() {
        let base = std::env::temp_dir().join(format!("migrator-nest-test-{}", std::process::id()));
        let source = base.join("source");
        let destination = source.join("nested_dest");
        assert!(roots_are_nested(&source, &destination));
    }

    #[test]
    fn roots_are_nested_is_false_for_sibling_roots() {
        let base = std::env::temp_dir().join(format!("migrator-nest-test-{}", std::process::id()));
        let source = base.join("source");
        let destination = base.join("destination");
        assert!(!roots_are_nested(&source, &destination));
    }

    #[test]
    fn group_by_key_collects_names_that_collide_after_normalization() {
        let entries = vec![entry("Report.txt"), entry("report.txt"), entry("other.txt")];
        let grouped = group_by_key(entries);

        let collision_key = normalize_key(Path::new("report.txt"));
        assert_eq!(grouped[&collision_key].len(), 2);
        assert_eq!(grouped[&normalize_key(Path::new("other.txt"))].len(), 1);
    }
}
