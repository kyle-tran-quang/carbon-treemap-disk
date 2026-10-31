//! Windows directory scanner.
//!
//! Uses the same parallel-worker architecture as the macOS scanner.  Workers
//! call `std::fs::read_dir` (backed by `FindFirstFileW`/`FindNextFileW`);
//! a future optimisation could switch to direct `NtQueryDirectoryFile` calls
//! for better throughput, but the portable path is the correct baseline.
//!
//! Offline / cloud-placeholder files are detected by the `FILE_ATTRIBUTE_OFFLINE`
//! and `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` flags on Windows 10+, avoiding
//! the latency cost of triggering a download from OneDrive, Google Drive, etc.
//!
//! Reparse points (junctions, symbolic links, mount points) are never followed:
//! they are sized like files and not recursed into.

use std::ffi::OsString;
use std::fs;
use std::io::{self, ErrorKind};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};

use crate::core::config::ScanOptions;
use crate::core::scan::{CancelToken, ScanIssue, ScanIssueKind, ScanOutput, ScanProgress};
use crate::core::tree::{NodeId, Tree};
use crate::platform::Scanner;

use super::WinSizeAccounting;

// FILE_ATTRIBUTE_* values from winnt.h
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
const FILE_ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;
const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;

const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Default thread cap; tune after benchmarking on Windows.
pub(super) const MAX_DEFAULT_THREADS: usize = 8;

#[derive(Debug)]
pub struct WinScanner {
    threads: usize,
}

impl WinScanner {
    pub fn new() -> Self {
        let threads =
            std::thread::available_parallelism().map_or(4, |n| n.get().min(MAX_DEFAULT_THREADS));
        Self { threads }
    }

    pub fn with_threads(threads: usize) -> Self {
        Self {
            threads: threads.max(1),
        }
    }
}

impl Default for WinScanner {
    fn default() -> Self {
        Self::new()
    }
}

struct Job {
    id: NodeId,
    path: PathBuf,
}

enum Entry {
    Dir(OsString),
    File(OsString, u64),
    /// A cloud-placeholder file that must not trigger a download.
    SkippedCloud(OsString),
}

struct DirResult {
    id: NodeId,
    path: PathBuf,
    entries: io::Result<Vec<Entry>>,
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

/// Returns true when the file-attribute flags indicate a cloud placeholder that
/// has not been materialised locally yet.
#[inline]
fn is_cloud_placeholder(attrs: u32) -> bool {
    attrs & (FILE_ATTRIBUTE_OFFLINE | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS) != 0
}

fn read_directory(
    path: &Path,
    root_volume: u32,
    options: &ScanOptions,
    accounting: &WinSizeAccounting,
) -> (io::Result<Vec<Entry>>, Vec<ScanIssue>) {
    let reader = match fs::read_dir(path) {
        Ok(r) => r,
        Err(e) => return (Err(e), Vec::new()),
    };
    let mut entries = Vec::new();
    let mut issues = Vec::new();

    for item in reader {
        let item = match item {
            Ok(item) => item,
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
            // Cross-volume check: compare the volume serial of the root to
            // that of this entry using GetVolumeInformationByHandleW is
            // expensive per-directory; instead we rely on the fact that
            // junctions/mount-points are reparse points (caught above) and
            // skip them.  True cross-volume directory traversal is rare on
            // Windows without explicit junctions, so this is an acceptable
            // simplification.  The `cross_volumes` option is honoured by
            // not blocking based on volume at all (we have no cheap serial).
            if options.cross_volumes || !is_different_volume(&item.path(), root_volume) {
                entries.push(Entry::Dir(item.file_name()));
            }
        } else if !meta.is_dir() {
            // Cloud placeholder: record as skipped when the option is off.
            if is_cloud_placeholder(attrs) && !options.include_cloud_folders {
                entries.push(Entry::SkippedCloud(item.file_name()));
                continue;
            }
            let path = item.path();
            if let Some(bytes) = accounting.bytes_for_path(&path, &meta) {
                entries.push(Entry::File(item.file_name(), bytes));
            }
        }
    }
    (Ok(entries), issues)
}

/// Returns the volume serial number of `path`, or `0` on error.
fn volume_serial(path: &Path) -> u32 {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle, OPEN_EXISTING,
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
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return 0;
    }
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetFileInformationByHandle(handle, &mut info) };
    unsafe { CloseHandle(handle) };
    if ok == 0 {
        0
    } else {
        info.dwVolumeSerialNumber
    }
}

