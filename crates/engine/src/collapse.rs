//! Decides, per file, whether to keep, collapse, or archive it (SPEC.md
//! section 2, Collapse mode). Combines three signals that stay independent
//! everywhere else in the engine:
//!
//! - Exact duplicates (`analyze`, BLAKE3): "not versions" - always collapse
//!   to one kept copy, unconditionally, regardless of N or family status.
//! - Version families (`versions`): "keep newest N", tips always kept.
//! - Protected extensions (SPEC.md section 6): never collapsed as a
//!   version, only ever as an exact duplicate.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::analyze;
use crate::cache::FingerprintCache;
use crate::error::Result;
use crate::reconcile::{ReconcilePlan, ReconcileState};
use crate::scan;
use crate::similarity::{self, SimilarityConfig};
use crate::versions::{self, VersionReport};

const COLLAPSED_ARCHIVE_DIR: &str = "_collapsed";

#[derive(Debug, Clone)]
pub struct CollapseConfig {
    /// N >= 1: how many of the newest/most-complete versions per family to
    /// keep.
    pub keep_newest: usize,
    /// Copy collapsed versions to `_collapsed/<relative path>` instead of
    /// dropping them.
    pub archive: bool,
    /// Lowercase extensions (no leading dot) that are never collapsed as
    /// versions, only as exact duplicates (SPEC.md section 6: raw research
    /// data and similar protected classes).
    pub protected_extensions: HashSet<String>,
    pub similarity: SimilarityConfig,
}

