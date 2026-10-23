//! macOS size accounting: allocated bytes, hard links counted once.

use std::collections::HashSet;
use std::fs::Metadata;
use std::os::unix::fs::MetadataExt;
use std::sync::Mutex;

use crate::platform::SizeAccounting;

/// `st_blocks` is always in 512-byte units, regardless of filesystem block size.
const BLOCK_UNIT: u64 = 512;

#[derive(Debug, Default)]
pub struct MacSizeAccounting {
    seen_links: Mutex<HashSet<(u64, u64)>>,
}

impl MacSizeAccounting {
    pub fn new() -> Self {
        Self::default()
    }

    /// Same rule as `SizeAccounting::bytes_for`, for callers that already hold
    /// the raw attributes instead of a `Metadata`.
    pub fn bytes_for_parts(&self, dev: u64, inode: u64, nlink: u64, allocated: u64) -> Option<u64> {
        if nlink > 1 && self.already_counted(dev, inode) {
            return None;
        }
        Some(allocated)
    }

    fn already_counted(&self, device: u64, inode: u64) -> bool {
        let mut seen = self.seen_links.lock().expect("hard-link set poisoned");
        !seen.insert((device, inode))
    }
}

impl SizeAccounting for MacSizeAccounting {
    fn bytes_for(&self, meta: &Metadata) -> Option<u64> {
        if meta.nlink() > 1 && self.already_counted(meta.dev(), meta.ino()) {
            return None;
        }
        Some(meta.blocks() * BLOCK_UNIT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ctd-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_file(path: &Path, len: usize) {
        fs::File::create(path)
            .unwrap()
            .write_all(&vec![1u8; len])
            .unwrap();
    }

    fn bytes(accounting: &MacSizeAccounting, path: &Path) -> Option<u64> {
        accounting.bytes_for(&fs::symlink_metadata(path).unwrap())
    }

    #[test]
    fn uses_allocated_size() {
        let dir = temp_dir("alloc");
        let file = dir.join("f");
        write_file(&file, 10);
        let counted = bytes(&MacSizeAccounting::new(), &file).unwrap();
        assert!(counted >= 10 && counted.is_multiple_of(BLOCK_UNIT));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn hard_links_are_counted_once() {
        let dir = temp_dir("hardlink");
        let original = dir.join("a");
        let link = dir.join("b");
        write_file(&original, 4096);
        fs::hard_link(&original, &link).unwrap();
        let accounting = MacSizeAccounting::new();
        assert!(bytes(&accounting, &original).is_some());
        assert_eq!(bytes(&accounting, &link), None);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn distinct_files_are_each_counted() {
        let dir = temp_dir("distinct");
        let accounting = MacSizeAccounting::new();
        for name in ["a", "b"] {
            let path = dir.join(name);
            write_file(&path, 4096);
            assert!(bytes(&accounting, &path).is_some());
        }
        fs::remove_dir_all(dir).unwrap();
    }
}
