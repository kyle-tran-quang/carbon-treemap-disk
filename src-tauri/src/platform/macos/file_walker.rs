//! macOS `FileWalker` for the duplicate finder.
//!
//! Same shape as `MacScanner`: worker threads read one directory each, and the
//! calling thread collects the results and queues the subdirectories. Unlike
//! the scanner it keeps every regular file, because duplicates are often
//! small, and it asks the kernel for the extra attributes the finder needs
//! (logical size, modification time, `st_flags`).
//!
//! Left out, and not followed: symlinks, sockets, devices, folders on another
//! volume, cloud-sync folders (unless enabled), folders matched by the
//! exclusion rules, and "dataless" cloud placeholders, because reading one
//! would download it.

use std::ffi::OsString;
use std::fs;
use std::io::{self, ErrorKind};
use std::os::macos::fs::MetadataExt as MacMetadataExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};

use crate::core::duplicate_exclusions::Exclusions;
use crate::core::duplicates::{DuplicateOptions, WalkOutput, WalkedFile};
use crate::core::scan::{CancelToken, ScanIssue, ScanIssueKind, ScanProgress};
use crate::platform::FileWalker;

use super::bulk::read_dir_bulk_files;
use super::cloud::CloudFolders;
use super::scanner::{MAX_DEFAULT_THREADS, ReadMode};

const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// `SF_DATALESS` from `<sys/stat.h>`: the content is not on this disk.
const SF_DATALESS: u32 = 0x4000_0000;

#[derive(Debug)]
pub struct MacFileWalker {
    threads: usize,
    mode: ReadMode,
    /// Used for the cloud-folder and home-relative exclusion rules.
    home: Option<PathBuf>,
}

impl MacFileWalker {
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

    #[cfg(test)]
    fn with_mode(mut self, mode: ReadMode) -> Self {
        self.mode = mode;
        self
    }

    #[cfg(test)]
    fn with_threads(mut self, threads: usize) -> Self {
        self.threads = threads.max(1);
        self
    }

    /// A root the user picked inside a cloud location is walked as asked.
    fn cloud_folders_to_skip(
        &self,
        root: &Path,
        options: &DuplicateOptions,
    ) -> Option<CloudFolders> {
        if options.include_cloud_folders {
            return None;
        }
        let cloud = CloudFolders::for_home(self.home.as_deref()?);
        (!cloud.contains_root(root)).then_some(cloud)
    }
}

impl Default for MacFileWalker {
    fn default() -> Self {
        Self::new()
    }
}

struct Context<'a> {
    root_dev: u32,
    min_size: u64,
    exclusions: Exclusions,
    home: Option<&'a Path>,
    skip_cloud: Option<CloudFolders>,
    mode: ReadMode,
}

/// What one directory contains.
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

/// What a subdirectory becomes.
enum DirAction {
    Read,
    Skip,
    SkipCloud,
}

impl Context<'_> {
    fn dir_action(&self, parent: &Path, name: &OsString) -> DirAction {
        if self.exclusions.check(self.home, parent, name).is_some() {
            return DirAction::Skip;
        }
        match &self.skip_cloud {
            Some(cloud) if cloud.is_cloud_folder(&parent.join(name)) => DirAction::SkipCloud,
            _ => DirAction::Read,
        }
    }

    fn wanted_size(&self, size: u64) -> bool {
        size >= self.min_size
    }
}

fn read_directory(path: &Path, ctx: &Context) -> (io::Result<Listing>, Vec<ScanIssue>) {
    match ctx.mode {
        ReadMode::Portable => read_directory_portable(path, ctx),
        ReadMode::Bulk => read_directory_bulk(path, ctx),
    }
}

