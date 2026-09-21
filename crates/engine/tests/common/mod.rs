use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A synthetic directory tree under the OS temp dir, removed on drop. Tests
/// must never point the engine at real user data (CLAUDE.md hard rule); this
/// helper guarantees every fixture is freshly created and disposable.
///
/// Each `tests/*.rs` file compiles `common` as its own copy, so not every
/// method is used by every test binary.
pub struct TempTree {
    root: PathBuf,
}

#[allow(dead_code)]
impl TempTree {
    pub fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("migrator-engine-test-{}-{}", std::process::id(), n));
        fs::create_dir_all(&root).expect("create temp tree root");
        Self { root }
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Writes `contents` to `relative` under the tree root, creating parent
    /// directories as needed, and returns the full path.
    pub fn write(&self, relative: &str, contents: &[u8]) -> PathBuf {
        let full = self.root.join(relative);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).expect("create parent dir");
        }
        fs::write(&full, contents).expect("write fixture file");
        full
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
