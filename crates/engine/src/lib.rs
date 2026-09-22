pub mod analyze;
pub mod cache;
pub mod collapse;
pub mod error;
pub mod extract;
pub mod hash;
pub mod journal;
pub mod manifest;
pub mod metadata;
pub mod policy;
pub mod reconcile;
pub mod scan;
pub mod similarity;
pub mod transfer;
mod union_find;
pub mod versions;

pub use analyze::{analyze, AnalyzeReport, DuplicateGroup};
pub use cache::FingerprintCache;
pub use collapse::{
    apply_collapse_to_plan, plan_collapse, CollapseAction, CollapseConfig, CollapseDecision,
    CollapseReason, CollapseReport,
};
pub use error::{EngineError, Result};
pub use extract::DocumentKind;
pub use journal::Journal;
pub use manifest::{
    check_integrity, verify, write_manifest, IntegrityCheck, ManifestWriteResult, VerifyChange,
    VerifyMove, VerifyReport,
};
pub use metadata::{best_available_date, DateSource, DatedFile};
pub use policy::{PreflightIssue, PreflightRules};
pub use reconcile::{plan, PlanEntry, ReconcilePlan, ReconcileState};
pub use scan::ScanEntry;
pub use similarity::{find_similar, Relationship, SimilarPair, SimilarityConfig, SimilarityReport};
pub use transfer::{
    run, run_with_progress, MovedPolicy, RunOptions, TransferOutcome, TransferResult,
    TransferSummary,
};
pub use versions::{build_version_report, RankedMember, VersionFamily, VersionReport};
