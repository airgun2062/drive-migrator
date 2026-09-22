mod common;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use common::TempTree;
use engine::extract::{classify, extract, DocumentKind};

fn write_zip_entries(path: &Path, entries: &[(String, String)]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let file = fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
    for (name, content) in entries {
        zip.start_file(name.as_str(), options).unwrap();
        zip.write_all(content.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}

fn write_docx(dir: &Path, file_name: &str, paragraphs: &[&str]) -> PathBuf {
    let body: String = paragraphs
        .iter()
        .map(|p| format!("<w:p><w:r><w:t>{p}</w:t></w:r></w:p>"))
        .collect();
    let document_xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
         <w:body>{body}</w:body></w:document>"
    );
    let path = dir.join(file_name);
    write_zip_entries(&path, &[("word/document.xml".to_string(), document_xml)]);
    path
}

fn write_pptx(dir: &Path, file_name: &str, slides: &[&str]) -> PathBuf {
    let entries: Vec<(String, String)> = slides
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let name = format!("ppt/slides/slide{}.xml", i + 1);
            let xml = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
                 <p:sld xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
                 xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\">\
                 <p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp>\
                 </p:spTree></p:cSld></p:sld>"
            );
            (name, xml)
        })
        .collect();
    let path = dir.join(file_name);
    write_zip_entries(&path, &entries);
    path
}

fn write_xlsx(dir: &Path, file_name: &str, rows: &[[&str; 2]]) -> PathBuf {
    let content_types = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
        <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
        <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
        <Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
        <Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>\
        </Types>".to_string();

    let root_rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
        <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/>\
        </Relationships>".to_string();

    let workbook_xml = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
        xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
        <sheets><sheet name=\"Sheet1\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>"
        .to_string();

    let workbook_rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
        <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/>\
        </Relationships>".to_string();

    let mut rows_xml = String::new();
    for (i, [a, b]) in rows.iter().enumerate() {
        let r = i + 1;
        rows_xml.push_str(&format!(
            "<row r=\"{r}\"><c r=\"A{r}\" t=\"inlineStr\"><is><t>{a}</t></is></c>\
             <c r=\"B{r}\" t=\"inlineStr\"><is><t>{b}</t></is></c></row>"
        ));
    }
    let sheet_xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
         <sheetData>{rows_xml}</sheetData></worksheet>"
    );

    let path = dir.join(file_name);
    write_zip_entries(
        &path,
        &[
            ("[Content_Types].xml".to_string(), content_types),
            ("_rels/.rels".to_string(), root_rels),
            ("xl/workbook.xml".to_string(), workbook_xml),
            ("xl/_rels/workbook.xml.rels".to_string(), workbook_rels),
            ("xl/worksheets/sheet1.xml".to_string(), sheet_xml),
        ],
    );
    path
}

#[test]
fn classifies_office_and_markup_extensions() {
    assert_eq!(classify(Path::new("a.docx")), Some(DocumentKind::Docx));
    assert_eq!(classify(Path::new("a.pptx")), Some(DocumentKind::Pptx));
    assert_eq!(classify(Path::new("a.xlsx")), Some(DocumentKind::Xlsx));
    assert_eq!(classify(Path::new("a.xml")), Some(DocumentKind::Xml));
    assert_eq!(classify(Path::new("a.yaml")), Some(DocumentKind::Yaml));
    assert_eq!(classify(Path::new("a.yml")), Some(DocumentKind::Yaml));
    assert_eq!(classify(Path::new("a.rtf")), Some(DocumentKind::Rtf));
}

#[test]
fn docx_extraction_reads_paragraph_text() {
    let tree = TempTree::new();
    let path = write_docx(
        tree.path(),
        "a.docx",
        &["the quick brown fox jumps over", "the lazy dog again today"],
    );

    let doc = extract(&path, DocumentKind::Docx).unwrap();
    assert!(!doc.tokens.is_empty());
}

#[test]
fn docx_files_with_the_same_text_produce_the_same_tokens() {
    let tree = TempTree::new();
    let a = write_docx(
        tree.path(),
        "a.docx",
        &["the quick brown fox jumps over the lazy dog"],
    );
    let b = write_docx(
        tree.path(),
        "b.docx",
        &["the quick brown fox jumps over the lazy dog"],
    );

    let doc_a = extract(&a, DocumentKind::Docx).unwrap();
    let doc_b = extract(&b, DocumentKind::Docx).unwrap();
    assert_eq!(doc_a.tokens, doc_b.tokens);
}

