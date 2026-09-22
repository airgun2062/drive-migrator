//! Extracts a "best available date" for a file, used to rank versions
//! within a family when coverage alone can't discriminate (SPEC.md section
//! 5: "internal metadata first ..., then coverage, then filesystem times as
//! a last resort").
//!
//! Deliberately scoped to the strongest, most direct signal per format:
//! docx/xlsx/pptx core properties use `dcterms:modified` (falling back to
//! `dcterms:created`), not `lastModifiedBy`/`revision`, which are weaker,
//! indirect signals. Every extractor degrades to `None` on anything
//! missing or malformed - a file with no readable internal date just falls
//! through to the filesystem-mtime tier, it never fails the caller.

use std::io::Read as _;
use std::path::Path;
use std::time::SystemTime;

use quick_xml::events::Event;
use quick_xml::Reader;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DateSource {
    OfficeMetadata,
    Exif,
    EmailHeader,
    FilesystemModified,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct DatedFile {
    pub unix_millis: i64,
    pub source: DateSource,
}

/// The date used to rank this file against others in its version family:
/// the first available internal-metadata signal, or the filesystem
/// modified time if none applies.
pub fn best_available_date(path: &Path, filesystem_modified: SystemTime) -> DatedFile {
    if let Some((unix_millis, source)) = internal_date(path) {
        return DatedFile {
            unix_millis,
            source,
        };
    }
    DatedFile {
        unix_millis: unix_millis_from_system_time(filesystem_modified),
        source: DateSource::FilesystemModified,
    }
}

fn internal_date(path: &Path) -> Option<(i64, DateSource)> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "docx" | "pptx" | "xlsx" => {
            office_core_properties_date(path).map(|d| (d, DateSource::OfficeMetadata))
        }
        "jpg" | "jpeg" | "tiff" => exif_date(path).map(|d| (d, DateSource::Exif)),
        "eml" => email_header_date(path).map(|d| (d, DateSource::EmailHeader)),
        _ => None,
    }
}

fn unix_millis_from_system_time(time: SystemTime) -> i64 {
    match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => d.as_millis() as i64,
        Err(e) => -(e.duration().as_millis() as i64),
    }
}

// ---- docx/pptx/xlsx: docProps/core.xml -------------------------------------

fn office_core_properties_date(path: &Path) -> Option<i64> {
    let xml = read_zip_entry_text(path, "docProps/core.xml")?;
    extract_first_xml_tag_text(&xml, "modified")
        .or_else(|| extract_first_xml_tag_text(&xml, "created"))
        .and_then(|s| parse_w3cdtf(&s))
}

fn read_zip_entry_text(path: &Path, entry_name: &str) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut entry = archive.by_name(entry_name).ok()?;
    let mut text = String::new();
    entry.read_to_string(&mut text).ok()?;
    Some(text)
}

/// Text content of the first element whose local name (ignoring any
/// namespace prefix) matches `local_tag`.
fn extract_first_xml_tag_text(xml: &str, local_tag: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut capturing = false;
    let mut out = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if local_name_matches(e.name().as_ref(), local_tag) {
                    capturing = true;
                }
            }
            Ok(Event::Text(t)) if capturing => {
                if let Ok(text) = t.unescape() {
                    out.push_str(&text);
                }
            }
            Ok(Event::End(e)) => {
                if local_name_matches(e.name().as_ref(), local_tag) {
                    return if out.is_empty() { None } else { Some(out) };
                }
            }
            Ok(Event::Eof) => return None,
            Err(_) => return None,
            _ => {}
        }
        buf.clear();
    }
}

fn local_name_matches(qname: &[u8], local: &str) -> bool {
    let s = std::str::from_utf8(qname).unwrap_or("");
    let name = s.rsplit(':').next().unwrap_or(s);
    name == local
}

// ---- jpg/jpeg/tiff: EXIF ----------------------------------------------------

fn exif_date(path: &Path) -> Option<i64> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(file);
    let exif_data = exif::Reader::new().read_from_container(&mut reader).ok()?;
    let field = exif_data
        .get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)
        .or_else(|| exif_data.get_field(exif::Tag::DateTime, exif::In::PRIMARY))?;
    parse_exif_datetime(&field.display_value().to_string())
}

