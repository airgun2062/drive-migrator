pub mod analyze;
pub mod cache;
pub mod error;
pub mod hash;
pub mod journal;
pub mod policy;
pub mod reconcile;
pub mod scan;

pub use analyze::{analyze, AnalyzeReport, DuplicateGroup};
pub use cache::FingerprintCache;
pub use error::{EngineError, Result};
pub use journal::Journal;
pub use policy::{PreflightIssue, PreflightRules};
pub use reconcile::{plan, PlanEntry, ReconcilePlan, ReconcileState};
pub use scan::ScanEntry;