fn read_directory_bulk(path: &Path, ctx: &Context) -> (io::Result<Listing>, Vec<ScanIssue>) {
    let bulk = match read_dir_bulk_files(path) {
        Ok(bulk) => bulk,
        // Some filesystems (certain network and FUSE mounts) lack support.
        Err(e) if matches!(e.raw_os_error(), Some(libc::ENOTSUP | libc::EINVAL)) => {
            return read_directory_portable(path, ctx);
        }
        Err(e) => return (Err(e), Vec::new()),
    };
    let issues = bulk
        .errors
        .iter()
        .map(|e| issue(path.join(&e.name), &e.error))
        .collect();
    let mut listing = Listing::default();
    for item in bulk.entries {
        match item.kind {
            super::bulk::BulkKind::Dir => {
                // Never cross onto another volume.
                if item.dev != ctx.root_dev {
                    continue;
                }
                match ctx.dir_action(path, &item.name) {
                    DirAction::Read => listing.dirs.push(item.name),
                    DirAction::Skip => {}
                    DirAction::SkipCloud => listing.skipped_cloud.push(path.join(&item.name)),
                }
            }
            super::bulk::BulkKind::Other => {
                if item.regular
                    && item.flags & SF_DATALESS == 0
                    && ctx.wanted_size(item.data_length)
                {
                    listing.files.push(WalkedFile {
                        path: path.join(&item.name),
                        size: item.data_length,
                        device: u64::from(item.dev),
                        inode: item.inode,
                        modified_secs: item.modified_secs,
                    });
                }
            }
        }
    }
    (Ok(listing), issues)
}

