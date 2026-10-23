//! macOS permission checks.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use crate::platform::{Access, FullDiskAccess, PermissionChecker};

/// Readable only by processes with Full Disk Access, which makes it a
/// reliable probe on current macOS releases.
const FULL_DISK_ACCESS_PROBE: &str = "/Library/Application Support/com.apple.TCC/TCC.db";

#[derive(Debug, Default)]
pub struct MacPermissionChecker;

impl MacPermissionChecker {
    pub fn new() -> Self {
        Self
    }
}

impl PermissionChecker for MacPermissionChecker {
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

    fn full_disk_access(&self) -> FullDiskAccess {
        match fs::File::open(FULL_DISK_ACCESS_PROBE) {
            Ok(_) => FullDiskAccess::Granted,
            Err(e) if e.kind() == ErrorKind::PermissionDenied => FullDiskAccess::Missing,
            Err(_) => FullDiskAccess::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_directory_is_accessible() {
        let checker = MacPermissionChecker::new();
        assert_eq!(
            checker.check_access(&std::env::temp_dir()),
            Access::Accessible
        );
    }

    #[test]
    fn missing_path_is_not_found() {
        let checker = MacPermissionChecker::new();
        assert_eq!(
            checker.check_access(Path::new("/no/such/path/ctd")),
            Access::NotFound
        );
    }
}
