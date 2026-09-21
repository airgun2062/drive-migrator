mod common;

use common::TempTree;
use engine::{plan, FingerprintCache, Journal, PreflightRules};

#[test]
fn records_a_plan_run_and_counts_its_entries() {
    let tree = TempTree::new();
    tree.write("source/a.txt", b"same");
    tree.write("dest/a.txt", b"same");
    tree.write("source/missing.txt", b"only in source");

    let mut cache = FingerprintCache::default();
    let report = plan(
        &tree.path().join("source"),
        &tree.path().join("dest"),
        &mut cache,
        &PreflightRules::default(),
    )
    .unwrap();
    let entry_count = report.entries.len();

    let journal_path = tree.path().join("journal.sqlite");
    let mut journal = Journal::open(&journal_path).unwrap();
    let run_id = journal.record_plan(&report).unwrap();

    assert_eq!(journal.latest_run_id().unwrap(), Some(run_id));
    assert_eq!(
        journal.count_entries_for_run(run_id).unwrap(),
        entry_count as i64
    );
}

#[test]
fn reopening_the_journal_keeps_previous_runs() {
    let tree = TempTree::new();
    tree.write("source/a.txt", b"content");
    tree.write("dest/a.txt", b"content");
    let journal_path = tree.path().join("journal.sqlite");

    let mut cache = FingerprintCache::default();
    let report = plan(
        &tree.path().join("source"),
        &tree.path().join("dest"),
        &mut cache,
        &PreflightRules::default(),
    )
    .unwrap();

    let first_run_id = {
        let mut journal = Journal::open(&journal_path).unwrap();
        journal.record_plan(&report).unwrap()
    };

    let mut journal = Journal::open(&journal_path).unwrap();
    let second_run_id = journal.record_plan(&report).unwrap();

    assert!(second_run_id > first_run_id);
    assert_eq!(journal.latest_run_id().unwrap(), Some(second_run_id));
}

#[test]
fn missing_journal_file_is_created() {
    let tree = TempTree::new();
    let journal_path = tree.path().join("nested/does-not-exist-yet/journal.sqlite");

    let journal = Journal::open(&journal_path).unwrap();
    assert_eq!(journal.latest_run_id().unwrap(), None);
    assert!(journal_path.exists());
}