impl Default for CollapseConfig {
    fn default() -> Self {
        Self {
            keep_newest: 1,
            archive: false,
            protected_extensions: HashSet::new(),
            similarity: SimilarityConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CollapseAction {
    Keep,
    /// Not copied to the destination at its own relative path; remains
    /// only on the source. Recorded in the manifest.
    Collapse,
    /// Collapsed, but copied to `_collapsed/<relative path>` instead of
    /// being dropped.
    Archive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CollapseReason {
    /// Not part of any duplicate group or version family, or the
    /// representative kept copy of an exact-duplicate group.
    Kept,
    /// A version family tip: nothing else in the family covers it, so it
    /// must be kept regardless of keep-newest-N.
    Tip,
    /// A version family member ranked within keep-newest-N.
    WithinKeepNewest,
    /// A protected extension: never collapsed as a version.
    Protected,
    /// One of several byte-identical copies; not the representative kept
    /// one.
    ExactDuplicate,
    /// A version family member beyond keep-newest-N, covered by a kept
    /// version.
    OlderVersion,
}

#[derive(Debug, Clone, Serialize)]
pub struct CollapseDecision {
    pub path: PathBuf,
    pub action: CollapseAction,
    pub reason: CollapseReason,
    pub size: u64,
}

#[derive(Debug, Default, Serialize)]
pub struct CollapseReport {
    pub decisions: Vec<CollapseDecision>,
    pub version_report: VersionReport,
    pub files_kept: usize,
    pub files_collapsed: usize,
    pub bytes_saved: u64,
}

/// Scans `roots` and decides what to keep, collapse, or archive. This is
/// analysis only - nothing is copied or deleted; `bytes_saved` is exactly
/// the "dry run shows the effect before anything happens" figure from
/// SPEC.md section 2.
pub fn plan_collapse(
    roots: &[PathBuf],
    cache: &mut FingerprintCache,
    config: &CollapseConfig,
) -> Result<CollapseReport> {
    let analyze_report = analyze::analyze(roots, cache)?;
    let similarity_report = similarity::find_similar(roots, &config.similarity)?;
    let version_report = versions::build_version_report(&similarity_report.pairs);

    let mut decisions: Vec<CollapseDecision> = Vec::new();
    let mut decided: HashSet<PathBuf> = HashSet::new();

    // Exact duplicates: keep exactly one copy (first by path, for
    // determinism); collapse the rest. Unconditional - not versions, so
    // family/protected-class status never applies here.
    for group in &analyze_report.duplicate_groups {
        let mut files = group.files.clone();
        files.sort();
        for (i, path) in files.into_iter().enumerate() {
            let (action, reason) = if i == 0 {
                (CollapseAction::Keep, CollapseReason::Kept)
            } else if config.archive {
                (CollapseAction::Archive, CollapseReason::ExactDuplicate)
            } else {
                (CollapseAction::Collapse, CollapseReason::ExactDuplicate)
            };
            decided.insert(path.clone());
            decisions.push(CollapseDecision {
                path,
                action,
                reason,
                size: group.size,
            });
        }
    }

    // Version families: keep-newest-N, tips always kept, protected
    // extensions never collapsed as a version.
    for family in &version_report.families {
        for (rank_index, member) in family.members.iter().enumerate() {
            if decided.contains(&member.path) {
                continue; // already decided as an exact duplicate
            }
            let protected = is_protected(&member.path, &config.protected_extensions);
            let size = std::fs::metadata(&member.path)
                .map(|m| m.len())
                .unwrap_or(0);

            let (action, reason) = if protected {
                (CollapseAction::Keep, CollapseReason::Protected)
            } else if member.is_tip {
                (CollapseAction::Keep, CollapseReason::Tip)
            } else if rank_index < config.keep_newest {
                (CollapseAction::Keep, CollapseReason::WithinKeepNewest)
            } else if config.archive {
                (CollapseAction::Archive, CollapseReason::OlderVersion)
            } else {
                (CollapseAction::Collapse, CollapseReason::OlderVersion)
            };

            decided.insert(member.path.clone());
            decisions.push(CollapseDecision {
                path: member.path.clone(),
                action,
                reason,
                size,
            });
        }
    }

    // Everything else scanned (not an exact duplicate, not in any family)
    // is always kept.
    for entry in scan::scan_roots(roots)? {
        if decided.insert(entry.path.clone()) {
            decisions.push(CollapseDecision {
                path: entry.path,
                action: CollapseAction::Keep,
                reason: CollapseReason::Kept,
                size: entry.size,
            });
        }
    }

    let files_kept = decisions
        .iter()
        .filter(|d| d.action == CollapseAction::Keep)
        .count();
    let files_collapsed = decisions.len() - files_kept;
    let bytes_saved = decisions
        .iter()
        .filter(|d| d.action != CollapseAction::Keep)
        .map(|d| d.size)
        .sum();

    Ok(CollapseReport {
        decisions,
        version_report,
        files_kept,
        files_collapsed,
        bytes_saved,
    })
}

fn is_protected(path: &Path, protected_extensions: &HashSet<String>) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| protected_extensions.contains(&e.to_ascii_lowercase()))
        .unwrap_or(false)
}

/// Restricts a reconcile plan to what Collapse mode should actually copy:
/// entries whose source is collapsed are dropped entirely (never copied,
/// remaining only on the source); entries whose source is archived are
/// redirected to `_collapsed/<relative path>` instead of their normal
/// destination. Only `Missing`/`Partial` entries are affected - files
/// already `Verified` at the destination from an earlier run are left
/// alone even if a fresh collapse decision would now drop them, since
/// deleting previously-copied destination files is a separate, destructive
/// operation this does not perform.
pub fn apply_collapse_to_plan(plan: &mut ReconcilePlan, collapse: &CollapseReport) {
    let decisions: HashMap<&Path, CollapseAction> = collapse
        .decisions
        .iter()
        .map(|d| (d.path.as_path(), d.action))
        .collect();

    plan.entries.retain_mut(|entry| {
        if !matches!(
            entry.state,
            ReconcileState::Missing | ReconcileState::Partial { .. }
        ) {
            return true;
        }
        let Some(source_path) = &entry.source_path else {
            return true;
        };
        match decisions.get(source_path.as_path()) {
            Some(CollapseAction::Collapse) => false,
            Some(CollapseAction::Archive) => {
                entry.relative_path = Path::new(COLLAPSED_ARCHIVE_DIR).join(&entry.relative_path);
                true
            }
            Some(CollapseAction::Keep) | None => true,
        }
    });
}
