mod common;

use std::collections::HashSet;

use common::TempTree;
use engine::similarity::{coverage, estimate_jaccard, media_match_score, minhash_signature};
use engine::{find_similar, Relationship, SimilarityConfig};

fn words(prefix: &str, count: usize) -> String {
    (0..count)
        .map(|i| format!("{prefix}{i}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn coverage_of_a_subset_is_full() {
    let a: HashSet<u64> = [1, 2, 3].into_iter().collect();
    let b: HashSet<u64> = [1, 2, 3, 4, 5].into_iter().collect();
    assert_eq!(coverage(&a, &b), 1.0);
}

#[test]
fn coverage_of_disjoint_sets_is_zero() {
    let a: HashSet<u64> = [1, 2, 3].into_iter().collect();
    let b: HashSet<u64> = [4, 5, 6].into_iter().collect();
    assert_eq!(coverage(&a, &b), 0.0);
}

#[test]
fn coverage_of_an_empty_set_is_zero() {
    let a: HashSet<u64> = HashSet::new();
    let b: HashSet<u64> = [1, 2, 3].into_iter().collect();
    assert_eq!(coverage(&a, &b), 0.0);
}

#[test]
fn minhash_signature_is_deterministic_for_the_same_seed() {
    let tokens: HashSet<u64> = [10, 20, 30, 40, 50].into_iter().collect();
    let sig_a = minhash_signature(&tokens, 64, 42);
    let sig_b = minhash_signature(&tokens, 64, 42);
    assert_eq!(estimate_jaccard(&sig_a, &sig_b), 1.0);
}

#[test]
fn estimate_jaccard_of_identical_token_sets_is_one() {
    let tokens: HashSet<u64> = [1, 2, 3, 4, 5].into_iter().collect();
    let sig = minhash_signature(&tokens, 64, 1);
    assert_eq!(estimate_jaccard(&sig, &sig), 1.0);
}

#[test]
fn estimate_jaccard_of_disjoint_token_sets_is_low() {
    let a: HashSet<u64> = (0..50).collect();
    let b: HashSet<u64> = (1000..1050).collect();
    let sig_a = minhash_signature(&a, 128, 7);
    let sig_b = minhash_signature(&b, 128, 7);
    assert!(estimate_jaccard(&sig_a, &sig_b) < 0.1);
}

#[test]
fn find_similar_detects_near_duplicates() {
    let tree = TempTree::new();
    // Near-identical but not byte-identical (a trailing word differs in
    // each) - genuinely identical content is now excluded before T3 as a
    // confirmed T1 exact duplicate instead (see
    // exact_duplicates_are_excluded_from_similarity_instead_of_reported_as_near_duplicate),
    // so this test would no longer exercise near-duplicate detection at all
    // if both files were the same content.
    let shared = words("shared", 40);
    tree.write("a.txt", format!("{shared} onlyinA").as_bytes());
    tree.write("b.txt", format!("{shared} onlyinB").as_bytes());

    let report = find_similar(&[tree.path().to_path_buf()], &SimilarityConfig::default()).unwrap();

    assert_eq!(report.pairs.len(), 1);
    assert_eq!(report.pairs[0].relationship, Relationship::NearDuplicate);
}

#[test]
fn find_similar_detects_containment() {
    let tree = TempTree::new();
    let a_content = words("core", 30);
    let b_content = format!("{a_content} {}", words("extra", 12));
    tree.write("a.txt", a_content.as_bytes());
    tree.write("b.txt", b_content.as_bytes());

    let report = find_similar(&[tree.path().to_path_buf()], &SimilarityConfig::default()).unwrap();

    assert_eq!(report.pairs.len(), 1);
    let pair = &report.pairs[0];

    // Don't assume which physical file landed as "a" vs "b": just check one
    // direction is full coverage and the other partial, classified as a
    // containment relationship either way.
    let (full, partial) = if pair.coverage_a_to_b > pair.coverage_b_to_a {
        (pair.coverage_a_to_b, pair.coverage_b_to_a)
    } else {
        (pair.coverage_b_to_a, pair.coverage_a_to_b)
    };
    assert_eq!(full, 1.0);
    assert!(partial < 0.8);
    assert!(matches!(
        pair.relationship,
        Relationship::BIsMoreComplete | Relationship::AIsMoreComplete
    ));
}

#[test]
fn find_similar_does_not_report_unrelated_files() {
    let tree = TempTree::new();
    tree.write("a.txt", words("alpha", 30).as_bytes());
    tree.write("b.txt", words("beta", 30).as_bytes());

    let report = find_similar(&[tree.path().to_path_buf()], &SimilarityConfig::default()).unwrap();

    assert!(report.pairs.is_empty());
}

#[test]
fn find_similar_counts_unsupported_files_as_skipped() {
    let tree = TempTree::new();
    tree.write("a.txt", words("alpha", 20).as_bytes());
    tree.write("b.bin", &[0u8, 1, 2, 3, 4]);
    tree.write("c.pdf", b"%PDF-1.4 fake pdf bytes");

    let report = find_similar(&[tree.path().to_path_buf()], &SimilarityConfig::default()).unwrap();

    assert_eq!(report.files_considered, 1);
    assert_eq!(report.files_skipped_unsupported, 2);
}

#[test]
fn exact_duplicates_are_excluded_from_similarity_instead_of_reported_as_near_duplicate() {
    let tree = TempTree::new();
    // Three byte-identical copies (T1 would confirm all three as one exact-
    // duplicate group) plus one genuinely different file.
    let content = words("shared", 40);
    tree.write("a.txt", content.as_bytes());
    tree.write("b.txt", content.as_bytes());
    tree.write("c.txt", content.as_bytes());
    tree.write("different.txt", words("unrelated", 40).as_bytes());

    let report = find_similar(&[tree.path().to_path_buf()], &SimilarityConfig::default()).unwrap();

    // The three identical copies must never appear as a "near duplicate"
    // pair between each other - SPEC.md section 4: "each tier sees only
    // what survives the previous one." Two of the three are excluded
    // (one representative remains); the unrelated file was never part of
    // any exact-duplicate group, so it isn't excluded, but it also has
    // nothing in common with the surviving representative, so no pairs
    // are reported at all.
    assert_eq!(report.files_excluded_exact_duplicates, 2);
    assert!(
        report.pairs.is_empty(),
        "expected no pairs - the exact-duplicate copies must not surface as a near-duplicate result: {:?}",
        report.pairs
    );
}

#[test]
fn extension_filter_excludes_every_file_not_in_the_allowed_set() {
    let tree = TempTree::new();
    let shared = words("shared", 40);
    tree.write("a.txt", format!("{shared} onlyinA").as_bytes());
    tree.write("b.txt", format!("{shared} onlyinB").as_bytes());

    let config = SimilarityConfig {
        extensions: Some(["docx".to_string()].into_iter().collect()),
        ..SimilarityConfig::default()
    };
    let report = find_similar(&[tree.path().to_path_buf()], &config).unwrap();

    assert_eq!(report.files_excluded_by_extension_filter, 2);
    assert_eq!(report.files_considered, 0);
    assert!(report.pairs.is_empty());
}

#[test]
fn extension_filter_lets_configured_extensions_through_and_still_finds_pairs() {
    let tree = TempTree::new();
    let shared = words("shared", 40);
    tree.write("a.txt", format!("{shared} onlyinA").as_bytes());
    tree.write("b.txt", format!("{shared} onlyinB").as_bytes());
    tree.write("c.csv", b"name,age\nalice,30\n");

    let config = SimilarityConfig {
        extensions: Some(["txt".to_string()].into_iter().collect()),
        ..SimilarityConfig::default()
    };
    let report = find_similar(&[tree.path().to_path_buf()], &config).unwrap();

    assert_eq!(report.files_excluded_by_extension_filter, 1);
    assert_eq!(report.files_considered, 2);
    assert_eq!(report.pairs.len(), 1);
}

fn hashes(seeds: &[&str]) -> Vec<blake3::Hash> {
    seeds.iter().map(|s| blake3::hash(s.as_bytes())).collect()
}

#[test]
fn media_match_score_is_none_when_neither_side_has_media() {
    assert_eq!(media_match_score(&[], &[]), None);
}

#[test]
fn media_match_score_matches_the_worked_example() {
    // 5 images in A, 10 in B, 2 shared - the example the score was
    // designed around: 2*2/(5+10) = 4/15.
    let a = hashes(&["shared1", "shared2", "a3", "a4", "a5"]);
    let b = hashes(&[
        "shared1", "shared2", "b3", "b4", "b5", "b6", "b7", "b8", "b9", "b10",
    ]);
    let score = media_match_score(&a, &b).unwrap();
    assert!((score - (4.0 / 15.0)).abs() < 1e-9);
}

#[test]
fn media_match_score_of_identical_sets_is_one() {
    let a = hashes(&["one", "two", "three"]);
    assert_eq!(media_match_score(&a, &a), Some(1.0));
}

#[test]
fn media_match_score_of_disjoint_sets_is_zero() {
    let a = hashes(&["one", "two"]);
    let b = hashes(&["three", "four"]);
    assert_eq!(media_match_score(&a, &b), Some(0.0));
}

#[test]
fn media_match_score_treats_media_as_a_set_not_a_multiset() {
    // The same image repeated must not inflate either side's count.
    let a = hashes(&["dup", "dup", "dup"]);
    let b = hashes(&["dup"]);
    assert_eq!(media_match_score(&a, &b), Some(1.0));
}

#[test]
fn media_match_score_is_none_only_when_both_sides_are_empty() {
    let a = hashes(&["one"]);
    assert_eq!(media_match_score(&a, &[]), Some(0.0));
}
