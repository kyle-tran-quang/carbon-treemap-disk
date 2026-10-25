//! macOS `ContentHasher` for the duplicate finder, using BLAKE3.
//!
//! Files are opened with `O_NOFOLLOW | O_NONBLOCK` and must be regular files,
//! so a symlink, FIFO or device that appeared after the walk is refused
//! instead of followed or waited on. Hashing is single-threaded per file: the
//! engine already hashes several files at once, and more threads per file only
//! hurt on slow disks.
//!
//! A file whose length differs from the expected one, at open or by the time
//! the read reaches the end, fails with `InvalidData`. The engine reports that
//! as a file that changed during the scan.

use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Read, Seek, SeekFrom};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use crate::core::duplicates::{ContentHasher, Fingerprint};
use crate::core::scan::CancelToken;

/// Bytes sampled from each end of a file for the partial fingerprint.
const PARTIAL_BLOCK: u64 = 64 * 1024;

/// Read size for the full hash. The cancel flag is checked once per chunk.
const CHUNK: u64 = 1024 * 1024;

#[derive(Debug, Default, Clone, Copy)]
pub struct MacContentHasher;

impl ContentHasher for MacContentHasher {
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

fn open_regular(path: &Path) -> io::Result<(File, u64)> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
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

/// Hashes everything from the current position to the end. `expected` is the
/// length the file had when it was opened; any other number of bytes means it
/// changed under us.
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
            Ok(read) => read,
            // A signal, not a cancel: the check above decides about that.
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
    use std::ffi::CString;
    use std::fs;
    use std::io::Write;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ctd-hash-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, bytes: &[u8]) {
        fs::File::create(path).unwrap().write_all(bytes).unwrap();
    }

    /// Deterministic, non-repeating-looking content of `len` bytes.
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
        let hasher = MacContentHasher;
        assert_eq!(
            hasher.partial(&a, size_of(&a), &cancel).unwrap(),
            hasher.partial(&b, size_of(&b), &cancel).unwrap()
        );
        assert_eq!(
            hasher.full(&a, &cancel).unwrap(),
            hasher.full(&b, &cancel).unwrap()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn full_hash_matches_blake3_of_the_content() {
        let dir = temp_dir("known");
        let path = dir.join("f");
        let bytes = pattern(2 * CHUNK as usize + 17);
        write(&path, &bytes);
        let got = MacContentHasher
            .full(&path, &CancelToken::default())
            .unwrap();
        assert_eq!(got, *blake3::hash(&bytes).as_bytes());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn same_size_with_different_content_differs_in_the_full_hash() {
        let dir = temp_dir("diff");
        let (a, b) = (dir.join("a"), dir.join("b"));
        let mut bytes = pattern(BIG);
        write(&a, &bytes);
        bytes[BIG - 1] ^= 1;
        write(&b, &bytes);
        let cancel = CancelToken::default();
        assert_ne!(
            MacContentHasher.full(&a, &cancel).unwrap(),
            MacContentHasher.full(&b, &cancel).unwrap()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn partial_sees_a_change_in_the_first_or_last_block() {
        let dir = temp_dir("ends");
        let base = dir.join("base");
        write(&base, &pattern(BIG));
        let cancel = CancelToken::default();
        let reference = MacContentHasher
            .partial(&base, size_of(&base), &cancel)
            .unwrap();
        for index in [0, BIG - 1] {
            let other = dir.join(format!("other-{index}"));
            let mut bytes = pattern(BIG);
            bytes[index] ^= 1;
            write(&other, &bytes);
            let got = MacContentHasher
                .partial(&other, size_of(&other), &cancel)
                .unwrap();
            assert_ne!(got, reference, "change at byte {index} not seen");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn partial_misses_a_middle_change_but_full_catches_it() {
        // This is why the engine always confirms a partial match with a full hash.
        let dir = temp_dir("middle");
        let (a, b) = (dir.join("a"), dir.join("b"));
        let mut bytes = pattern(BIG);
        write(&a, &bytes);
        bytes[BIG / 2] ^= 1;
        write(&b, &bytes);
        let cancel = CancelToken::default();
        assert_eq!(
            MacContentHasher.partial(&a, size_of(&a), &cancel).unwrap(),
            MacContentHasher.partial(&b, size_of(&b), &cancel).unwrap()
        );
        assert_ne!(
            MacContentHasher.full(&a, &cancel).unwrap(),
            MacContentHasher.full(&b, &cancel).unwrap()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn small_files_are_fingerprinted_whole() {
        let dir = temp_dir("small");
        let (a, b) = (dir.join("a"), dir.join("b"));
        let mut bytes = pattern(1000);
        write(&a, &bytes);
        bytes[500] ^= 1;
        write(&b, &bytes);
        let cancel = CancelToken::default();
        assert_ne!(
            MacContentHasher.partial(&a, 1000, &cancel).unwrap(),
            MacContentHasher.partial(&b, 1000, &cancel).unwrap()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancel_returns_interrupted() {
        let dir = temp_dir("cancel");
        let path = dir.join("f");
        write(&path, &pattern(BIG));
        let cancel = CancelToken::default();
        cancel.cancel();
        let full = MacContentHasher.full(&path, &cancel).unwrap_err();
        let partial = MacContentHasher
            .partial(&path, size_of(&path), &cancel)
            .unwrap_err();
        assert_eq!(full.kind(), ErrorKind::Interrupted);
        assert_eq!(partial.kind(), ErrorKind::Interrupted);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_is_an_error() {
        let dir = temp_dir("missing");
        let err = MacContentHasher
            .full(&dir.join("nope"), &CancelToken::default())
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unreadable_file_is_an_error() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("denied");
        let path = dir.join("f");
        write(&path, &pattern(100));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        let result = MacContentHasher.full(&path, &CancelToken::default());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let _ = fs::remove_dir_all(&dir);
        // Running as root can read it anyway.
        if let Err(err) = result {
            assert_eq!(err.kind(), ErrorKind::PermissionDenied);
        }
    }

    #[test]
    fn symlink_is_refused_not_followed() {
        let dir = temp_dir("symlink");
        let target = dir.join("target");
        let link = dir.join("link");
        write(&target, &pattern(BIG));
        symlink(&target, &link).unwrap();
        let cancel = CancelToken::default();
        assert!(MacContentHasher.full(&link, &cancel).is_err());
        assert!(
            MacContentHasher
                .partial(&link, size_of(&target), &cancel)
                .is_err()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn fifo_is_refused_without_blocking() {
        let dir = temp_dir("fifo");
        let fifo = dir.join("pipe");
        let c_path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: `c_path` is a valid NUL-terminated string that outlives the call.
        let made = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
        assert_eq!(made, 0, "mkfifo failed");
        let err = MacContentHasher
            .full(&fifo, &CancelToken::default())
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn directory_is_refused() {
        let dir = temp_dir("dir");
        let err = MacContentHasher
            .full(&dir, &CancelToken::default())
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn partial_rejects_a_size_that_no_longer_matches() {
        let dir = temp_dir("sizechanged");
        let path = dir.join("f");
        write(&path, &pattern(BIG));
        let err = MacContentHasher
            .partial(&path, size_of(&path) + 1, &CancelToken::default())
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stream_that_ends_early_or_runs_long_is_rejected() {
        let dir = temp_dir("stream");
        let path = dir.join("f");
        write(&path, &pattern(5000));
        let cancel = CancelToken::default();

        // The file shrank after the length was read.
        let mut file = File::open(&path).unwrap();
        let err = hash_stream(&mut file, 6000, &cancel).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);

        // The file grew after the length was read.
        let mut file = File::open(&path).unwrap();
        let err = hash_stream(&mut file, 4000, &cancel).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);

        // Correct length passes.
        let mut file = File::open(&path).unwrap();
        assert!(hash_stream(&mut file, 5000, &cancel).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    /// Manual, read-only report on the real home folder. Run with
    /// `cargo test --release real_home_hash -- --ignored --nocapture`.
    /// Opens files for reading only; nothing is moved or written.
    #[test]
    #[ignore = "manual: reads real files under $HOME"]
    fn real_home_hash_report() {
        use crate::core::duplicates::{DuplicateOptions, DuplicateRequest, find_duplicates};
        use crate::platform::FileWalker;
        use crate::platform::macos::MacFileWalker;
        use std::collections::{BTreeSet, HashMap};
        use std::time::Instant;

        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME is set"));
        let options = DuplicateOptions::default();
        let cancel = CancelToken::default();
        let walker = MacFileWalker::new();

        let started = Instant::now();
        let scan = find_duplicates(
            &walker,
            &MacContentHasher,
            &DuplicateRequest {
                scan_roots: std::slice::from_ref(&home),
                protected_roots: &[],
                options: &options,
            },
            &cancel,
            &|_| {},
        )
        .unwrap();
        let elapsed = started.elapsed();
        let in_groups: usize = scan.groups.iter().map(|g| g.files.len()).sum();
        println!(
            "end to end: {:.2?}, {} files listed, {} groups ({} files), {:.2} GB reclaimable, \
             {:.2} GB hashed in full ({:.0} MB/s overall), {} issues",
            elapsed,
            scan.files_listed,
            scan.groups.len(),
            in_groups,
            scan.reclaimable_bytes() as f64 / 1e9,
            scan.bytes_hashed as f64 / 1e9,
            scan.bytes_hashed as f64 / 1e6 / elapsed.as_secs_f64(),
            scan.issues.len(),
        );
        for issue in scan.issues.iter().take(5) {
            println!("issue: {} {:?}", issue.path.display(), issue.kind);
        }

        // Count files that matched on size and partial fingerprint but have no
        // full-hash twin. The engine only keeps the confirmed groups, so redo
        // the cheap stages here.
        let walked = walker.walk(&home, &options, &cancel, &|_| {}).unwrap();
        let mut seen = BTreeSet::new();
        let mut by_size: HashMap<u64, Vec<_>> = HashMap::new();
        for file in walked.files {
            if seen.insert((file.device, file.inode)) {
                by_size.entry(file.size).or_default().push(file);
            }
        }
        let started = Instant::now();
        let mut partial_matches = 0usize;
        let mut sampled = 0usize;
        let mut by_partial: HashMap<(u64, Fingerprint), usize> = HashMap::new();
        for (size, files) in by_size.iter().filter(|(_, f)| f.len() > 1) {
            for file in files {
                sampled += 1;
                if let Ok(fp) = MacContentHasher.partial(&file.path, *size, &cancel) {
                    *by_partial.entry((*size, fp)).or_default() += 1;
                }
            }
        }
        for count in by_partial.values().filter(|c| **c > 1) {
            partial_matches += count;
        }
        println!(
            "partial stage: {sampled} files sampled in {:.2?}; {partial_matches} matched on size \
             and partial fingerprint; {in_groups} confirmed by full hash; {} partial-only false \
             positives",
            started.elapsed(),
            partial_matches.saturating_sub(in_groups),
        );
    }
}
