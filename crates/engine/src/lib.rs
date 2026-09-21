pub mod analyze;
pub mod cache;
pub mod error;
pub mod extract;
pub mod hash;
pub mod journal;
pub mod manifest;
pub mod policy;
pub mod reconcile;
pub mod scan;
pub mod similarity;
pub mod transfer;

pub use analyze::{analyze, AnalyzeReport, DuplicateGroup};
pub use cache::FingerprintCache;
pub use error::{EngineError, Result};
pub use extract::DocumentKind;
pub use journal::Journal;
pub use manifest::{
    check_integrity, verify, write_manifest, IntegrityCheck, ManifestWriteResult, VerifyChange,
    VerifyMove, VerifyReport,
};
pub use policy::{PreflightIssue, PreflightRules};
pub use reconcile::{plan, PlanEntry, ReconcilePlan, ReconcileState};
pub use scan::ScanEntry;
pub use similarity::{find_similar, Relationship, SimilarPair, SimilarityConfig, SimilarityReport};
pub use transfer::{
    run, MovedPolicy, RunOptions, TransferOutcome, TransferResult, TransferSummary,
};
