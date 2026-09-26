//! Best-effort structured logging to `<repo root>/logs/<YYYY-MM-DD-HH>.log`.
//!
//! One file per wall-clock hour (a fresh file starts as soon as the hour
//! changes), so a long-running operation's log stays a manageable size and
//! it's obvious which file holds a given moment. `Log::info`/`Log::error`
//! are the primary diagnostic tool during development: every operation this
//! engine performs, and every error it catches rather than propagates,
//! writes a line here with a short `tag` naming the subsystem, so a failure
//! can be traced without needing to reproduce it under a debugger or attach
//! screenshots.
//!
//! Logging never panics and never returns an error: a full disk or a
//! permissions problem on the log directory must not be allowed to break
//! whatever operation was being logged. A failed write is silently dropped.
//!
//! ARCHITECTURE_REFERENCE.md section 1.15 documents this module; see
//! LESSONS_LEARNED.md for why it exists (screenshot-based debugging of a
//! frozen or misbehaving app doesn't scale - a log file naming the exact
//! file/operation, with a timestamp, does).

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static LOG_LOCK: Mutex<()> = Mutex::new(());

/// `<repo root>/logs`, resolved from this crate's own `Cargo.toml` location
/// (`CARGO_MANIFEST_DIR` is always `<repo>/crates/engine`) rather than the
/// process's current working directory - a GUI shell in particular may run
/// its backend from a directory other than the repo root, so a
/// CWD-relative `./logs` would scatter log files depending on how the
/// binary was launched.
fn logs_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(|repo_root| repo_root.join("logs"))
        .unwrap_or_else(|| PathBuf::from("logs"))
}

/// Howard Hinnant's `civil_from_days`: the inverse of the `days_from_civil`
/// algorithm used in `metadata.rs`. Public-domain; avoids pulling in a
/// date/time crate just to turn a log timestamp into Y-M-D-H fields.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

struct Clock {
    year: i64,
    month: u32,
    day: u32,
    hour: u64,
    minute: u64,
    second: u64,
    millis: u32,
}

fn clock_from_unix(secs: u64, millis: u32) -> Clock {
    let days = (secs / 86400) as i64;
    let time_of_day = secs % 86400;
    let (year, month, day) = civil_from_days(days);
    Clock {
        year,
        month,
        day,
        hour: time_of_day / 3600,
        minute: (time_of_day % 3600) / 60,
        second: time_of_day % 60,
        millis,
    }
}

fn clock_now(now: SystemTime) -> Clock {
    let since_epoch = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    clock_from_unix(since_epoch.as_secs(), since_epoch.subsec_millis())
}

fn hourly_file_name(c: &Clock) -> String {
    format!(
        "{:04}-{:02}-{:02}-{:02}.log",
        c.year, c.month, c.day, c.hour
    )
}

fn timestamp(c: &Clock) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        c.year, c.month, c.day, c.hour, c.minute, c.second, c.millis
    )
}

fn write_line(level: &str, tag: &str, message: &str) {
    let now = clock_now(SystemTime::now());
    let line = format!("{} [{level}] [{tag}] {message}\n", timestamp(&now));

    let _guard = LOG_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = logs_dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(hourly_file_name(&now)))
    else {
        return;
    };
    let _ = file.write_all(line.as_bytes());
}

/// Structured logging: `Log::info(tag, message)` / `Log::error(tag,
/// message)`. `tag` is a short, stable name for the subsystem or operation
/// (e.g. `"scan"`, `"hash"`, `"transfer"`, `"cli.analyze"`) so log lines
/// can be traced back to where they came from without a stack trace.
pub struct Log;

impl Log {
    pub fn info(tag: &str, message: &str) {
        write_line("INFO ", tag, message);
    }

    pub fn error(tag: &str, message: &str) {
        write_line("ERROR", tag, message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_from_days_matches_the_unix_epoch() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(1), (1970, 1, 2));
    }

    #[test]
    fn hourly_file_name_and_timestamp_format_as_expected() {
        // 2026-09-25T21:41:06.119Z, i.e. the moment referenced in
        // LESSONS_LEARNED.md's freeze investigation. Verified independently
        // via `date -u -d "2026-09-25 21:41:06" +%s`, not hand-computed.
        let secs = 1790372466u64;
        let clock = clock_from_unix(secs, 119);
        assert_eq!(hourly_file_name(&clock), "2026-09-25-21.log");
        assert_eq!(timestamp(&clock), "2026-09-25T21:41:06.119Z");
    }

    #[test]
    fn hour_boundary_rolls_over_to_a_new_file_name() {
        // Verified via `date -u -d "2026-09-25 21:59:59"/"22:00:00" +%s`.
        let just_before = clock_from_unix(1790373599, 999); // 21:59:59.999
        let just_after = clock_from_unix(1790373600, 0); // 22:00:00.000
        assert_eq!(hourly_file_name(&just_before), "2026-09-25-21.log");
        assert_eq!(hourly_file_name(&just_after), "2026-09-25-22.log");
    }
}