fn read_directory_portable(path: &Path, ctx: &Context) -> (io::Result<Listing>, Vec<ScanIssue>) {
    let reader = match fs::read_dir(path) {
        Ok(reader) => reader,
        Err(e) => return (Err(e), Vec::new()),
    };
    let mut listing = Listing::default();
    let mut issues = Vec::new();
    for item in reader {
        let item = match item {
            Ok(item) => item,
            Err(e) => {
                issues.push(issue(path.to_path_buf(), &e));
                continue;
            }
        };
        // `DirEntry::metadata` does not follow symlinks.
        let meta = match item.metadata() {
            Ok(meta) => meta,
            Err(e) => {
                issues.push(issue(item.path(), &e));
                continue;
            }
        };
        let device = meta.dev() as u32;
        if meta.is_dir() {
            if device != ctx.root_dev {
                continue;
            }
            let name = item.file_name();
            match ctx.dir_action(path, &name) {
                DirAction::Read => listing.dirs.push(name),
                DirAction::Skip => {}
                DirAction::SkipCloud => listing.skipped_cloud.push(path.join(&name)),
            }
        } else if meta.file_type().is_file()
            && meta.st_flags() & SF_DATALESS == 0
            && ctx.wanted_size(meta.len())
        {
            listing.files.push(WalkedFile {
                path: item.path(),
                size: meta.len(),
                device: u64::from(device),
                inode: meta.ino(),
                modified_secs: meta.mtime().max(0) as u64,
            });
        }
    }
    (Ok(listing), issues)
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

impl FileWalker for MacFileWalker {
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
        let ctx = Context {
            root_dev: root_meta.dev() as u32,
            // Empty files are never duplicates worth listing.
            min_size: options.min_size.max(1),
            exclusions: options.exclusions,
            home: self.home.as_deref(),
            skip_cloud: self.cloud_folders_to_skip(root, options),
            mode: self.mode,
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
            // `collect` owns `job_tx`; dropping it on every exit path lets the
            // workers finish so the scope can join them.
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
            Ok(result) => result,
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
    use std::os::unix::fs::{PermissionsExt, symlink};

    const KIB: u64 = 1024;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ctd-walk-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_file(path: &Path, len: u64) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
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

    fn walker() -> MacFileWalker {
        MacFileWalker::new().with_threads(4).with_home(None)
    }

    fn walk_with(
        walker: &MacFileWalker,
        root: &Path,
        options: &DuplicateOptions,
    ) -> io::Result<WalkOutput> {
        walker.walk(root, options, &CancelToken::default(), &|_| {})
    }

    fn walk(root: &Path, min_size: u64) -> WalkOutput {
        walk_with(&walker(), root, &options(min_size)).unwrap()
    }

    /// Paths relative to `root`, sorted.
    fn names(out: &WalkOutput, root: &Path) -> BTreeSet<String> {
        out.files
            .iter()
            .map(|f| {
                f.path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn lists_regular_files_at_or_above_the_minimum_size() {
        let root = temp_dir("sizes");
        write_file(&root.join("big.bin"), 8 * KIB);
        write_file(&root.join("exact.bin"), 4 * KIB);
        write_file(&root.join("small.bin"), 100);
        write_file(&root.join("empty.bin"), 0);
        write_file(&root.join("sub/deep/leaf.bin"), 16 * KIB);
        let out = walk(&root, 4 * KIB);
        assert_eq!(
            names(&out, &root),
            set(&["big.bin", "exact.bin", "sub/deep/leaf.bin"])
        );
        assert!(out.issues.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reports_logical_size_inode_and_modification_time() {
        let root = temp_dir("meta");
        let file = root.join("f.bin");
        write_file(&file, 10_000);
        let out = walk(&root, 1);
        let meta = fs::symlink_metadata(&file).unwrap();
        let found = &out.files[0];
        assert_eq!(found.size, 10_000);
        assert_eq!(found.inode, meta.ino());
        assert_eq!(found.device, u64::from(meta.dev() as u32));
        assert_eq!(found.modified_secs, meta.mtime() as u64);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn zero_minimum_still_skips_empty_files() {
        let root = temp_dir("zero");
        write_file(&root.join("empty"), 0);
        write_file(&root.join("one"), 1);
        assert_eq!(names(&walk(&root, 0), &root), set(&["one"]));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn symlinks_are_not_listed_or_followed() {
        let root = temp_dir("links");
        write_file(&root.join("real/data.bin"), 8 * KIB);
        symlink(root.join("real/data.bin"), root.join("file-link")).unwrap();
        symlink(root.join("real"), root.join("dir-link")).unwrap();
        let out = walk(&root, 1);
        assert_eq!(names(&out, &root), set(&["real/data.bin"]));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hard_links_report_the_same_device_and_inode() {
        let root = temp_dir("hard");
        write_file(&root.join("a.bin"), 8 * KIB);
        fs::hard_link(root.join("a.bin"), root.join("b.bin")).unwrap();
        let out = walk(&root, 1);
        assert_eq!(out.files.len(), 2);
        assert_eq!(out.files[0].inode, out.files[1].inode);
        assert_eq!(out.files[0].device, out.files[1].device);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn default_exclusions_skip_packages_node_modules_and_git() {
        let root = temp_dir("excl");
        write_file(&root.join("keep.bin"), 8 * KIB);
        write_file(&root.join("Tool.app/Contents/x.bin"), 8 * KIB);
        write_file(&root.join("proj/node_modules/m.bin"), 8 * KIB);
        write_file(&root.join("proj/.git/objects/o.bin"), 8 * KIB);
        write_file(&root.join("proj/src/s.bin"), 8 * KIB);
        let out = walk(&root, 1);
        assert_eq!(names(&out, &root), set(&["keep.bin", "proj/src/s.bin"]));

        let all = DuplicateOptions {
            exclusions: Exclusions::none(),
            ..options(1)
        };
        let out = walk_with(&walker(), &root, &all).unwrap();
        assert_eq!(out.files.len(), 5);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_root_that_is_itself_an_excluded_name_is_walked() {
        let base = temp_dir("rootname");
        let root = base.join("node_modules");
        write_file(&root.join("m.bin"), 8 * KIB);
        assert_eq!(names(&walk(&root, 1), &root), set(&["m.bin"]));
        fs::remove_dir_all(base).unwrap();
    }

    /// home/
    ///   Documents/a.bin
    ///   Library/Caches/c.bin
    ///   Library/CloudStorage/Box-Box/synced.bin
    ///   .Trash/t.bin
    fn build_home(name: &str) -> PathBuf {
        let home = temp_dir(name);
        write_file(&home.join("Documents/a.bin"), 8 * KIB);
        write_file(&home.join("Library/Caches/c.bin"), 8 * KIB);
        write_file(
            &home.join("Library/CloudStorage/Box-Box/synced.bin"),
            8 * KIB,
        );
        write_file(&home.join(".Trash/t.bin"), 8 * KIB);
        home
    }

    fn walk_home(root: &Path, home: &Path, options: &DuplicateOptions) -> WalkOutput {
        let walker = MacFileWalker::new()
            .with_threads(4)
            .with_home(Some(home.to_path_buf()));
        walk_with(&walker, root, options).unwrap()
    }

    #[test]
    fn library_and_trash_are_left_out_of_a_home_walk() {
        let home = build_home("home");
        let out = walk_home(&home, &home, &options(1));
        assert_eq!(names(&out, &home), set(&["Documents/a.bin"]));
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn choosing_library_as_the_root_walks_it_as_asked() {
        let home = build_home("libroot");
        let root = home.join("Library");
        let out = walk_home(&root, &home, &options(1));
        // CloudStorage is still skipped; Caches is read.
        assert_eq!(names(&out, &root), set(&["Caches/c.bin"]));
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn cloud_mounts_are_skipped_and_reported_unless_enabled() {
        let home = build_home("cloud");
        let library_open = DuplicateOptions {
            exclusions: Exclusions::none(),
            ..options(1)
        };
        let out = walk_home(&home, &home, &library_open);
        assert!(!names(&out, &home).contains("Library/CloudStorage/Box-Box/synced.bin"));
        assert_eq!(
            out.skipped_cloud_folders,
            vec![home.join("Library/CloudStorage/Box-Box")]
        );

        let with_cloud = DuplicateOptions {
            include_cloud_folders: true,
            ..library_open
        };
        let out = walk_home(&home, &home, &with_cloud);
        assert!(names(&out, &home).contains("Library/CloudStorage/Box-Box/synced.bin"));
        assert!(out.skipped_cloud_folders.is_empty());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn a_root_inside_a_cloud_mount_is_walked_as_asked() {
        let home = build_home("cloudroot");
        let root = home.join("Library/CloudStorage/Box-Box");
        let out = walk_home(&root, &home, &options(1));
        assert_eq!(names(&out, &root), set(&["synced.bin"]));
        assert!(out.skipped_cloud_folders.is_empty());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn an_unreadable_folder_becomes_an_issue_not_a_failure() {
        let root = temp_dir("denied");
        write_file(&root.join("ok.bin"), 8 * KIB);
        write_file(&root.join("locked/hidden.bin"), 8 * KIB);
        let locked = root.join("locked");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let out = walk(&root, 1);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        // A process running as root can read the folder anyway.
        if !names(&out, &root).contains("locked/hidden.bin") {
            assert_eq!(names(&out, &root), set(&["ok.bin"]));
            assert_eq!(out.issues.len(), 1);
            assert_eq!(out.issues[0].path, locked);
            assert_eq!(out.issues[0].kind, ScanIssueKind::PermissionDenied);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_missing_or_non_directory_root_is_an_error() {
        let root = temp_dir("badroot");
        let missing = walk_with(&walker(), &root.join("nope"), &options(1)).unwrap_err();
        assert_eq!(missing.kind(), ErrorKind::NotFound);
        write_file(&root.join("f.bin"), 10);
        let not_dir = walk_with(&walker(), &root.join("f.bin"), &options(1)).unwrap_err();
        assert_eq!(not_dir.kind(), ErrorKind::InvalidInput);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_symlinked_root_is_refused() {
        let root = temp_dir("linkroot");
        write_file(&root.join("real/a.bin"), 8 * KIB);
        symlink(root.join("real"), root.join("link")).unwrap();
        let err = walk_with(&walker(), &root.join("link"), &options(1)).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_cancelled_walk_returns_interrupted() {
        let root = temp_dir("cancel");
        write_file(&root.join("a.bin"), 8 * KIB);
        let cancel = CancelToken::default();
        cancel.cancel();
        let err = walker()
            .walk(&root, &options(1), &cancel, &|_| {})
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Interrupted);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bulk_and_portable_readers_agree() {
        let root = temp_dir("modes");
        write_file(&root.join("a.bin"), 8 * KIB);
        write_file(&root.join("sub/b.bin"), 9 * KIB);
        write_file(&root.join("sub/small.bin"), 10);
        write_file(&root.join("x.app/inner.bin"), 8 * KIB);
        fs::hard_link(root.join("a.bin"), root.join("sub/a-link.bin")).unwrap();
        symlink(root.join("a.bin"), root.join("ln")).unwrap();

        let summary = |mode| {
            let w = walker().with_mode(mode);
            let out = walk_with(&w, &root, &options(KIB)).unwrap();
            let mut files: Vec<_> = out
                .files
                .iter()
                .map(|f| (f.path.clone(), f.size, f.device, f.inode, f.modified_secs))
                .collect();
            files.sort();
            files
        };
        let bulk = summary(ReadMode::Bulk);
        assert_eq!(bulk.len(), 3);
        assert_eq!(bulk, summary(ReadMode::Portable));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn progress_ends_with_the_totals() {
        let root = temp_dir("progress");
        write_file(&root.join("a.bin"), 8 * KIB);
        write_file(&root.join("sub/b.bin"), 4 * KIB);
        let last = std::sync::Mutex::new(None);
        walker()
            .walk(&root, &options(1), &CancelToken::default(), &|p| {
                *last.lock().unwrap() = Some(p);
            })
            .unwrap();
        let last = last.into_inner().unwrap().unwrap();
        assert_eq!(last.items_scanned, 2);
        assert_eq!(last.bytes, 12 * KIB);
        fs::remove_dir_all(root).unwrap();
    }

    /// Manual, read-only report on the real home folder. Run with
    /// `cargo test --release real_home_walk -- --ignored --nocapture`.
    /// Lists files only; nothing is read, moved or written.
    #[test]
    #[ignore]
    fn real_home_walk_report() {
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME is set"));
        let walker = MacFileWalker::new();
        let run = |label: &str, options: DuplicateOptions| {
            let started = Instant::now();
            let out = walker
                .walk(&home, &options, &CancelToken::default(), &|_| {})
                .unwrap();
            let bytes: u64 = out.files.iter().map(|f| f.size).sum();
            let inodes: BTreeSet<_> = out.files.iter().map(|f| (f.device, f.inode)).collect();
            println!(
                "{label}: {} files ({} distinct inodes), {:.2} GB, {} issues, {} cloud folders skipped, {:.2?}",
                out.files.len(),
                inodes.len(),
                bytes as f64 / 1e9,
                out.issues.len(),
                out.skipped_cloud_folders.len(),
                started.elapsed(),
            );
            out
        };
        let default = run(
            "default (1 MiB, exclusions on)",
            DuplicateOptions::default(),
        );
        run(
            "exclusions off",
            DuplicateOptions {
                exclusions: Exclusions::none(),
                ..DuplicateOptions::default()
            },
        );
        run(
            "default, min size 4 KiB",
            DuplicateOptions {
                min_size: 4 * KIB,
                ..DuplicateOptions::default()
            },
        );
        let heap: usize = default
            .files
            .iter()
            .map(|f| f.path.as_os_str().len() + std::mem::size_of::<WalkedFile>())
            .sum();
        println!(
            "default result held in memory: about {:.1} MB",
            heap as f64 / 1e6
        );
        for issue in default.issues.iter().take(5) {
            println!("issue: {} {:?}", issue.path.display(), issue.kind);
        }
    }
}
