mod common;

use common::TempTree;
use engine::{
    apply_collapse_to_plan, plan as reconcile_plan, plan_collapse, run as transfer_run,
    CollapseConfig, FingerprintCache, PreflightRules, RunOptions,
};

fn words(prefix: &str, count: usize) -> String {
    (0..count)
        .map(|i| format!("{prefix}{i}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn collapse_mode_copies_the_tip_but_not_the_collapsed_version() {
    let tree = TempTree::new();
    let a_content = words("core", 30);
    let b_content = format!("{a_content} {}", words("extra", 12));
    tree.write("source/a.txt", a_content.as_bytes());
    tree.write("source/b.txt", b_content.as_bytes());

    let source = tree.path().join("source");
    let dest = tree.path().join("dest");

    let mut collapse_cache = FingerprintCache::default();
    let collapse_report = plan_collapse(
        std::slice::from_ref(&source),
        &mut collapse_cache,
        &CollapseConfig::default(),
    )
    .unwrap();

    let mut reconcile_cache = FingerprintCache::default();
    let mut plan = reconcile_plan(
        &source,
        &dest,
        &mut reconcile_cache,
        &PreflightRules::default(),
    )
    .unwrap();
    apply_collapse_to_plan(&mut plan, &collapse_report);

    let summary = transfer_run(&plan, &RunOptions::default()).unwrap();
    assert_eq!(summary.failed, 0);

    // b.txt (the tip, more complete) is copied; a.txt (collapsed) is not.
    assert!(dest.join("b.txt").exists());
    assert!(!dest.join("a.txt").exists());
    assert!(!dest.join("_collapsed").exists());
}

#[test]
fn collapse_mode_with_archive_redirects_the_collapsed_version_instead_of_dropping_it() {
    let tree = TempTree::new();
    let a_content = words("core", 30);
    let b_content = format!("{a_content} {}", words("extra", 12));
    tree.write("source/a.txt", a_content.as_bytes());
    tree.write("source/b.txt", b_content.as_bytes());

    let source = tree.path().join("source");
    let dest = tree.path().join("dest");

    let config = CollapseConfig {
        archive: true,
        ..CollapseConfig::default()
    };
    let mut collapse_cache = FingerprintCache::default();
    let collapse_report =
        plan_collapse(std::slice::from_ref(&source), &mut collapse_cache, &config).unwrap();

    let mut reconcile_cache = FingerprintCache::default();
    let mut plan = reconcile_plan(
        &source,
        &dest,
        &mut reconcile_cache,
        &PreflightRules::default(),
    )
    .unwrap();
    apply_collapse_to_plan(&mut plan, &collapse_report);

    let summary = transfer_run(&plan, &RunOptions::default()).unwrap();
    assert_eq!(summary.failed, 0);

    assert!(dest.join("b.txt").exists());
    assert!(!dest.join("a.txt").exists());
    // a.txt landed in the archive instead of being dropped.
    assert!(dest.join("_collapsed").join("a.txt").exists());
    let archived = std::fs::read(dest.join("_collapsed").join("a.txt")).unwrap();
    assert_eq!(archived, a_content.as_bytes());
}

#[test]
fn collapse_mode_keeps_one_copy_of_an_exact_duplicate_group() {
    let tree = TempTree::new();
    tree.write("source/a.bin", b"identical content");
    tree.write("source/b.bin", b"identical content");

    let source = tree.path().join("source");
    let dest = tree.path().join("dest");

    let mut collapse_cache = FingerprintCache::default();
    let collapse_report = plan_collapse(
        std::slice::from_ref(&source),
        &mut collapse_cache,
        &CollapseConfig::default(),
    )
    .unwrap();

    let mut reconcile_cache = FingerprintCache::default();
    let mut plan = reconcile_plan(
        &source,
        &dest,
        &mut reconcile_cache,
        &PreflightRules::default(),
    )
    .unwrap();
    apply_collapse_to_plan(&mut plan, &collapse_report);

    let summary = transfer_run(&plan, &RunOptions::default()).unwrap();
    assert_eq!(summary.failed, 0);
    assert_eq!(summary.copied, 1);

    // Exactly one of the two ever lands at the destination.
    let a_exists = dest.join("a.bin").exists();
    let b_exists = dest.join("b.bin").exists();
    assert_ne!(a_exists, b_exists, "exactly one copy should exist");
}

#[test]
fn already_verified_files_are_left_alone_even_if_collapse_would_now_drop_them() {
    // First, a normal (non-collapse) run copies both versions.
    let tree = TempTree::new();
    let a_content = words("core", 30);
    let b_content = format!("{a_content} {}", words("extra", 12));
    tree.write("source/a.txt", a_content.as_bytes());
    tree.write("source/b.txt", b_content.as_bytes());

    let source = tree.path().join("source");
    let dest = tree.path().join("dest");

    let mut cache = FingerprintCache::default();
    let plan = reconcile_plan(&source, &dest, &mut cache, &PreflightRules::default()).unwrap();
    transfer_run(&plan, &RunOptions::default()).unwrap();
    assert!(dest.join("a.txt").exists());
    assert!(dest.join("b.txt").exists());

    // Now re-plan in collapse mode: a.txt already exists at the
    // destination (Verified), so collapse must not delete it even though a
    // fresh collapse decision would classify it as collapsible.
    let mut collapse_cache = FingerprintCache::default();
    let collapse_report = plan_collapse(
        std::slice::from_ref(&source),
        &mut collapse_cache,
        &CollapseConfig::default(),
    )
    .unwrap();
    let mut reconcile_cache = FingerprintCache::default();
    let mut plan2 = reconcile_plan(
        &source,
        &dest,
        &mut reconcile_cache,
        &PreflightRules::default(),
    )
    .unwrap();
    apply_collapse_to_plan(&mut plan2, &collapse_report);
    let summary = transfer_run(&plan2, &RunOptions::default()).unwrap();

    assert_eq!(summary.copied, 0);
    assert!(
        dest.join("a.txt").exists(),
        "collapse must not delete an already-verified destination file"
    );
}
