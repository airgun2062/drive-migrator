//! One bad file must never take down a whole comparison. Regression tests
//! for two real failures hit testing `similar` against real research data:
//! a genuinely corrupted docx/pptx/xlsx (a truncated zip that was never a
//! valid archive) used to abort the entire run via `?`-propagation, and a
//! Microsoft Office lock file (`~$report.xlsx`, created while the real file
//! is open elsewhere) was being misread as a corrupted document, since it
//! shares the real file's extension but is not a zip archive at all.

mod common;

use common::TempTree;
use engine::{find_similar, SimilarityConfig};

fn words(n: usize) -> String {
    (0..n)
        .map(|i| format!("w{i}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn a_genuinely_corrupted_pptx_is_skipped_instead_of_aborting_the_whole_comparison() {
    let tree = TempTree::new();
    // Not a valid zip archive at all (truncated/never-written), matching
    // the real "Could not find EOCD" failure hit against real data.
    tree.write("broken.pptx", b"this is not a zip archive");
    tree.write("a.txt", words(40).as_bytes());
    tree.write("b.txt", format!("{} extra", words(40)).as_bytes());

    let report = find_similar(
        std::slice::from_ref(&tree.path().to_path_buf()),
        &SimilarityConfig::default(),
    )
    .expect("a corrupted document must not abort the whole comparison");

    // The broken pptx didn't abort anything; the two real text files were
    // still compared normally.
    assert_eq!(report.files_considered, 2);
}
