//! Exact-duplicate detection. OS-agnostic and read-only: nothing here moves,
//! writes or deletes.
//!
//! The pipeline narrows the candidate set in stages so that most files are
//! never read:
//!
//! 1. **List.** A `FileWalker` returns regular files at or above the minimum
//!    size. Each file is flagged `protected` when it lies under a protected
//!    (reference) folder. A protected file may be kept but is never offered
//!    for moving.
//! 2. **Collapse hard links.** Paths that share a device and inode are one
//!    file. Only one path per inode goes forward, so hard links are never
//!    reported as duplicates of each other.
//! 3. **Group by size.** A file with a unique size cannot have a duplicate.
//! 4. **Partial fingerprint.** A `ContentHasher` samples part of each file.
//! 5. **Full hash.** Only files that still match are read completely.
//!
//! Files that cannot be read are recorded as issues and dropped, which can
//! only remove something from the result. Sizes are the sizes seen by the
//! walker; later steps (plan and execute) must re-check the disk.

use std::collections::{HashMap, HashSet};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use super::config::DEFAULT_DUPLICATE_MIN_SIZE_BYTES;
use super::duplicate_exclusions::Exclusions;
use super::scan::{CancelToken, ScanIssue, ScanIssueKind, ScanProgress};
use crate::platform::FileWalker;

/// Output size of the content hash. 32 bytes fits BLAKE3 and SHA-256.
pub type Fingerprint = [u8; 32];

/// Worker threads for the hashing stages. Not tuned: hashing is read-bound, so
/// more threads can hurt on slow or external disks (see sub-task 10).
const DEFAULT_HASH_THREADS: usize = 4;

const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateOptions {
    /// Files smaller than this are ignored. Empty files are always ignored.
    pub min_size: u64,
    /// Cloud-sync folders are skipped unless this is set.
    pub include_cloud_folders: bool,
    /// Folders left out below the scan root.
    pub exclusions: Exclusions,
    /// Threads for the fingerprint and hash stages.
    pub threads: usize,
}

impl Default for DuplicateOptions {
    fn default() -> Self {
        let cores = thread::available_parallelism().map_or(1, |n| n.get());
        Self {
            min_size: DEFAULT_DUPLICATE_MIN_SIZE_BYTES,
            include_cloud_folders: false,
            exclusions: Exclusions::default(),
            threads: cores.min(DEFAULT_HASH_THREADS),
        }
    }
}

/// A regular file as reported by a `FileWalker`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkedFile {
    pub path: PathBuf,
    pub size: u64,
    pub device: u64,
    pub inode: u64,
    /// Seconds since the Unix epoch.
    pub modified_secs: u64,
}

#[derive(Debug, Clone, Default)]
pub struct WalkOutput {
    pub files: Vec<WalkedFile>,
    pub issues: Vec<ScanIssue>,
    /// Cloud-sync folders left out on purpose.
    pub skipped_cloud_folders: Vec<PathBuf>,
}

/// Reads file content to tell whether two files are identical. Both methods
/// must be deterministic for unchanged content, and should poll `cancel`
/// between blocks and return `ErrorKind::Interrupted` when it is set.
pub trait ContentHasher: Send + Sync {
    /// Fingerprint of a sample of the file (for example its first and last
    /// blocks). Equal files must give equal fingerprints.
    fn partial(&self, path: &Path, size: u64, cancel: &CancelToken) -> io::Result<Fingerprint>;

    /// Fingerprint of the whole content.
    fn full(&self, path: &Path, cancel: &CancelToken) -> io::Result<Fingerprint>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Listing,
    Fingerprinting,
    Hashing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateProgress {
    pub stage: Stage,
    pub done: u64,
    /// Zero while the total is not known (listing).
    pub total: u64,
    /// File bytes read by the full-hash stage so far.
    pub bytes: u64,
    pub current_path: PathBuf,
}

pub struct DuplicateRequest<'a> {
    pub scan_roots: &'a [PathBuf],
    /// Files under these folders can be kept but never moved.
    pub protected_roots: &'a [PathBuf],
    pub options: &'a DuplicateOptions,
}

