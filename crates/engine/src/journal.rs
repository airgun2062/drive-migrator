use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{EngineError, Result};
use crate::reconcile::{PartialReason, ReconcilePlan, ReconcileState};
use crate::transfer::{TransferOutcome, TransferResult};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS plan_runs (
    id INTEGER PRIMARY KEY,
    source_root TEXT NOT NULL,
    destination_root TEXT NOT NULL,
    created_unix_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS plan_entries (
    id INTEGER PRIMARY KEY,
    run_id INTEGER NOT NULL REFERENCES plan_runs(id),
    relative_path TEXT NOT NULL,
    source_path TEXT,
    destination_path TEXT,
    size INTEGER,
    state TEXT NOT NULL,
    detail TEXT
);

CREATE INDEX IF NOT EXISTS plan_entries_run_id ON plan_entries(run_id);

CREATE TABLE IF NOT EXISTS transfer_events (
    id INTEGER PRIMARY KEY,
    run_id INTEGER NOT NULL REFERENCES plan_runs(id),
    relative_path TEXT NOT NULL,
    outcome TEXT NOT NULL,
    bytes INTEGER,
    detail TEXT,
    created_unix_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS transfer_events_run_id ON transfer_events(run_id);
"#;

/// Records intent and decisions in a SQLite (WAL mode) database (SPEC.md
/// section 7). The journal is never trusted on its own: every run re-scans
/// both roots and reconciles fresh (SPEC.md section 3, principle 2).
pub struct Journal {
    conn: Connection,
}

impl Journal {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|source| {
                    EngineError::JournalDirCreate {
                        path: path.to_path_buf(),
                        source,
                    }
                })?;
            }
        }
        let conn = Connection::open(path).map_err(|source| EngineError::JournalOpen {
            path: path.to_path_buf(),
            source,
        })?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|source| EngineError::JournalOpen {
                path: path.to_path_buf(),
                source,
            })?;
        conn.execute_batch(SCHEMA)
            .map_err(|source| EngineError::JournalOpen {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(Self { conn })
    }

    /// Persists a reconcile plan as a new run and returns its id.
    pub fn record_plan(&mut self, plan: &ReconcilePlan) -> Result<i64> {
        let tx = self.conn.transaction().map_err(EngineError::JournalQuery)?;

        tx.execute(
            "INSERT INTO plan_runs (source_root, destination_root, created_unix_ms) VALUES (?1, ?2, ?3)",
            params![
                plan.source_root.to_string_lossy(),
                plan.destination_root.to_string_lossy(),
                unix_millis_now(),
            ],
        )
        .map_err(EngineError::JournalQuery)?;
        let run_id = tx.last_insert_rowid();

        for entry in &plan.entries {
            let (state, detail) = encode_state(&entry.state);
            tx.execute(
                "INSERT INTO plan_entries (run_id, relative_path, source_path, destination_path, size, state, detail) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    run_id,
                    entry.relative_path.to_string_lossy(),
                    entry.source_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
                    entry
                        .destination_path
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned()),
                    entry.size.map(|s| s as i64),
                    state,
                    detail,
                ],
            )
            .map_err(EngineError::JournalQuery)?;
        }

        tx.commit().map_err(EngineError::JournalQuery)?;
        Ok(run_id)
    }

    pub fn latest_run_id(&self) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT id FROM plan_runs ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(EngineError::JournalQuery)
    }

    pub fn count_entries_for_run(&self, run_id: i64) -> Result<i64> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM plan_entries WHERE run_id = ?1",
                params![run_id],
                |row| row.get(0),
            )
            .map_err(EngineError::JournalQuery)
    }

    /// Records every attempted transfer (copied or failed) for a run.
    pub fn record_transfer_results(
        &mut self,
        run_id: i64,
        results: &[TransferResult],
    ) -> Result<()> {
        let tx = self.conn.transaction().map_err(EngineError::JournalQuery)?;
        let created = unix_millis_now();

        for result in results {
            let outcome = match result.outcome {
                TransferOutcome::Copied => "copied",
                TransferOutcome::Failed => "failed",
            };
            tx.execute(
                "INSERT INTO transfer_events (run_id, relative_path, outcome, bytes, detail, created_unix_ms) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    run_id,
                    result.relative_path.to_string_lossy(),
                    outcome,
                    result.bytes.map(|b| b as i64),
                    result.detail,
                    created,
                ],
            )
            .map_err(EngineError::JournalQuery)?;
        }

        tx.commit().map_err(EngineError::JournalQuery)?;
        Ok(())
    }

    pub fn count_transfer_events_for_run(&self, run_id: i64) -> Result<i64> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM transfer_events WHERE run_id = ?1",
                params![run_id],
                |row| row.get(0),
            )
            .map_err(EngineError::JournalQuery)
    }
}

fn unix_millis_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn encode_state(state: &ReconcileState) -> (&'static str, Option<String>) {
    match state {
        ReconcileState::Verified => ("verified", None),
        ReconcileState::Partial { reason } => {
            let PartialReason::SizeMismatch {
                source_size,
                destination_size,
            } = reason;
            (
                "partial",
                Some(format!(
                    "{{\"source_size\":{source_size},\"destination_size\":{destination_size}}}"
                )),
            )
        }
        ReconcileState::Missing => ("missing", None),
        ReconcileState::Moved {
            destination_relative,
        } => (
            "moved",
            Some(destination_relative.to_string_lossy().into_owned()),
        ),
        ReconcileState::Conflict => ("conflict", None),
        ReconcileState::Blocked { issues } => ("blocked", serde_json::to_string(issues).ok()),
        ReconcileState::DestinationOnly => ("destination_only", None),
    }
}
