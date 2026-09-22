mod common;

use std::path::Path;

use common::TempTree;
use engine::extract::{classify, extract, DocumentKind};

#[test]
fn classifies_supported_extensions() {
    assert_eq!(classify(Path::new("a.txt")), Some(DocumentKind::PlainText));
    assert_eq!(classify(Path::new("a.md")), Some(DocumentKind::PlainText));
    assert_eq!(classify(Path::new("a.html")), Some(DocumentKind::Html));
    assert_eq!(classify(Path::new("a.HTM")), Some(DocumentKind::Html));
    assert_eq!(classify(Path::new("a.json")), Some(DocumentKind::Json));
    assert_eq!(
        classify(Path::new("a.csv")),
        Some(DocumentKind::DelimitedRows)
    );
    assert_eq!(
        classify(Path::new("a.tsv")),
        Some(DocumentKind::DelimitedRows)
    );
    assert_eq!(classify(Path::new("a.rs")), Some(DocumentKind::SourceCode));
    assert_eq!(classify(Path::new("a.py")), Some(DocumentKind::SourceCode));
}

#[test]
fn unsupported_extensions_and_extensionless_files_are_not_classified() {
    assert_eq!(classify(Path::new("a.pdf")), None);
    assert_eq!(classify(Path::new("a.xls")), None);
    assert_eq!(classify(Path::new("a.doc")), None);
    assert_eq!(classify(Path::new("no_extension")), None);
}

#[test]
fn identical_text_files_produce_identical_tokens() {
    let tree = TempTree::new();
    let a = tree.write("a.txt", b"the quick brown fox jumps over the lazy dog");
    let b = tree.write("b.txt", b"the quick brown fox jumps over the lazy dog");

    let doc_a = extract(&a, DocumentKind::PlainText).unwrap();
    let doc_b = extract(&b, DocumentKind::PlainText).unwrap();

    assert!(!doc_a.tokens.is_empty());
    assert_eq!(doc_a.tokens, doc_b.tokens);
}

#[test]
fn different_text_files_produce_different_tokens() {
    let tree = TempTree::new();
    let a = tree.write("a.txt", b"the quick brown fox jumps over the lazy dog");
    let b = tree.write(
        "b.txt",
        b"a completely unrelated sentence about something else entirely",
    );

    let doc_a = extract(&a, DocumentKind::PlainText).unwrap();
    let doc_b = extract(&b, DocumentKind::PlainText).unwrap();

    assert!(doc_a.tokens.intersection(&doc_b.tokens).next().is_none());
}

#[test]
fn html_extraction_strips_tags() {
    let tree = TempTree::new();
    let html = tree.write(
        "a.html",
        b"<html><body><h1>Report Title</h1><p>the quick brown fox jumps over the lazy dog</p></body></html>",
    );
    let text_only = tree.write(
        "b.txt",
        b"Report Title the quick brown fox jumps over the lazy dog",
    );

    let doc_html = extract(&html, DocumentKind::Html).unwrap();
    let doc_text = extract(&text_only, DocumentKind::PlainText).unwrap();

    // Word-shingle tokens should overlap heavily once tags are stripped,
    // even though the raw bytes are very different.
    let overlap = doc_html.tokens.intersection(&doc_text.tokens).count();
    assert!(
        overlap > 0,
        "expected overlap between stripped HTML and equivalent plain text"
    );
}

#[test]
fn json_canonicalization_ignores_key_order_and_whitespace() {
    let tree = TempTree::new();
    let a = tree.write("a.json", b"{\"b\": 2, \"a\": 1}");
    let b = tree.write("b.json", b"{\n  \"a\":   1,\n  \"b\": 2\n}\n");

    let doc_a = extract(&a, DocumentKind::Json).unwrap();
    let doc_b = extract(&b, DocumentKind::Json).unwrap();

    assert_eq!(doc_a.tokens, doc_b.tokens);
}

#[test]
fn csv_rows_become_individual_tokens() {
    let tree = TempTree::new();
    let path = tree.write("a.csv", b"name,age\nalice,30\nbob,40\n");

    let doc = extract(&path, DocumentKind::DelimitedRows).unwrap();
    // Two data rows (header is not treated specially since has_headers is
    // false; it is just another row).
    assert_eq!(doc.tokens.len(), 3);
}

#[test]
fn csv_with_a_shared_row_overlaps_partially() {
    let tree = TempTree::new();
    let a = tree.write("a.csv", b"alice,30\nbob,40\n");
    let b = tree.write("b.csv", b"alice,30\ncarol,50\n");

    let doc_a = extract(&a, DocumentKind::DelimitedRows).unwrap();
    let doc_b = extract(&b, DocumentKind::DelimitedRows).unwrap();

    assert_eq!(doc_a.tokens.intersection(&doc_b.tokens).count(), 1);
}

#[test]
fn source_code_is_treated_as_shingled_text() {
    let tree = TempTree::new();
    let path = tree.write("a.rs", b"fn main() { println!(\"hello world\"); }");

    let doc = extract(&path, DocumentKind::SourceCode).unwrap();
    assert!(!doc.tokens.is_empty());
}
