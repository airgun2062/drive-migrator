mod common;

use std::time::SystemTime;

use common::TempTree;
use engine::cache::FingerprintCache;
use engine::scan::ScanEntry;

#[test]
fn round_trips_through_disk() {
    let tree = TempTree::new();
    let file_path = tree.write("a.txt", b"hello world");
    let cache_path = tree.path().join("sub/cache.json");

    let entry = ScanEntry {
        path: file_path,
        size: 11,
        modified: SystemTime::now(),
    };
    let hash = blake3::hash(b"hello world");

    let mut cache = FingerprintCache::default();
    assert!(cache.lookup(&entry).is_none());
    cache.record(&entry, hash);
    assert_eq!(cache.lookup(&entry), Some(hash));

    cache.save(&cache_path).unwrap();
    let reloaded = FingerprintCache::load(&cache_path).unwrap();
    assert_eq!(reloaded.lookup(&entry), Some(hash));
}

#[test]
fn stale_size_invalidates_the_cache_entry() {
    let tree = TempTree::new();
    let file_path = tree.write("a.txt", b"hello world");
    let modified = SystemTime::now();

    let mut cache = FingerprintCache::default();
    let entry = ScanEntry {
        path: file_path.clone(),
        size: 11,
        modified,
    };
    cache.record(&entry, blake3::hash(b"hello world"));

    let changed = ScanEntry {
        path: file_path,
        size: 999,
        modified,
    };
    assert!(cache.lookup(&changed).is_none());
}

#[test]
fn missing_cache_file_loads_as_empty() {
    let tree = TempTree::new();
    let cache = FingerprintCache::load(&tree.path().join("does-not-exist.json")).unwrap();
    let entry = ScanEntry {
        path: tree.path().join("whatever.txt"),
        size: 0,
        modified: SystemTime::now(),
    };
    assert!(cache.lookup(&entry).is_none());
}
