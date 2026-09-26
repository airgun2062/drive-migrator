mod common;

use std::path::PathBuf;

use common::TempTree;
use engine::{build_version_report, DateSource, Relationship, SimilarPair};

fn pair(a: &str, b: &str, relationship: Relationship) -> SimilarPair {
    SimilarPair {
        a: PathBuf::from(a),
        b: PathBuf::from(b),
        jaccard_estimate: 0.9,
        coverage_a_to_b: 0.9,
        coverage_b_to_a: 0.9,
        relationship,
        media_match_score: None,
    }
}

#[test]
fn diverged_pairs_are_flagged_not_merged_into_a_family() {
    let pairs = vec![pair("a.txt", "b.txt", Relationship::Diverged)];
    let report = build_version_report(&pairs);

    assert!(report.families.is_empty());
    assert_eq!(report.flagged_for_manual_merge.len(), 1);
}

#[test]
fn near_duplicate_pair_forms_a_two_member_family_with_neither_a_mandatory_tip() {
    let pairs = vec![pair("a.txt", "b.txt", Relationship::NearDuplicate)];
    let report = build_version_report(&pairs);

    assert_eq!(report.families.len(), 1);
    let family = &report.families[0];
    assert_eq!(family.members.len(), 2);
    // Each fully covers the other, so neither is individually mandatory -
    // keep-newest-N alone decides what survives.
    assert!(family.members.iter().all(|m| !m.is_tip));
}

#[test]
fn containment_pair_ranks_the_more_complete_member_first_and_marks_it_the_tip() {
    // b's content contains a's plus more.
    let pairs = vec![pair("a.txt", "b.txt", Relationship::BIsMoreComplete)];
    let report = build_version_report(&pairs);

    assert_eq!(report.families.len(), 1);
    let family = &report.families[0];
    assert_eq!(family.members.len(), 2);

    let most_complete = &family.members[0];
    let least_complete = &family.members[1];
    assert_eq!(most_complete.rank, 1);
    assert_eq!(most_complete.path, PathBuf::from("b.txt"));
    assert!(
        most_complete.is_tip,
        "the most complete member has unique content nothing else represents"
    );
    assert_eq!(least_complete.rank, 2);
    assert_eq!(least_complete.path, PathBuf::from("a.txt"));
    assert!(
        !least_complete.is_tip,
        "fully covered by the kept member, safe to collapse"
    );
}

#[test]
fn three_tier_chain_ranks_by_transitive_completeness_not_just_direct_edges() {
    // c covers b, b covers a - but `similar` only reports adjacent
    // comparisons, not c-vs-a directly. c must still outrank a, not tie
    // with b, even without a direct edge between them.
    let pairs = vec![
        pair("a.txt", "b.txt", Relationship::BIsMoreComplete), // b covers a
        pair("b.txt", "c.txt", Relationship::BIsMoreComplete), // c covers b
    ];
    let report = build_version_report(&pairs);

    assert_eq!(report.families.len(), 1);
    let family = &report.families[0];
    assert_eq!(family.members.len(), 3);

    let ranks: Vec<(PathBuf, usize)> = family
        .members
        .iter()
        .map(|m| (m.path.clone(), m.rank))
        .collect();
    assert_eq!(
        ranks,
        vec![
            (PathBuf::from("c.txt"), 1),
            (PathBuf::from("b.txt"), 2),
            (PathBuf::from("a.txt"), 3),
        ]
    );

    // Only the most complete member (c, nothing covers it) is a mandatory
    // tip; b and a are each covered by something kept above them.
    assert!(family.members[0].is_tip);
    assert!(!family.members[1].is_tip);
    assert!(!family.members[2].is_tip);
}

#[test]
fn two_families_stay_independent_when_not_connected() {
    let pairs = vec![
        pair("a.txt", "b.txt", Relationship::NearDuplicate),
        pair("x.txt", "y.txt", Relationship::NearDuplicate),
    ];
    let report = build_version_report(&pairs);

    assert_eq!(report.families.len(), 2);
    for family in &report.families {
        assert_eq!(family.members.len(), 2);
    }
}

#[test]
fn a_family_can_include_both_containment_and_near_duplicate_edges() {
    // b covers a; c is a near-duplicate of b (so also related to the
    // family). All three should land in one family.
    let pairs = vec![
        pair("a.txt", "b.txt", Relationship::BIsMoreComplete),
        pair("b.txt", "c.txt", Relationship::NearDuplicate),
    ];
    let report = build_version_report(&pairs);

    assert_eq!(report.families.len(), 1);
    assert_eq!(report.families[0].members.len(), 3);
}

#[test]
fn docx_core_properties_date_wins_the_ranking_tiebreak_over_filesystem_time() {
    // Two near-duplicate docx files where coverage can't discriminate
    // (mutual NearDuplicate): the one with the later internal metadata
    // date should rank first, regardless of which one has the newer
    // on-disk mtime.
    let tree = TempTree::new();
    let newer_core = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <cp:coreProperties xmlns:dcterms=\"http://purl.org/dc/terms/\">\
        <dcterms:modified>2024-06-01T00:00:00Z</dcterms:modified>\
        </cp:coreProperties>";
    let older_core = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <cp:coreProperties xmlns:dcterms=\"http://purl.org/dc/terms/\">\
        <dcterms:modified>2020-01-01T00:00:00Z</dcterms:modified>\
        </cp:coreProperties>";

    let newer_path = tree.path().join("newer.docx");
    let older_path = tree.path().join("older.docx");
    write_docx_with_core_props(&newer_path, newer_core);
    write_docx_with_core_props(&older_path, older_core);

    let pairs = vec![pair(
        older_path.to_str().unwrap(),
        newer_path.to_str().unwrap(),
        Relationship::NearDuplicate,
    )];
    let report = build_version_report(&pairs);

    assert_eq!(report.families.len(), 1);
    let members = &report.families[0].members;
    assert_eq!(members[0].path, newer_path);
    assert_eq!(members[0].date_source, DateSource::OfficeMetadata);
    assert_eq!(members[1].path, older_path);
}

fn write_docx_with_core_props(path: &std::path::Path, core_xml: &str) {
    use std::io::Write as _;
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
    zip.start_file("docProps/core.xml", options).unwrap();
    zip.write_all(core_xml.as_bytes()).unwrap();
    zip.finish().unwrap();
}
