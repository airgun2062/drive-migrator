use std::collections::HashSet;
use std::io::Read as _;
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::error::{EngineError, Result};

/// Word shingle size for text-like documents (SPEC.md section 4, T3).
const SHINGLE_SIZE: usize = 5;

/// Formats supported for similarity. docx/pptx/xlsx/xml/yaml/rtf extraction
/// is a good-enough recovery of the visible text, not a full parse of each
/// format's semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    PlainText,
    Html,
    Json,
    Csv,
    Tsv,
    SourceCode,
    Docx,
    Pptx,
    Xlsx,
    Xml,
    Yaml,
    Rtf,
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
        "csv" => Some(DocumentKind::Csv),
        "tsv" => Some(DocumentKind::Tsv),
        "docx" => Some(DocumentKind::Docx),
        "pptx" => Some(DocumentKind::Pptx),
        "xlsx" => Some(DocumentKind::Xlsx),
        "xml" => Some(DocumentKind::Xml),
        "yaml" | "yml" => Some(DocumentKind::Yaml),
        "rtf" => Some(DocumentKind::Rtf),
        _ if SOURCE_CODE_EXTENSIONS.contains(&ext.as_str()) => Some(DocumentKind::SourceCode),
        _ => None,
    }
}

/// A document reduced to the unit SPEC.md section 5 calls a "chunk": word
/// shingle hashes for text-like documents, one hash per row for delimited
/// data (csv/tsv/xlsx). Everything downstream (MinHash, coverage) works on
/// this uniformly.
#[derive(Debug, Clone)]
pub struct ExtractedDocument {
    pub kind: DocumentKind,
    pub tokens: HashSet<u64>,
    /// Embedded media (images, etc.) hashed independently of the text
    /// tokens above, so two documents that score a perfect text match can
    /// still be told apart by what they actually embed. Always empty for
    /// non-container formats; may also be empty for docx/pptx/xlsx with no
    /// embedded media.
    pub media_hashes: Vec<blake3::Hash>,
}

/// Routes to exactly one handler per `DocumentKind` - Rust's idiomatic
/// answer to "pick the right handler for this file's type": an exhaustive
/// match over a closed enum, checked at compile time, rather than a
/// trait-object factory (there's no dynamic dispatch here, and none is
/// needed - every kind is known up front).
pub fn extract(path: &Path, kind: DocumentKind) -> Result<ExtractedDocument> {
    let (tokens, media_hashes) = match kind {
        DocumentKind::PlainText | DocumentKind::SourceCode => {
            (similarity_default(path)?, Vec::new())
        }
        DocumentKind::Html => (similarity_html(path)?, Vec::new()),
        DocumentKind::Json => (similarity_json(path)?, Vec::new()),
        DocumentKind::Csv => (similarity_csv(path)?, Vec::new()),
        DocumentKind::Tsv => (similarity_tsv(path)?, Vec::new()),
        DocumentKind::Docx => (
            similarity_docx(path)?,
            collect_media_hashes(path, "word/media/")?,
        ),
        DocumentKind::Pptx => (
            similarity_pptx(path)?,
            collect_media_hashes(path, "ppt/media/")?,
        ),
        DocumentKind::Xlsx => (
            similarity_xlsx(path)?,
            collect_media_hashes(path, "xl/media/")?,
        ),
        DocumentKind::Xml => (similarity_xml(path)?, Vec::new()),
        DocumentKind::Yaml => (similarity_yaml(path)?, Vec::new()),
        DocumentKind::Rtf => (similarity_rtf(path)?, Vec::new()),
    };
    Ok(ExtractedDocument {
        kind,
        tokens,
        media_hashes,
    })
}

/// Shared fallback: plain-text shingling. Used directly for `PlainText`/
/// `SourceCode`, and as the fallback for any structured format whose
/// specific parse fails below - see `similarity_csv`/`similarity_tsv`/
/// `similarity_json`/`similarity_yaml`.
fn similarity_default(path: &Path) -> Result<HashSet<u64>> {
    Ok(shingle_tokens(&read_text(path)?))
}

/// Reads the file's full raw content as text - not just the DOM's visible
/// text nodes - so a difference that lives entirely in an attribute value
/// (`<script src="PPINetwork.js">` vs `<script src="@NETWORK_NAME@.js">`,
/// for instance - files generated from the same template but wired to
/// different data) is still comparable. Tag/attribute syntax (`<`, `>`,
/// `=`, `"`, `/`) is not special-cased; `shingle_tokens` already splits on
/// every non-alphanumeric character, so `<div class="foo">` naturally
/// tokenizes as "div", "class", "foo" as their own words rather than being
/// stripped away as markup.
fn similarity_html(path: &Path) -> Result<HashSet<u64>> {
    similarity_default(path)
}

