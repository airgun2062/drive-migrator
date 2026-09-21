pub mod analyze;
pub mod cache;
pub mod error;
pub mod hash;
pub mod scan;

pub use analyze::{analyze, AnalyzeReport, DuplicateGroup};
pub use cache::FingerprintCache;
pub use error::{EngineError, Result};
pub use scan::ScanEntry;
