use std::collections::HashSet;
use std::path::Path;

use crate::error::{EngineError, Result};

/// Word shingle size for text-like documents (SPEC.md section 4, T3).
const SHINGLE_SIZE: usize = 5;

/// Formats supported for similarity in this pass. SPEC.md section 4 also
/// lists docx, pptx, rtf, xlsx, xml, and yaml/yml; those need extra crates
/// (zip/quick-xml, calamine) and are deferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    PlainText,
    Html,
    Json,
    DelimitedRows,
    SourceCode,
}

const SOURCE_CODE_EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "ts", "tsx", "jsx", "go", "java", "kt", "c", "h", "cpp", "hpp", "cc", "cs",
    "rb", "php", "sh", "bash", "swift", "scala", "m", "mm",
];

/// Determines how (or whether) a file participates in similarity detection,
/// based on its extension. `None` means the file is not supported yet.
pub fn classify(path: &Path) -> Option<DocumentKind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "txt" | "md" => Some(DocumentKind::PlainText),
        "html" | "htm" => Some(DocumentKind::Html),
        "json" => Some(DocumentKind::Json),
        "csv" | "tsv" => Some(DocumentKind::DelimitedRows),
        _ if SOURCE_CODE_EXTENSIONS.contains(&ext.as_str()) => Some(DocumentKind::SourceCode),
        _ => None,
    }
}

/// A document reduced to the unit SPEC.md section 5 calls a "chunk": word
/// shingle hashes for text-like documents, one hash per row for delimited
/// data. Everything downstream (MinHash, coverage) works on this uniformly.
#[derive(Debug, Clone)]
pub struct ExtractedDocument {
    pub kind: DocumentKind,
    pub tokens: HashSet<u64>,
}

pub fn extract(path: &Path, kind: DocumentKind) -> Result<ExtractedDocument> {
    let tokens = match kind {
        DocumentKind::PlainText | DocumentKind::SourceCode => shingle_tokens(&read_text(path)?),
        DocumentKind::Html => shingle_tokens(&extract_html_text(path)?),
        DocumentKind::Json => shingle_tokens(&canonical_json_text(path)?),
        DocumentKind::DelimitedRows => row_tokens(path)?,
    };
    Ok(ExtractedDocument { kind, tokens })
}

fn read_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).map_err(|source| EngineError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn extract_html_text(path: &Path) -> Result<String> {
    let html = read_text(path)?;
    let document = scraper::Html::parse_document(&html);
    Ok(document.root_element().text().collect::<Vec<_>>().join(" "))
}

fn canonical_json_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).map_err(|source| EngineError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|source| EngineError::JsonParse {
            path: path.to_path_buf(),
            source,
        })?;
    let canonical = serde_json::to_vec(&value).map_err(|source| EngineError::JsonSerialize {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(String::from_utf8_lossy(&canonical).into_owned())
}

fn row_tokens(path: &Path) -> Result<HashSet<u64>> {
    let bytes = std::fs::read(path).map_err(|source| EngineError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let delimiter = if path.extension().and_then(|e| e.to_str()) == Some("tsv") {
        b'\t'
    } else {
        b','
    };
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .flexible(true)
        .from_reader(bytes.as_slice());

    let mut tokens = HashSet::new();
    for result in reader.records() {
        let record = result.map_err(|source| EngineError::CsvParse {
            path: path.to_path_buf(),
            source,
        })?;
        let row_text: String = record.iter().collect::<Vec<_>>().join("\u{1f}");
        tokens.insert(hash_str(&row_text));
    }
    Ok(tokens)
}

/// Splits `text` into lowercase words and hashes every consecutive
/// `SHINGLE_SIZE`-word window. Shorter documents hash as a single token
/// rather than producing no tokens at all.
fn shingle_tokens(text: &str) -> HashSet<u64> {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect();

    if words.is_empty() {
        return HashSet::new();
    }
    if words.len() < SHINGLE_SIZE {
        return HashSet::from([hash_str(&words.join(" "))]);
    }

    words
        .windows(SHINGLE_SIZE)
        .map(|w| hash_str(&w.join(" ")))
        .collect()
}

fn hash_str(s: &str) -> u64 {
    let hash = blake3::hash(s.as_bytes());
    let bytes = hash.as_bytes();
    let mut value = 0u64;
    for &b in &bytes[..8] {
        value = (value << 8) | b as u64;
    }
    value
}
