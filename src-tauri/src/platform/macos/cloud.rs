//! Recognises cloud-sync folders by location.
//!
//! Listing a File Provider mount (Box, Dropbox, OneDrive, Google Drive) or
//! iCloud Drive can take minutes because the provider fetches metadata on
//! first access. The scan leaves them out by default. They are recognised by
//! path only, so the check costs no filesystem calls.
//!
//! Not covered: iCloud "Desktop and Documents" sync, whose files live in the
//! ordinary `~/Desktop` and `~/Documents` folders.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct CloudFolders {
    /// Each child of this folder is one provider mount.
    mounts_parent: PathBuf,
    /// Skipped as a whole.
    icloud_drive: PathBuf,
}

impl CloudFolders {
    pub fn for_home(home: &Path) -> Self {
        let library = home.join("Library");
        Self {
            mounts_parent: library.join("CloudStorage"),
            icloud_drive: library.join("Mobile Documents"),
        }
    }

    pub fn is_cloud_folder(&self, path: &Path) -> bool {
        path == self.icloud_drive || path.parent() == Some(self.mounts_parent.as_path())
    }

    /// True when `root` is a cloud location or inside one. Choosing such a
    /// folder is deliberate, so nothing below it is skipped.
    pub fn contains_root(&self, root: &Path) -> bool {
        root.starts_with(&self.mounts_parent) || root.starts_with(&self.icloud_drive)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folders() -> CloudFolders {
        CloudFolders::for_home(Path::new("/Users/me"))
    }

    #[test]
    fn provider_mounts_and_icloud_drive_are_cloud_folders() {
        let cloud = folders();
        assert!(cloud.is_cloud_folder(Path::new("/Users/me/Library/CloudStorage/Box-Box")));
        assert!(cloud.is_cloud_folder(Path::new("/Users/me/Library/Mobile Documents")));
    }

    #[test]
    fn ordinary_folders_and_the_parents_are_not() {
        let cloud = folders();
        assert!(!cloud.is_cloud_folder(Path::new("/Users/me/Library")));
        assert!(!cloud.is_cloud_folder(Path::new("/Users/me/Library/CloudStorage")));
        assert!(!cloud.is_cloud_folder(Path::new("/Users/me/Documents")));
        assert!(!cloud.is_cloud_folder(Path::new("/Users/me/Library/Caches")));
    }

    #[test]
    fn folders_inside_a_mount_are_not_matched_separately() {
        let cloud = folders();
        assert!(!cloud.is_cloud_folder(Path::new("/Users/me/Library/CloudStorage/Box-Box/a")));
    }

    #[test]
    fn a_root_inside_a_cloud_location_is_recognised() {
        let cloud = folders();
        assert!(cloud.contains_root(Path::new("/Users/me/Library/CloudStorage")));
        assert!(cloud.contains_root(Path::new("/Users/me/Library/CloudStorage/Box-Box/x")));
        assert!(cloud.contains_root(Path::new("/Users/me/Library/Mobile Documents")));
        assert!(!cloud.contains_root(Path::new("/Users/me")));
        assert!(!cloud.contains_root(Path::new("/Users/me/Library")));
    }
}
