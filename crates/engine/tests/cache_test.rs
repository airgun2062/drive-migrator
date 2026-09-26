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
    let reloaded = FingerprintCache::load(&cache_path);
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
    let cache = FingerprintCache::load(&tree.path().join("does-not-exist.json"));
    let entry = ScanEntry {
        path: tree.path().join("whatever.txt"),
        size: 0,
        modified: SystemTime::now(),
    };
    assert!(cache.lookup(&entry).is_none());
}

#[test]
fn a_corrupted_cache_file_loads_as_empty_instead_of_erroring() {
    let tree = TempTree::new();
    let cache_path = tree.write("cache.json", b"{ not valid json at all");
    // The cache is a pure optimization - a corrupted or incompatible-schema
    // file must never abort whatever operation asked for it; it should
    // just start fresh, the same as a missing file.
    let cache = FingerprintCache::load(&cache_path);
    let entry = ScanEntry {
        path: tree.path().join("whatever.txt"),
        size: 0,
        modified: SystemTime::now(),
    };
    assert!(cache.lookup(&entry).is_none());
}

#[test]
fn sample_hash_round_trips_independently_of_the_full_hash() {
    let tree = TempTree::new();
    let file_path = tree.write("a.txt", b"hello world");
    let modified = SystemTime::now();
    let entry = ScanEntry {
        path: file_path,
        size: 11,
        modified,
    };
    let sample = blake3::hash(b"hell");

    let mut cache = FingerprintCache::default();
    assert!(cache.lookup_sample(&entry).is_none());
    cache.record_sample(&entry, sample);

    // A file resolved only by its sample (proven unique, never full-hashed)
    // still has no full hash cached - the two are tracked independently.
    assert_eq!(cache.lookup_sample(&entry), Some(sample));
    assert!(cache.lookup(&entry).is_none());
}

#[test]
fn recording_the_full_hash_preserves_an_already_cached_sample_hash() {
    let tree = TempTree::new();
    let file_path = tree.write("a.txt", b"hello world");
    let modified = SystemTime::now();
    let entry = ScanEntry {
        path: file_path,
        size: 11,
        modified,
    };
    let sample = blake3::hash(b"hell");
    let full = blake3::hash(b"hello world");

    let mut cache = FingerprintCache::default();
    cache.record_sample(&entry, sample);
    cache.record(&entry, full);

    // Recording the full hash afterward (the normal sample-then-full flow)
    // must not clobber the sample hash recorded moments earlier for the
    // same file - both should be readable afterward.
    assert_eq!(cache.lookup_sample(&entry), Some(sample));
    assert_eq!(cache.lookup(&entry), Some(full));
}

#[test]
fn a_changed_file_does_not_inherit_the_previous_versions_sample_hash() {
    let tree = TempTree::new();
    let file_path = tree.write("a.txt", b"hello world");
    let modified = SystemTime::now();

    let mut cache = FingerprintCache::default();
    let original = ScanEntry {
        path: file_path.clone(),
        size: 11,
        modified,
    };
    cache.record_sample(&original, blake3::hash(b"hell"));
    cache.record(&original, blake3::hash(b"hello world"));

    let changed = ScanEntry {
        path: file_path,
        size: 999,
        modified,
    };
    assert!(cache.lookup_sample(&changed).is_none());
    assert!(cache.lookup(&changed).is_none());
}
