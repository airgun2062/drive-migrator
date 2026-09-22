//! Groups the pairwise relationships `similarity::find_similar` reports
//! into version families (SPEC.md section 5): a graph, not a chain, since
//! "X3 and X4 may both derive from X2." Ranks members within a family using
//! the same priority cascade as `metadata::best_available_date`, plus
//! coverage as the primary structural signal.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::SystemTime;

use serde::Serialize;

use crate::metadata::{best_available_date, DateSource};
use crate::similarity::{Relationship, SimilarPair};
use crate::union_find::UnionFind;

#[derive(Debug, Clone, Serialize)]
pub struct RankedMember {
    pub path: PathBuf,
    /// 1 = most complete/newest in the family.
    pub rank: usize,
    pub date_unix_millis: i64,
    pub date_source: DateSource,
    /// A tip: no other member of the family covers this one, so it must be
    /// kept regardless of keep-newest-N (SPEC.md section 2: "Diverged tips
    /// ... are always kept, whatever N is" - generalized here to any
    /// member nothing else in the family covers, not only ones reached via
    /// a Diverged edge, since those never join a family at all).
    pub is_tip: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct VersionFamily {
    /// Sorted by rank, most complete/newest first.
    pub members: Vec<RankedMember>,
}

#[derive(Debug, Default, Serialize)]
pub struct VersionReport {
    pub families: Vec<VersionFamily>,
    /// Pairs `similarity::find_similar` classified `Diverged`: related
    /// enough to be candidates, but each has unique content. Never merged
    /// into a family (SPEC.md section 5: "flag for manual merge and never
    /// auto-pick").
    pub flagged_for_manual_merge: Vec<SimilarPair>,
}

/// Builds version families from `similar`'s pairwise output. Families with
/// only one member (nothing else related to them) are dropped - there is
/// nothing to rank or collapse.
pub fn build_version_report(pairs: &[SimilarPair]) -> VersionReport {
    let mut path_index: HashMap<PathBuf, usize> = HashMap::new();
    let mut paths: Vec<PathBuf> = Vec::new();
    for pair in pairs {
        for p in [&pair.a, &pair.b] {
            if !path_index.contains_key(p) {
                path_index.insert(p.clone(), paths.len());
                paths.push(p.clone());
            }
        }
    }

    let mut uf = UnionFind::new(paths.len());
    let mut flagged = Vec::new();
    // covers[i] = the set of members `i` is at least as complete as,
    // directly from a BIsMoreComplete/AIsMoreComplete/NearDuplicate edge.
    let mut covers: Vec<HashSet<usize>> = vec![HashSet::new(); paths.len()];

    for pair in pairs {
        let ia = path_index[&pair.a];
        let ib = path_index[&pair.b];
        match pair.relationship {
            Relationship::Diverged => {
                flagged.push(pair.clone());
                continue;
            }
            Relationship::NearDuplicate => {
                uf.union(ia, ib);
                covers[ia].insert(ib);
                covers[ib].insert(ia);
            }
            Relationship::BIsMoreComplete => {
                uf.union(ia, ib);
                covers[ib].insert(ia);
            }
            Relationship::AIsMoreComplete => {
                uf.union(ia, ib);
                covers[ia].insert(ib);
            }
        }
    }

    let mut families: Vec<VersionFamily> = uf
        .groups()
        .into_iter()
        .filter(|g| g.len() > 1)
        .map(|member_indices| rank_family(&member_indices, &paths, &covers))
        .collect();
    families.sort_by_key(|f| std::cmp::Reverse(f.members.len()));

    VersionReport {
        families,
        flagged_for_manual_merge: flagged,
    }
}

fn rank_family(
    member_indices: &[usize],
    paths: &[PathBuf],
    covers: &[HashSet<usize>],
) -> VersionFamily {
    let n = member_indices.len();

    // local_covers[a][b] = member_indices[a] covers member_indices[b],
    // indexed by position within this family rather than the global path
    // index. Built from `similar`'s direct pairwise edges only so far.
    let mut local_covers = vec![vec![false; n]; n];
    for (a, &i) in member_indices.iter().enumerate() {
        for (b, &j) in member_indices.iter().enumerate() {
            if a != b && covers[i].contains(&j) {
                local_covers[a][b] = true;
            }
        }
    }

    // Transitive closure (Warshall's algorithm - family sizes are small, so
    // O(n^3) is cheap): if `similar` only compared adjacent generations in
    // a chain (A covers B covers C) rather than every pair, A should still
    // rank above C, not tie with B.
    for k in 0..n {
        let covers_k = local_covers[k].clone();
        for row in &mut local_covers {
            if row[k] {
                for (j, &via_k) in covers_k.iter().enumerate() {
                    if via_k {
                        row[j] = true;
                    }
                }
            }
        }
    }

    let mut scored: Vec<(usize, usize, i64, DateSource)> = (0..n)
        .map(|a| {
            let i = member_indices[a];
            let completeness = local_covers[a].iter().filter(|&&covered| covered).count();
            let modified = std::fs::metadata(&paths[i])
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let dated = best_available_date(&paths[i], modified);
            (a, completeness, dated.unix_millis, dated.source)
        })
        .collect();

    // Higher completeness first (covers more of the family); ties broken by
    // a more recent date (SPEC.md section 5: "both high, so the timestamp
    // decides"); final tiebreak on path for determinism.
    scored.sort_by(|x, y| {
        y.1.cmp(&x.1)
            .then_with(|| y.2.cmp(&x.2))
            .then_with(|| paths[member_indices[x.0]].cmp(&paths[member_indices[y.0]]))
    });

    let is_tip = |a: usize| -> bool { !(0..n).any(|other| other != a && local_covers[other][a]) };

    let members = scored
        .into_iter()
        .enumerate()
        .map(
            |(rank0, (a, _, date_unix_millis, date_source))| RankedMember {
                path: paths[member_indices[a]].clone(),
                rank: rank0 + 1,
                date_unix_millis,
                date_source,
                is_tip: is_tip(a),
            },
        )
        .collect();

    VersionFamily { members }
}
