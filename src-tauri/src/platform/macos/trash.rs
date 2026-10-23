//! macOS Trash adapter.

use std::io;
use std::path::Path;

use trash::TrashContext;
use trash::macos::{DeleteMethod, TrashContextExtMacos};

use crate::platform::TrashProvider;

/// Moves items to the user's Trash with `NSFileManager.trashItemAtURL`.
///
/// The alternative, asking Finder through AppleScript, keeps "Put Back" but
/// makes macOS prompt for Automation permission the first time. This adapter
/// avoids the prompt. Items still land in the Trash and can be dragged out;
/// "Put Back" may be missing for them (an upstream macOS limitation noted in
/// the `trash` crate docs). Nothing here deletes permanently.
pub struct MacTrashProvider {
    context: TrashContext,
}

impl MacTrashProvider {
    pub fn new() -> Self {
        let mut context = TrashContext::default();
        context.set_delete_method(DeleteMethod::NsFileManager);
        Self { context }
    }
}

impl Default for MacTrashProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl TrashProvider for MacTrashProvider {
    fn move_to_trash(&self, path: &Path) -> io::Result<()> {
        self.context.delete(path).map_err(|e| match e {
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
    use std::fs;

    /// Manual check against the real Trash. Run with:
    /// `cargo test macos::trash -- --ignored --nocapture`
    /// It leaves one tiny file in the user's Trash.
    #[test]
    #[ignore]
    fn moves_a_real_file_to_the_trash() {
        let dir = std::env::temp_dir().join(format!("ctd-trash-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("ctd-trash-test-{}.txt", std::process::id()));
        fs::write(&file, b"carbon treemap trash test").unwrap();

        MacTrashProvider::new().move_to_trash(&file).unwrap();

        assert!(!file.exists(), "the file must leave its original location");
        let _ = fs::remove_dir(&dir);
    }

    #[test]
    fn a_missing_path_is_reported_as_not_found() {
        let error = MacTrashProvider::new()
            .move_to_trash(Path::new("/no/such/parent/ctd/file.txt"))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }
}
