//! Parallel macOS directory scanner.
//!
//! Worker threads read directories and stat entries; the calling thread is the
//! single owner of the `Tree` and applies each directory's result as it
//! arrives. Because results are applied incrementally, partial sizes are
//! available while the scan runs. Symlinks are never followed; they are
//! treated as files and sized by the platform accounting.

use std::ffi::OsString;
use std::fs;
use std::io::{self, ErrorKind};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};

use crate::core::config::ScanOptions;
use crate::core::scan::{CancelToken, ScanIssue, ScanIssueKind, ScanOutput, ScanProgress};
use crate::core::tree::{NodeId, Tree};
use crate::platform::{Scanner, SizeAccounting};

use super::MacSizeAccounting;
use super::bulk::{BulkKind, read_dir_bulk};
use super::cloud::CloudFolders;

/// Benchmarks showed no gain (and a small loss) beyond 8 threads.
pub(super) const MAX_DEFAULT_THREADS: usize = 8;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How a single directory is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadMode {
    /// `readdir` plus one `lstat` per entry.
    Portable,
    /// `getattrlistbulk`: attributes for many entries per syscall.
    Bulk,
}

#[derive(Debug)]
pub struct MacScanner {
    threads: usize,
    mode: ReadMode,
    /// Used to find the cloud-sync folders to skip. `None` skips nothing.
    home: Option<PathBuf>,
}

impl MacScanner {
    pub fn new() -> Self {
        let threads =
            std::thread::available_parallelism().map_or(4, |n| n.get().min(MAX_DEFAULT_THREADS));
        Self {
            threads,
            mode: ReadMode::Bulk,
            home: std::env::var_os("HOME").map(PathBuf::from),
        }
    }

    #[cfg(test)]
    fn with_home(mut self, home: Option<PathBuf>) -> Self {
        self.home = home;
        self
    }

    /// A root the user picked inside a cloud location is scanned as asked.
    fn cloud_folders_to_skip(&self, root: &Path, options: &ScanOptions) -> Option<CloudFolders> {
        if options.include_cloud_folders {
            return None;
        }
        let cloud = CloudFolders::for_home(self.home.as_deref()?);
        (!cloud.contains_root(root)).then_some(cloud)
    }

    pub fn with_threads(threads: usize) -> Self {
        Self {
            threads: threads.max(1),
            ..Self::new()
        }
    }

    pub fn with_mode(mut self, mode: ReadMode) -> Self {
        self.mode = mode;
        self
    }
}

impl Default for MacScanner {
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
    /// A cloud-sync folder that is deliberately not read.
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

struct ReadContext<'a> {
    accounting: &'a MacSizeAccounting,
    root_dev: u64,
    options: &'a ScanOptions,
    mode: ReadMode,
    /// Set only when cloud folders are to be skipped.
    skip_cloud: Option<CloudFolders>,
}

impl ReadContext<'_> {
    /// Decides what a subdirectory becomes: read, or skipped as cloud storage.
    fn dir_entry(&self, parent: &Path, name: OsString) -> Entry {
        match &self.skip_cloud {
            Some(cloud) if cloud.is_cloud_folder(&parent.join(&name)) => Entry::SkippedCloud(name),
            _ => Entry::Dir(name),
        }
    }
}

/// Reads one directory. Per-entry failures become issues; only a failure to
/// open the directory itself is returned as an error.
fn read_directory(path: &Path, ctx: &ReadContext) -> (io::Result<Vec<Entry>>, Vec<ScanIssue>) {
    match ctx.mode {
        ReadMode::Portable => read_directory_portable(path, ctx),
        ReadMode::Bulk => read_directory_bulk(path, ctx),
    }
}

fn read_directory_bulk(path: &Path, ctx: &ReadContext) -> (io::Result<Vec<Entry>>, Vec<ScanIssue>) {
    let listing = match read_dir_bulk(path) {
        Ok(listing) => listing,
        // Some filesystems (certain network and FUSE mounts) lack support.
        Err(e) if matches!(e.raw_os_error(), Some(libc::ENOTSUP | libc::EINVAL)) => {
            return read_directory_portable(path, ctx);
        }
        Err(e) => return (Err(e), Vec::new()),
    };
    let issues = listing
        .errors
        .iter()
        .map(|e| issue(path.join(&e.name), &e.error))
        .collect();
    let mut entries = Vec::with_capacity(listing.entries.len());
    for item in listing.entries {
        match item.kind {
            BulkKind::Dir => {
                // `st_dev` widens a 32-bit device id, so compare the low half.
                if ctx.options.cross_volumes || item.dev == ctx.root_dev as u32 {
                    entries.push(ctx.dir_entry(path, item.name));
                }
            }
            BulkKind::Other => {
                let counted = ctx.accounting.bytes_for_parts(
                    u64::from(item.dev),
                    item.inode,
                    u64::from(item.nlink),
                    item.allocated,
                );
                if let Some(bytes) = counted {
                    entries.push(Entry::File(item.name, bytes));
                }
            }
        }
    }
    (Ok(entries), issues)
}

