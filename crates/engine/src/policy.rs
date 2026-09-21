use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

/// Destination compatibility rules checked before anything is copied
/// (SPEC.md section 7, "Preflight compatibility report"). Defaults are the
/// most restrictive common denominator (Windows-safe), since P2 does not yet
/// detect the destination filesystem.
#[derive(Debug, Clone)]
pub struct PreflightRules {
    pub max_path_length: usize,
    pub check_windows_reserved_names: bool,
    pub check_illegal_characters: bool,
    pub check_trailing_dot_or_space: bool,
    /// `Some(limit)` enables the FAT32 4 GiB file size check.
    pub fat32_max_file_size: Option<u64>,
}

impl Default for PreflightRules {
    fn default() -> Self {
        Self {
            max_path_length: 255,
            check_windows_reserved_names: true,
            check_illegal_characters: true,
            check_trailing_dot_or_space: true,
            fat32_max_file_size: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind")]
pub enum PreflightIssue {
    PathTooLong {
        length: usize,
        limit: usize,
    },
    IllegalCharacter {
        character: char,
    },
    ReservedName {
        component: String,
    },
    TrailingDotOrSpace {
        component: String,
    },
    FileTooLargeForFat32 {
        size: u64,
        limit: u64,
    },
    /// A different source path normalizes (NFC, case-folded) to the same
    /// destination key, so both cannot be written to a case-insensitive or
    /// NFC-normalizing destination.
    PathCollision {
        with: Vec<PathBuf>,
    },
}

const ILLEGAL_CHARS: [char; 7] = [':', '*', '?', '"', '<', '>', '|'];
const RESERVED_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

pub fn check_path(relative: &Path, rules: &PreflightRules) -> Vec<PreflightIssue> {
    let mut issues = Vec::new();
    let as_str = relative.to_string_lossy();

    if as_str.len() > rules.max_path_length {
        issues.push(PreflightIssue::PathTooLong {
            length: as_str.len(),
            limit: rules.max_path_length,
        });
    }

    if rules.check_illegal_characters {
        let mut seen = HashSet::new();
        for ch in as_str.chars() {
            if ILLEGAL_CHARS.contains(&ch) && seen.insert(ch) {
                issues.push(PreflightIssue::IllegalCharacter { character: ch });
            }
        }
    }

    for component in relative.components() {
        let Component::Normal(part) = component else {
            continue;
        };
        let part = part.to_string_lossy();

        if rules.check_windows_reserved_names {
            let stem = part.split('.').next().unwrap_or(&part);
            if RESERVED_NAMES
                .iter()
                .any(|name| name.eq_ignore_ascii_case(stem))
            {
                issues.push(PreflightIssue::ReservedName {
                    component: part.to_string(),
                });
            }
        }

        if rules.check_trailing_dot_or_space && (part.ends_with('.') || part.ends_with(' ')) {
            issues.push(PreflightIssue::TrailingDotOrSpace {
                component: part.to_string(),
            });
        }
    }

    issues
}

pub fn check_file(relative: &Path, size: u64, rules: &PreflightRules) -> Vec<PreflightIssue> {
    let mut issues = check_path(relative, rules);
    if let Some(limit) = rules.fat32_max_file_size {
        if size > limit {
            issues.push(PreflightIssue::FileTooLargeForFat32 { size, limit });
        }
    }
    issues
}