/// EXIF's native format: "YYYY:MM:DD HH:MM:SS" (colons in the date too).
fn parse_exif_datetime(s: &str) -> Option<i64> {
    let (date_part, time_part) = s.trim().split_once(' ')?;
    let mut date_fields = date_part.split(':');
    let year: i64 = date_fields.next()?.parse().ok()?;
    let month: u32 = date_fields.next()?.parse().ok()?;
    let day: u32 = date_fields.next()?.parse().ok()?;

    let mut time_fields = time_part.split(':');
    let hour: i64 = time_fields.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let min: i64 = time_fields.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let sec: i64 = time_fields.next().and_then(|s| s.parse().ok()).unwrap_or(0);

    let days = days_from_civil(year, month, day)?;
    Some((days * 86_400 + hour * 3600 + min * 60 + sec) * 1000)
}

// ---- eml: Date header --------------------------------------------------------

fn email_header_date(path: &Path) -> Option<i64> {
    let bytes = std::fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    for line in text.lines() {
        if let Some(value) = line
            .strip_prefix("Date:")
            .or_else(|| line.strip_prefix("date:"))
        {
            return parse_rfc5322_date(value.trim());
        }
    }
    None
}

/// A practical subset of RFC 5322 dates: optional "Day, " prefix, numeric
/// day, 3-letter month name, 2- or 4-digit year, HH:MM[:SS], and a numeric
/// `+HHMM`/`-HHMM` offset. Named zones (GMT, UTC, ...) are treated as +0000.
fn parse_rfc5322_date(s: &str) -> Option<i64> {
    let s = match s.split_once(", ") {
        Some((_, rest)) => rest,
        None => s,
    };
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() < 4 {
        return None;
    }

    let day: u32 = parts[0].parse().ok()?;
    let month = month_number(parts[1])?;
    let mut year: i64 = parts[2].parse().ok()?;
    if year < 100 {
        year += if year < 70 { 2000 } else { 1900 };
    }

    let mut time_fields = parts[3].split(':');
    let hour: i64 = time_fields.next().and_then(|s| s.parse().ok())?;
    let min: i64 = time_fields.next().and_then(|s| s.parse().ok())?;
    let sec: i64 = time_fields.next().and_then(|s| s.parse().ok()).unwrap_or(0);

    let offset_minutes = parts.get(4).and_then(|tz| parse_tz_offset(tz)).unwrap_or(0);

    let days = days_from_civil(year, month, day)?;
    let unix_seconds = days * 86_400 + hour * 3600 + min * 60 + sec - offset_minutes * 60;
    Some(unix_seconds * 1000)
}

fn month_number(name: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    if name.len() < 3 {
        return None;
    }
    let key = name[..3].to_ascii_lowercase();
    MONTHS.iter().position(|m| *m == key).map(|i| i as u32 + 1)
}

fn parse_tz_offset(tz: &str) -> Option<i64> {
    if tz.len() == 5 && (tz.starts_with('+') || tz.starts_with('-')) {
        let sign: i64 = if tz.starts_with('-') { -1 } else { 1 };
        let hours: i64 = tz[1..3].parse().ok()?;
        let mins: i64 = tz[3..5].parse().ok()?;
        Some(sign * (hours * 60 + mins))
    } else {
        None
    }
}

// ---- W3CDTF (docProps core properties date format) --------------------------

/// "YYYY-MM-DD", optionally followed by "THH:MM:SS" and a "Z" or
/// "+HH:MM"/"-HH:MM" offset.
fn parse_w3cdtf(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date_part, time_part) = match s.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (s, None),
    };

    let mut date_fields = date_part.split('-');
    let year: i64 = date_fields.next()?.parse().ok()?;
    let month: u32 = date_fields.next()?.parse().ok()?;
    let day: u32 = date_fields.next()?.parse().ok()?;

    let (hour, min, sec, offset_minutes) = match time_part {
        Some(t) => {
            let t = t.trim();
            let (t, offset_minutes) = if let Some(rest) = t.strip_suffix('Z') {
                (rest, 0i64)
            } else if let Some(pos) = t.rfind(['+', '-']) {
                if pos >= 6 {
                    let (time_str, tz) = t.split_at(pos);
                    let sign: i64 = if tz.starts_with('-') { -1 } else { 1 };
                    let tz = tz.trim_start_matches(['+', '-']);
                    let mut tz_fields = tz.split(':');
                    let tz_h: i64 = tz_fields.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                    let tz_m: i64 = tz_fields.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                    (time_str, sign * (tz_h * 60 + tz_m))
                } else {
                    (t, 0)
                }
            } else {
                (t, 0)
            };
            let mut time_fields = t.split(':');
            let hour: i64 = time_fields.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let min: i64 = time_fields.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let sec: i64 = time_fields.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            (hour, min, sec, offset_minutes)
        }
        None => (0, 0, 0, 0),
    };

    let days = days_from_civil(year, month, day)?;
    let unix_seconds = days * 86_400 + hour * 3600 + min * 60 + sec - offset_minutes * 60;
    Some(unix_seconds * 1000)
}

