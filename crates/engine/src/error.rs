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
}

pub type Result<T> = std::result::Result<T, EngineError>;