#[derive(Debug)]
pub enum DuplicateError {
    Cancelled,
    /// A root could not be read at all. The scan stops, because an unreadable
    /// protected folder would otherwise go unprotected.
    RootUnreadable {
        path: PathBuf,
        error: io::Error,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateFile {
    /// Unique within one scan. Numbered from 1.
    pub id: u32,
    pub path: PathBuf,
    pub modified_secs: u64,
    /// Under a protected folder: may be kept, never moved.
    pub protected: bool,
}

/// Two or more files with identical content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateGroup {
    /// Unique within one scan. Numbered from 1.
    pub id: u32,
    /// Size of each file.
    pub size: u64,
    /// At least two, ordered by path. At least one is not protected.
    pub files: Vec<DuplicateFile>,
}

impl DuplicateGroup {
    /// Files that could be moved while one copy stays. With a protected copy
    /// every unprotected file qualifies; otherwise one copy must remain.
    pub fn reclaimable_files(&self) -> usize {
        let movable = self.files.iter().filter(|f| !f.protected).count();
        if movable < self.files.len() {
            movable
        } else {
            self.files.len() - 1
        }
    }

    pub fn reclaimable_bytes(&self) -> u64 {
        self.reclaimable_files() as u64 * self.size
    }
}

#[derive(Debug, Default)]
pub struct DuplicateScan {
    /// Most reclaimable space first.
    pub groups: Vec<DuplicateGroup>,
    pub issues: Vec<ScanIssue>,
    pub skipped_cloud_folders: Vec<PathBuf>,
    /// Extra hard-link paths folded into another path to the same inode.
    pub hard_links_collapsed: usize,
    /// Files that passed the size filter.
    pub files_listed: u64,
    /// File bytes read by the full-hash stage.
    pub bytes_hashed: u64,
}

impl DuplicateScan {
    pub fn reclaimable_bytes(&self) -> u64 {
        self.groups.iter().map(|g| g.reclaimable_bytes()).sum()
    }
}

struct Entry {
    file: WalkedFile,
    protected: bool,
}

fn check(cancel: &CancelToken) -> Result<(), DuplicateError> {
    if cancel.is_cancelled() {
        Err(DuplicateError::Cancelled)
    } else {
        Ok(())
    }
}

fn issue(path: &Path, error: &io::Error) -> ScanIssue {
    let kind = if error.kind() == ErrorKind::PermissionDenied {
        ScanIssueKind::PermissionDenied
    } else {
        ScanIssueKind::Io(error.to_string())
    };
    ScanIssue {
        path: path.to_path_buf(),
        kind,
    }
}

/// Lower-cased because default macOS volumes are case-insensitive. On a
/// case-sensitive volume this can only protect more, never less.
pub(super) fn fold(path: &Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().to_lowercase())
}

fn is_under(path: &Path, folded_roots: &[PathBuf]) -> bool {
    let path = fold(path);
    folded_roots.iter().any(|root| path.starts_with(root))
}

/// Runs `f` over `items` on up to `threads` threads. Results keep the order of
/// `items`; an item that was not started because of cancellation is `None`.
fn par_map<T: Sync, R: Send>(
    items: &[T],
    threads: usize,
    cancel: &CancelToken,
    f: impl Fn(&T) -> R + Sync,
) -> Vec<Option<R>> {
    let next = AtomicUsize::new(0);
    let workers = threads.clamp(1, items.len().max(1));
    let mut out: Vec<Option<R>> = (0..items.len()).map(|_| None).collect();
    thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut local = Vec::new();
                    while !cancel.is_cancelled() {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= items.len() {
                            break;
                        }
                        local.push((i, f(&items[i])));
                    }
                    local
                })
            })
            .collect();
        for handle in handles {
            for (i, result) in handle.join().expect("duplicate worker panicked") {
                out[i] = Some(result);
            }
        }
    });
    out
}

/// Throttled progress for one stage, safe to call from worker threads.
struct Reporter<'a> {
    on_progress: &'a (dyn Fn(DuplicateProgress) + Sync),
    stage: Stage,
    total: u64,
    done: AtomicU64,
    bytes: &'a AtomicU64,
    last_emit: Mutex<Instant>,
}

impl<'a> Reporter<'a> {
    fn new(
        on_progress: &'a (dyn Fn(DuplicateProgress) + Sync),
        stage: Stage,
        total: u64,
        bytes: &'a AtomicU64,
    ) -> Self {
        Self {
            on_progress,
            stage,
            total,
            done: AtomicU64::new(0),
            bytes,
            last_emit: Mutex::new(Instant::now()),
        }
    }

    fn emit(&self, path: &Path) {
        (self.on_progress)(DuplicateProgress {
            stage: self.stage,
            done: self.done.load(Ordering::Relaxed),
            total: self.total,
            bytes: self.bytes.load(Ordering::Relaxed),
            current_path: path.to_path_buf(),
        });
    }

