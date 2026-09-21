mod common;

use common::TempTree;
use engine::{plan, FingerprintCache, PreflightRules, ReconcileState};

fn state_of<'a>(entries: &'a [engine::PlanEntry], relative: &str) -> &'a ReconcileState {
    &entries
        .iter()
        .find(|e| e.relative_path.to_string_lossy().replace('\\', "/") == relative)
        .unwrap_or_else(|| panic!("no plan entry for {relative}"))
        .state
}

#[test]
fn verified_when_content_matches_at_the_same_path() {
    let tree = TempTree::new();
    tree.write("source/docs/report.txt", b"same content");
    tree.write("dest/docs/report.txt", b"same content");

    let mut cache = FingerprintCache::default();
    let report = plan(
        &tree.path().join("source"),
        &tree.path().join("dest"),
        &mut cache,
        &PreflightRules::default(),
    )
    .unwrap();

    assert_eq!(
        *state_of(&report.entries, "docs/report.txt"),
        ReconcileState::Verified
    );
    assert_eq!(report.summary.verified, 1);
}

#[test]
fn conflict_when_same_path_same_size_different_content() {
    let tree = TempTree::new();
    tree.write("source/a.txt", b"AAAAAAAAAA");
    tree.write("dest/a.txt", b"BBBBBBBBBB");

    let mut cache = FingerprintCache::default();
    let report = plan(
        &tree.path().join("source"),
        &tree.path().join("dest"),
        &mut cache,
        &PreflightRules::default(),
    )
    .unwrap();

    assert_eq!(
        *state_of(&report.entries, "a.txt"),
        ReconcileState::Conflict
    );
    assert_eq!(report.summary.conflict, 1);
}

#[test]
fn partial_when_same_path_different_size() {
    let tree = TempTree::new();
    tree.write("source/a.txt", b"a full copy of the file");
    tree.write("dest/a.txt", b"a partial copy");

    let mut cache = FingerprintCache::default();
    let report = plan(
        &tree.path().join("source"),
        &tree.path().join("dest"),
        &mut cache,
        &PreflightRules::default(),
    )
    .unwrap();

    assert!(matches!(
        state_of(&report.entries, "a.txt"),
        ReconcileState::Partial { .. }
    ));
    assert_eq!(report.summary.partial, 1);
}

#[test]
fn missing_when_not_present_at_destination_anywhere() {
    let tree = TempTree::new();
    tree.write("source/only_here.txt", b"unique content");
    tree.write("dest/unrelated.txt", b"something else entirely");

    let mut cache = FingerprintCache::default();
    let report = plan(
        &tree.path().join("source"),
        &tree.path().join("dest"),
        &mut cache,
        &PreflightRules::default(),
    )
    .unwrap();

    assert_eq!(
        *state_of(&report.entries, "only_here.txt"),
        ReconcileState::Missing
    );
    assert_eq!(report.summary.missing, 1);
}

#[test]
fn moved_when_content_exists_under_a_different_destination_path() {
    let tree = TempTree::new();
    tree.write("source/new_name.txt", b"content that already moved");
    tree.write("dest/old_name.txt", b"content that already moved");

    let mut cache = FingerprintCache::default();
    let report = plan(
        &tree.path().join("source"),
        &tree.path().join("dest"),
        &mut cache,
        &PreflightRules::default(),
    )
    .unwrap();

    match state_of(&report.entries, "new_name.txt") {
        ReconcileState::Moved {
            destination_relative,
        } => {
            assert_eq!(
                destination_relative.to_string_lossy().replace('\\', "/"),
                "old_name.txt"
            );
        }
        other => panic!("expected Moved, got {other:?}"),
    }
    assert_eq!(report.summary.moved, 1);

    // The destination file backing the move must not also show as
    // destination-only, since it is accounted for.
    assert!(report
        .entries
        .iter()
        .all(|e| e.relative_path.to_string_lossy() != "old_name.txt"
            || !matches!(e.state, ReconcileState::DestinationOnly)));
}

#[test]
fn destination_only_when_no_source_counterpart_exists() {
    let tree = TempTree::new();
    tree.write("source/a.txt", b"source content");
    tree.write("dest/a.txt", b"source content");
    tree.write("dest/leftover.txt", b"nothing matches this");

    let mut cache = FingerprintCache::default();
    let report = plan(
        &tree.path().join("source"),
        &tree.path().join("dest"),
        &mut cache,
        &PreflightRules::default(),
    )
    .unwrap();

    assert_eq!(
        *state_of(&report.entries, "leftover.txt"),
        ReconcileState::DestinationOnly
    );
    assert_eq!(report.summary.destination_only, 1);
}

// Illegal characters and case/NFC collisions can't be exercised through real
// files here: Windows rejects `:` in filenames outright, and the default
// filesystem on both Windows and macOS is case-insensitive, so two
// intentionally "colliding" writes just collapse into one file before the
// test even runs. `policy_test.rs` covers illegal-character detection as a
// pure function, and `reconcile.rs`'s own unit tests cover key collisions
// directly on in-memory entries.
#[test]
fn blocked_when_source_path_exceeds_the_configured_length_limit() {
    let tree = TempTree::new();
    tree.write("source/a.txt", b"short file, long rule");

    let rules = PreflightRules {
        max_path_length: 3,
        ..PreflightRules::default()
    };

    let mut cache = FingerprintCache::default();
    let report = plan(
        &tree.path().join("source"),
        &tree.path().join("dest"),
        &mut cache,
        &rules,
    )
    .unwrap();

    assert!(matches!(
        state_of(&report.entries, "a.txt"),
        ReconcileState::Blocked { .. }
    ));
    assert_eq!(report.summary.blocked, 1);
}

#[test]
fn cache_reuse_does_not_change_the_classification() {
    let tree = TempTree::new();
    tree.write("source/a.txt", b"identical content here");
    tree.write("dest/a.txt", b"identical content here");
    let cache_path = tree.path().join("cache.json");

    let mut cache = FingerprintCache::load(&cache_path).unwrap();
    let first = plan(
        &tree.path().join("source"),
        &tree.path().join("dest"),
        &mut cache,
        &PreflightRules::default(),
    )
    .unwrap();
    cache.save(&cache_path).unwrap();

    let mut cache2 = FingerprintCache::load(&cache_path).unwrap();
    let second = plan(
        &tree.path().join("source"),
        &tree.path().join("dest"),
        &mut cache2,
        &PreflightRules::default(),
    )
    .unwrap();

    assert_eq!(*state_of(&first.entries, "a.txt"), ReconcileState::Verified);
    assert_eq!(
        *state_of(&second.entries, "a.txt"),
        ReconcileState::Verified
    );
}