/// Tries the strict, canonicalized JSON parse first; a file whose format is
/// text-readable but structurally corrupted (malformed syntax) falls back
/// to plain-text treatment rather than being excluded from comparison
/// entirely - it still has real, comparable word content.
fn similarity_json(path: &Path) -> Result<HashSet<u64>> {
    match canonical_json_text(path) {
        Ok(text) => Ok(shingle_tokens(&text)),
        Err(_) => similarity_default(path),
    }
}

/// Same fallback rule as `similarity_json`: a `.yaml`/`.yml` file that
/// fails to parse still gets compared as plain text.
fn similarity_yaml(path: &Path) -> Result<HashSet<u64>> {
    match canonical_yaml_text(path) {
        Ok(text) => Ok(shingle_tokens(&text)),
        Err(_) => similarity_default(path),
    }
}

/// Same fallback rule again: a `.csv` with bad delimiters, non-UTF-8 bytes,
/// or a mislabeled extension (e.g. actually tab-separated, or not really
/// delimited data at all) still gets compared as plain text rather than
/// excluded.
fn similarity_csv(path: &Path) -> Result<HashSet<u64>> {
    match delimited_rows_tokens(path, b',') {
        Ok(tokens) => Ok(tokens),
        Err(_) => similarity_default(path),
    }
}

fn similarity_tsv(path: &Path) -> Result<HashSet<u64>> {
    match delimited_rows_tokens(path, b'\t') {
        Ok(tokens) => Ok(tokens),
        Err(_) => similarity_default(path),
    }
}

fn similarity_docx(path: &Path) -> Result<HashSet<u64>> {
    Ok(shingle_tokens(&extract_docx_text(path)?))
}

fn similarity_pptx(path: &Path) -> Result<HashSet<u64>> {
    Ok(shingle_tokens(&extract_pptx_text(path)?))
}

/// No text-fallback here, unlike csv/tsv/json/yaml above: docx/pptx/xlsx
/// are zip-based binary containers, so a corrupted one has no text-readable
/// bytes to fall back to - it stays excluded from comparison on failure,
/// same as before.
fn similarity_xlsx(path: &Path) -> Result<HashSet<u64>> {
    xlsx_row_tokens(path)
}

fn similarity_xml(path: &Path) -> Result<HashSet<u64>> {
    Ok(shingle_tokens(&extract_all_xml_text(&read_text(path)?)))
}

fn similarity_rtf(path: &Path) -> Result<HashSet<u64>> {
    Ok(shingle_tokens(&strip_rtf(&read_text(path)?)))
}

