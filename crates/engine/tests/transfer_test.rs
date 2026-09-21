mod common;

use std::fs;

use common::TempTree;
use engine::{
    plan, run, FingerprintCache, MovedPolicy, PreflightRules, RunOptions, TransferOutcome,
};

fn default_plan(tree: &TempTree) -> engine::ReconcilePlan {
    let mut cache = FingerprintCache::default();
    plan(
        &tree.path().join("source"),
        &tree.path().join("dest"),
        &mut cache,
        &PreflightRules::default(),
    )
    .unwrap()
}

#[test]
fn copies_a_missing_file_and_verifies_it() {
    let tree = TempTree::new();
    tree.write("source/docs/report.txt", b"brand new content");

    let report = default_plan(&tree);
    let summary = run(&report, &RunOptions::default()).unwrap();

    assert_eq!(summary.copied, 1);
    assert_eq!(summary.failed, 0);
    assert_eq!(summary.bytes_copied, "brand new content".len() as u64);

    let copied = fs::read(tree.path().join("dest/docs/report.txt")).unwrap();
    assert_eq!(copied, b"brand new content");

    // No stray .part file left behind.
    assert!(!tree.path().join("dest/docs/report.txt.part").exists());
}

#[test]
fn overwrites_a_partial_file_with_a_correct_copy() {
    let tree = TempTree::new();
    tree.write("source/a.txt", b"the complete file");
    tree.write("dest/a.txt", b"incomplete");

    let report = default_plan(&tree);
    let summary = run(&report, &RunOptions::default()).unwrap();

    assert_eq!(summary.copied, 1);
    let copied = fs::read(tree.path().join("dest/a.txt")).unwrap();
    assert_eq!(copied, b"the complete file");
}

#[test]
fn leaves_verified_conflict_and_blocked_files_untouched() {
    let tree = TempTree::new();
    tree.write("source/verified.txt", b"same");
    tree.write("dest/verified.txt", b"same");
    tree.write("source/conflict.txt", b"AAAAAAAAAA");
    tree.write("dest/conflict.txt", b"BBBBBBBBBB");

    let report = default_plan(&tree);
    let summary = run(&report, &RunOptions::default()).unwrap();

    assert_eq!(summary.copied, 0);
    assert_eq!(summary.skipped_verified, 1);
    assert_eq!(summary.skipped_conflict, 1);
    assert_eq!(
        fs::read(tree.path().join("dest/conflict.txt")).unwrap(),
        b"BBBBBBBBBB"
    );
}

#[test]
fn moved_files_are_skipped_by_default_and_copied_when_requested() {
    let tree = TempTree::new();
    tree.write("source/new_name.txt", b"already elsewhere");
    tree.write("dest/old_name.txt", b"already elsewhere");

    let report = default_plan(&tree);

    let leave_summary = run(
        &report,
        &RunOptions {
            on_moved: MovedPolicy::Leave,
        },
    )
    .unwrap();
    assert_eq!(leave_summary.copied, 0);
    assert_eq!(leave_summary.skipped_moved, 1);
    assert!(!tree.path().join("dest/new_name.txt").exists());

    let copy_summary = run(
        &report,
        &RunOptions {
            on_moved: MovedPolicy::Copy,
        },
    )
    .unwrap();
    assert_eq!(copy_summary.copied, 1);
    assert_eq!(
        fs::read(tree.path().join("dest/new_name.txt")).unwrap(),
        b"already elsewhere"
    );
}

#[test]
fn discards_a_stale_part_file_before_copying() {
    let tree = TempTree::new();
    tree.write("source/a.txt", b"fresh content");
    tree.write("dest/a.txt.part", b"leftover from a crashed run");

    let report = default_plan(&tree);
    let summary = run(&report, &RunOptions::default()).unwrap();

    assert_eq!(summary.copied, 1);
    assert_eq!(
        fs::read(tree.path().join("dest/a.txt")).unwrap(),
        b"fresh content"
    );
}

#[test]
fn preserves_the_source_modified_time() {
    let tree = TempTree::new();
    let source_path = tree.write("source/a.txt", b"timestamped content");

    let report = default_plan(&tree);
    run(&report, &RunOptions::default()).unwrap();

    let source_modified = fs::metadata(&source_path).unwrap().modified().unwrap();
    let dest_modified = fs::metadata(tree.path().join("dest/a.txt"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(source_modified, dest_modified);
}

#[test]
fn never_writes_into_the_source_root() {
    let tree = TempTree::new();
    tree.write("source/a.txt", b"must not be touched");

    let before = fs::read(tree.path().join("source/a.txt")).unwrap();
    let before_entries: Vec<_> = fs::read_dir(tree.path().join("source")).unwrap().collect();

    let report = default_plan(&tree);
    run(&report, &RunOptions::default()).unwrap();

    let after = fs::read(tree.path().join("source/a.txt")).unwrap();
    let after_entries: Vec<_> = fs::read_dir(tree.path().join("source")).unwrap().collect();
    assert_eq!(before, after);
    assert_eq!(before_entries.len(), after_entries.len());
}

#[test]
fn failures_do_not_stop_the_rest_of_the_run() {
    let tree = TempTree::new();
    tree.write("source/ok.txt", b"this one works");
    // A source file whose path does not actually exist on disk by the time
    // transfer runs would fail; simulate by pointing a plan at a source file
    // that gets removed between planning and running.
    let doomed = tree.write("source/doomed.txt", b"will be removed");

    let report = default_plan(&tree);
    fs::remove_file(&doomed).unwrap();

    let summary = run(&report, &RunOptions::default()).unwrap();

    assert_eq!(summary.copied, 1);
    assert_eq!(summary.failed, 1);
    assert!(summary
        .results
        .iter()
        .any(|r| r.outcome == TransferOutcome::Failed));
    assert!(fs::read(tree.path().join("dest/ok.txt")).is_ok());
}
