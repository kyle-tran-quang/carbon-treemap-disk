//! Windows size accounting: compressed/sparse allocated size, hard links once.
//!
//! NTFS can store a file using less disk space than its logical size when the
//! file is compressed or sparse.  `GetCompressedFileSizeW` returns the actual
//! number of bytes allocated on disk, matching what Explorer and `dir` report
//! as the "Size on disk" column.
//!
//! Hard links on NTFS share the same `(volume serial, file index)` pair.
//! The first path seen is counted; subsequent paths for the same underlying
//! data return `None` so the scanner skips them.

use std::collections::HashSet;
use std::fs::Metadata;
use std::os::windows::fs::MetadataExt;
use std::path::Path;
use std::sync::Mutex;

use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    GetCompressedFileSizeW, GetFileInformationByHandle, INVALID_FILE_SIZE, OPEN_EXISTING,
};

use crate::platform::SizeAccounting;

const BLOCK_SIZE: u64 = 512;

#[derive(Debug, Default)]
pub struct WinSizeAccounting {
    seen_links: Mutex<HashSet<(u32, u64)>>,
}

impl WinSizeAccounting {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the allocated size for a file identified by its path.
    ///
    /// Returns `None` when the file is a duplicate hard link that has already
    /// been counted.
    pub fn bytes_for_path(&self, path: &Path, meta: &Metadata) -> Option<u64> {
        // Reparse points (junctions, symlinks) are treated as atomic items
        // sized by their metadata; we never recurse into them.
        // `file_size()` gives the logical size but we want what is actually
        // allocated on disk, taking compression/sparseness into account.
        if let Some((key, links)) = file_key(path)
            && links > 1
            && !self
                .seen_links
                .lock()
                .expect("hard-link set poisoned")
                .insert(key)
        {
            return None;
        }
        Some(compressed_size(path).unwrap_or_else(|| {
            // Fall back to rounding logical size up to the nearest block.
            round_to_block(meta.file_size())
        }))
    }
}

impl SizeAccounting for WinSizeAccounting {
    fn bytes_for(&self, meta: &Metadata) -> Option<u64> {
        // The `SizeAccounting` trait receives a `Metadata` but not a `Path`,
        // so we cannot call `GetCompressedFileSizeW` here.  The scanner uses
        // `bytes_for_path` directly; this impl is present to satisfy the trait
        // and is used only when the scanner falls back to the portable path
        // and cannot supply the path separately.  Without a path we cannot
        // compute a file key, so hard links are conservatively counted.
        Some(round_to_block(meta.file_size()))
    }
}

/// Returns `((volume_serial, file_index), link_count)` for deduplicating hard links.
fn file_key(path: &Path) -> Option<((u32, u64), u32)> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: well-formed wide path; handle is closed before returning.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            0, // no access needed; we only query metadata
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return None;
    }
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetFileInformationByHandle(handle, &mut info) };
    unsafe { CloseHandle(handle) };
    if ok == 0 {
        return None;
    }
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    Some(((info.dwVolumeSerialNumber, index), info.nNumberOfLinks))
}

/// Returns the on-disk (compressed/sparse) size of a file, or `None` on error.
fn compressed_size(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut high: u32 = 0;
    // SAFETY: wide pointer is valid and null-terminated; high is valid.
    let low = unsafe { GetCompressedFileSizeW(wide.as_ptr(), &mut high) };
    if low == INVALID_FILE_SIZE && unsafe { windows_sys::Win32::Foundation::GetLastError() } != 0 {
        return None;
    }
    Some((u64::from(high) << 32) | u64::from(low))
}

fn round_to_block(size: u64) -> u64 {
    size.div_ceil(BLOCK_SIZE) * BLOCK_SIZE
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ctd-win-size-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_file(path: &std::path::Path, len: usize) {
        fs::File::create(path)
            .unwrap()
            .write_all(&vec![1u8; len])
            .unwrap();
    }

    #[test]
    fn hard_links_counted_once() {
        let dir = temp_dir("hl");
        let original = dir.join("a.bin");
        write_file(&original, 8192);
        let link = dir.join("b.bin");
        fs::hard_link(&original, &link).unwrap();
        let accounting = WinSizeAccounting::new();
        let meta_a = fs::symlink_metadata(&original).unwrap();
        let meta_b = fs::symlink_metadata(&link).unwrap();
        assert!(accounting.bytes_for_path(&original, &meta_a).is_some());
        assert_eq!(accounting.bytes_for_path(&link, &meta_b), None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn distinct_files_counted_separately() {
        let dir = temp_dir("dist");
        let accounting = WinSizeAccounting::new();
        for name in ["a.bin", "b.bin"] {
            let path = dir.join(name);
            write_file(&path, 4096);
            let meta = fs::symlink_metadata(&path).unwrap();
            assert!(accounting.bytes_for_path(&path, &meta).is_some());
        }
        let _ = fs::remove_dir_all(dir);
    }
}
