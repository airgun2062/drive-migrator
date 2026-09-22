mod common;

use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use common::TempTree;
use engine::{best_available_date, DateSource};

fn write_zip_entries(path: &Path, entries: &[(&str, String)]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let file = fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
    for (name, content) in entries {
        zip.start_file(*name, options).unwrap();
        zip.write_all(content.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}

fn far_future() -> SystemTime {
    // A filesystem-mtime stand-in that would sort after any real internal
    // metadata date, so tests can tell whether the internal date was
    // actually used instead of silently falling back.
    UNIX_EPOCH + std::time::Duration::from_secs(4_000_000_000)
}

#[test]
fn docx_uses_the_modified_core_property_over_the_filesystem_time() {
    let tree = TempTree::new();
    let path = tree.path().join("a.docx");
    let core_xml = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" \
        xmlns:dcterms=\"http://purl.org/dc/terms/\">\
        <dcterms:created>2020-01-01T00:00:00Z</dcterms:created>\
        <dcterms:modified>2024-06-15T12:00:00Z</dcterms:modified>\
        </cp:coreProperties>";
    write_zip_entries(
        &path,
        &[
            ("docProps/core.xml", core_xml.to_string()),
            ("word/document.xml", "<w:document/>".to_string()),
        ],
    );

    let dated = best_available_date(&path, far_future());
    assert_eq!(dated.source, DateSource::OfficeMetadata);
    // 2024-06-15T12:00:00Z, well before the far_future() stand-in.
    assert!(dated.unix_millis > 1_700_000_000_000);
    assert!(dated.unix_millis < 1_720_000_000_000);
}

#[test]
fn docx_falls_back_to_created_when_modified_is_absent() {
    let tree = TempTree::new();
    let path = tree.path().join("a.docx");
    let core_xml = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <cp:coreProperties xmlns:dcterms=\"http://purl.org/dc/terms/\">\
        <dcterms:created>2020-01-01T00:00:00Z</dcterms:created>\
        </cp:coreProperties>";
    write_zip_entries(&path, &[("docProps/core.xml", core_xml.to_string())]);

    let dated = best_available_date(&path, far_future());
    assert_eq!(dated.source, DateSource::OfficeMetadata);
    // 2020-01-01, well before the 2024ish window used above.
    assert!(dated.unix_millis < 1_600_000_000_000);
}

#[test]
fn docx_without_core_properties_falls_back_to_filesystem_time() {
    let tree = TempTree::new();
    let path = tree.path().join("a.docx");
    write_zip_entries(&path, &[("word/document.xml", "<w:document/>".to_string())]);

    let stand_in = far_future();
    let dated = best_available_date(&path, stand_in);
    assert_eq!(dated.source, DateSource::FilesystemModified);
    assert_eq!(
        dated.unix_millis,
        stand_in.duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
    );
}

#[test]
fn eml_uses_the_date_header_over_the_filesystem_time() {
    let tree = TempTree::new();
    let eml = "From: a@example.test\r\nTo: b@example.test\r\n\
               Subject: test\r\nDate: Mon, 1 Jan 2024 00:00:00 +0000\r\n\r\nbody\r\n";
    let path = tree.write("a.eml", eml.as_bytes());

    let dated = best_available_date(&path, far_future());
    assert_eq!(dated.source, DateSource::EmailHeader);
    // 2024-01-01T00:00:00Z.
    assert_eq!(dated.unix_millis, 1_704_067_200_000);
}

#[test]
fn eml_without_a_date_header_falls_back_to_filesystem_time() {
    let tree = TempTree::new();
    let path = tree.write("a.eml", b"From: a@example.test\r\n\r\nbody\r\n");

    let stand_in = far_future();
    let dated = best_available_date(&path, stand_in);
    assert_eq!(dated.source, DateSource::FilesystemModified);
}

#[test]
fn plain_text_always_uses_the_filesystem_time() {
    let tree = TempTree::new();
    let path = tree.write("a.txt", b"hello");

    let stand_in = far_future();
    let dated = best_available_date(&path, stand_in);
    assert_eq!(dated.source, DateSource::FilesystemModified);
    assert_eq!(
        dated.unix_millis,
        stand_in.duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
    );
}
