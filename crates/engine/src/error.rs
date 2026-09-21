use std::io;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("failed to walk {path}: {source}")]
    Walk {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to read metadata for {path}: {source}")]
    Metadata {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to open {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to seek in {path}: {source}")]
    Seek {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to read cache file {path}: {source}")]
    CacheRead {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cache file {path} is not valid JSON: {source}")]
    CacheParse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to write cache file {path}: {source}")]
    CacheWrite {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to serialize fingerprint cache: {0}")]
    CacheSerialize(#[source] serde_json::Error),
    #[error("{path} is not inside root {root}")]
    PathNotUnderRoot { root: PathBuf, path: PathBuf },
    #[error("failed to open journal {path}: {source}")]
    JournalOpen {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error("failed to create journal directory for {path}: {source}")]
    JournalDirCreate {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("journal query failed: {0}")]
    JournalQuery(#[source] rusqlite::Error),
    #[error("failed to create directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to fsync {path}: {source}")]
    Fsync {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to rename {from} to {to}: {source}")]
    Rename {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to remove {path}: {source}")]
    Remove {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("verification failed for {path}: the copied bytes do not match the source hash")]
    VerifyMismatch { path: PathBuf },
    #[error("source and destination roots must not be nested inside one another: {source_root} / {destination_root}")]
    NestedRoots {
        source_root: PathBuf,
        destination_root: PathBuf,
    },
    #[error("failed to change permissions on {path}: {source}")]
    SetPermissions {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to open manifest database {path}: {source}")]
    ManifestDbOpen {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error("manifest database query failed: {0}")]
    ManifestDbQuery(#[source] rusqlite::Error),
    #[error("failed to serialize manifest: {0}")]
    ManifestSerialize(#[source] serde_json::Error),
    #[error("manifest file {path} is not valid JSON: {source}")]
    ManifestParse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to parse {path} as JSON: {source}")]
    JsonParse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to serialize canonical JSON for {path}: {source}")]
    JsonSerialize {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to parse {path} as delimited rows: {source}")]
    CsvParse {
        path: PathBuf,
        #[source]
        source: csv::Error,
    },
}

pub type Result<T> = std::result::Result<T, EngineError>;
