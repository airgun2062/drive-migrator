use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::Serialize;

use crate::analyze::{self, DuplicateGroup};
use crate::applog::Log;
use crate::cache::FingerprintCache;
use crate::error::Result;
use crate::extract::{self, ExtractedDocument};
use crate::scan;
use crate::union_find::UnionFind;

/// A prime close to 2^61, comfortably inside u64 and large enough that
/// collisions between distinct 64-bit token hashes are negligible.
const MERSENNE_PRIME_61: u64 = (1u64 << 61) - 1;

#[derive(Debug, Clone)]
pub struct SimilarityConfig {
    pub num_hashes: usize,
    pub bands: usize,
    /// Minimum estimated Jaccard similarity for a pair to be reported at
    /// all.
    pub duplicate_cutoff: f64,
    /// Coverage threshold used to call a direction "high" when classifying
    /// a pair's relationship (SPEC.md section 5).
    pub high_coverage: f64,
    pub seed: u64,
    /// When set, only files whose lowercase extension (no leading dot) is in
    /// this set are considered at all - every other file is excluded before
    /// classification/extraction, the same as an unsupported format. `None`
    /// (the default) considers every extension `extract::classify` supports.
    pub extensions: Option<HashSet<String>>,
}

impl Default for SimilarityConfig {
    /// These defaults are not calibrated against labeled data (SPEC.md
    /// section 4 calls for ~200 hand-labeled pairs to choose real
    /// thresholds) - they are reasonable starting points only.
    fn default() -> Self {
        Self {
            num_hashes: 128,
            bands: 32,
            duplicate_cutoff: 0.5,
            high_coverage: 0.8,
            seed: 0x5EED,
            extensions: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Relationship {
    /// Both directions cover well: effectively the same content.
    NearDuplicate,
    /// Neither direction covers well: each has unique content (SPEC.md
    /// section 5: "flag for manual merge and never auto-pick").
    Diverged,
    /// `a`'s content is (almost) entirely contained in `b`, which also has
    /// more.
    BIsMoreComplete,
    /// The reverse of `BIsMoreComplete`.
    AIsMoreComplete,
}

#[derive(Debug, Clone, Serialize)]
pub struct SimilarPair {
    pub a: PathBuf,
    pub b: PathBuf,
    pub jaccard_estimate: f64,
    pub coverage_a_to_b: f64,
    pub coverage_b_to_a: f64,
    pub relationship: Relationship,
    /// Set-based (Sorensen-Dice) overlap of embedded media between `a` and
    /// `b` - `None` when neither has any (including every format without
    /// the concept of embedded media). Purely informational: `relationship`
    /// above is still classified from text alone (SPEC.md section 5); this
    /// is surfaced alongside it so a human choosing between near-duplicate
    /// versions can also see how their embedded media compares.
    pub media_match_score: Option<f64>,
}

#[derive(Debug, Default, Serialize)]
pub struct SimilarityReport {
    pub files_considered: usize,
    pub files_skipped_unsupported: usize,
    /// Files that were part of a confirmed T1 exact-duplicate group (SPEC.md
    /// section 4: "each tier sees only what survives the previous one") and
    /// so were excluded here rather than re-discovered as a misleading
    /// "near duplicate" with a perfect 1.0 score - one representative per
    /// group still goes through comparison normally.
    pub files_excluded_exact_duplicates: usize,
    /// Files excluded because `SimilarityConfig::extensions` was set and
    /// this file's extension wasn't in it. Zero whenever no filter is
    /// configured.
    pub files_excluded_by_extension_filter: usize,
    pub pairs: Vec<SimilarPair>,
}

/// Finds near-duplicate and version-like relationships among the supported
/// text-like formats under `roots`. Uses MinHash + LSH for candidate
/// generation (SPEC.md section 4, T3) so it never compares every pair, then
/// verifies each candidate pairwise (never assumes a whole LSH group is
/// mutually similar, since that could transitively merge unrelated files).
///
/// Runs its own T1 exact-duplicate pass first, with a fresh, unpersisted
/// cache, so files already confirmed byte-identical to each other never
/// reach T3 except as one representative (see
/// `find_similar_excluding_exact_duplicates`). Callers that already have an
/// `AnalyzeReport` from calling `analyze` themselves (`collapse::
/// plan_collapse`, for instance) should call
/// `find_similar_excluding_exact_duplicates` directly instead, to reuse
/// that result and their own persisted cache rather than paying for a
/// second, redundant exact-hash pass here.
pub fn find_similar(roots: &[PathBuf], config: &SimilarityConfig) -> Result<SimilarityReport> {
    let mut cache = FingerprintCache::default();
    let analyze_report = analyze::analyze(roots, &mut cache)?;
    find_similar_excluding_exact_duplicates(roots, config, &analyze_report.duplicate_groups)
}

/// Same as `find_similar`, but takes already-known exact-duplicate groups
/// (from `analyze`) instead of computing them again, and excludes every
/// member of each group except one representative (first by path, for
/// determinism - the same tie-break `collapse` uses) from ever being
/// extracted or compared. A group's relationship to *other*, genuinely
/// different documents is still discovered normally through that one
/// representative; what's eliminated is the redundant, misleading
/// "near-duplicate, 1.0/1.0/1.0" result T3 would otherwise report for two
/// files that T1 already proved are the same file.
pub fn find_similar_excluding_exact_duplicates(
    roots: &[PathBuf],
    config: &SimilarityConfig,
    duplicate_groups: &[DuplicateGroup],
) -> Result<SimilarityReport> {
    let entries = scan::scan_roots(roots)?;

    let excluded = exact_duplicate_exclusions(duplicate_groups);

    let extractable_total = entries
        .iter()
        .filter(|e| {
            !excluded.contains(&e.path)
                && matches_extension_filter(&e.path, &config.extensions)
                && extract::classify(&e.path).is_some()
        })
        .count();
    Log::info(
        "similarity",
        &format!(
            "comparing started: {extractable_total} extractable files ({} excluded as confirmed exact duplicates)",
            excluded.len()
        ),
    );

    let mut paths = Vec::new();
    let mut documents: Vec<ExtractedDocument> = Vec::new();
    let mut skipped = 0usize;
    let mut excluded_exact_duplicates = 0usize;
    let mut excluded_by_extension_filter = 0usize;

    for entry in &entries {
        if !matches_extension_filter(&entry.path, &config.extensions) {
            excluded_by_extension_filter += 1;
            continue;
        }
        if excluded.contains(&entry.path) {
            excluded_exact_duplicates += 1;
            continue;
        }
        match extract::classify(&entry.path) {
            Some(kind) => {
                // A single unreadable or genuinely corrupted file (a
                // truncated zip that never was a valid docx/pptx/xlsx, for
                // instance) must never abort the whole comparison - one
                // file that can't be extracted is excluded from
                // comparison, the same as an unsupported format, not a
                // reason to stop looking at everything else.
                let document = match extract::extract(&entry.path, kind) {
                    Ok(document) => document,
                    Err(err) => {
                        Log::error(
                            "similarity",
                            &format!(
                                "extraction failed for {} - excluded from comparison: {err}",
                                entry.path.display()
                            ),
                        );
                        skipped += 1;
                        continue;
                    }
                };
                if document.tokens.is_empty() {
                    skipped += 1;
                    continue;
                }
                paths.push(entry.path.clone());
                documents.push(document);
            }
            None => skipped += 1,
        }
    }

    let signatures: Vec<MinHashSignature> = documents
        .iter()
        .map(|d| minhash_signature(&d.tokens, config.num_hashes, config.seed))
        .collect();

    let mut index = LshIndex::new(config.num_hashes, config.bands);
    for (i, signature) in signatures.iter().enumerate() {
        index.insert(i, signature);
    }

    let mut pairs = Vec::new();
    for group in index.candidate_groups(documents.len()) {
        for i in 0..group.len() {
            for j in (i + 1)..group.len() {
                let (a_idx, b_idx) = (group[i], group[j]);
                let jaccard = estimate_jaccard(&signatures[a_idx], &signatures[b_idx]);
                if jaccard < config.duplicate_cutoff {
                    continue;
                }
                let coverage_a_to_b = coverage(&documents[a_idx].tokens, &documents[b_idx].tokens);
                let coverage_b_to_a = coverage(&documents[b_idx].tokens, &documents[a_idx].tokens);
                let relationship =
                    classify_relationship(coverage_a_to_b, coverage_b_to_a, config.high_coverage);
                let media_match_score = media_match_score(
                    &documents[a_idx].media_hashes,
                    &documents[b_idx].media_hashes,
                );
                pairs.push(SimilarPair {
                    a: paths[a_idx].clone(),
                    b: paths[b_idx].clone(),
                    jaccard_estimate: jaccard,
                    coverage_a_to_b,
                    coverage_b_to_a,
                    relationship,
                    media_match_score,
                });
            }
        }
    }
    pairs.sort_by(|x, y| {
        y.jaccard_estimate
            .partial_cmp(&x.jaccard_estimate)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Log::info(
        "similarity",
        &format!(
            "comparing finished: {} files considered, {skipped} skipped, {excluded_exact_duplicates} excluded as exact duplicates, {excluded_by_extension_filter} excluded by extension filter, {} pairs found",
            documents.len(),
            pairs.len()
        ),
    );

    Ok(SimilarityReport {
        files_considered: documents.len(),
        files_skipped_unsupported: skipped,
        files_excluded_exact_duplicates: excluded_exact_duplicates,
        files_excluded_by_extension_filter: excluded_by_extension_filter,
        pairs,
    })
}

/// Whether `path`'s lowercase extension is allowed by `extensions` - always
/// true when no filter is configured (`None`).
fn matches_extension_filter(path: &std::path::Path, extensions: &Option<HashSet<String>>) -> bool {
    let Some(allowed) = extensions else {
        return true;
    };
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| allowed.contains(&e.to_ascii_lowercase()))
        .unwrap_or(false)
}

/// Every path that should be excluded from T3 because it's a redundant
/// member of a confirmed T1 exact-duplicate group - every path in every
/// group except one representative (first by path, for determinism).
fn exact_duplicate_exclusions(duplicate_groups: &[DuplicateGroup]) -> HashSet<PathBuf> {
    let mut excluded = HashSet::new();
    for group in duplicate_groups {
        let mut files = group.files.clone();
        files.sort();
        excluded.extend(files.into_iter().skip(1));
    }
    excluded
}

fn classify_relationship(coverage_a_to_b: f64, coverage_b_to_a: f64, high: f64) -> Relationship {
    match (coverage_a_to_b >= high, coverage_b_to_a >= high) {
        (true, true) => Relationship::NearDuplicate,
        (true, false) => Relationship::BIsMoreComplete,
        (false, true) => Relationship::AIsMoreComplete,
        (false, false) => Relationship::Diverged,
    }
}

/// Share of `a`'s tokens also present in `b` (SPEC.md section 5:
/// `coverage(A -> B)`). This is exact-containment over shingles/row hashes,
/// not the embedding-cosine version the spec describes for T4, which is
/// deferred; see SPEC.md section 12.
pub fn coverage(a_tokens: &HashSet<u64>, b_tokens: &HashSet<u64>) -> f64 {
    if a_tokens.is_empty() {
        return 0.0;
    }
    let intersection = a_tokens.intersection(b_tokens).count();
    intersection as f64 / a_tokens.len() as f64
}

/// Sorensen-Dice overlap of two documents' embedded media: `2 * |A ∩ B| /
/// (|A| + |B|)`, treating each document's media as a set - a document
/// embedding the same image twice counts it once, same as `coverage` does
/// for text tokens. `None` when both sides are empty, since "no media in
/// either document" isn't a meaningful comparison point.
pub fn media_match_score(a: &[blake3::Hash], b: &[blake3::Hash]) -> Option<f64> {
    if a.is_empty() && b.is_empty() {
        return None;
    }
    let a_set: HashSet<&blake3::Hash> = a.iter().collect();
    let b_set: HashSet<&blake3::Hash> = b.iter().collect();
    let matches = a_set.intersection(&b_set).count();
    Some(2.0 * matches as f64 / (a_set.len() + b_set.len()) as f64)
}

#[derive(Debug, Clone)]
pub struct MinHashSignature {
    values: Vec<u64>,
}

/// Builds a MinHash signature: for each of `num_hashes` pseudo-random linear
/// hash functions, the minimum value over all of `tokens`. Two documents
/// with similar token sets get similar signatures, with the fraction of
/// matching positions estimating their Jaccard similarity.
pub fn minhash_signature(tokens: &HashSet<u64>, num_hashes: usize, seed: u64) -> MinHashSignature {
    let coefficients = generate_coefficients(num_hashes, seed);
    let mut values = vec![u64::MAX; num_hashes];
    for &token in tokens {
        let x = (token % MERSENNE_PRIME_61) as u128;
        for (i, &(a, b)) in coefficients.iter().enumerate() {
            let h = ((a as u128 * x + b as u128) % MERSENNE_PRIME_61 as u128) as u64;
            if h < values[i] {
                values[i] = h;
            }
        }
    }
    MinHashSignature { values }
}

fn generate_coefficients(num_hashes: usize, seed: u64) -> Vec<(u64, u64)> {
    let mut rng = SplitMix64::new(seed);
    (0..num_hashes)
        .map(|_| {
            let a = (rng.next() % (MERSENNE_PRIME_61 - 1)) + 1;
            let b = rng.next() % MERSENNE_PRIME_61;
            (a, b)
        })
        .collect()
}

/// Deterministic, non-cryptographic PRNG used only to generate MinHash
/// coefficients, so the same seed always yields the same signature.
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

pub fn estimate_jaccard(a: &MinHashSignature, b: &MinHashSignature) -> f64 {
    if a.values.is_empty() {
        return 0.0;
    }
    let matches = a
        .values
        .iter()
        .zip(&b.values)
        .filter(|(x, y)| x == y)
        .count();
    matches as f64 / a.values.len() as f64
}

/// Locality-sensitive hashing over MinHash signatures: documents are
/// candidates for closer comparison if they share at least one band's
/// bucket. This is what lets `find_similar` avoid comparing every pair.
struct LshIndex {
    bands: usize,
    rows_per_band: usize,
    buckets: HashMap<(usize, u64), Vec<usize>>,
}

impl LshIndex {
    fn new(num_hashes: usize, bands: usize) -> Self {
        let bands = bands.max(1);
        let rows_per_band = (num_hashes / bands).max(1);
        Self {
            bands,
            rows_per_band,
            buckets: HashMap::new(),
        }
    }

    fn insert(&mut self, doc_index: usize, signature: &MinHashSignature) {
        for band in 0..self.bands {
            let start = band * self.rows_per_band;
            let end = (start + self.rows_per_band).min(signature.values.len());
            if start >= end {
                break;
            }
            let bucket_hash = hash_u64_slice(&signature.values[start..end]);
            self.buckets
                .entry((band, bucket_hash))
                .or_default()
                .push(doc_index);
        }
    }

    /// Connected components of documents that share at least one bucket.
    /// Each is a *candidate* group: `find_similar` still verifies every
    /// pair within it, since LSH co-occurrence alone can transitively chain
    /// unrelated documents together (SPEC.md section 4: "avoid transitive
    /// merging").
    fn candidate_groups(&self, doc_count: usize) -> Vec<Vec<usize>> {
        let mut uf = UnionFind::new(doc_count);
        for members in self.buckets.values() {
            for w in members.windows(2) {
                uf.union(w[0], w[1]);
            }
        }
        uf.groups().into_iter().filter(|g| g.len() > 1).collect()
    }
}

fn hash_u64_slice(values: &[u64]) -> u64 {
    let mut bytes = Vec::with_capacity(values.len() * 8);
    for v in values {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    let hash = blake3::hash(&bytes);
    let b = hash.as_bytes();
    let mut out = 0u64;
    for &byte in &b[..8] {
        out = (out << 8) | byte as u64;
    }
    out
}
