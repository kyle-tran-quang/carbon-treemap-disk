//! Platform trait definitions and per-OS adapters.
//! `core/` depends only on the traits declared here.
//!
//! Trait signatures use only `std` and `core/` types so that an adapter for
//! another OS can implement them without changes to shared code.

use std::fs::Metadata;
use std::io;
use std::path::Path;

use crate::core::config::ScanOptions;
use crate::core::duplicates::{DuplicateOptions, WalkOutput};
use crate::core::scan::{CancelToken, ScanOutput, ScanProgress};

#[cfg(target_os = "macos")]
pub mod macos;

/// Walks a directory tree and builds the arena `Tree`.
/// Implementations must not follow symlinks, must record unreadable paths as
/// issues instead of failing, and must stop promptly when `cancel` is set.
pub trait Scanner: Send + Sync {
    fn scan(
        &self,
        root: &Path,
        options: &ScanOptions,
        cancel: &CancelToken,
        on_progress: &(dyn Fn(ScanProgress) + Sync),
    ) -> io::Result<ScanOutput>;
}

/// Lists the regular files under one root for the duplicate finder.
/// Implementations must not follow symlinks, must skip anything that is not a
/// regular file, should skip files below `options.min_size` (the finder drops
/// them anyway, but keeping them costs memory), must report the device and
/// inode of every file, must record unreadable paths as issues instead of
/// failing, and must stop promptly when `cancel` is set. An error is for a root
/// that cannot be read at all.
pub trait FileWalker: Send + Sync {
    fn walk(
        &self,
        root: &Path,
        options: &DuplicateOptions,
        cancel: &CancelToken,
        on_progress: &(dyn Fn(ScanProgress) + Sync),
    ) -> io::Result<WalkOutput>;
}

/// Decides how many bytes a file contributes to disk usage.
/// Differs per OS (APFS clones, sparse files and hard links on macOS; hard
/// links, junctions and compression on Windows). Implementations keep any
/// de-duplication state internally, so one instance covers one scan.
pub trait SizeAccounting: Send + Sync {
    /// Bytes to attribute to a file given its `lstat` metadata, or `None` when
    /// the underlying data was already counted (for example another hard link
    /// to the same file). Takes metadata so the walker's single stat is reused.
    fn bytes_for(&self, meta: &Metadata) -> Option<u64>;
}

/// Moves items to the OS trash. Must never delete permanently.
pub trait TrashProvider: Send + Sync {
    fn move_to_trash(&self, path: &Path) -> io::Result<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Accessible,
    Denied,
    NotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullDiskAccess {
    Granted,
    Missing,
    /// The probe could not determine the state.
    Unknown,
}

pub trait PermissionChecker: Send + Sync {
    fn check_access(&self, path: &Path) -> Access;
    fn full_disk_access(&self) -> FullDiskAccess;
}
