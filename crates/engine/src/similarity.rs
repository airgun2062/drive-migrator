use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::Serialize;

use crate::error::Result;
use crate::extract::{self, ExtractedDocument};
use crate::scan;

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
}

#[derive(Debug, Default, Serialize)]
pub struct SimilarityReport {
    pub files_considered: usize,
    pub files_skipped_unsupported: usize,
    pub pairs: Vec<SimilarPair>,
}

/// Finds near-duplicate and version-like relationships among the supported
/// text-like formats under `roots`. Uses MinHash + LSH for candidate
/// generation (SPEC.md section 4, T3) so it never compares every pair, then
/// verifies each candidate pairwise (never assumes a whole LSH group is
/// mutually similar, since that could transitively merge unrelated files).
pub fn find_similar(roots: &[PathBuf], config: &SimilarityConfig) -> Result<SimilarityReport> {
    let entries = scan::scan_roots(roots)?;

    let mut paths = Vec::new();
    let mut documents: Vec<ExtractedDocument> = Vec::new();
    let mut skipped = 0usize;

    for entry in &entries {
        match extract::classify(&entry.path) {
            Some(kind) => {
                let document = extract::extract(&entry.path, kind)?;
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
                pairs.push(SimilarPair {
                    a: paths[a_idx].clone(),
                    b: paths[b_idx].clone(),
                    jaccard_estimate: jaccard,
                    coverage_a_to_b,
                    coverage_b_to_a,
                    relationship,
                });
            }
        }
    }
    pairs.sort_by(|x, y| {
        y.jaccard_estimate
            .partial_cmp(&x.jaccard_estimate)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(SimilarityReport {
        files_considered: documents.len(),
        files_skipped_unsupported: skipped,
        pairs,
    })
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
        uf.groups()
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

struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }

    fn find(&mut self, x: usize) -> usize {
        if self.parent[x] != x {
            self.parent[x] = self.find(self.parent[x]);
        }
        self.parent[x]
    }

    fn union(&mut self, a: usize, b: usize) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra != rb {
            self.parent[ra] = rb;
        }
    }

    fn groups(&mut self) -> Vec<Vec<usize>> {
        let mut map: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in 0..self.parent.len() {
            let root = self.find(i);
            map.entry(root).or_default().push(i);
        }
        map.into_values().filter(|g| g.len() > 1).collect()
    }
}
