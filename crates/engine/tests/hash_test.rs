mod common;

use common::TempTree;
use engine::hash::{full_hash, sample_hash};

#[test]
fn full_hash_matches_for_identical_content_only() {
    let tree = TempTree::new();
    let a = tree.write("a.bin", b"the quick brown fox");
    let b = tree.write("b.bin", b"the quick brown fox");
    let c = tree.write("c.bin", b"the quick brown FOX");

    let ha = full_hash(&a).unwrap();
    let hb = full_hash(&b).unwrap();
    let hc = full_hash(&c).unwrap();

    assert_eq!(ha, hb);
    assert_ne!(ha, hc);
}

#[test]
fn sample_hash_can_miss_a_difference_in_the_middle() {
    // Larger than 2x the sample chunk so the middle bytes are never sampled.
    let big = 200_000usize;
    let mut left = vec![7u8; big];
    let mut right = left.clone();
    left[big / 2] = 1;
    right[big / 2] = 2;

    let tree = TempTree::new();
    let a = tree.write("a.bin", &left);
    let b = tree.write("b.bin", &right);

    let sa = sample_hash(&a, big as u64).unwrap();
    let sb = sample_hash(&b, big as u64).unwrap();
    // Same head/tail bytes -> same sample hash, even though the files differ.
    // This is the documented limitation: sampling can reject, never confirm.
    assert_eq!(sa, sb);

    let fa = full_hash(&a).unwrap();
    let fb = full_hash(&b).unwrap();
    assert_ne!(fa, fb);
}

#[test]
fn sample_hash_rejects_a_difference_near_the_start() {
    let big = 200_000usize;
    let mut left = vec![7u8; big];
    let mut right = left.clone();
    left[0] = 1;
    right[0] = 2;

    let tree = TempTree::new();
    let a = tree.write("a.bin", &left);
    let b = tree.write("b.bin", &right);

    assert_ne!(
        sample_hash(&a, big as u64).unwrap(),
        sample_hash(&b, big as u64).unwrap()
    );
}

#[test]
fn empty_files_hash_the_same() {
    let tree = TempTree::new();
    let a = tree.write("a.bin", b"");
    let b = tree.write("b.bin", b"");

    assert_eq!(full_hash(&a).unwrap(), full_hash(&b).unwrap());
    assert_eq!(sample_hash(&a, 0).unwrap(), sample_hash(&b, 0).unwrap());
}
