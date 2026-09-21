mod common;

use common::TempTree;
use engine::scan::scan_root;

#[test]
fn finds_files_recursively_and_skips_directories() {
    let tree = TempTree::new();
    tree.write("a.txt", b"hello");
    tree.write("nested/b.txt", b"world!");
    tree.write("nested/deeper/c.txt", b"!");

    let mut entries = scan_root(tree.path()).expect("scan should succeed");
    entries.sort_by(|a, b| a.path.cmp(&b.path));

    assert_eq!(entries.len(), 3);
    assert!(entries
        .iter()
        .any(|e| e.path.ends_with("a.txt") && e.size == 5));
    assert!(entries
        .iter()
        .any(|e| e.path.ends_with("b.txt") && e.size == 6));
    assert!(entries
        .iter()
        .any(|e| e.path.ends_with("c.txt") && e.size == 1));
}

#[test]
fn empty_root_yields_no_entries() {
    let tree = TempTree::new();
    let entries = scan_root(tree.path()).expect("scan should succeed");
    assert!(entries.is_empty());
}

#[test]
fn nonexistent_root_scans_as_empty() {
    let tree = TempTree::new();
    let entries = scan_root(&tree.path().join("does-not-exist")).expect("scan should succeed");
    assert!(entries.is_empty());
}
