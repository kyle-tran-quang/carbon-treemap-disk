//! Windows `FileWalker` for the duplicate finder.
//!
//! Same parallel-worker architecture as `WinScanner`.  Only regular files
//! (not symlinks, junctions, reparse points or devices) are reported.
//! Offline / cloud-placeholder files are skipped the same way as the scanner.

use std::ffi::OsString;
use std::fs;
use std::io::{self, ErrorKind};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};

use crate::core::duplicate_exclusions::Exclusions;
use crate::core::duplicates::{DuplicateOptions, WalkOutput, WalkedFile};
use crate::core::scan::{CancelToken, ScanIssue, ScanIssueKind, ScanProgress};
use crate::platform::FileWalker;

use super::scanner::MAX_DEFAULT_THREADS;

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
const FILE_ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;
const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;

const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug)]
pub struct WinFileWalker {
    threads: usize,
    home: Option<PathBuf>,
}

impl WinFileWalker {
    pub fn new() -> Self {
        let threads =
            std::thread::available_parallelism().map_or(4, |n| n.get().min(MAX_DEFAULT_THREADS));
        Self {
            threads,
            home: home_dir(),
        }
    }

    #[cfg(test)]
    fn with_threads(mut self, threads: usize) -> Self {
        self.threads = threads.max(1);
        self
    }

    #[cfg(test)]
    fn with_home(mut self, home: Option<PathBuf>) -> Self {
        self.home = home;
        self
    }
}

impl Default for WinFileWalker {
    fn default() -> Self {
        Self::new()
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

struct Context<'a> {
    min_size: u64,
    exclusions: Exclusions,
    home: Option<&'a Path>,
    include_cloud: bool,
}

impl Context<'_> {
    fn should_skip_dir(&self, parent: &Path, name: &OsString) -> bool {
        self.exclusions.check(self.home, parent, name).is_some()
    }
}

#[derive(Default)]
struct Listing {
    files: Vec<WalkedFile>,
    dirs: Vec<OsString>,
    skipped_cloud: Vec<PathBuf>,
}

struct DirResult {
    path: PathBuf,
    listing: io::Result<Listing>,
    issues: Vec<ScanIssue>,
}

fn issue(path: PathBuf, error: &io::Error) -> ScanIssue {
    let kind = if error.kind() == ErrorKind::PermissionDenied {
        ScanIssueKind::PermissionDenied
    } else {
        ScanIssueKind::Io(error.to_string())
    };
    ScanIssue { path, kind }
}

fn read_directory(path: &Path, ctx: &Context) -> (io::Result<Listing>, Vec<ScanIssue>) {
    let reader = match fs::read_dir(path) {
        Ok(r) => r,
        Err(e) => return (Err(e), Vec::new()),
    };
    let mut listing = Listing::default();
    let mut issues = Vec::new();

    for item in reader {
        let item = match item {
            Ok(i) => i,
            Err(e) => {
                issues.push(issue(path.to_path_buf(), &e));
                continue;
            }
        };
        let meta = match item.metadata() {
            Ok(m) => m,
            Err(e) => {
                issues.push(issue(item.path(), &e));
                continue;
            }
        };
        let attrs = meta.file_attributes();
        let is_reparse = attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0;

        if meta.is_dir() && !is_reparse {
            let name = item.file_name();
            if !ctx.should_skip_dir(path, &name) {
                listing.dirs.push(name);
            }
        } else if meta.file_type().is_file() && !is_reparse {
            // Cloud placeholder: skip unless the option is on.
            let cloud =
                attrs & (FILE_ATTRIBUTE_OFFLINE | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS) != 0;
            if cloud && !ctx.include_cloud {
                listing.skipped_cloud.push(item.path());
                continue;
            }
            let size = meta.file_size();
            if size >= ctx.min_size {
                let (device, inode) = win_file_id(item.path().as_path()).unwrap_or((0, 0));
                let modified_secs = meta
                    .last_write_time()
                    .checked_sub(116_444_736_000_000_000) // FILETIME epoch offset (100-ns units)
                    .map(|ft| ft / 10_000_000) // to seconds
                    .unwrap_or(0);
                listing.files.push(WalkedFile {
                    path: item.path(),
                    size,
                    device,
                    inode,
                    modified_secs,
                });
            }
        }
    }
    (Ok(listing), issues)
}

/// Returns `(volume_serial_as_u64, file_index)` for hard-link deduplication.
fn win_file_id(path: &Path) -> Option<(u64, u64)> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        GetFileInformationByHandle, OPEN_EXISTING,
    };

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetFileInformationByHandle(handle, &mut info) };
    unsafe { CloseHandle(handle) };
    if ok == 0 {
        return None;
    }
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    Some((u64::from(info.dwVolumeSerialNumber), index))
}

fn worker(
    jobs: &Receiver<PathBuf>,
    results: &Sender<DirResult>,
    ctx: &Context,
    cancel: &CancelToken,
) {
    while let Ok(path) = jobs.recv() {
        if cancel.is_cancelled() {
            break;
        }
        let (listing, issues) = read_directory(&path, ctx);
        if results
            .send(DirResult {
                path,
                listing,
                issues,
            })
            .is_err()
        {
            break;
        }
    }
}

