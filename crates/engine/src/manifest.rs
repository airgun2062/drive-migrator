use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::error::{EngineError, Result};
use crate::hash;
use crate::scan;

const MANIFEST_FILENAME: &str = "MIGRATION_MANIFEST.json";
const MANIFEST_CHECKSUM_FILENAME: &str = "MIGRATION_MANIFEST.json.sha256";
const MIGRATOR_DIR: &str = ".migrator";
const BACKUP_FILENAME: &str = "manifest.backup.json";
const BACKUP_CHECKSUM_FILENAME: &str = "manifest.backup.json.sha256";
const DB_FILENAME: &str = "manifest.db";
const FORMAT_VERSION: u32 = 1;
const WARNING: &str =
    "GENERATED FILE. Edit at your own peril. The app checksums this file and will flag any change.";

#[derive(Debug, Clone, Serialize)]
struct ManifestDocument {
    #[serde(rename = "_warning")]
    warning: String,
    format_version: u32,
    created_unix_ms: i64,
    source_root: Option<String>,
    destination_root: String,
    summary: ManifestSummary,
    duplicate_groups: Vec<ManifestDuplicateGroup>,
    /// Populated starting with collapse mode (SPEC.md roadmap P6); always
    /// empty for now.
    version_families: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
struct ManifestSummary {
    file_count: usize,
    total_bytes: u64,
    duplicate_group_count: usize,
}

#[derive(Debug, Clone, Serialize)]
struct ManifestDuplicateGroup {
    sha256: String,
    size: u64,
    dest_paths: Vec<String>,
}

#[derive(Debug, Clone)]
struct ManifestFileRecord {
    relative_path: PathBuf,
    sha256: String,
    size: u64,
    modified_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ManifestWriteResult {
    pub file_count: usize,
    pub total_bytes: u64,
    pub duplicate_group_count: usize,
    pub manifest_path: PathBuf,
    pub sha256: String,
    pub app_local_hash_recorded: bool,
}

/// Scans `destination_root`, computes SHA-256 for every file (SPEC.md
/// section 4: SHA-256 for the manifest, distinct from BLAKE3 used for
/// analysis), and writes the manifest to disk: `MIGRATION_MANIFEST.json` +
/// checksum, `.migrator/manifest.db` (source of truth), a backup JSON copy +
/// checksum, and a copy of the hash outside the destination drive. Never
/// trusts a cache: every byte is re-read fresh, since this is the record
/// tampering is checked against.
pub fn write_manifest(
    destination_root: &Path,
    source_root: Option<&Path>,
) -> Result<ManifestWriteResult> {
    let files = collect_file_records(destination_root)?;
    let total_bytes: u64 = files.iter().map(|f| f.size).sum();
    let file_count = files.len();

    let duplicate_groups = group_duplicates(&files);
    let duplicate_group_count = duplicate_groups.len();

    let created_unix_ms = unix_millis_now();
    let document = ManifestDocument {
        warning: WARNING.to_string(),
        format_version: FORMAT_VERSION,
        created_unix_ms,
        source_root: source_root.map(|p| p.to_string_lossy().into_owned()),
        destination_root: destination_root.to_string_lossy().into_owned(),
        summary: ManifestSummary {
            file_count,
            total_bytes,
            duplicate_group_count,
        },
        duplicate_groups,
        version_families: Vec::new(),
    };

    write_db(destination_root, &document, &files)?;

    let canonical = canonical_bytes(&document)?;
    let pretty = pretty_bytes(&document)?;
    let sha256 = hash::sha256_hex(&canonical);

    write_protected(&manifest_path(destination_root), &pretty)?;
    write_protected(&manifest_checksum_path(destination_root), sha256.as_bytes())?;
    write_protected(&backup_path(destination_root), &pretty)?;
    write_protected(&backup_checksum_path(destination_root), sha256.as_bytes())?;

    let app_local_hash_recorded = record_app_local_hash(destination_root, &sha256)?;

    Ok(ManifestWriteResult {
        file_count,
        total_bytes,
        duplicate_group_count,
        manifest_path: manifest_path(destination_root),
        sha256,
        app_local_hash_recorded,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct IntegrityCheck {
    pub visible_hash: Option<String>,
    pub backup_hash: Option<String>,
    pub app_local_hash: Option<String>,
    /// True when at least two of the (available) hashes above agree.
    pub trusted: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct VerifyChange {
    pub relative_path: PathBuf,
    pub recorded_sha256: String,
    pub current_sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct VerifyMove {
    pub recorded_path: PathBuf,
    pub current_path: PathBuf,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct VerifyReport {
    pub destination_root: PathBuf,
    pub manifest_created_unix_ms: i64,
    pub integrity: IntegrityCheck,
    pub unchanged: usize,
    pub changed: Vec<VerifyChange>,
    pub moved: Vec<VerifyMove>,
    pub deleted: Vec<PathBuf>,
    pub unrecorded: Vec<PathBuf>,
}

/// Re-scans the destination and reports what changed since the manifest was
/// written: files moved, renamed, changed, or deleted (SPEC.md section 8).
pub fn verify(destination_root: &Path) -> Result<VerifyReport> {
    let integrity = check_integrity(destination_root);
    let manifest_created_unix_ms = read_created_unix_ms(destination_root)?;
    let recorded = read_db_files(destination_root)?;
    let current = collect_file_records(destination_root)?;

    let mut current_by_path: HashMap<PathBuf, String> = HashMap::new();
    let mut current_by_hash: HashMap<String, PathBuf> = HashMap::new();
    for entry in &current {
        current_by_path.insert(entry.relative_path.clone(), entry.sha256.clone());
        current_by_hash
            .entry(entry.sha256.clone())
            .or_insert_with(|| entry.relative_path.clone());
    }

    let mut unchanged = 0;
    let mut changed = Vec::new();
    let mut moved = Vec::new();
    let mut deleted = Vec::new();
    let mut claimed: HashSet<PathBuf> = HashSet::new();

    for record in &recorded {
        match current_by_path.get(&record.relative_path) {
            Some(current_sha256) if *current_sha256 == record.sha256 => {
                unchanged += 1;
                claimed.insert(record.relative_path.clone());
            }
            Some(current_sha256) => {
                changed.push(VerifyChange {
                    relative_path: record.relative_path.clone(),
                    recorded_sha256: record.sha256.clone(),
                    current_sha256: current_sha256.clone(),
                });
                claimed.insert(record.relative_path.clone());
            }
            None => match current_by_hash.get(&record.sha256) {
                Some(current_path) => {
                    moved.push(VerifyMove {
                        recorded_path: record.relative_path.clone(),
                        current_path: current_path.clone(),
                        sha256: record.sha256.clone(),
                    });
                    claimed.insert(current_path.clone());
                }
                None => deleted.push(record.relative_path.clone()),
            },
        }
    }

    let mut unrecorded: Vec<PathBuf> = current_by_path
        .keys()
        .filter(|p| !claimed.contains(*p))
        .cloned()
        .collect();
    unrecorded.sort();
    changed.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    moved.sort_by(|a, b| a.recorded_path.cmp(&b.recorded_path));
    deleted.sort();

    Ok(VerifyReport {
        destination_root: destination_root.to_path_buf(),
        manifest_created_unix_ms,
        integrity,
        unchanged,
        changed,
        moved,
        deleted,
        unrecorded,
    })
}

/// Compares the visible manifest, the backup, and the app-local hash (SPEC.md
/// section 8: "on open ... use a two-of-three vote"). A missing or unparsable
/// copy just drops out of the vote rather than failing the whole check.
pub fn check_integrity(destination_root: &Path) -> IntegrityCheck {
    let visible_hash = read_canonical_hash(&manifest_path(destination_root));
    let backup_hash = read_canonical_hash(&backup_path(destination_root));
    let app_local_hash = read_app_local_hash(destination_root);

    let trusted = majority_matches(&[
        visible_hash.clone(),
        backup_hash.clone(),
        app_local_hash.clone(),
    ]);

    IntegrityCheck {
        visible_hash,
        backup_hash,
        app_local_hash,
        trusted,
    }
}

fn majority_matches(values: &[Option<String>]) -> bool {
    let present: Vec<&String> = values.iter().flatten().collect();
    if present.len() < 2 {
        return false;
    }
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for v in &present {
        *counts.entry(v.as_str()).or_default() += 1;
    }
    counts.values().any(|&c| c >= 2)
}

fn collect_file_records(destination_root: &Path) -> Result<Vec<ManifestFileRecord>> {
    let mut files = Vec::new();
    for entry in scan::scan_root(destination_root)? {
        if is_manifest_artifact(destination_root, &entry.path) {
            continue;
        }
        let relative = entry.path.strip_prefix(destination_root).map_err(|_| {
            EngineError::PathNotUnderRoot {
                root: destination_root.to_path_buf(),
                path: entry.path.clone(),
            }
        })?;
        let sha256 = hash::full_sha256_hex(&entry.path)?;
        files.push(ManifestFileRecord {
            relative_path: relative.to_path_buf(),
            sha256,
            size: entry.size,
            modified_unix_ms: unix_millis(entry.modified),
        });
    }
    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(files)
}

fn group_duplicates(files: &[ManifestFileRecord]) -> Vec<ManifestDuplicateGroup> {
    let mut by_hash: HashMap<&str, Vec<&ManifestFileRecord>> = HashMap::new();
    for f in files {
        by_hash.entry(f.sha256.as_str()).or_default().push(f);
    }
    let mut groups: Vec<ManifestDuplicateGroup> = by_hash
        .into_iter()
        .filter(|(_, members)| members.len() > 1)
        .map(|(sha256, members)| {
            let mut dest_paths: Vec<String> = members
                .iter()
                .map(|m| m.relative_path.to_string_lossy().into_owned())
                .collect();
            dest_paths.sort();
            ManifestDuplicateGroup {
                sha256: sha256.to_string(),
                size: members[0].size,
                dest_paths,
            }
        })
        .collect();
    groups.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.sha256.cmp(&b.sha256)));
    groups
}

/// True for a path that is part of the manifest's own bookkeeping
/// (`.migrator/` or the root manifest/checksum files), so a re-run never
/// treats its own previous output as destination content.
fn is_manifest_artifact(destination_root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(destination_root) else {
        return false;
    };
    let mut components = relative.components();
    match components.next() {
        Some(Component::Normal(first)) => {
            if first == MIGRATOR_DIR {
                return true;
            }
            components.next().is_none()
                && (first == MANIFEST_FILENAME || first == MANIFEST_CHECKSUM_FILENAME)
        }
        _ => false,
    }
}

fn manifest_path(destination_root: &Path) -> PathBuf {
    destination_root.join(MANIFEST_FILENAME)
}

fn manifest_checksum_path(destination_root: &Path) -> PathBuf {
    destination_root.join(MANIFEST_CHECKSUM_FILENAME)
}

fn migrator_dir(destination_root: &Path) -> PathBuf {
    destination_root.join(MIGRATOR_DIR)
}

fn backup_path(destination_root: &Path) -> PathBuf {
    migrator_dir(destination_root).join(BACKUP_FILENAME)
}

fn backup_checksum_path(destination_root: &Path) -> PathBuf {
    migrator_dir(destination_root).join(BACKUP_CHECKSUM_FILENAME)
}

fn db_path(destination_root: &Path) -> PathBuf {
    migrator_dir(destination_root).join(DB_FILENAME)
}

fn canonical_bytes(document: &ManifestDocument) -> Result<Vec<u8>> {
    let value = serde_json::to_value(document).map_err(EngineError::ManifestSerialize)?;
    serde_json::to_vec(&value).map_err(EngineError::ManifestSerialize)
}

fn pretty_bytes(document: &ManifestDocument) -> Result<Vec<u8>> {
    let value = serde_json::to_value(document).map_err(EngineError::ManifestSerialize)?;
    serde_json::to_vec_pretty(&value).map_err(EngineError::ManifestSerialize)
}

/// Re-parses whatever JSON is on disk and re-serializes it canonically
/// before hashing, so reformatting the visible file does not change its
/// hash (SPEC.md section 8). `None` if the file is missing or not valid
/// JSON, so it simply drops out of the two-of-three vote.
fn read_canonical_hash(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let canonical = serde_json::to_vec(&value).ok()?;
    Some(hash::sha256_hex(&canonical))
}

/// Writes `bytes` to `path`, clearing and re-setting the read-only attribute
/// around the write so a previously protected file can still be replaced.
fn write_protected(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|source| EngineError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }
    }
    if path.exists() {
        clear_readonly(path)?;
    }
    fs::write(path, bytes).map_err(|source| EngineError::Write {
        path: path.to_path_buf(),
        source,
    })?;
    set_readonly(path)
}

fn set_readonly(path: &Path) -> Result<()> {
    let mut permissions = fs::metadata(path)
        .map_err(|source| EngineError::Metadata {
            path: path.to_path_buf(),
            source,
        })?
        .permissions();
    // `set_readonly(true)` only clears write bits from whatever mode is
    // already set, so it is safe on every platform.
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions).map_err(|source| EngineError::SetPermissions {
        path: path.to_path_buf(),
        source,
    })
}

/// Clears the read-only bit so an already-protected manifest file can be
/// replaced. `Permissions::set_readonly(false)` would make the file world
/// writable on Unix (it does not just restore owner permissions), so Unix
/// sets an explicit owner-only mode instead; Windows' read-only attribute
/// has no such implication.
fn clear_readonly(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return fs::set_permissions(path, fs::Permissions::from_mode(0o644)).map_err(|source| {
            EngineError::SetPermissions {
                path: path.to_path_buf(),
                source,
            }
        });
    }
    #[cfg(not(unix))]
    {
        let mut permissions = fs::metadata(path)
            .map_err(|source| EngineError::Metadata {
                path: path.to_path_buf(),
                source,
            })?
            .permissions();
        // Windows' read-only attribute is a simple flag with no Unix-style
        // world-writable implication, so clearing it here is safe.
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions).map_err(|source| EngineError::SetPermissions {
            path: path.to_path_buf(),
            source,
        })
    }
}

const DB_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS manifest_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS manifest_files (
    relative_path TEXT PRIMARY KEY,
    sha256 TEXT NOT NULL,
    size INTEGER NOT NULL,
    modified_unix_ms INTEGER
);
"#;

fn write_db(
    destination_root: &Path,
    document: &ManifestDocument,
    files: &[ManifestFileRecord],
) -> Result<()> {
    let path = db_path(destination_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| EngineError::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let mut conn = Connection::open(&path).map_err(|source| EngineError::ManifestDbOpen {
        path: path.clone(),
        source,
    })?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|source| EngineError::ManifestDbOpen {
            path: path.clone(),
            source,
        })?;
    conn.execute_batch(DB_SCHEMA)
        .map_err(|source| EngineError::ManifestDbOpen {
            path: path.clone(),
            source,
        })?;

    let tx = conn.transaction().map_err(EngineError::ManifestDbQuery)?;
    tx.execute("DELETE FROM manifest_files", [])
        .map_err(EngineError::ManifestDbQuery)?;
    for f in files {
        tx.execute(
            "INSERT INTO manifest_files (relative_path, sha256, size, modified_unix_ms) VALUES (?1, ?2, ?3, ?4)",
            params![f.relative_path.to_string_lossy(), f.sha256, f.size as i64, f.modified_unix_ms],
        )
        .map_err(EngineError::ManifestDbQuery)?;
    }
    for (key, value) in [
        (
            "format_version".to_string(),
            document.format_version.to_string(),
        ),
        (
            "created_unix_ms".to_string(),
            document.created_unix_ms.to_string(),
        ),
        (
            "destination_root".to_string(),
            document.destination_root.clone(),
        ),
        (
            "source_root".to_string(),
            document.source_root.clone().unwrap_or_default(),
        ),
    ] {
        tx.execute(
            "INSERT INTO manifest_meta (key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
        .map_err(EngineError::ManifestDbQuery)?;
    }
    tx.commit().map_err(EngineError::ManifestDbQuery)?;
    Ok(())
}

fn read_db_files(destination_root: &Path) -> Result<Vec<ManifestFileRecord>> {
    let path = db_path(destination_root);
    let conn = Connection::open(&path).map_err(|source| EngineError::ManifestDbOpen {
        path: path.clone(),
        source,
    })?;
    let mut stmt = conn
        .prepare("SELECT relative_path, sha256, size, modified_unix_ms FROM manifest_files")
        .map_err(EngineError::ManifestDbQuery)?;
    let rows = stmt
        .query_map([], |row| {
            let relative_path: String = row.get(0)?;
            let sha256: String = row.get(1)?;
            let size: i64 = row.get(2)?;
            let modified_unix_ms: Option<i64> = row.get(3)?;
            Ok(ManifestFileRecord {
                relative_path: PathBuf::from(relative_path),
                sha256,
                size: size as u64,
                modified_unix_ms,
            })
        })
        .map_err(EngineError::ManifestDbQuery)?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(EngineError::ManifestDbQuery)?);
    }
    Ok(out)
}