/// Howard Hinnant's `days_from_civil`: proleptic Gregorian calendar date to
/// days since the Unix epoch. Public-domain algorithm; avoids pulling in a
/// date/time crate just to convert a handful of parsed date fields.
fn days_from_civil(y: i64, m: u32, d: u32) -> Option<i64> {
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_from_civil_matches_the_unix_epoch() {
        assert_eq!(days_from_civil(1970, 1, 1), Some(0));
        assert_eq!(days_from_civil(1969, 12, 31), Some(-1));
        assert_eq!(days_from_civil(1970, 1, 2), Some(1));
    }

    #[test]
    fn days_from_civil_rejects_out_of_range_month_or_day() {
        assert_eq!(days_from_civil(2024, 13, 1), None);
        assert_eq!(days_from_civil(2024, 1, 32), None);
        assert_eq!(days_from_civil(2024, 0, 1), None);
    }

    #[test]
    fn parse_w3cdtf_handles_the_epoch_with_z_suffix() {
        assert_eq!(parse_w3cdtf("1970-01-01T00:00:00Z"), Some(0));
    }

    #[test]
    fn parse_w3cdtf_handles_a_date_only_value() {
        assert_eq!(parse_w3cdtf("1970-01-02"), Some(86_400_000));
    }

    #[test]
    fn parse_w3cdtf_applies_a_positive_offset() {
        // 05:00 local at +05:00 is 00:00 UTC, i.e. the epoch.
        assert_eq!(parse_w3cdtf("1970-01-01T05:00:00+05:00"), Some(0));
    }

    #[test]
    fn parse_w3cdtf_applies_a_negative_offset() {
        // 19:00 local the day before at -05:00 is 00:00 UTC the next day.
        assert_eq!(parse_w3cdtf("1969-12-31T19:00:00-05:00"), Some(0));
    }

    #[test]
    fn parse_w3cdtf_orders_dates_correctly() {
        let earlier = parse_w3cdtf("2024-01-01T00:00:00Z").unwrap();
        let later = parse_w3cdtf("2024-06-01T00:00:00Z").unwrap();
        assert!(earlier < later);
    }

    #[test]
    fn parse_w3cdtf_rejects_garbage() {
        assert_eq!(parse_w3cdtf("not a date"), None);
    }

    #[test]
    fn parse_exif_datetime_handles_the_native_colon_format() {
        assert_eq!(parse_exif_datetime("1970:01:01 00:00:00"), Some(0));
        assert_eq!(parse_exif_datetime("1970:01:02 00:00:00"), Some(86_400_000));
    }

    #[test]
    fn parse_rfc5322_date_handles_the_epoch() {
        assert_eq!(
            parse_rfc5322_date("Thu, 1 Jan 1970 00:00:00 +0000"),
            Some(0)
        );
    }

    #[test]
    fn parse_rfc5322_date_without_a_day_name_prefix() {
        assert_eq!(parse_rfc5322_date("1 Jan 1970 00:00:00 +0000"), Some(0));
    }

    #[test]
    fn parse_rfc5322_date_applies_a_positive_offset() {
        assert_eq!(
            parse_rfc5322_date("Thu, 1 Jan 1970 05:00:00 +0500"),
            Some(0)
        );
    }

    #[test]
    fn parse_rfc5322_date_applies_a_negative_offset() {
        assert_eq!(
            parse_rfc5322_date("Wed, 31 Dec 1969 19:00:00 -0500"),
            Some(0)
        );
    }

    #[test]
    fn parse_rfc5322_date_treats_a_named_zone_as_utc() {
        assert_eq!(parse_rfc5322_date("Thu, 1 Jan 1970 00:00:00 GMT"), Some(0));
    }

    #[test]
    fn month_number_is_case_insensitive() {
        assert_eq!(month_number("Jan"), Some(1));
        assert_eq!(month_number("DEC"), Some(12));
        assert_eq!(month_number("jul"), Some(7));
        assert_eq!(month_number("bogus"), None);
    }

    #[test]
    fn best_available_date_falls_back_to_filesystem_time_for_unsupported_formats() {
        let now = SystemTime::now();
        let dated = best_available_date(Path::new("plain.txt"), now);
        assert_eq!(dated.source, DateSource::FilesystemModified);
        assert_eq!(dated.unix_millis, unix_millis_from_system_time(now));
    }
}