impl FileWalker for WinFileWalker {
    fn walk(
        &self,
        root: &Path,
        options: &DuplicateOptions,
        cancel: &CancelToken,
        on_progress: &(dyn Fn(ScanProgress) + Sync),
    ) -> io::Result<WalkOutput> {
        let root_meta = fs::symlink_metadata(root)?;
        if !root_meta.is_dir() {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "scan root is not a directory",
            ));
        }
        if root_meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "scan root is a reparse point",
            ));
        }
        let ctx = Context {
            min_size: options.min_size.max(1),
            exclusions: options.exclusions,
            home: self.home.as_deref(),
            include_cloud: options.include_cloud_folders,
        };
        let (job_tx, job_rx) = unbounded::<PathBuf>();
        let (result_tx, result_rx) = unbounded::<DirResult>();

        std::thread::scope(|scope| {
            for _ in 0..self.threads {
                let (jobs, results) = (job_rx.clone(), result_tx.clone());
                let ctx = &ctx;
                scope.spawn(move || worker(&jobs, &results, ctx, cancel));
            }
            drop(result_tx);
            collect(root, cancel, on_progress, job_tx, &result_rx)
        })
    }
}

fn collect(
    root: &Path,
    cancel: &CancelToken,
    on_progress: &(dyn Fn(ScanProgress) + Sync),
    job_tx: Sender<PathBuf>,
    result_rx: &Receiver<DirResult>,
) -> io::Result<WalkOutput> {
    let mut output = WalkOutput::default();
    let (mut items, mut bytes) = (0u64, 0u64);
    let mut last_emit = Instant::now();
    if job_tx.send(root.to_path_buf()).is_err() {
        return Err(io::Error::other("walk workers unavailable"));
    }
    let mut pending = 1usize;

    while pending > 0 {
        if cancel.is_cancelled() {
            return Err(io::Error::new(ErrorKind::Interrupted, "walk cancelled"));
        }
        let result = match result_rx.recv_timeout(CANCEL_POLL_INTERVAL) {
            Ok(r) => r,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                return Err(io::Error::other("walk workers stopped unexpectedly"));
            }
        };
        pending -= 1;
        output.issues.extend(result.issues);
        match result.listing {
            Err(e) if result.path == root => return Err(e),
            Err(e) => output.issues.push(issue(result.path.clone(), &e)),
            Ok(listing) => {
                for name in listing.dirs {
                    if job_tx.send(result.path.join(name)).is_ok() {
                        pending += 1;
                    }
                }
                items += listing.files.len() as u64;
                bytes += listing.files.iter().map(|f| f.size).sum::<u64>();
                output.files.extend(listing.files);
                output.skipped_cloud_folders.extend(listing.skipped_cloud);
            }
        }
        if last_emit.elapsed() >= PROGRESS_INTERVAL {
            last_emit = Instant::now();
            on_progress(ScanProgress {
                items_scanned: items,
                bytes,
                current_path: result.path,
            });
        }
    }
    on_progress(ScanProgress {
        items_scanned: items,
        bytes,
        current_path: root.to_path_buf(),
    });
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::io::Write;

    const KIB: u64 = 1024;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ctd-win-walk-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_file(path: &Path, len: u64) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let len = usize::try_from(len).unwrap();
        fs::File::create(path)
            .unwrap()
            .write_all(&vec![7u8; len])
            .unwrap();
    }

    fn options(min_size: u64) -> DuplicateOptions {
        DuplicateOptions {
            min_size,
            ..DuplicateOptions::default()
        }
    }

    fn walker() -> WinFileWalker {
        WinFileWalker::new().with_threads(4).with_home(None)
    }

    fn walk(root: &Path, min_size: u64) -> WalkOutput {
        walker()
            .walk(root, &options(min_size), &CancelToken::default(), &|_| {})
            .unwrap()
    }

    fn names(out: &WalkOutput, root: &Path) -> BTreeSet<String> {
        out.files
            .iter()
            .map(|f| {
                f.path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
                    .replace('\\', "/")
            })
            .collect()
    }

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn lists_files_at_or_above_minimum_size() {
        let root = temp_dir("sizes");
        write_file(&root.join("big.bin"), 8 * KIB);
        write_file(&root.join("small.bin"), 100);
        write_file(&root.join("sub\\leaf.bin"), 4 * KIB);
        let out = walk(&root, 4 * KIB);
        assert_eq!(names(&out, &root), set(&["big.bin", "sub/leaf.bin"]));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn hard_links_report_same_device_and_inode() {
        let root = temp_dir("hard");
        write_file(&root.join("a.bin"), 8 * KIB);
        fs::hard_link(root.join("a.bin"), root.join("b.bin")).unwrap();
        let out = walk(&root, 1);
        assert_eq!(out.files.len(), 2);
        assert_eq!(out.files[0].inode, out.files[1].inode);
        assert_eq!(out.files[0].device, out.files[1].device);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_root_is_an_error() {
        let err = walker()
            .walk(
                Path::new(r"C:\no\such\ctd"),
                &options(1),
                &CancelToken::default(),
                &|_| {},
            )
            .unwrap_err();
        assert!(err.kind() == ErrorKind::NotFound || err.kind() == ErrorKind::PermissionDenied);
    }

    #[test]
    fn cancelled_walk_returns_interrupted() {
        let root = temp_dir("cancel");
        write_file(&root.join("a.bin"), 8 * KIB);
        let cancel = CancelToken::default();
        cancel.cancel();
        let err = walker()
            .walk(&root, &options(1), &cancel, &|_| {})
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Interrupted);
        let _ = fs::remove_dir_all(root);
    }
}
