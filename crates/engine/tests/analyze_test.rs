mod common;

use common::TempTree;
use engine::{analyze, FingerprintCache};

#[test]
fn finds_exact_duplicates_across_two_roots() {
    let tree = TempTree::new();
    tree.write("root_a/docs/report.txt", b"same content");
    tree.write("root_b/backup/report_copy.txt", b"same content");
    tree.write("root_a/unique.txt", b"only here");

    let root_a = tree.path().join("root_a");
    let root_b = tree.path().join("root_b");

    let mut cache = FingerprintCache::default();
    let report = analyze(&[root_a, root_b], &mut cache).expect("analyze should succeed");

    assert_eq!(report.files_scanned, 3);
    assert_eq!(report.duplicate_groups.len(), 1);
    assert_eq!(report.duplicate_groups[0].files.len(), 2);
    assert_eq!(report.duplicate_files, 2);
    assert_eq!(report.reclaimable_bytes, "same content".len() as u64);
}

#[test]
fn files_with_same_size_but_different_content_are_not_duplicates() {
    let tree = TempTree::new();
    tree.write("a.txt", b"AAAAAAAAAA");
    tree.write("b.txt", b"BBBBBBBBBB");

    let mut cache = FingerprintCache::default();
    let report = analyze(&[tree.path().to_path_buf()], &mut cache).unwrap();

    assert_eq!(report.duplicate_groups.len(), 0);
}

#[test]
fn three_way_duplicate_forms_a_single_group() {
    let tree = TempTree::new();
    tree.write("a.txt", b"triplicate");
    tree.write("b.txt", b"triplicate");
    tree.write("c.txt", b"triplicate");

    let mut cache = FingerprintCache::default();
    let report = analyze(&[tree.path().to_path_buf()], &mut cache).unwrap();

    assert_eq!(report.duplicate_groups.len(), 1);
    assert_eq!(report.duplicate_groups[0].files.len(), 3);
    assert_eq!(report.duplicate_files, 3);
}

#[test]
fn cache_is_reused_on_second_analyze_and_hash_is_unchanged() {
    let tree = TempTree::new();
    tree.write("a.txt", b"duplicate content here");
    tree.write("b.txt", b"duplicate content here");

    let cache_path = tree.path().join("cache.json");
    let mut cache = FingerprintCache::load(&cache_path).unwrap();
    let first = analyze(&[tree.path().to_path_buf()], &mut cache).unwrap();
    cache.save(&cache_path).unwrap();

    let mut cache2 = FingerprintCache::load(&cache_path).unwrap();
    let second = analyze(&[tree.path().to_path_buf()], &mut cache2).unwrap();

    assert_eq!(first.duplicate_groups.len(), 1);
    assert_eq!(second.duplicate_groups.len(), 1);
    assert_eq!(
        first.duplicate_groups[0].hash,
        second.duplicate_groups[0].hash
    );
}

#[test]
fn empty_files_are_reported_as_duplicates() {
    let tree = TempTree::new();
    tree.write("a.empty", b"");
    tree.write("b.empty", b"");

    let mut cache = FingerprintCache::default();
    let report = analyze(&[tree.path().to_path_buf()], &mut cache).unwrap();

    assert_eq!(report.duplicate_groups.len(), 1);
    assert_eq!(report.duplicate_groups[0].size, 0);
    assert_eq!(report.reclaimable_bytes, 0);
}