fn read_created_unix_ms(destination_root: &Path) -> Result<i64> {
    let path = db_path(destination_root);
    let conn = Connection::open(&path).map_err(|source| EngineError::ManifestDbOpen {
        path: path.clone(),
        source,
    })?;
    let value: String = conn
        .query_row(
            "SELECT value FROM manifest_meta WHERE key = 'created_unix_ms'",
            [],
            |row| row.get(0),
        )
        .map_err(EngineError::ManifestDbQuery)?;
    Ok(value.parse().unwrap_or(0))
}

fn unix_millis_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn unix_millis(time: SystemTime) -> Option<i64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as i64)
}

/// Records a copy of the manifest hash outside the destination drive, so
/// tamper evidence does not rely solely on files an attacker who controls
/// the destination could also modify (SPEC.md section 8). `Ok(false)` when
/// no app data directory could be determined; that is not an error, just a
/// weaker (two-way instead of three-way) integrity check going forward.
fn record_app_local_hash(destination_root: &Path, sha256_hex: &str) -> Result<bool> {
    let Some(path) = app_local_hash_path(destination_root) else {
        return Ok(false);
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| EngineError::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(&path, sha256_hex.as_bytes())
        .map_err(|source| EngineError::Write { path, source })?;
    Ok(true)
}

fn read_app_local_hash(destination_root: &Path) -> Option<String> {
    let path = app_local_hash_path(destination_root)?;
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn app_local_hash_path(destination_root: &Path) -> Option<PathBuf> {
    let dir = app_data_dir()?.join("migrator").join("manifest-hashes");
    let key = hash::sha256_hex(destination_root.to_string_lossy().as_bytes());
    Some(dir.join(format!("{key}.sha256")))
}

fn app_data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let dir = std::env::var_os("APPDATA").map(PathBuf::from);

    #[cfg(target_os = "macos")]
    let dir = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("Library").join("Application Support"));

    #[cfg(all(unix, not(target_os = "macos")))]
    let dir = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share"))
        });

    dir
}
