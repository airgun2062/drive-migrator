// Mirrors of engine's serde-serialized types (crates/engine), verified
// against real `migrator ... --json` output rather than guessed from the
// Rust source, since a couple of these (ReconcileState, PreflightIssue) use
// non-obvious serde tagging.

export type TransferModeArg = 'copy' | 'collapse'

export interface ScanSettings {
  source: string
  destination: string
  mode: TransferModeArg
  onMovedCopy: boolean
  keepNewest: number
  archive: boolean
  protectedExtensions: string[]
  maxPathLength: number
  fat32: boolean
  cachePath?: string
  journalPath?: string
  duplicateCutoff: number
  highCoverage: number
}

export function defaultSettings(): ScanSettings {
  return {
    source: '',
    destination: '',
    mode: 'copy',
    onMovedCopy: false,
    keepNewest: 1,
    archive: false,
    protectedExtensions: [],
    maxPathLength: 255,
    fat32: false,
    duplicateCutoff: 0.5,
    highCoverage: 0.8,
  }
}

// --- reconcile ---

export type PartialReason = {
  SizeMismatch: { source_size: number; destination_size: number }
}

export type PreflightIssue =
  | { kind: 'PathTooLong'; length: number; limit: number }
  | { kind: 'IllegalCharacter'; character: string }
  | { kind: 'ReservedName'; component: string }
  | { kind: 'TrailingDotOrSpace'; component: string }
  | { kind: 'FileTooLargeForFat32'; size: number; limit: number }
  | { kind: 'PathCollision'; with: string[] }

// ReconcileState is internally tagged with tag "state", and it is itself
// the value of a field also named "state" on PlanEntry - so in JSON it is
// PlanEntry.state.state, not PlanEntry.state directly.
export type ReconcileState =
  | { state: 'verified' }
  | { state: 'partial'; reason: PartialReason }
  | { state: 'missing' }
  | { state: 'moved'; destination_relative: string }
  | { state: 'conflict' }
  | { state: 'blocked'; issues: PreflightIssue[] }
  | { state: 'destination_only' }

export interface PlanEntry {
  relative_path: string
  source_path: string | null
  destination_path: string | null
  size: number | null
  state: ReconcileState
}

export interface ReconcileSummary {
  verified: number
  partial: number
  missing: number
  moved: number
  conflict: number
  blocked: number
  destination_only: number
}

export interface ReconcilePlan {
  source_root: string
  destination_root: string
  entries: PlanEntry[]
  summary: ReconcileSummary
}

// --- analyze ---

export interface DuplicateGroup {
  hash: string
  size: number
  files: string[]
}

export interface AnalyzeReport {
  roots: string[]
  files_scanned: number
  bytes_scanned: number
  duplicate_groups: DuplicateGroup[]
  duplicate_files: number
  reclaimable_bytes: number
}

// --- similarity ---

export type Relationship = 'near_duplicate' | 'diverged' | 'b_is_more_complete' | 'a_is_more_complete'

export interface SimilarPair {
  a: string
  b: string
  jaccard_estimate: number
  coverage_a_to_b: number
  coverage_b_to_a: number
  relationship: Relationship
}

export interface SimilarityReport {
  files_considered: number
  files_skipped_unsupported: number
  pairs: SimilarPair[]
}

// --- versions ---

export type DateSource = 'office_metadata' | 'exif' | 'email_header' | 'filesystem_modified'

export interface RankedMember {
  path: string
  rank: number
  date_unix_millis: number
  date_source: DateSource
  is_tip: boolean
}

export interface VersionFamily {
  members: RankedMember[]
}

export interface VersionReport {
  families: VersionFamily[]
  flagged_for_manual_merge: SimilarPair[]
}

// --- collapse ---

export type CollapseAction = 'keep' | 'collapse' | 'archive'
export type CollapseReason =
  | 'kept'
  | 'tip'
  | 'within_keep_newest'
  | 'protected'
  | 'exact_duplicate'
  | 'older_version'

export interface CollapseDecision {
  path: string
  action: CollapseAction
  reason: CollapseReason
  size: number
}

export interface CollapseReport {
  decisions: CollapseDecision[]
  version_report: VersionReport
  files_kept: number
  files_collapsed: number
  bytes_saved: number
}

// --- scan bundle (gui-specific) ---

export interface ScanBundle {
  plan: ReconcilePlan
  analyze: AnalyzeReport
  similarity: SimilarityReport
  collapse: CollapseReport | null
}

// --- transfer ---

export type TransferOutcome = 'copied' | 'failed'

export interface TransferResult {
  relative_path: string
  outcome: TransferOutcome
  bytes: number | null
  detail: string | null
}

export interface TransferSummary {
  copied: number
  failed: number
  skipped_verified: number
  skipped_conflict: number
  skipped_blocked: number
  skipped_moved: number
  bytes_copied: number
  results: TransferResult[]
}

export interface TransferProgressEvent {
  relativePath: string
  outcome: TransferOutcome
  bytes: number | null
  filesDone: number
}

// --- manifest / verify ---

export interface ManifestWriteResult {
  file_count: number
  total_bytes: number
  duplicate_group_count: number
  version_family_count: number
  manifest_path: string
  sha256: string
  app_local_hash_recorded: boolean
}

export interface IntegrityCheck {
  visible_hash: string | null
  backup_hash: string | null
  app_local_hash: string | null
  trusted: boolean
}

export interface VerifyChange {
  relative_path: string
  recorded_sha256: string
  current_sha256: string
}

export interface VerifyMove {
  recorded_path: string
  current_path: string
  sha256: string
}

export interface VerifyReport {
  destination_root: string
  manifest_created_unix_ms: number
  integrity: IntegrityCheck
  unchanged: number
  changed: VerifyChange[]
  moved: VerifyMove[]
  deleted: string[]
  unrecorded: string[]
}