#[test]
fn pptx_extraction_reads_text_from_every_slide_in_order() {
    let tree = TempTree::new();
    let path = write_pptx(
        tree.path(),
        "a.pptx",
        &[
            "quarterly revenue growth expenses forecast",
            "appendix charts data tables detail region",
        ],
    );

    let doc = extract(&path, DocumentKind::Pptx).unwrap();
    assert!(!doc.tokens.is_empty());

    let single_slide = write_pptx(
        tree.path(),
        "b.pptx",
        &["quarterly revenue growth expenses forecast"],
    );
    let doc_single = extract(&single_slide, DocumentKind::Pptx).unwrap();

    // The two-slide deck's tokens are a superset of the one-slide deck's,
    // since the first slide's text is identical.
    assert!(doc_single.tokens.is_subset(&doc.tokens));
    assert!(doc.tokens.len() > doc_single.tokens.len());
}

#[test]
fn xlsx_rows_become_individual_tokens() {
    let tree = TempTree::new();
    let path = write_xlsx(tree.path(), "a.xlsx", &[["alice", "30"], ["bob", "40"]]);

    let doc = extract(&path, DocumentKind::Xlsx).unwrap();
    assert_eq!(doc.tokens.len(), 2);
}

#[test]
fn xlsx_with_a_shared_row_overlaps_partially() {
    let tree = TempTree::new();
    let a = write_xlsx(tree.path(), "a.xlsx", &[["alice", "30"], ["bob", "40"]]);
    let b = write_xlsx(tree.path(), "b.xlsx", &[["alice", "30"], ["carol", "50"]]);

    let doc_a = extract(&a, DocumentKind::Xlsx).unwrap();
    let doc_b = extract(&b, DocumentKind::Xlsx).unwrap();
    assert_eq!(doc_a.tokens.intersection(&doc_b.tokens).count(), 1);
}

#[test]
fn xml_extraction_reads_text_and_ignores_attributes() {
    let tree = TempTree::new();
    let path = tree.write(
        "a.xml",
        b"<root><item priority=\"high\">hello world foo bar baz</item><meta ignore=\"true\"/></root>",
    );

    let doc = extract(&path, DocumentKind::Xml).unwrap();
    assert!(!doc.tokens.is_empty());

    let expected = tree.write("b.txt", b"hello world foo bar baz");
    let doc_expected = extract(&expected, DocumentKind::PlainText).unwrap();
    assert_eq!(doc.tokens, doc_expected.tokens);
}

#[test]
fn yaml_canonicalization_ignores_key_order_and_formatting() {
    let tree = TempTree::new();
    let a = tree.write("a.yaml", b"b: 2\na: 1\n");
    let b = tree.write("b.yaml", b"a:   1\nb:    2\n");

    let doc_a = extract(&a, DocumentKind::Yaml).unwrap();
    let doc_b = extract(&b, DocumentKind::Yaml).unwrap();
    assert_eq!(doc_a.tokens, doc_b.tokens);
}

#[test]
fn yaml_with_different_content_produces_different_tokens() {
    let tree = TempTree::new();
    let a = tree.write("a.yaml", b"name: alice\nrole: engineer\n");
    let b = tree.write("b.yaml", b"name: bob\nrole: designer\n");

    let doc_a = extract(&a, DocumentKind::Yaml).unwrap();
    let doc_b = extract(&b, DocumentKind::Yaml).unwrap();
    assert_ne!(doc_a.tokens, doc_b.tokens);
}

#[test]
fn rtf_extraction_strips_control_words_and_keeps_body_text() {
    let tree = TempTree::new();
    let rtf = br#"{\rtf1\ansi\deff0
{\fonttbl{\f0 Calibri;}}
{\*\generator Riched20 10.0.19041}
\viewkind4\uc1
\pard\sa200\sl276\slmult1\f0\fs22
The quick brown fox jumps over the lazy dog\par
Second paragraph of real content here today\par
}"#;
    let path = tree.write("a.rtf", rtf);

    let doc = extract(&path, DocumentKind::Rtf).unwrap();
    assert!(!doc.tokens.is_empty());

    let body_only = tree.write(
        "b.txt",
        b"The quick brown fox jumps over the lazy dog\nSecond paragraph of real content here today",
    );
    let doc_body = extract(&body_only, DocumentKind::PlainText).unwrap();

    // The recovered text should overlap heavily with the real body content.
    let overlap = doc.tokens.intersection(&doc_body.tokens).count();
    assert!(
        overlap > 0,
        "expected overlap between stripped RTF and its body text"
    );

    // And font table / generator metadata should not leak into the tokens.
    let noise = tree.write("c.txt", b"Calibri Riched20 fonttbl generator");
    let doc_noise = extract(&noise, DocumentKind::PlainText).unwrap();
    assert!(doc.tokens.intersection(&doc_noise.tokens).next().is_none());
}
