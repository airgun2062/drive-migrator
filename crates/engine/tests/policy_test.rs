use std::path::Path;

use engine::{PreflightIssue, PreflightRules};

fn rules() -> PreflightRules {
    PreflightRules::default()
}

#[test]
fn flags_illegal_characters() {
    let issues = engine::policy::check_path(Path::new("bad:name.txt"), &rules());
    assert!(issues
        .iter()
        .any(|i| matches!(i, PreflightIssue::IllegalCharacter { character: ':' })));
}

#[test]
fn flags_reserved_windows_names() {
    let issues = engine::policy::check_path(Path::new("docs/CON.txt"), &rules());
    assert!(issues
        .iter()
        .any(|i| matches!(i, PreflightIssue::ReservedName { .. })));
}

#[test]
fn flags_trailing_dot_or_space() {
    let issues = engine::policy::check_path(Path::new("folder /file.txt"), &rules());
    assert!(issues
        .iter()
        .any(|i| matches!(i, PreflightIssue::TrailingDotOrSpace { .. })));
}

#[test]
fn flags_path_too_long() {
    let long_name = "a".repeat(300);
    let issues = engine::policy::check_path(Path::new(&long_name), &rules());
    assert!(issues
        .iter()
        .any(|i| matches!(i, PreflightIssue::PathTooLong { .. })));
}

#[test]
fn clean_path_has_no_issues() {
    let issues = engine::policy::check_path(Path::new("docs/report.txt"), &rules());
    assert!(issues.is_empty());
}

#[test]
fn fat32_limit_is_opt_in() {
    let mut r = rules();
    let over_limit = 5_000_000_000u64;

    assert!(engine::policy::check_file(Path::new("big.bin"), over_limit, &r).is_empty());

    r.fat32_max_file_size = Some(4_294_967_295);
    let issues = engine::policy::check_file(Path::new("big.bin"), over_limit, &r);
    assert!(issues
        .iter()
        .any(|i| matches!(i, PreflightIssue::FileTooLargeForFat32 { .. })));
}
