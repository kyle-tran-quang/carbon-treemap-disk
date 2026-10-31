//! Windows `ContentHasher` for the duplicate finder, using BLAKE3.
//!
//! On Windows there is no `O_NOFOLLOW` flag.  Instead the file is opened with
//! `FILE_FLAG_OPEN_REPARSE_POINT` so that opening a symlink returns a handle
//! to the link itself rather than its target.  After opening we verify the
//! file is a regular file (not a directory or reparse point) before reading.
//!
//! The hashing logic is otherwise identical to the macOS implementation.

use std::fs::File;
use std::io::{self, ErrorKind, Read, Seek, SeekFrom};
use std::path::Path;

use crate::core::duplicates::{ContentHasher, Fingerprint};
use crate::core::scan::CancelToken;

/// Bytes sampled from each end of a file for the partial fingerprint.
const PARTIAL_BLOCK: u64 = 64 * 1024;

/// Read size for the full hash. The cancel flag is checked once per chunk.
const CHUNK: u64 = 1024 * 1024;

// FILE_ATTRIBUTE_* constants from winnt.h.
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;

#[derive(Debug, Default, Clone, Copy)]
pub struct WinContentHasher;

impl ContentHasher for WinContentHasher {
    fn partial(&self, path: &Path, size: u64, cancel: &CancelToken) -> io::Result<Fingerprint> {
        let (mut file, len) = open_regular(path)?;
        if len != size {
            return Err(changed());
        }
        if cancel.is_cancelled() {
            return Err(ErrorKind::Interrupted.into());
        }
        if len <= 2 * PARTIAL_BLOCK {
            return hash_stream(&mut file, len, cancel);
        }

        let mut hasher = blake3::Hasher::new();
        hasher.update(&len.to_le_bytes());
        let mut block = vec![0u8; usize_from(PARTIAL_BLOCK)];
        read_block(&mut file, &mut block)?;
        hasher.update(&block);
        file.seek(SeekFrom::Start(len - PARTIAL_BLOCK))?;
        read_block(&mut file, &mut block)?;
        hasher.update(&block);
        Ok(*hasher.finalize().as_bytes())
    }

    fn full(&self, path: &Path, cancel: &CancelToken) -> io::Result<Fingerprint> {
        let (mut file, len) = open_regular(path)?;
        hash_stream(&mut file, len, cancel)
    }
}

/// Opens the file at `path`, refusing symlinks and reparse points.
/// Returns the open file handle and its length.
fn open_regular(path: &Path) -> io::Result<(File, u64)> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{GENERIC_READ, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    // Open without following the reparse point.
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: we own the handle and it is valid.
    let file = unsafe {
        use std::os::windows::io::FromRawHandle;
        File::from_raw_handle(handle as _)
    };

    let meta = file.metadata()?;
    let attrs = {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes()
    };
    if attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0 || attrs & FILE_ATTRIBUTE_DIRECTORY != 0 {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    Ok((file, meta.len()))
}

fn changed() -> io::Error {
    io::Error::new(ErrorKind::InvalidData, "file changed while being read")
}

fn usize_from(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

fn read_block(file: &mut File, block: &mut [u8]) -> io::Result<()> {
    file.read_exact(block).map_err(|err| {
        if err.kind() == ErrorKind::UnexpectedEof {
            changed()
        } else {
            err
        }
    })
}

fn hash_stream(file: &mut File, expected: u64, cancel: &CancelToken) -> io::Result<Fingerprint> {
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; usize_from(expected.clamp(1, CHUNK))];
    let mut total = 0u64;
    loop {
        if cancel.is_cancelled() {
            return Err(ErrorKind::Interrupted.into());
        }
        let read = match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        };
        total += read as u64;
        if total > expected {
            return Err(changed());
        }
        hasher.update(&buf[..read]);
    }
    if total != expected {
        return Err(changed());
    }
    Ok(*hasher.finalize().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ctd-win-hash-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, bytes: &[u8]) {
        fs::File::create(path).unwrap().write_all(bytes).unwrap();
    }

    fn pattern(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i.wrapping_mul(31) >> 3) as u8).collect()
    }

    fn size_of(path: &Path) -> u64 {
        fs::metadata(path).unwrap().len()
    }

    const BIG: usize = 3 * PARTIAL_BLOCK as usize + 1000;

    #[test]
    fn equal_content_gives_equal_fingerprints() {
        let dir = temp_dir("equal");
        let (a, b) = (dir.join("a"), dir.join("b"));
        write(&a, &pattern(BIG));
        write(&b, &pattern(BIG));
        let cancel = CancelToken::default();
        let h = WinContentHasher;
        assert_eq!(
            h.partial(&a, size_of(&a), &cancel).unwrap(),
            h.partial(&b, size_of(&b), &cancel).unwrap()
        );
        assert_eq!(h.full(&a, &cancel).unwrap(), h.full(&b, &cancel).unwrap());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn full_hash_matches_blake3_of_the_content() {
        let dir = temp_dir("known");
        let path = dir.join("f");
        let bytes = pattern(2 * CHUNK as usize + 17);
        write(&path, &bytes);
        let got = WinContentHasher
            .full(&path, &CancelToken::default())
            .unwrap();
        assert_eq!(got, *blake3::hash(&bytes).as_bytes());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn cancel_returns_interrupted() {
        let dir = temp_dir("cancel");
        let path = dir.join("f");
        write(&path, &pattern(BIG));
        let cancel = CancelToken::default();
        cancel.cancel();
        assert_eq!(
            WinContentHasher.full(&path, &cancel).unwrap_err().kind(),
            ErrorKind::Interrupted
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn directory_is_refused() {
        let dir = temp_dir("dir");
        let err = WinContentHasher
            .full(&dir, &CancelToken::default())
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_file_is_an_error() {
        let dir = temp_dir("missing");
        let err = WinContentHasher
            .full(&dir.join("nope"), &CancelToken::default())
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn partial_rejects_a_size_that_no_longer_matches() {
        let dir = temp_dir("sizechange");
        let path = dir.join("f");
        write(&path, &pattern(BIG));
        let err = WinContentHasher
            .partial(&path, size_of(&path) + 1, &CancelToken::default())
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
        let _ = fs::remove_dir_all(dir);
    }
}
