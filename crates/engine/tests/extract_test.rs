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
    assert_eq!(classify(Path::new("a.csv")), Some(DocumentKind::Csv));
    assert_eq!(classify(Path::new("a.tsv")), Some(DocumentKind::Tsv));
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
fn html_extraction_reads_the_full_raw_text_including_markup() {
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

    // The real prose words still overlap heavily with equivalent plain
    // text - HTML is not treated as a wholly different vocabulary - even
    // though the HTML version also carries extra tokens from its markup
    // ("html", "body", "h1", "p"), since tags are no longer stripped.
    let overlap = doc_html.tokens.intersection(&doc_text.tokens).count();
    assert!(
        overlap > 0,
        "expected overlap between HTML's raw text and equivalent plain text"
    );
}

#[test]
fn html_files_differing_only_in_an_attribute_value_are_not_reported_identical() {
    let tree = TempTree::new();
    // The real case this guards against: two files generated from the same
    // template, wired to different data purely via a src/href attribute -
    // a DOM-text-only extraction (which ignores attribute values entirely)
    // would see these as byte-for-byte identical "visible text" and report
    // a false jaccard_estimate of exactly 1.0, even though the files are
    // genuinely different (SPEC.md's near-duplicate detection must not
    // consider "the same template instantiated with different data" to be
    // the same file).
    let a = tree.write(
        "viewer_a.html",
        b"<html><body><script src=\"NetworkA.js\"> </script></body></html>",
    );
    let b = tree.write(
        "viewer_b.html",
        b"<html><body><script src=\"@NETWORK_NAME@.js\"> </script></body></html>",
    );

    let doc_a = extract(&a, DocumentKind::Html).unwrap();
    let doc_b = extract(&b, DocumentKind::Html).unwrap();

    assert_ne!(
        doc_a.tokens, doc_b.tokens,
        "attribute-only differences must still produce different token sets"
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

    let doc = extract(&path, DocumentKind::Csv).unwrap();
    // Two data rows (header is not treated specially since has_headers is
    // false; it is just another row).
    assert_eq!(doc.tokens.len(), 3);
}

#[test]
fn csv_with_a_shared_row_overlaps_partially() {
    let tree = TempTree::new();
    let a = tree.write("a.csv", b"alice,30\nbob,40\n");
    let b = tree.write("b.csv", b"alice,30\ncarol,50\n");

    let doc_a = extract(&a, DocumentKind::Csv).unwrap();
    let doc_b = extract(&b, DocumentKind::Csv).unwrap();

    assert_eq!(doc_a.tokens.intersection(&doc_b.tokens).count(), 1);
}

#[test]
fn tsv_rows_become_individual_tokens() {
    let tree = TempTree::new();
    let path = tree.write("a.tsv", b"name\tage\nalice\t30\nbob\t40\n");

    let doc = extract(&path, DocumentKind::Tsv).unwrap();
    assert_eq!(doc.tokens.len(), 3);
}

#[test]
fn malformed_csv_falls_back_to_plain_text_instead_of_erroring() {
    let tree = TempTree::new();
    // Invalid UTF-8 breaks csv::Reader's row parsing (it validates UTF-8
    // per field), but the file still has real word content worth comparing
    // - it should extract successfully via the plain-text fallback rather
    // than erroring or being dropped.
    let path = tree.write(
        "bad.csv",
        b"the quick brown fox jumps over the lazy dog\xFF,end\n",
    );
    let same_words_txt = tree.write(
        "same.txt",
        b"the quick brown fox jumps over the lazy dog end",
    );

    let doc_csv = extract(&path, DocumentKind::Csv).unwrap();
    let doc_txt = extract(&same_words_txt, DocumentKind::PlainText).unwrap();

    assert!(!doc_csv.tokens.is_empty());
    let overlap = doc_csv.tokens.intersection(&doc_txt.tokens).count();
    assert!(
        overlap > 0,
        "expected the fallback plain-text tokens to overlap with equivalent real text"
    );
}

#[test]
fn malformed_json_falls_back_to_plain_text_instead_of_erroring() {
    let tree = TempTree::new();
    // Missing closing brace - serde_json errors ("EOF while parsing an
    // object"), but the file still has real word content worth comparing.
    let path = tree.write(
        "bad.json",
        b"{\"words\": \"the quick brown fox jumps over the lazy dog\"",
    );
    let same_words_txt = tree.write(
        "same.txt",
        b"words the quick brown fox jumps over the lazy dog",
    );

    let doc_json = extract(&path, DocumentKind::Json).unwrap();
    let doc_txt = extract(&same_words_txt, DocumentKind::PlainText).unwrap();

    assert!(!doc_json.tokens.is_empty());
    let overlap = doc_json.tokens.intersection(&doc_txt.tokens).count();
    assert!(
        overlap > 0,
        "expected the fallback plain-text tokens to overlap with equivalent real text"
    );
}

#[test]
fn source_code_is_treated_as_shingled_text() {
    let tree = TempTree::new();
    let path = tree.write("a.rs", b"fn main() { println!(\"hello world\"); }");

    let doc = extract(&path, DocumentKind::SourceCode).unwrap();
    assert!(!doc.tokens.is_empty());
}
