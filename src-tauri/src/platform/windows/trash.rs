//! Windows Recycle Bin adapter.
//!
//! Uses the `trash` crate which calls `IFileOperation::Delete` (which moves
//! the item to the Recycle Bin rather than permanently deleting it) on Windows.
//! "Put Back" works reliably from the Windows Recycle Bin, unlike the macOS
//! NSFileManager method.  Nothing here deletes permanently.

use std::io;
use std::path::Path;

use crate::platform::TrashProvider;

#[derive(Debug, Default)]
pub struct WinTrashProvider;

impl WinTrashProvider {
    pub fn new() -> Self {
        Self
    }
}

impl TrashProvider for WinTrashProvider {
    fn move_to_trash(&self, path: &Path) -> io::Result<()> {
        trash::delete(path).map_err(|e| match e {
            trash::Error::CanonicalizePath { .. } | trash::Error::CouldNotAccess { .. } => {
                io::Error::new(io::ErrorKind::NotFound, e.to_string())
            }
            other => io::Error::other(other.to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_path_is_reported_as_not_found() {
        let error = WinTrashProvider::new()
            .move_to_trash(Path::new(r"C:\no\such\parent\ctd\file.txt"))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    /// Manual check against the real Recycle Bin.  Run with:
    /// `cargo test win::trash -- --ignored --nocapture`
    /// Leaves one tiny file in the Recycle Bin.
    #[test]
    #[ignore]
    fn moves_a_real_file_to_the_recycle_bin() {
        let dir = std::env::temp_dir().join(format!("ctd-win-trash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("ctd-trash-test-{}.txt", std::process::id()));
        std::fs::write(&file, b"carbon treemap trash test").unwrap();

        WinTrashProvider::new().move_to_trash(&file).unwrap();

        assert!(!file.exists(), "the file must leave its original location");
        let _ = std::fs::remove_dir(&dir);
    }
}