/// Heuristic: returns true when `path` is on a different volume than `root_serial`.
fn is_different_volume(path: &Path, root_serial: u32) -> bool {
    if root_serial == 0 {
        return false; // can't tell; allow traversal
    }
    let serial = volume_serial(path);
    serial != 0 && serial != root_serial
}

fn worker(
    jobs: &Receiver<Job>,
    results: &Sender<DirResult>,
    root_volume: u32,
    options: &ScanOptions,
    accounting: &WinSizeAccounting,
    cancel: &CancelToken,
) {
    while let Ok(job) = jobs.recv() {
        if cancel.is_cancelled() {
            break;
        }
        let (entries, issues) = read_directory(&job.path, root_volume, options, accounting);
        if results
            .send(DirResult {
                id: job.id,
                path: job.path,
                entries,
                issues,
            })
            .is_err()
        {
            break;
        }
    }
}

struct Progress<'a> {
    on_progress: &'a (dyn Fn(ScanProgress) + Sync),
    last_emit: Instant,
    items: u64,
    bytes: u64,
}

impl Progress<'_> {
    fn record(&mut self, items: u64, bytes: u64) {
        self.items += items;
        self.bytes += bytes;
    }

    fn emit(&mut self, current: &Path, force: bool) {
        if force || self.last_emit.elapsed() >= PROGRESS_INTERVAL {
            self.last_emit = Instant::now();
            (self.on_progress)(ScanProgress {
                items_scanned: self.items,
                bytes: self.bytes,
                current_path: current.to_path_buf(),
            });
        }
    }
}

impl Scanner for WinScanner {
    fn scan(
        &self,
        root: &Path,
        options: &ScanOptions,
        cancel: &CancelToken,
        on_progress: &(dyn Fn(ScanProgress) + Sync),
    ) -> io::Result<ScanOutput> {
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
        let root_volume = volume_serial(root);
        let accounting = WinSizeAccounting::new();

        let (job_tx, job_rx) = unbounded::<Job>();
        let (result_tx, result_rx) = unbounded::<DirResult>();

        std::thread::scope(|scope| {
            for _ in 0..self.threads {
                let (jobs, results) = (job_rx.clone(), result_tx.clone());
                let accounting = &accounting;
                scope.spawn(move || {
                    worker(&jobs, &results, root_volume, options, accounting, cancel)
                });
            }
            drop(result_tx);
            collect(root, options, cancel, on_progress, job_tx, &result_rx)
        })
    }
}