    fn record(&self, path: &Path, bytes: u64) {
        self.done.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
        if let Ok(mut last) = self.last_emit.try_lock()
            && last.elapsed() >= PROGRESS_INTERVAL
        {
            *last = Instant::now();
            self.emit(path);
        }
    }

    /// Always emits, so the last report shows the stage as complete.
    fn finish(&self, path: &Path) {
        self.done.store(self.total, Ordering::Relaxed);
        self.emit(path);
    }
}

struct Run<'a> {
    threads: usize,
    cancel: &'a CancelToken,
    on_progress: &'a (dyn Fn(DuplicateProgress) + Sync),
    bytes_hashed: &'a AtomicU64,
}

impl Run<'_> {
    /// Fingerprints `candidates` (indices into `entries`). Files that fail are
    /// recorded in `issues` and left out of the result.
    fn stage(
        &self,
        stage: Stage,
        entries: &[Entry],
        candidates: &[usize],
        issues: &mut Vec<ScanIssue>,
        hash: impl Fn(&Entry) -> io::Result<Fingerprint> + Sync,
    ) -> Result<Vec<(usize, Fingerprint)>, DuplicateError> {
        let reporter = Reporter::new(
            self.on_progress,
            stage,
            candidates.len() as u64,
            self.bytes_hashed,
        );
        let counts_bytes = stage == Stage::Hashing;
        let results = par_map(candidates, self.threads, self.cancel, |&i| {
            let entry = &entries[i];
            let result = hash(entry);
            let bytes = if counts_bytes && result.is_ok() {
                entry.file.size
            } else {
                0
            };
            reporter.record(&entry.file.path, bytes);
            result
        });
        reporter.finish(Path::new(""));
        check(self.cancel)?;

        let mut hashed = Vec::with_capacity(candidates.len());
        for (&i, result) in candidates.iter().zip(results) {
            match result {
                Some(Ok(fingerprint)) => hashed.push((i, fingerprint)),
                Some(Err(error)) => issues.push(issue(&entries[i].file.path, &error)),
                None => {}
            }
        }
        Ok(hashed)
    }
}

/// Groups of two or more indices that share a size and a fingerprint.
fn matching_groups(hashed: Vec<(usize, Fingerprint)>, entries: &[Entry]) -> Vec<Vec<usize>> {
    let mut by_key: HashMap<(u64, Fingerprint), Vec<usize>> = HashMap::new();
    for (i, fingerprint) in hashed {
        by_key
            .entry((entries[i].file.size, fingerprint))
            .or_default()
            .push(i);
    }
    by_key.into_values().filter(|g| g.len() > 1).collect()
}

fn collapse_hard_links(entries: Vec<Entry>) -> (Vec<Entry>, usize) {
    let mut by_inode: HashMap<(u64, u64), Vec<Entry>> = HashMap::new();
    for entry in entries {
        by_inode
            .entry((entry.file.device, entry.file.inode))
            .or_default()
            .push(entry);
    }
    let mut collapsed = 0;
    let mut kept = Vec::with_capacity(by_inode.len());
    for (_, mut links) in by_inode {
        collapsed += links.len() - 1;
        // Links arrive in path order, so the default pick is the first path.
        // A protected link is preferred so that the file stays protected.
        let pick = links.iter().position(|e| e.protected).unwrap_or(0);
        kept.push(links.swap_remove(pick));
    }
    kept.sort_by(|a, b| a.file.path.cmp(&b.file.path));
    (kept, collapsed)
}

/// Indices of entries whose size is shared with at least one other entry.
fn sharing_a_size(entries: &[Entry]) -> Vec<usize> {
    let mut counts: HashMap<u64, usize> = HashMap::new();
    for entry in entries {
        *counts.entry(entry.file.size).or_default() += 1;
    }
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| counts[&e.file.size] > 1)
        .map(|(i, _)| i)
        .collect()
}

fn build_groups(matches: Vec<Vec<usize>>, entries: &[Entry]) -> Vec<DuplicateGroup> {
    let mut groups: Vec<DuplicateGroup> = matches
        .into_iter()
        .filter_map(|mut indices| {
            indices.sort_unstable();
            let members: Vec<&Entry> = indices.iter().map(|&i| &entries[i]).collect();
            if members.iter().all(|e| e.protected) {
                return None;
            }
            Some(DuplicateGroup {
                id: 0,
                size: members[0].file.size,
                files: members
                    .into_iter()
                    .map(|e| DuplicateFile {
                        id: 0,
                        path: e.file.path.clone(),
                        modified_secs: e.file.modified_secs,
                        protected: e.protected,
                    })
                    .collect(),
            })
        })
        .collect();

    groups.sort_by(|a, b| {
        b.reclaimable_bytes()
            .cmp(&a.reclaimable_bytes())
            .then(b.size.cmp(&a.size))
            .then_with(|| a.files[0].path.cmp(&b.files[0].path))
    });

    let mut next_file_id = 1;
    for (group, id) in groups.iter_mut().zip(1..) {
        group.id = id;
        for file in &mut group.files {
            file.id = next_file_id;
            next_file_id += 1;
        }
    }
    groups
}

