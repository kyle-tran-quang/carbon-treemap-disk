//! OS-agnostic scan result types shared by `Scanner` implementations.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::tree::Tree;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanIssueKind {
    PermissionDenied,
    Io(String),
}

/// A path that could not be read. Recorded, never fatal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanIssue {
    pub path: PathBuf,
    pub kind: ScanIssueKind,
}

#[derive(Debug)]
pub struct ScanOutput {
    pub tree: Tree,
    pub issues: Vec<ScanIssue>,
    /// Cloud-sync folders left out on purpose. They are not in the tree.
    pub skipped_cloud_folders: Vec<PathBuf>,
}

/// Snapshot passed to the progress callback during a scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanProgress {
    pub items_scanned: u64,
    pub bytes: u64,
    pub current_path: PathBuf,
}

/// Cheap, cloneable flag used to cancel a running scan from another thread.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_is_visible_through_clones() {
        let token = CancelToken::default();
        let other = token.clone();
        assert!(!other.is_cancelled());
        token.cancel();
        assert!(other.is_cancelled());
    }
}