fn read_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).map_err(|source| EngineError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
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

/// Parses `path` as delimited rows using `delimiter`, one hash per row
/// (cells joined with a unit-separator character before hashing). Shared by
/// `similarity_csv`/`similarity_tsv`, which each just pick the delimiter
/// for their own format.
fn delimited_rows_tokens(path: &Path, delimiter: u8) -> Result<HashSet<u64>> {
    let bytes = std::fs::read(path).map_err(|source| EngineError::Read {
        path: path.to_path_buf(),
        source,
    })?;
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

/// Opens a `.docx`/`.pptx` as a zip archive and reads one entry's bytes as
/// text.
fn read_zip_entry_text(path: &Path, entry_name: &str) -> Result<String> {
    let file = std::fs::File::open(path).map_err(|source| EngineError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    let mut archive = zip::ZipArchive::new(file).map_err(|source| EngineError::ZipOpen {
        path: path.to_path_buf(),
        source,
    })?;
    let mut entry = archive
        .by_name(entry_name)
        .map_err(|source| EngineError::ZipOpen {
            path: path.to_path_buf(),
            source,
        })?;
    let mut text = String::new();
    entry
        .read_to_string(&mut text)
        .map_err(|source| EngineError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(text)
}

/// BLAKE3-hashes every zip entry under `media_prefix` (e.g. `word/media/`,
/// `ppt/media/`, `xl/media/`) - a separate archive-read pass rather than
/// piggybacking on `read_zip_entry_text`/`extract_pptx_text`'s own open,
/// since xlsx has no equivalent shared open to begin with (`xlsx_row_tokens`
/// goes through `calamine`, which doesn't expose the raw zip). A single
/// unreadable entry is skipped rather than failing the whole document - only
/// a completely unopenable archive is an error here, same as the text
/// extractors above.
fn collect_media_hashes(path: &Path, media_prefix: &str) -> Result<Vec<blake3::Hash>> {
    let file = std::fs::File::open(path).map_err(|source| EngineError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    let mut archive = zip::ZipArchive::new(file).map_err(|source| EngineError::ZipOpen {
        path: path.to_path_buf(),
        source,
    })?;

    let mut hashes = Vec::new();
    for i in 0..archive.len() {
        let Ok(mut entry) = archive.by_index(i) else {
            continue;
        };
        if !entry.name().starts_with(media_prefix) {
            continue;
        }
        let mut bytes = Vec::new();
        if entry.read_to_end(&mut bytes).is_err() {
            continue;
        }
        hashes.push(blake3::hash(&bytes));
    }
    Ok(hashes)
}

fn extract_docx_text(path: &Path) -> Result<String> {
    let xml = read_zip_entry_text(path, "word/document.xml")?;
    Ok(extract_xml_tag_text(&xml, "t"))
}

fn extract_pptx_text(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path).map_err(|source| EngineError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    let mut archive = zip::ZipArchive::new(file).map_err(|source| EngineError::ZipOpen {
        path: path.to_path_buf(),
        source,
    })?;

    let mut slides: Vec<(u32, String)> = Vec::new();
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|source| EngineError::ZipOpen {
            path: path.to_path_buf(),
            source,
        })?;
        if let Some(n) = slide_number(entry.name()) {
            slides.push((n, entry.name().to_string()));
        }
    }
    slides.sort_by_key(|(n, _)| *n);

    let mut out = String::new();
    for (_, name) in slides {
        let mut entry = archive
            .by_name(&name)
            .map_err(|source| EngineError::ZipOpen {
                path: path.to_path_buf(),
                source,
            })?;
        let mut xml = String::new();
        entry
            .read_to_string(&mut xml)
            .map_err(|source| EngineError::Read {
                path: path.to_path_buf(),
                source,
            })?;
        out.push_str(&extract_xml_tag_text(&xml, "t"));
        out.push(' ');
    }
    Ok(out)
}

fn slide_number(entry_name: &str) -> Option<u32> {
    let rest = entry_name.strip_prefix("ppt/slides/slide")?;
    rest.strip_suffix(".xml")?.parse().ok()
}

/// Text content of every element whose local name (ignoring any namespace
/// prefix) matches `local_tag`. Used for docx (`w:t`) and pptx (`a:t`) text
/// runs.
fn extract_xml_tag_text(xml: &str, local_tag: &str) -> String {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut capturing = false;
    let mut out = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if local_name_matches(e.name().as_ref(), local_tag) {
                    capturing = true;
                }
            }
            Ok(Event::End(e)) => {
                if local_name_matches(e.name().as_ref(), local_tag) {
                    capturing = false;
                    out.push(' ');
                }
            }
            Ok(Event::Text(t)) => {
                if capturing {
                    if let Ok(text) = t.unescape() {
                        out.push_str(&text);
                    }
                }
            }
            Ok(Event::Eof) => break,
            // Malformed XML: keep whatever text was recovered so far rather
            // than failing the whole extraction.
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

/// All text content in an XML document, regardless of element (SPEC.md
/// section 4: xml is parsed and canonicalized; this pass extracts visible
/// text rather than doing a full structural canonicalization/diff).
fn extract_all_xml_text(xml: &str) -> String {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut out = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Text(t)) => {
                if let Ok(text) = t.unescape() {
                    out.push_str(&text);
                    out.push(' ');
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

fn local_name_matches(qname: &[u8], local: &str) -> bool {
    let s = std::str::from_utf8(qname).unwrap_or("");
    let name = s.rsplit(':').next().unwrap_or(s);
    name == local
}

/// Cell values per row (SPEC.md section 4: "xlsx, csv, tsv | Cell values
/// per row... | Row-level hashing and Jaccard"), across every sheet.
fn xlsx_row_tokens(path: &Path) -> Result<HashSet<u64>> {
    use calamine::Reader as _;
    let mut workbook: calamine::Sheets<_> =
        calamine::open_workbook_auto(path).map_err(|source| EngineError::XlsxRead {
            path: path.to_path_buf(),
            source,
        })?;

    let mut tokens = HashSet::new();
    let sheet_names = workbook.sheet_names().to_vec();
    for sheet_name in sheet_names {
        let Some(range) = workbook.worksheet_range(&sheet_name).ok() else {
            continue;
        };
        for row in range.rows() {
            let row_text: String = row
                .iter()
                .map(|cell| cell.to_string())
                .collect::<Vec<_>>()
                .join("\u{1f}");
            tokens.insert(hash_str(&row_text));
        }
    }
    Ok(tokens)
}

fn canonical_yaml_text(path: &Path) -> Result<String> {
    let text = read_text(path)?;
    let docs =
        yaml_rust2::YamlLoader::load_from_str(&text).map_err(|source| EngineError::YamlParse {
            path: path.to_path_buf(),
            source,
        })?;

    let value = if docs.len() == 1 {
        yaml_to_json(&docs[0])
    } else {
        serde_json::Value::Array(docs.iter().map(yaml_to_json).collect())
    };
    let canonical = serde_json::to_vec(&value).map_err(|source| EngineError::JsonSerialize {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(String::from_utf8_lossy(&canonical).into_owned())
}

/// Converts a parsed YAML value into `serde_json::Value` so it can reuse the
/// same canonical (sorted-key, no-whitespace) serialization already used
/// for JSON (SPEC.md section 4: "Parse and canonicalize").
fn yaml_to_json(yaml: &yaml_rust2::Yaml) -> serde_json::Value {
    use yaml_rust2::Yaml;
    match yaml {
        Yaml::Real(s) => s
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(serde_json::Value::Number)
            .unwrap_or_else(|| serde_json::Value::String(s.clone())),
        Yaml::Integer(i) => serde_json::Value::Number((*i).into()),
        Yaml::String(s) => serde_json::Value::String(s.clone()),
        Yaml::Boolean(b) => serde_json::Value::Bool(*b),
        Yaml::Array(items) => serde_json::Value::Array(items.iter().map(yaml_to_json).collect()),
        Yaml::Hash(map) => {
            let mut object = serde_json::Map::new();
            for (k, v) in map {
                let key = match k {
                    Yaml::String(s) => s.clone(),
                    other => format!("{other:?}"),
                };
                object.insert(key, yaml_to_json(v));
            }
            serde_json::Value::Object(object)
        }
        Yaml::Null | Yaml::BadValue | Yaml::Alias(_) => serde_json::Value::Null,
    }
}

/// Recovers plain text from RTF by walking control words rather than fully
/// parsing the format: groups introduced by `\*` or a known non-text
/// destination (font table, style sheet, embedded objects, ...) are
/// skipped, `\par`/`\line` become newlines, and `\'` hex byte escapes are
/// dropped rather than decoded through the document's codepage. Good enough
/// for shingling, not a faithful RTF renderer.
fn strip_rtf(text: &str) -> String {
    const SKIP_DESTINATIONS: &[&str] = &[
        "fonttbl",
        "colortbl",
        "stylesheet",
        "info",
        "generator",
        "pict",
        "object",
        "footer",
        "header",
        "footnote",
        "annotation",
        "field",
        "themedata",
        "colorschememapping",
        "latentstyles",
        "rsidtbl",
        "xmlnstbl",
        "listtable",
        "listoverridetable",
        "datastore",
    ];

    // Operating on `Vec<char>` (rather than byte offsets into the `&str`)
    // keeps every slice below trivially safe, with no UTF-8 boundary
    // reasoning required.
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    let mut out = String::new();
    let mut skip_stack: Vec<bool> = vec![false];

    while i < chars.len() {
        let c = chars[i];
        match c {
            '{' => {
                let parent_skip = *skip_stack.last().unwrap_or(&false);
                skip_stack.push(parent_skip);
                i += 1;
            }
            '}' => {
                skip_stack.pop();
                i += 1;
            }
            '\\' => {
                i += 1;
                if i >= chars.len() {
                    break;
                }
                let next = chars[i];
                if next == '\'' {
                    i += 1;
                    let mut consumed = 0;
                    while consumed < 2 && i < chars.len() && chars[i].is_ascii_hexdigit() {
                        i += 1;
                        consumed += 1;
                    }
                } else if next.is_ascii_alphabetic() {
                    let start = i;
                    while i < chars.len() && chars[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    let word: String = chars[start..i].iter().collect();
                    if i < chars.len() && chars[i] == '-' {
                        i += 1;
                    }
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                    if i < chars.len() && chars[i] == ' ' {
                        i += 1;
                    }

                    let skipping = *skip_stack.last().unwrap_or(&false);
                    match word.as_str() {
                        "par" | "line" if !skipping => out.push('\n'),
                        "tab" if !skipping => out.push('\t'),
                        _ => {
                            if SKIP_DESTINATIONS.contains(&word.as_str()) {
                                if let Some(top) = skip_stack.last_mut() {
                                    *top = true;
                                }
                            }
                        }
                    }
                } else if next == '*' {
                    if let Some(top) = skip_stack.last_mut() {
                        *top = true;
                    }
                    i += 1;
                } else {
                    if !*skip_stack.last().unwrap_or(&false)
                        && (next == '\\' || next == '{' || next == '}')
                    {
                        out.push(next);
                    }
                    i += 1;
                }
            }
            _ => {
                if !*skip_stack.last().unwrap_or(&false) {
                    out.push(c);
                }
                i += 1;
            }
        }
    }
    out
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
