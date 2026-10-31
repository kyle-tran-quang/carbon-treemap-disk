//! Windows permission checker.
//!
//! Windows does not have a blanket "Full Disk Access" permission equivalent
//! to macOS TCC.  Per-folder access is determined by NTFS ACLs; the scanner
//! surfaces individual permission errors as scan issues.
//!
//! `full_disk_access` always returns `Granted` here: the concept does not map
//! to Windows, so the FullDiskAccessNotice in the UI is never shown.
//! `check_access` uses a standard open/read-dir probe, identical to macOS.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use crate::platform::{Access, FullDiskAccess, PermissionChecker};

#[derive(Debug, Default)]
pub struct WinPermissionChecker;

impl WinPermissionChecker {
    pub fn new() -> Self {
        Self
    }
}

impl PermissionChecker for WinPermissionChecker {
    fn check_access(&self, path: &Path) -> Access {
        let result = if path.is_dir() {
            fs::read_dir(path).map(|_| ())
        } else {
            fs::File::open(path).map(|_| ())
        };
        match result {
            Ok(()) => Access::Accessible,
            Err(e) if e.kind() == ErrorKind::PermissionDenied => Access::Denied,
            Err(e) if e.kind() == ErrorKind::NotFound => Access::NotFound,
            Err(_) => Access::Denied,
        }
    }

    /// Windows has no blanket equivalent of macOS Full Disk Access.
    /// Return `Granted` so the onboarding notice is never shown.
    fn full_disk_access(&self) -> FullDiskAccess {
        FullDiskAccess::Granted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_directory_is_accessible() {
        let checker = WinPermissionChecker::new();
        assert_eq!(
            checker.check_access(&std::env::temp_dir()),
            Access::Accessible
        );
    }

    #[test]
    fn missing_path_is_not_found() {
        let checker = WinPermissionChecker::new();
        assert_eq!(
            checker.check_access(Path::new(r"C:\no\such\path\ctd")),
            Access::NotFound
        );
    }

    #[test]
    fn full_disk_access_is_always_granted() {
        assert_eq!(
            WinPermissionChecker::new().full_disk_access(),
            FullDiskAccess::Granted
        );
    }
}
