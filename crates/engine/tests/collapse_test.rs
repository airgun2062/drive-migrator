mod common;

use std::collections::HashSet;

use common::TempTree;
use engine::{plan_collapse, CollapseAction, CollapseConfig, CollapseReason, FingerprintCache};

fn words(prefix: &str, count: usize) -> String {
    (0..count)
        .map(|i| format!("{prefix}{i}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn exact_duplicates_keep_one_copy_and_collapse_the_rest() {
    let tree = TempTree::new();
    tree.write("a.bin", b"identical content here");
    tree.write("b.bin", b"identical content here");
    tree.write("c.bin", b"identical content here");

    let mut cache = FingerprintCache::default();
    let report = plan_collapse(
        &[tree.path().to_path_buf()],
        &mut cache,
        &CollapseConfig::default(),
    )
    .unwrap();

    let kept: Vec<_> = report
        .decisions
        .iter()
        .filter(|d| d.action == CollapseAction::Keep)
        .collect();
    let collapsed: Vec<_> = report
        .decisions
        .iter()
        .filter(|d| d.action == CollapseAction::Collapse)
        .collect();

    assert_eq!(kept.len(), 1);
    assert_eq!(collapsed.len(), 2);
    assert!(collapsed
        .iter()
        .all(|d| d.reason == CollapseReason::ExactDuplicate));
    assert_eq!(report.files_kept, 1);
    assert_eq!(report.files_collapsed, 2);
    assert_eq!(
        report.bytes_saved,
        "identical content here".len() as u64 * 2
    );
}

#[test]
fn unrelated_files_are_always_kept() {
    let tree = TempTree::new();
    tree.write("a.txt", words("alpha", 30).as_bytes());
    tree.write("b.txt", words("beta", 30).as_bytes());

    let mut cache = FingerprintCache::default();
    let report = plan_collapse(
        &[tree.path().to_path_buf()],
        &mut cache,
        &CollapseConfig::default(),
    )
    .unwrap();

    assert_eq!(report.decisions.len(), 2);
    assert!(report
        .decisions
        .iter()
        .all(|d| d.action == CollapseAction::Keep));
    assert!(report
        .decisions
        .iter()
        .all(|d| d.reason == CollapseReason::Kept));
}

#[test]
fn containment_family_keeps_the_more_complete_version_as_a_tip_and_collapses_the_other() {
    let tree = TempTree::new();
    let a_content = words("core", 30);
    let b_content = format!("{a_content} {}", words("extra", 12));
    tree.write("a.txt", a_content.as_bytes());
    tree.write("b.txt", b_content.as_bytes());

    let mut cache = FingerprintCache::default();
    let config = CollapseConfig {
        keep_newest: 1,
        ..CollapseConfig::default()
    };
    let report = plan_collapse(&[tree.path().to_path_buf()], &mut cache, &config).unwrap();

    assert_eq!(report.version_report.families.len(), 1);

    let kept: Vec<_> = report
        .decisions
        .iter()
        .filter(|d| d.action == CollapseAction::Keep)
        .collect();
    let collapsed: Vec<_> = report
        .decisions
        .iter()
        .filter(|d| d.action != CollapseAction::Keep)
        .collect();
    assert_eq!(kept.len(), 1);
    assert_eq!(collapsed.len(), 1);
    assert_eq!(kept[0].reason, CollapseReason::Tip);
    assert_eq!(collapsed[0].reason, CollapseReason::OlderVersion);
    assert_eq!(collapsed[0].action, CollapseAction::Collapse);
}

#[test]
fn archive_option_archives_instead_of_dropping() {
    let tree = TempTree::new();
    let a_content = words("core", 30);
    let b_content = format!("{a_content} {}", words("extra", 12));
    tree.write("a.txt", a_content.as_bytes());
    tree.write("b.txt", b_content.as_bytes());
    tree.write("dup1.bin", b"exact dup content");
    tree.write("dup2.bin", b"exact dup content");

    let mut cache = FingerprintCache::default();
    let config = CollapseConfig {
        archive: true,
        ..CollapseConfig::default()
    };
    let report = plan_collapse(&[tree.path().to_path_buf()], &mut cache, &config).unwrap();

    let archived: Vec<_> = report
        .decisions
        .iter()
        .filter(|d| d.action == CollapseAction::Archive)
        .collect();
    assert_eq!(archived.len(), 2); // one exact-dup copy, one older version
    assert!(report
        .decisions
        .iter()
        .all(|d| d.action != CollapseAction::Collapse));
}

#[test]
fn protected_extension_is_kept_even_when_it_would_otherwise_collapse() {
    let tree = TempTree::new();
    let a_content = words("core", 30);
    let b_content = format!("{a_content} {}", words("extra", 12));
    // The less-complete member gets the protected extension; without
    // protection it would collapse as an older version.
    tree.write("a.rtf", a_content.as_bytes());
    tree.write("b.rtf", b_content.as_bytes());

    let mut cache = FingerprintCache::default();
    let config = CollapseConfig {
        protected_extensions: HashSet::from(["rtf".to_string()]),
        ..CollapseConfig::default()
    };
    let report = plan_collapse(&[tree.path().to_path_buf()], &mut cache, &config).unwrap();

    assert!(report
        .decisions
        .iter()
        .all(|d| d.action == CollapseAction::Keep));
    assert!(report
        .decisions
        .iter()
        .any(|d| d.reason == CollapseReason::Protected));
}

#[test]
fn keep_newest_n_greater_than_one_keeps_more_members() {
    let tree = TempTree::new();
    let a_content = words("core", 30);
    let b_content = format!("{a_content} {}", words("extra", 12));
    tree.write("a.txt", a_content.as_bytes());
    tree.write("b.txt", b_content.as_bytes());

    let mut cache = FingerprintCache::default();
    let config = CollapseConfig {
        keep_newest: 2,
        ..CollapseConfig::default()
    };
    let report = plan_collapse(&[tree.path().to_path_buf()], &mut cache, &config).unwrap();

    // With N=2 and only 2 members, both are within keep-newest-N.
    assert!(report
        .decisions
        .iter()
        .all(|d| d.action == CollapseAction::Keep));
}