fn read_directory_portable(
    path: &Path,
    ctx: &ReadContext,
) -> (io::Result<Vec<Entry>>, Vec<ScanIssue>) {
    let (accounting, root_dev, options) = (ctx.accounting, ctx.root_dev, ctx.options);
    let reader = match fs::read_dir(path) {
        Ok(reader) => reader,
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
            Ok(meta) => meta,
            Err(e) => {
                issues.push(issue(item.path(), &e));
                continue;
            }
        };
        if meta.is_dir() {
            if options.cross_volumes || meta.dev() == root_dev {
                entries.push(ctx.dir_entry(path, item.file_name()));
            }
        } else if let Some(bytes) = accounting.bytes_for(&meta) {
            entries.push(Entry::File(item.file_name(), bytes));
        }
    }
    (Ok(entries), issues)
}

fn worker(
    jobs: &Receiver<Job>,
    results: &Sender<DirResult>,
    ctx: &ReadContext,
    cancel: &CancelToken,
) {
    while let Ok(job) = jobs.recv() {
        if cancel.is_cancelled() {
            break;
        }
        let (entries, issues) = read_directory(&job.path, ctx);
        let result = DirResult {
            id: job.id,
            path: job.path,
            entries,
            issues,
        };
        if results.send(result).is_err() {
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

impl Scanner for MacScanner {
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
        let accounting = MacSizeAccounting::new();
        let ctx = ReadContext {
            accounting: &accounting,
            root_dev: root_meta.dev(),
            options,
            mode: self.mode,
            skip_cloud: self.cloud_folders_to_skip(root, options),
        };
        let (job_tx, job_rx) = unbounded::<Job>();
        let (result_tx, result_rx) = unbounded::<DirResult>();

        std::thread::scope(|scope| {
            for _ in 0..self.threads {
                let (jobs, results) = (job_rx.clone(), result_tx.clone());
                let ctx = &ctx;
                scope.spawn(move || worker(&jobs, &results, ctx, cancel));
            }
            drop(result_tx);
            // `collect` owns `job_tx`; dropping it on every exit path lets the
            // workers finish so the scope can join them.
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
            Ok(result) => result,
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
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::sync::Mutex;

    const KIB: u64 = 1024;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ctd-scan-{name}-{}", std::process::id()));
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
        MacScanner::with_threads(4).with_home(None).scan(
            root,
            &options(),
            &CancelToken::default(),
            &|_| {},
        )
    }

    fn child(tree: &Tree, parent: NodeId, name: &str) -> NodeId {
        tree.children(parent)
            .find(|&id| tree.name(id) == name)
            .unwrap_or_else(|| panic!("no child named {name}"))
    }

    /// root/
    ///   big.bin             256 KiB        -> node
    ///   tiny1, tiny2        100 bytes each -> aggregated
    ///   sub/deep/leaf.bin   128 KiB        -> node
    ///   empty/
    fn build_fixture(root: &Path) {
        write_file(&root.join("big.bin"), 256 * KIB);
        write_file(&root.join("tiny1"), 100);
        write_file(&root.join("tiny2"), 100);
        fs::create_dir_all(root.join("sub/deep")).unwrap();
        write_file(&root.join("sub/deep/leaf.bin"), 128 * KIB);
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
        assert!(tree.children(NodeId::ROOT).all(|c| tree.name(c) != "tiny1"));

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
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn symlinks_are_not_followed() {
        let root = temp_dir("symlink");
        build_fixture(&root);
        symlink(&root, root.join("loop")).unwrap();
        symlink(root.join("sub"), root.join("alias")).unwrap();
        let out = run(&root).unwrap();
        let tree = &out.tree;

        // Links are counted as two extra files and never become directories.
        assert_eq!(tree.file_count(NodeId::ROOT), 6);
        let dirs: Vec<_> = tree
            .children(NodeId::ROOT)
            .filter(|&c| tree.kind(c) == NodeKind::Dir)
            .map(|c| tree.name(c))
            .collect();
        assert!(dirs.iter().all(|n| matches!(*n, "sub" | "empty")));
        assert_eq!(dirs.len(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hard_links_are_counted_once() {
        let root = temp_dir("hardlink");
        write_file(&root.join("a.bin"), 256 * KIB);
        fs::hard_link(root.join("a.bin"), root.join("b.bin")).unwrap();
        let out = run(&root).unwrap();
        assert_eq!(out.tree.children(NodeId::ROOT).count(), 1);
        assert_eq!(out.tree.file_count(NodeId::ROOT), 1);
        assert!(out.tree.size(NodeId::ROOT) < 2 * 256 * KIB);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unreadable_directory_is_recorded_not_fatal() {
        let root = temp_dir("denied");
        build_fixture(&root);
        let locked = root.join("locked");
        fs::create_dir(&locked).unwrap();
        write_file(&locked.join("secret.bin"), 256 * KIB);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        let out = run(&root);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        let out = out.unwrap();

        let node = child(&out.tree, NodeId::ROOT, "locked");
        assert_eq!(out.tree.kind(node), NodeKind::Inaccessible);
        assert_eq!(
            out.issues,
            vec![ScanIssue {
                path: locked,
                kind: ScanIssueKind::PermissionDenied
            }]
        );
        assert!(out.tree.size(NodeId::ROOT) > 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unreadable_root_is_an_error() {
        let root = temp_dir("root-denied");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o000)).unwrap();
        let result = run(&root);
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(result.unwrap_err().kind(), ErrorKind::PermissionDenied);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn file_root_is_rejected() {
        let root = temp_dir("file-root");
        let file = root.join("f");
        write_file(&file, 1);
        assert_eq!(run(&file).unwrap_err().kind(), ErrorKind::InvalidInput);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_root_is_an_error() {
        let result = run(Path::new("/no/such/root/ctd"));
        assert_eq!(result.unwrap_err().kind(), ErrorKind::NotFound);
    }

    #[test]
    fn pre_cancelled_scan_is_interrupted() {
        let root = temp_dir("cancel");
        build_fixture(&root);
        let cancel = CancelToken::default();
        cancel.cancel();
        let result = MacScanner::with_threads(2).scan(&root, &options(), &cancel, &|_| {});
        assert_eq!(result.unwrap_err().kind(), ErrorKind::Interrupted);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancel_during_scan_does_not_hang() {
        let root = temp_dir("cancel-mid");
        for i in 0..200 {
            let dir = root.join(format!("d{i}"));
            fs::create_dir(&dir).unwrap();
            write_file(&dir.join("f"), 10);
        }
        let cancel = CancelToken::default();
        let trigger = cancel.clone();
        let result = MacScanner::with_threads(2).scan(&root, &options(), &cancel, &|_| {
            trigger.cancel();
        });
        // The final progress callback fires after the last directory, so a
        // fast scan may complete before the cancel is observed.
        if let Err(e) = result {
            assert_eq!(e.kind(), ErrorKind::Interrupted);
        }
        fs::remove_dir_all(root).unwrap();
    }
    /// home/
    ///   Documents/a.bin
    ///   Library/CloudStorage/Box-Box/synced.bin
    ///   Library/Caches/c.bin
    fn build_home(name: &str) -> PathBuf {
        let home = temp_dir(name);
        fs::create_dir_all(home.join("Documents")).unwrap();
        fs::create_dir_all(home.join("Library/CloudStorage/Box-Box")).unwrap();
        fs::create_dir_all(home.join("Library/Caches")).unwrap();
        write_file(&home.join("Documents/a.bin"), 128 * KIB);
        write_file(
            &home.join("Library/CloudStorage/Box-Box/synced.bin"),
            128 * KIB,
        );
        write_file(&home.join("Library/Caches/c.bin"), 128 * KIB);
        home
    }

    fn scan_with_home(root: &Path, home: &Path, include_cloud: bool) -> ScanOutput {
        let options = ScanOptions {
            include_cloud_folders: include_cloud,
            ..options()
        };
        MacScanner::with_threads(4)
            .with_home(Some(home.to_path_buf()))
            .scan(root, &options, &CancelToken::default(), &|_| {})
            .unwrap()
    }

    fn has_path(tree: &Tree, names: &[&str]) -> bool {
        let mut node = NodeId::ROOT;
        for name in names {
            match tree.children(node).find(|&id| tree.name(id) == *name) {
                Some(id) => node = id,
                None => return false,
            }
        }
        true
    }

    #[test]
    fn cloud_mounts_are_skipped_by_default_and_reported() {
        let home = build_home("cloud-skip");
        let out = scan_with_home(&home, &home, false);
        assert!(has_path(&out.tree, &["Library", "Caches", "c.bin"]));
        assert!(has_path(&out.tree, &["Library", "CloudStorage"]));
        assert!(!has_path(
            &out.tree,
            &["Library", "CloudStorage", "Box-Box"]
        ));
        assert_eq!(
            out.skipped_cloud_folders,
            vec![home.join("Library/CloudStorage/Box-Box")]
        );
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn cloud_mounts_are_scanned_when_enabled() {
        let home = build_home("cloud-include");
        let out = scan_with_home(&home, &home, true);
        assert!(has_path(
            &out.tree,
            &["Library", "CloudStorage", "Box-Box", "synced.bin"]
        ));
        assert!(out.skipped_cloud_folders.is_empty());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn skipping_cloud_mounts_lowers_the_total_by_their_size() {
        let home = build_home("cloud-total");
        let without = scan_with_home(&home, &home, false).tree.size(NodeId::ROOT);
        let with = scan_with_home(&home, &home, true).tree.size(NodeId::ROOT);
        assert!(with >= without + 128 * KIB);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn a_root_chosen_inside_a_cloud_mount_is_scanned_as_asked() {
        let home = build_home("cloud-root");
        let root = home.join("Library/CloudStorage/Box-Box");
        let out = scan_with_home(&root, &home, false);
        assert!(has_path(&out.tree, &["synced.bin"]));
        assert!(out.skipped_cloud_folders.is_empty());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn the_portable_reader_skips_cloud_mounts_too() {
        let home = build_home("cloud-portable");
        let out = MacScanner::with_threads(2)
            .with_mode(ReadMode::Portable)
            .with_home(Some(home.clone()))
            .scan(&home, &options(), &CancelToken::default(), &|_| {})
            .unwrap();
        assert_eq!(out.skipped_cloud_folders.len(), 1);
        assert!(!has_path(
            &out.tree,
            &["Library", "CloudStorage", "Box-Box"]
        ));
        fs::remove_dir_all(home).unwrap();
    }

    /// Manual benchmark. Run with:
    /// `CTD_BENCH_ROOT=/path cargo test --release bench_real_dir -- --ignored --nocapture`
    /// Modes alternate per round so cache state favours neither.
    #[test]
    #[ignore]
    fn bench_real_dir() {
        let root = std::env::var("CTD_BENCH_ROOT").expect("set CTD_BENCH_ROOT");
        let threads_list = [1, 4, 8, 14];
        for round in 1..=3 {
            for &threads in &threads_list {
                for mode in [ReadMode::Portable, ReadMode::Bulk] {
                    let start = Instant::now();
                    let out = MacScanner::with_threads(threads)
                        .with_mode(mode)
                        .scan(
                            Path::new(&root),
                            &ScanOptions::default(),
                            &CancelToken::default(),
                            &|_| {},
                        )
                        .unwrap();
                    println!(
                        "round={round} threads={threads:>2} {mode:?}\t{:>8.2?} nodes={} files={} bytes={} issues={}",
                        start.elapsed(),
                        out.tree.len(),
                        out.tree.file_count(NodeId::ROOT),
                        out.tree.size(NodeId::ROOT),
                        out.issues.len()
                    );
                }
            }
        }
    }

    #[test]
    fn bulk_and_portable_modes_agree() {
        let root = temp_dir("parity");
        build_fixture(&root);
        write_file(&root.join("a.bin"), 256 * KIB);
        fs::hard_link(root.join("a.bin"), root.join("sub/a-link.bin")).unwrap();
        symlink(&root, root.join("loop")).unwrap();
        let scan = |mode| {
            MacScanner::with_threads(4)
                .with_mode(mode)
                .scan(&root, &options(), &CancelToken::default(), &|_| {})
                .unwrap()
        };
        let (portable, bulk) = (scan(ReadMode::Portable), scan(ReadMode::Bulk));
        for out in [&portable, &bulk] {
            assert_eq!(out.tree.file_count(NodeId::ROOT), 6);
        }
        assert_eq!(portable.tree.len(), bulk.tree.len());
        assert_eq!(
            portable.tree.size(NodeId::ROOT),
            bulk.tree.size(NodeId::ROOT)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bulk_mode_records_unreadable_directory() {
        let root = temp_dir("bulk-denied");
        build_fixture(&root);
        let locked = root.join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let out = MacScanner::with_threads(2).with_mode(ReadMode::Bulk).scan(
            &root,
            &options(),
            &CancelToken::default(),
            &|_| {},
        );
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        let out = out.unwrap();
        let node = child(&out.tree, NodeId::ROOT, "locked");
        assert_eq!(out.tree.kind(node), NodeKind::Inaccessible);
        assert_eq!(out.issues.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn progress_reports_final_totals() {
        let root = temp_dir("progress");
        build_fixture(&root);
        let last = Mutex::new(None);
        MacScanner::with_threads(2)
            .scan(&root, &options(), &CancelToken::default(), &|p| {
                *last.lock().unwrap() = Some(p);
            })
            .unwrap();
        let last = last.into_inner().unwrap().unwrap();
        // Entries: big.bin, tiny1, tiny2, sub, empty, deep, leaf.bin.
        assert_eq!(last.items_scanned, 7);
        assert!(last.bytes >= 384 * KIB);
        assert_eq!(last.current_path, root);
        fs::remove_dir_all(root).unwrap();
    }
}
