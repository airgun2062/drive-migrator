mod common;

use std::fs;
use std::path::Path;

use common::TempTree;
use engine::{check_integrity, verify, write_manifest};

fn clear_readonly(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
    }
    #[cfg(not(unix))]
    {
        let mut perms = fs::metadata(path).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        fs::set_permissions(path, perms).unwrap();
    }
}

#[test]
fn write_manifest_creates_all_expected_files_and_protects_them() {
    let tree = TempTree::new();
    tree.write("dest/docs/report.txt", b"hello manifest");

    let dest = tree.path().join("dest");
    let result = write_manifest(&dest, None).unwrap();

    assert_eq!(result.file_count, 1);
    assert_eq!(result.total_bytes, "hello manifest".len() as u64);
    assert_eq!(result.duplicate_group_count, 0);

    let visible = dest.join("MIGRATION_MANIFEST.json");
    let checksum = dest.join("MIGRATION_MANIFEST.json.sha256");
    let db = dest.join(".migrator/manifest.db");
    let backup = dest.join(".migrator/manifest.backup.json");
    let backup_checksum = dest.join(".migrator/manifest.backup.json.sha256");

    for path in [&visible, &checksum, &db, &backup, &backup_checksum] {
        assert!(path.exists(), "expected {path:?} to exist");
    }

    assert!(fs::metadata(&visible).unwrap().permissions().readonly());
    assert!(fs::metadata(&backup).unwrap().permissions().readonly());

    let checksum_contents = fs::read_to_string(&checksum).unwrap();
    assert_eq!(checksum_contents, result.sha256);
}

#[test]
fn write_manifest_detects_duplicate_content() {
    let tree = TempTree::new();
    tree.write("dest/a.txt", b"same content");
    tree.write("dest/copy.txt", b"same content");
    tree.write("dest/unique.txt", b"different");

    let result = write_manifest(&tree.path().join("dest"), None).unwrap();

    assert_eq!(result.file_count, 3);
    assert_eq!(result.duplicate_group_count, 1);
}

#[test]
fn write_manifest_is_idempotent_and_does_not_count_its_own_artifacts() {
    let tree = TempTree::new();
    tree.write("dest/a.txt", b"content");

    let dest = tree.path().join("dest");
    let first = write_manifest(&dest, None).unwrap();
    let second = write_manifest(&dest, None).unwrap();

    assert_eq!(first.file_count, 1);
    assert_eq!(second.file_count, 1);
}

#[test]
fn integrity_is_trusted_immediately_after_a_write() {
    let tree = TempTree::new();
    tree.write("dest/a.txt", b"content");

    let dest = tree.path().join("dest");
    write_manifest(&dest, None).unwrap();

    let integrity = check_integrity(&dest);
    assert!(integrity.trusted);
    assert!(integrity.visible_hash.is_some());
    assert_eq!(integrity.visible_hash, integrity.backup_hash);
}

#[test]
fn tampering_the_visible_manifest_is_caught_against_the_trusted_majority() {
    let tree = TempTree::new();
    tree.write("dest/a.txt", b"content");

    let dest = tree.path().join("dest");
    let result = write_manifest(&dest, None).unwrap();

    let visible = dest.join("MIGRATION_MANIFEST.json");
    clear_readonly(&visible);
    fs::write(&visible, br#"{"tampered": true}"#).unwrap();

    let integrity = check_integrity(&dest);
    // Backup and app-local still agree, so the system still knows the
    // correct value...
    assert!(integrity.trusted);
    // ...but the visible copy specifically no longer matches it.
    assert_ne!(integrity.visible_hash, Some(result.sha256));
    assert_eq!(integrity.backup_hash, integrity.app_local_hash);
}

#[test]
fn integrity_is_untrusted_with_fewer_than_two_available_copies() {
    let tree = TempTree::new();
    tree.write("dest/a.txt", b"content");

    let dest = tree.path().join("dest");
    write_manifest(&dest, None).unwrap();

    let visible = dest.join("MIGRATION_MANIFEST.json");
    let backup = dest.join(".migrator/manifest.backup.json");
    clear_readonly(&visible);
    clear_readonly(&backup);
    fs::remove_file(&visible).unwrap();
    fs::remove_file(&backup).unwrap();

    // Only the app-local copy remains; one data point cannot form a majority.
    let integrity = check_integrity(&dest);
    assert!(!integrity.trusted);
}

#[test]
fn verify_reports_unchanged_files_right_after_a_write() {
    let tree = TempTree::new();
    tree.write("dest/a.txt", b"content");

    let dest = tree.path().join("dest");
    write_manifest(&dest, None).unwrap();

    let report = verify(&dest).unwrap();
    assert_eq!(report.unchanged, 1);
    assert!(report.changed.is_empty());
    assert!(report.moved.is_empty());
    assert!(report.deleted.is_empty());
    assert!(report.unrecorded.is_empty());
}

#[test]
fn verify_reports_a_changed_file() {
    let tree = TempTree::new();
    tree.write("dest/a.txt", b"original content");

    let dest = tree.path().join("dest");
    write_manifest(&dest, None).unwrap();

    fs::write(dest.join("a.txt"), b"edited content").unwrap();

    let report = verify(&dest).unwrap();
    assert_eq!(report.changed.len(), 1);
    assert_eq!(report.changed[0].relative_path, Path::new("a.txt"));
    assert_eq!(report.unchanged, 0);
}

#[test]
fn verify_reports_a_deleted_file() {
    let tree = TempTree::new();
    tree.write("dest/a.txt", b"content");

    let dest = tree.path().join("dest");
    write_manifest(&dest, None).unwrap();

    fs::remove_file(dest.join("a.txt")).unwrap();

    let report = verify(&dest).unwrap();
    assert_eq!(report.deleted, vec![Path::new("a.txt").to_path_buf()]);
}

#[test]
fn verify_reports_a_moved_file() {
    let tree = TempTree::new();
    tree.write("dest/old_name.txt", b"content that will move");

    let dest = tree.path().join("dest");
    write_manifest(&dest, None).unwrap();

    fs::rename(dest.join("old_name.txt"), dest.join("new_name.txt")).unwrap();

    let report = verify(&dest).unwrap();
    assert_eq!(report.moved.len(), 1);
    assert_eq!(report.moved[0].recorded_path, Path::new("old_name.txt"));
    assert_eq!(report.moved[0].current_path, Path::new("new_name.txt"));
    assert!(report.deleted.is_empty());
    assert!(report.unrecorded.is_empty());
}

#[test]
fn verify_reports_an_unrecorded_file() {
    let tree = TempTree::new();
    tree.write("dest/a.txt", b"content");

    let dest = tree.path().join("dest");
    write_manifest(&dest, None).unwrap();

    tree.write("dest/new_since_manifest.txt", b"added later");

    let report = verify(&dest).unwrap();
    assert_eq!(
        report.unrecorded,
        vec![Path::new("new_since_manifest.txt").to_path_buf()]
    );
    assert_eq!(report.unchanged, 1);
}