fn collect(
    root: &Path,
    options: &ScanOptions,
    cancel: &CancelToken,
    on_progress: &(dyn Fn(ScanProgress) + Sync),
    job_tx: Sender<Job>,
    result_rx: &Receiver<DirResult>,
) -> io::Result<ScanOutput> {
    let mut tree = Tree::new(&root.to_string_lossy());
    let mut issues = Vec::new();
    let mut skipped_cloud_folders = Vec::new();
    let mut progress = Progress {
        on_progress,
        last_emit: Instant::now(),
        items: 0,
        bytes: 0,
    };
    let send_job = |id, path| job_tx.send(Job { id, path }).is_ok();
    if !send_job(NodeId::ROOT, root.to_path_buf()) {
        return Err(io::Error::other("scan workers unavailable"));
    }
    let mut pending = 1usize;

    while pending > 0 {
        if cancel.is_cancelled() {
            return Err(io::Error::new(ErrorKind::Interrupted, "scan cancelled"));
        }
        let result = match result_rx.recv_timeout(CANCEL_POLL_INTERVAL) {
            Ok(r) => r,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                return Err(io::Error::other("scan workers stopped unexpectedly"));
            }
        };
        pending -= 1;
        issues.extend(result.issues);
        match result.entries {
            Err(e) if result.id == NodeId::ROOT => return Err(e),
            Err(e) => {
                tree.mark_inaccessible(result.id);
                issues.push(issue(result.path.clone(), &e));
            }
            Ok(entries) => {
                for entry in entries {
                    match entry {
                        Entry::Dir(name) => {
                            let child = tree.add_dir(result.id, &name.to_string_lossy());
                            if send_job(child, result.path.join(&name)) {
                                pending += 1;
                            }
                            progress.record(1, 0);
                        }
                        Entry::File(name, bytes) => {
                            tree.add_file_or_aggregate(
                                result.id,
                                &name.to_string_lossy(),
                                bytes,
                                options.small_file_threshold,
                            );
                            progress.record(1, bytes);
                        }
                        Entry::SkippedCloud(name) => {
                            skipped_cloud_folders.push(result.path.join(name));
                        }
                    }
                }
            }
        }
        progress.emit(&result.path, false);
    }
    progress.emit(root, true);
    tree.rollup();
    Ok(ScanOutput {
        tree,
        issues,
        skipped_cloud_folders,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tree::NodeKind;
    use std::io::Write;
    use std::sync::Mutex;

    const KIB: u64 = 1024;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ctd-win-scan-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_file(path: &Path, len: u64) {
        let len = usize::try_from(len).unwrap();
        fs::File::create(path)
            .unwrap()
            .write_all(&vec![7u8; len])
            .unwrap();
    }

    fn options() -> ScanOptions {
        ScanOptions {
            small_file_threshold: 64 * KIB,
            ..ScanOptions::default()
        }
    }

    fn run(root: &Path) -> io::Result<ScanOutput> {
        WinScanner::with_threads(4).scan(root, &options(), &CancelToken::default(), &|_| {})
    }

    fn child(tree: &Tree, parent: NodeId, name: &str) -> NodeId {
        tree.children(parent)
            .find(|&id| tree.name(id) == name)
            .unwrap_or_else(|| panic!("no child named {name}"))
    }

    /// root/
    ///   big.bin             256 KiB  -> node
    ///   tiny1, tiny2        100 B    -> aggregated
    ///   sub/deep/leaf.bin   128 KiB  -> node
    ///   empty/
    fn build_fixture(root: &Path) {
        write_file(&root.join("big.bin"), 256 * KIB);
        write_file(&root.join("tiny1"), 100);
        write_file(&root.join("tiny2"), 100);
        fs::create_dir_all(root.join("sub\\deep")).unwrap();
        write_file(&root.join("sub\\deep\\leaf.bin"), 128 * KIB);
        fs::create_dir(root.join("empty")).unwrap();
    }

    #[test]
    fn scans_fixture_with_aggregation_and_rollup() {
        let root = temp_dir("fixture");
        build_fixture(&root);
        let out = run(&root).unwrap();
        let tree = &out.tree;

        let big = child(tree, NodeId::ROOT, "big.bin");
        assert_eq!(tree.kind(big), NodeKind::File);
        assert!(tree.size(big) >= 256 * KIB);
        assert_eq!(tree.small_files(NodeId::ROOT).count, 2);

        let sub = child(tree, NodeId::ROOT, "sub");
        let deep = child(tree, sub, "deep");
        let leaf = child(tree, deep, "leaf.bin");
        assert_eq!(tree.size(deep), tree.size(leaf));
        assert_eq!(tree.size(sub), tree.size(deep));
        assert_eq!(tree.size(child(tree, NodeId::ROOT, "empty")), 0);

        let expected = tree.size(big) + tree.small_files(NodeId::ROOT).bytes + tree.size(sub);
        assert_eq!(tree.size(NodeId::ROOT), expected);
        assert_eq!(tree.file_count(NodeId::ROOT), 4);
        assert!(out.issues.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn hard_links_are_counted_once() {
        let root = temp_dir("hardlink");
        write_file(&root.join("a.bin"), 256 * KIB);
        fs::hard_link(root.join("a.bin"), root.join("b.bin")).unwrap();
        let out = run(&root).unwrap();
        assert_eq!(out.tree.children(NodeId::ROOT).count(), 1);
        assert_eq!(out.tree.file_count(NodeId::ROOT), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn file_root_is_rejected() {
        let root = temp_dir("file-root");
        let file = root.join("f.txt");
        write_file(&file, 1);
        assert_eq!(run(&file).unwrap_err().kind(), ErrorKind::InvalidInput);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_root_is_an_error() {
        let result = run(Path::new(r"C:\no\such\root\ctd"));
        assert!(result.is_err());
    }

    #[test]
    fn pre_cancelled_scan_is_interrupted() {
        let root = temp_dir("cancel");
        build_fixture(&root);
        let cancel = CancelToken::default();
        cancel.cancel();
        let result = WinScanner::with_threads(2).scan(&root, &options(), &cancel, &|_| {});
        assert_eq!(result.unwrap_err().kind(), ErrorKind::Interrupted);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn progress_reports_final_totals() {
        let root = temp_dir("progress");
        build_fixture(&root);
        let last = Mutex::new(None);
        WinScanner::with_threads(2)
            .scan(&root, &options(), &CancelToken::default(), &|p| {
                *last.lock().unwrap() = Some(p);
            })
            .unwrap();
        let last = last.into_inner().unwrap().unwrap();
        assert_eq!(last.items_scanned, 7);
        assert!(last.bytes >= 384 * KIB);
        assert_eq!(last.current_path, root);
        let _ = fs::remove_dir_all(root);
    }
}