/// Finds files with identical content. Read-only.
pub fn find_duplicates(
    walker: &dyn FileWalker,
    hasher: &dyn ContentHasher,
    request: &DuplicateRequest,
    cancel: &CancelToken,
    on_progress: &(dyn Fn(DuplicateProgress) + Sync),
) -> Result<DuplicateScan, DuplicateError> {
    let options = request.options;
    let mut issues = Vec::new();
    let mut skipped_cloud_folders = Vec::new();
    let mut listed = Vec::new();

    for root in request.scan_roots.iter().chain(request.protected_roots) {
        check(cancel)?;
        let on_walk = |p: ScanProgress| {
            on_progress(DuplicateProgress {
                stage: Stage::Listing,
                done: p.items_scanned,
                total: 0,
                bytes: p.bytes,
                current_path: p.current_path,
            });
        };
        let output = walker
            .walk(root, options, cancel, &on_walk)
            .map_err(|error| {
                if cancel.is_cancelled() {
                    DuplicateError::Cancelled
                } else {
                    DuplicateError::RootUnreadable {
                        path: root.clone(),
                        error,
                    }
                }
            })?;
        listed.extend(output.files);
        issues.extend(output.issues);
        skipped_cloud_folders.extend(output.skipped_cloud_folders);
    }
    check(cancel)?;

    let protected_roots: Vec<PathBuf> = request.protected_roots.iter().map(|p| fold(p)).collect();
    let min_size = options.min_size.max(1);
    let mut seen = HashSet::new();
    let mut entries: Vec<Entry> = listed
        .into_iter()
        .filter(|f| f.size >= min_size)
        .filter(|f| seen.insert(f.path.clone()))
        .map(|file| {
            let protected = is_under(&file.path, &protected_roots);
            Entry { file, protected }
        })
        .collect();
    entries.sort_by(|a, b| a.file.path.cmp(&b.file.path));
    let files_listed = entries.len() as u64;
    on_progress(DuplicateProgress {
        stage: Stage::Listing,
        done: files_listed,
        total: files_listed,
        bytes: 0,
        current_path: PathBuf::new(),
    });

    let (entries, hard_links_collapsed) = collapse_hard_links(entries);

    let bytes_hashed = AtomicU64::new(0);
    let run = Run {
        threads: options.threads,
        cancel,
        on_progress,
        bytes_hashed: &bytes_hashed,
    };

    let candidates = sharing_a_size(&entries);
    let sampled = run.stage(
        Stage::Fingerprinting,
        &entries,
        &candidates,
        &mut issues,
        |e| hasher.partial(&e.file.path, e.file.size, cancel),
    )?;
    let mut candidates: Vec<usize> = matching_groups(sampled, &entries)
        .into_iter()
        .flatten()
        .collect();
    candidates.sort_unstable();

    let hashed = run.stage(Stage::Hashing, &entries, &candidates, &mut issues, |e| {
        hasher.full(&e.file.path, cancel)
    })?;
    let groups = build_groups(matching_groups(hashed, &entries), &entries);

    Ok(DuplicateScan {
        groups,
        issues,
        skipped_cloud_folders,
        hard_links_collapsed,
        files_listed,
        bytes_hashed: bytes_hashed.load(Ordering::Relaxed),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    /// Returns the files that lie under the requested root.
    #[derive(Default)]
    struct FakeWalker {
        files: Vec<WalkedFile>,
        failing: HashSet<PathBuf>,
    }

    impl FileWalker for FakeWalker {
        fn walk(
            &self,
            root: &Path,
            _options: &DuplicateOptions,
            _cancel: &CancelToken,
            _on_progress: &(dyn Fn(ScanProgress) + Sync),
        ) -> io::Result<WalkOutput> {
            if self.failing.contains(root) {
                return Err(io::Error::from(ErrorKind::PermissionDenied));
            }
            Ok(WalkOutput {
                files: self
                    .files
                    .iter()
                    .filter(|f| f.path.starts_with(root))
                    .cloned()
                    .collect(),
                ..WalkOutput::default()
            })
        }
    }

    fn digest(bytes: &[u8]) -> Fingerprint {
        let mut hasher = DefaultHasher::new();
        bytes.hash(&mut hasher);
        let word = hasher.finish().to_le_bytes();
        let mut out = [0; 32];
        for chunk in out.chunks_mut(8) {
            chunk.copy_from_slice(&word);
        }
        out
    }

    /// Samples the first and last four bytes, so files that differ only in the
    /// middle share a partial fingerprint.
    #[derive(Default)]
    struct FakeHasher {
        content: HashMap<PathBuf, Vec<u8>>,
        unreadable: HashSet<PathBuf>,
        calls: Mutex<Vec<(&'static str, PathBuf)>>,
        cancel_after: Option<(usize, CancelToken)>,
    }

    impl FakeHasher {
        fn log(&self, kind: &'static str, path: &Path) -> io::Result<&Vec<u8>> {
            let mut calls = self.calls.lock().unwrap();
            calls.push((kind, path.to_path_buf()));
            if let Some((n, token)) = &self.cancel_after
                && calls.len() >= *n
            {
                token.cancel();
            }
            if self.unreadable.contains(path) {
                return Err(io::Error::from(ErrorKind::PermissionDenied));
            }
            Ok(&self.content[path])
        }

        fn hashed(&self, kind: &str) -> Vec<String> {
            let mut paths: Vec<String> = self
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(k, _)| *k == kind)
                .map(|(_, p)| p.to_str().unwrap().to_string())
                .collect();
            paths.sort();
            paths
        }

        fn total_calls(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    impl ContentHasher for FakeHasher {
        fn partial(
            &self,
            path: &Path,
            _size: u64,
            _cancel: &CancelToken,
        ) -> io::Result<Fingerprint> {
            let bytes = self.log("partial", path)?;
            if bytes.len() <= 8 {
                return Ok(digest(bytes));
            }
            let mut sample = bytes[..4].to_vec();
            sample.extend_from_slice(&bytes[bytes.len() - 4..]);
            Ok(digest(&sample))
        }

        fn full(&self, path: &Path, _cancel: &CancelToken) -> io::Result<Fingerprint> {
            Ok(digest(self.log("full", path)?))
        }
    }

    struct World {
        walker: FakeWalker,
        hasher: FakeHasher,
    }

    /// Files are (path, content, inode). All on device 1.
    fn world(files: &[(&str, &[u8], u64)]) -> World {
        World {
            walker: FakeWalker {
                files: files
                    .iter()
                    .map(|(path, bytes, inode)| WalkedFile {
                        path: PathBuf::from(path),
                        size: bytes.len() as u64,
                        device: 1,
                        inode: *inode,
                        modified_secs: 100,
                    })
                    .collect(),
                failing: HashSet::new(),
            },
            hasher: FakeHasher {
                content: files
                    .iter()
                    .map(|(path, bytes, _)| (PathBuf::from(path), bytes.to_vec()))
                    .collect(),
                ..FakeHasher::default()
            },
        }
    }

    fn options(min_size: u64, threads: usize) -> DuplicateOptions {
        DuplicateOptions {
            min_size,
            include_cloud_folders: false,
            exclusions: Exclusions::default(),
            threads,
        }
    }

    fn run_with(
        w: &World,
        protected: &[&str],
        options: &DuplicateOptions,
        cancel: &CancelToken,
    ) -> Result<DuplicateScan, DuplicateError> {
        let scan_roots = [PathBuf::from("/r")];
        let protected_roots: Vec<PathBuf> = protected.iter().map(PathBuf::from).collect();
        find_duplicates(
            &w.walker,
            &w.hasher,
            &DuplicateRequest {
                scan_roots: &scan_roots,
                protected_roots: &protected_roots,
                options,
            },
            cancel,
            &|_| {},
        )
    }

    fn run(w: &World) -> DuplicateScan {
        run_with(w, &[], &options(1, 1), &CancelToken::default()).unwrap()
    }

    fn paths(group: &DuplicateGroup) -> Vec<&str> {
        group
            .files
            .iter()
            .map(|f| f.path.to_str().unwrap())
            .collect()
    }

    const SAME: &[u8] = b"AAAAxxxxBBBB";
    const SAME_ENDS: &[u8] = b"AAAAyyyyBBBB";

    #[test]
    fn identical_files_are_grouped_and_others_are_not() {
        let w = world(&[
            ("/r/a", SAME, 1),
            ("/r/b", SAME, 2),
            ("/r/c", SAME_ENDS, 3),
            ("/r/d", b"zz", 4),
        ]);
        let scan = run(&w);
        assert_eq!(scan.groups.len(), 1);
        assert_eq!(paths(&scan.groups[0]), ["/r/a", "/r/b"]);
        assert_eq!(scan.groups[0].size, 12);
    }

    #[test]
    fn same_ends_but_different_middle_is_not_a_duplicate() {
        let w = world(&[("/r/a", SAME, 1), ("/r/c", SAME_ENDS, 3)]);
        let scan = run(&w);
        assert!(scan.groups.is_empty());
        // The partial fingerprints matched, so only the full hash told them apart.
        assert_eq!(w.hasher.hashed("full"), ["/r/a", "/r/c"]);
    }

    #[test]
    fn files_with_a_unique_size_are_never_read() {
        let w = world(&[
            ("/r/a", b"one", 1),
            ("/r/b", b"three", 2),
            ("/r/c", b"sixsix", 3),
        ]);
        let scan = run(&w);
        assert!(scan.groups.is_empty());
        assert_eq!(w.hasher.total_calls(), 0);
    }

    #[test]
    fn hard_links_are_one_file() {
        let w = world(&[("/r/a", SAME, 1), ("/r/b", SAME, 1)]);
        let scan = run(&w);
        assert!(scan.groups.is_empty());
        assert_eq!(scan.hard_links_collapsed, 1);
    }

    #[test]
    fn a_hard_link_pair_and_a_real_copy_make_one_group_of_two() {
        let w = world(&[("/r/a", SAME, 1), ("/r/b", SAME, 1), ("/r/c", SAME, 2)]);
        let scan = run(&w);
        assert_eq!(scan.groups.len(), 1);
        assert_eq!(paths(&scan.groups[0]), ["/r/a", "/r/c"]);
        assert_eq!(scan.hard_links_collapsed, 1);
        assert!(!w.hasher.hashed("partial").contains(&"/r/b".to_string()));
    }

    #[test]
    fn the_same_inode_on_another_device_is_a_different_file() {
        let mut w = world(&[("/r/a", SAME, 1), ("/r/b", SAME, 1)]);
        w.walker.files[1].device = 2;
        let scan = run(&w);
        assert_eq!(scan.groups.len(), 1);
        assert_eq!(scan.hard_links_collapsed, 0);
    }

    #[test]
    fn small_and_empty_files_are_ignored() {
        let w = world(&[
            ("/r/a", b"1234", 1),
            ("/r/b", b"1234", 2),
            ("/r/e1", b"", 3),
            ("/r/e2", b"", 4),
        ]);
        let by_min_size = run_with(&w, &[], &options(8, 1), &CancelToken::default()).unwrap();
        assert!(by_min_size.groups.is_empty());
        let by_zero_min = run_with(&w, &[], &options(0, 1), &CancelToken::default()).unwrap();
        assert_eq!(by_zero_min.groups.len(), 1);
        assert_eq!(paths(&by_zero_min.groups[0]), ["/r/a", "/r/b"]);
    }

    #[test]
    fn files_under_a_protected_folder_are_flagged() {
        let w = world(&[("/r/keep/a", SAME, 1), ("/r/b", SAME, 2)]);
        let scan = run_with(&w, &["/r/keep"], &options(1, 1), &CancelToken::default()).unwrap();
        assert_eq!(scan.groups.len(), 1);
        let files = &scan.groups[0].files;
        assert_eq!(paths(&scan.groups[0]), ["/r/b", "/r/keep/a"]);
        assert!(!files[0].protected);
        assert!(files[1].protected);
    }

    #[test]
    fn a_group_with_only_protected_files_is_dropped() {
        let w = world(&[("/r/keep/a", SAME, 1), ("/r/keep/b", SAME, 2)]);
        let scan = run_with(&w, &["/r/keep"], &options(1, 1), &CancelToken::default()).unwrap();
        assert!(scan.groups.is_empty());
    }

    #[test]
    fn protection_ignores_letter_case() {
        let w = world(&[("/r/keep/a", SAME, 1), ("/r/b", SAME, 2)]);
        let scan = run_with(&w, &["/r/KEEP"], &options(1, 1), &CancelToken::default()).unwrap();
        assert!(scan.groups[0].files.iter().any(|f| f.protected));
    }

    #[test]
    fn a_hard_link_to_a_protected_file_stays_protected() {
        let w = world(&[("/r/a", SAME, 1), ("/r/keep/z", SAME, 1), ("/r/c", SAME, 2)]);
        let scan = run_with(&w, &["/r/keep"], &options(1, 1), &CancelToken::default()).unwrap();
        let group = &scan.groups[0];
        assert_eq!(paths(group), ["/r/c", "/r/keep/z"]);
        assert!(group.files[1].protected);
    }

    #[test]
    fn a_file_reachable_from_both_roots_is_listed_once() {
        let w = world(&[("/r/keep/a", SAME, 1), ("/r/b", SAME, 2)]);
        let scan = run_with(&w, &["/r/keep"], &options(1, 1), &CancelToken::default()).unwrap();
        assert_eq!(scan.files_listed, 2);
        assert_eq!(scan.groups[0].files.len(), 2);
    }

    #[test]
    fn an_unreadable_file_becomes_an_issue_and_leaves_the_group() {
        let mut w = world(&[("/r/a", SAME, 1), ("/r/b", SAME, 2), ("/r/c", SAME, 3)]);
        w.hasher.unreadable.insert(PathBuf::from("/r/b"));
        let scan = run(&w);
        assert_eq!(paths(&scan.groups[0]), ["/r/a", "/r/c"]);
        assert_eq!(scan.issues.len(), 1);
        assert_eq!(scan.issues[0].path, PathBuf::from("/r/b"));
        assert_eq!(scan.issues[0].kind, ScanIssueKind::PermissionDenied);
    }

    #[test]
    fn a_root_that_cannot_be_read_fails_the_scan() {
        let mut w = world(&[("/r/a", SAME, 1), ("/r/b", SAME, 2)]);
        w.walker.failing.insert(PathBuf::from("/r"));
        let result = run_with(&w, &[], &options(1, 1), &CancelToken::default());
        assert!(matches!(
            result,
            Err(DuplicateError::RootUnreadable { ref path, .. }) if path == Path::new("/r")
        ));
    }

    #[test]
    fn an_unreadable_protected_root_fails_the_scan() {
        let mut w = world(&[("/r/a", SAME, 1), ("/r/b", SAME, 2)]);
        w.walker.failing.insert(PathBuf::from("/r/keep"));
        let result = run_with(&w, &["/r/keep"], &options(1, 1), &CancelToken::default());
        assert!(matches!(result, Err(DuplicateError::RootUnreadable { .. })));
    }

    #[test]
    fn cancelling_before_the_start_returns_cancelled_and_reads_nothing() {
        let w = world(&[("/r/a", SAME, 1), ("/r/b", SAME, 2)]);
        let cancel = CancelToken::default();
        cancel.cancel();
        let result = run_with(&w, &[], &options(1, 1), &cancel);
        assert!(matches!(result, Err(DuplicateError::Cancelled)));
        assert_eq!(w.hasher.total_calls(), 0);
    }

    #[test]
    fn cancelling_while_hashing_stops_the_scan() {
        let mut w = world(&[("/r/a", SAME, 1), ("/r/b", SAME, 2), ("/r/c", SAME, 3)]);
        let cancel = CancelToken::default();
        w.hasher.cancel_after = Some((1, cancel.clone()));
        let result = run_with(&w, &[], &options(1, 1), &cancel);
        assert!(matches!(result, Err(DuplicateError::Cancelled)));
        assert_eq!(w.hasher.total_calls(), 1);
    }

    #[test]
    fn cancelling_during_the_full_hash_stage_stops_the_scan() {
        let mut w = world(&[("/r/a", SAME, 1), ("/r/b", SAME, 2), ("/r/c", SAME, 3)]);
        let cancel = CancelToken::default();
        // Three sample reads come first, so the fourth call is the first full read.
        w.hasher.cancel_after = Some((4, cancel.clone()));
        let result = run_with(&w, &[], &options(1, 1), &cancel);
        assert!(matches!(result, Err(DuplicateError::Cancelled)));
        assert_eq!(w.hasher.hashed("partial").len(), 3);
        assert_eq!(w.hasher.hashed("full").len(), 1);
    }

    #[test]
    fn the_result_does_not_depend_on_the_thread_count() {
        let names: Vec<String> = (0..10)
            .flat_map(|k| [format!("/r/k{k}a"), format!("/r/k{k}b")])
            .chain((0..5).map(|k| format!("/r/solo{k}")))
            .collect();
        let contents: Vec<Vec<u8>> = (0..10u8)
            .flat_map(|k| [vec![k; 16], vec![k; 16]])
            .chain((0..5u8).map(|k| vec![200 + k; 17 + usize::from(k)]))
            .collect();
        let files: Vec<(&str, &[u8], u64)> = names
            .iter()
            .zip(&contents)
            .zip(1..)
            .map(|((n, c), inode)| (n.as_str(), c.as_slice(), inode))
            .collect();

        let summary = |threads: usize| {
            let w = world(&files);
            let scan = run_with(&w, &[], &options(1, threads), &CancelToken::default()).unwrap();
            scan.groups
                .iter()
                .map(|g| (g.id, g.size, paths(g).join(",")))
                .collect::<Vec<_>>()
        };
        let one = summary(1);
        assert_eq!(one.len(), 10);
        assert_eq!(one, summary(4));
    }

    #[test]
    fn groups_are_ordered_by_reclaimable_space_and_ids_are_unique() {
        let big = [7u8; 100];
        let small = [3u8; 10];
        let w = world(&[
            ("/r/s1", &small, 1),
            ("/r/s2", &small, 2),
            ("/r/s3", &small, 3),
            ("/r/b1", &big, 4),
            ("/r/b2", &big, 5),
        ]);
        let scan = run(&w);
        assert_eq!(scan.groups.len(), 2);
        assert_eq!(scan.groups[0].size, 100);
        assert_eq!(scan.groups[0].reclaimable_bytes(), 100);
        assert_eq!(scan.groups[1].reclaimable_bytes(), 20);
        assert_eq!(scan.reclaimable_bytes(), 120);

        let group_ids: Vec<u32> = scan.groups.iter().map(|g| g.id).collect();
        assert_eq!(group_ids, [1, 2]);
        let file_ids: Vec<u32> = scan
            .groups
            .iter()
            .flat_map(|g| g.files.iter().map(|f| f.id))
            .collect();
        assert_eq!(file_ids, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn reclaimable_files_depend_on_protection() {
        let file = |id, protected| DuplicateFile {
            id,
            path: PathBuf::from(format!("/r/{id}")),
            modified_secs: 0,
            protected,
        };
        let plain = DuplicateGroup {
            id: 1,
            size: 10,
            files: vec![file(1, false), file(2, false), file(3, false)],
        };
        assert_eq!(plain.reclaimable_files(), 2);
        let with_protected = DuplicateGroup {
            id: 2,
            size: 10,
            files: vec![file(1, true), file(2, false), file(3, false)],
        };
        assert_eq!(with_protected.reclaimable_files(), 2);
        assert_eq!(with_protected.reclaimable_bytes(), 20);
    }

    #[test]
    fn bytes_hashed_counts_only_the_full_hash_stage() {
        let w = world(&[("/r/a", SAME, 1), ("/r/b", SAME, 2)]);
        let scan = run(&w);
        assert_eq!(scan.bytes_hashed, 24);
        assert_eq!(scan.files_listed, 2);
    }

    #[test]
    fn progress_reports_each_stage_and_ends_complete() {
        let w = world(&[("/r/a", SAME, 1), ("/r/b", SAME, 2)]);
        let seen = Mutex::new(Vec::new());
        let scan_roots = [PathBuf::from("/r")];
        let opts = options(1, 1);
        find_duplicates(
            &w.walker,
            &w.hasher,
            &DuplicateRequest {
                scan_roots: &scan_roots,
                protected_roots: &[],
                options: &opts,
            },
            &CancelToken::default(),
            &|p| seen.lock().unwrap().push(p),
        )
        .unwrap();
        let seen = seen.into_inner().unwrap();
        let mut stages: Vec<Stage> = seen.iter().map(|p| p.stage).collect();
        stages.dedup();
        assert_eq!(
            stages,
            [Stage::Listing, Stage::Fingerprinting, Stage::Hashing]
        );
        let last = seen.last().unwrap();
        assert_eq!((last.done, last.total), (2, 2));
    }

    #[test]
    fn cloud_folders_reported_by_the_walker_are_passed_through() {
        struct CloudWalker;
        impl FileWalker for CloudWalker {
            fn walk(
                &self,
                _root: &Path,
                _options: &DuplicateOptions,
                _cancel: &CancelToken,
                _on_progress: &(dyn Fn(ScanProgress) + Sync),
            ) -> io::Result<WalkOutput> {
                Ok(WalkOutput {
                    skipped_cloud_folders: vec![PathBuf::from("/r/Box")],
                    ..WalkOutput::default()
                })
            }
        }
        let scan_roots = [PathBuf::from("/r")];
        let opts = options(1, 1);
        let scan = find_duplicates(
            &CloudWalker,
            &FakeHasher::default(),
            &DuplicateRequest {
                scan_roots: &scan_roots,
                protected_roots: &[],
                options: &opts,
            },
            &CancelToken::default(),
            &|_| {},
        )
        .unwrap();
        assert_eq!(scan.skipped_cloud_folders, [PathBuf::from("/r/Box")]);
    }
}
