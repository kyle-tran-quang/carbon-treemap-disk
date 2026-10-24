//! Finds Safe cleanup candidates. Read-only: nothing here writes, moves or
//! deletes.
//!
//! Discovery walks only the catalog roots handed to it. For each root it
//! offers either the root's direct children or the root itself, then drops
//! anything excluded by name, reached through a symlink, outside its root
//! after resolving, unreadable, or modified recently. Sizing goes through the
//! `Scanner` and `SizeAccounting` traits so it matches the treemap's numbers.

use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use super::cleanup::{ItemKind, resolve, serialize_path};
use super::cleanup_catalog::{CatalogRoot, Category, NameRules, Offer, is_recent, stays_inside};
use super::config::ScanOptions;
use super::scan::CancelToken;
use super::tree::NodeId;
use crate::platform::{Scanner, SizeAccounting};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SkipReason {
    /// Changed within the recent window, so probably in use.
    Recent,
    /// Could not be read or measured.
    Unreadable,
    /// A symlink. Never followed and never offered.
    Symlink,
    /// Resolves to somewhere outside its catalog root.
    OutsideRoot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    /// Valid for the discovery that produced it.
    pub id: u32,
    pub category: Category,
    #[serde(serialize_with = "serialize_path")]
    pub path: PathBuf,
    pub name: String,
    pub kind: ItemKind,
    pub size: u64,
    pub files: u64,
    /// Seconds since the Unix epoch.
    pub modified_secs: u64,
    /// Some entries inside could not be read, so `size` is a lower bound.
    pub incomplete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Skipped {
    pub category: Category,
    #[serde(serialize_with = "serialize_path")]
    pub path: PathBuf,
    pub name: String,
    pub reason: SkipReason,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Discovery {
    /// Largest first.
    pub candidates: Vec<Candidate>,
    pub skipped: Vec<Skipped>,
    /// Entries left out by name (for example `com.apple.*`). Not listed.
    pub excluded: usize,
}

impl Discovery {
    pub fn total_bytes(&self) -> u64 {
        self.candidates.iter().map(|c| c.size).sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryProgress {
    pub found: usize,
    pub current: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Cancelled;

pub struct DiscoveryContext<'a> {
    pub roots: &'a [CatalogRoot],
    pub rules: &'a NameRules,
    pub now: SystemTime,
    pub recent_window: Duration,
    pub scanner: &'a dyn Scanner,
    pub sizing: &'a dyn SizeAccounting,
}

/// What `examine` decided about one path, before an id is assigned.
enum Examined {
    Candidate(Candidate),
    Skipped(SkipReason),
}

struct Collector<'a> {
    cancel: &'a CancelToken,
    on_progress: &'a dyn Fn(&DiscoveryProgress),
    out: Discovery,
}

impl Collector<'_> {
    fn skip(&mut self, category: Category, path: PathBuf, reason: SkipReason) {
        let name = file_name(&path);
        self.out.skipped.push(Skipped {
            category,
            path,
            name,
            reason,
        });
    }

    fn progress(&self, current: &Path) {
        (self.on_progress)(&DiscoveryProgress {
            found: self.out.candidates.len(),
            current: current.to_path_buf(),
        });
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Runs discovery. Returns `Err(Cancelled)` as soon as `cancel` is set.
pub fn discover(
    ctx: &DiscoveryContext,
    cancel: &CancelToken,
    on_progress: &dyn Fn(&DiscoveryProgress),
) -> Result<Discovery, Cancelled> {
    let mut collector = Collector {
        cancel,
        on_progress,
        out: Discovery::default(),
    };
    for root in ctx.roots {
        visit_root(ctx, root, &mut collector)?;
    }

    let out = &mut collector.out;
    out.candidates
        .sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
    for (index, candidate) in out.candidates.iter_mut().enumerate() {
        candidate.id = u32::try_from(index + 1).unwrap_or(u32::MAX);
    }
    Ok(collector.out)
}

fn visit_root(
    ctx: &DiscoveryContext,
    root: &CatalogRoot,
    collector: &mut Collector,
) -> Result<(), Cancelled> {
    // The parent is resolved and the final component is kept, so a root that
    // is itself a symlink is seen as one instead of being followed.
    let resolved = match resolve(&root.path) {
        Ok(resolved) => resolved,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(_) => {
            collector.skip(root.category, root.path.clone(), SkipReason::Unreadable);
            return Ok(());
        }
    };
    match fs::symlink_metadata(&resolved) {
        Ok(meta) if meta.file_type().is_symlink() => {
            collector.skip(root.category, resolved, SkipReason::Symlink);
            return Ok(());
        }
        Ok(_) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(_) => {
            collector.skip(root.category, resolved, SkipReason::Unreadable);
            return Ok(());
        }
    }

    let paths = match root.offer {
        Offer::Whole => vec![resolved.clone()],
        Offer::Children => match list_children(ctx, &resolved, collector) {
            Ok(paths) => paths,
            Err(_) => {
                collector.skip(root.category, resolved, SkipReason::Unreadable);
                return Ok(());
            }
        },
    };

    for path in paths {
        if collector.cancel.is_cancelled() {
            return Err(Cancelled);
        }
        collector.progress(&path);
        match examine(ctx, collector.cancel, root, &resolved, &path)? {
            Examined::Candidate(candidate) => collector.out.candidates.push(candidate),
            Examined::Skipped(reason) => collector.skip(root.category, path, reason),
        }
    }
    Ok(())
}

/// Direct children of `root` that the name rules allow. Excluded ones are
/// counted, not returned.
fn list_children(
    ctx: &DiscoveryContext,
    root: &Path,
    collector: &mut Collector,
) -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(root)? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        if ctx.rules.excludes(&name.to_string_lossy()) {
            collector.out.excluded += 1;
        } else {
            paths.push(root.join(name));
        }
    }
    Ok(paths)
}

fn examine(
    ctx: &DiscoveryContext,
    cancel: &CancelToken,
    root: &CatalogRoot,
    root_resolved: &Path,
    path: &Path,
) -> Result<Examined, Cancelled> {
    let skipped = |reason| Ok(Examined::Skipped(reason));
    let Ok(meta) = fs::symlink_metadata(path) else {
        return skipped(SkipReason::Unreadable);
    };
    if meta.file_type().is_symlink() {
        return skipped(SkipReason::Symlink);
    }
    let Ok(canonical) = path.canonicalize() else {
        return skipped(SkipReason::Unreadable);
    };
    if !stays_inside(root_resolved, &canonical, root.offer) {
        return skipped(SkipReason::OutsideRoot);
    }
    let Ok(modified) = meta.modified() else {
        return skipped(SkipReason::Unreadable);
    };
    if is_recent(modified, ctx.now, ctx.recent_window) {
        return skipped(SkipReason::Recent);
    }

    let (kind, size, files, incomplete) = if meta.is_dir() {
        match ctx
            .scanner
            .scan(path, &ScanOptions::default(), cancel, &|_| {})
        {
            Ok(output) => (
                ItemKind::Dir,
                output.tree.size(NodeId::ROOT),
                output.tree.file_count(NodeId::ROOT),
                !output.issues.is_empty(),
            ),
            Err(e) if e.kind() == ErrorKind::Interrupted => return Err(Cancelled),
            Err(_) => return skipped(SkipReason::Unreadable),
        }
    } else if meta.is_file() {
        (
            ItemKind::File,
            ctx.sizing.bytes_for(&meta).unwrap_or(0),
            1,
            false,
        )
    } else {
        return skipped(SkipReason::Unreadable);
    };

    Ok(Examined::Candidate(Candidate {
        id: 0,
        category: root.category,
        path: path.to_path_buf(),
        name: file_name(path),
        kind,
        size,
        files,
        modified_secs: modified
            .duration_since(UNIX_EPOCH)
            .map_or(0, |age| age.as_secs()),
        incomplete,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::scan::ScanOutput;
    use crate::core::tree::Tree;
    use crate::platform::Scanner as ScannerTrait;
    use std::fs::Metadata;
    use std::os::unix::fs::{PermissionsExt, symlink};

    const NEVER_RECENT: Duration = Duration::from_secs(60);

    struct FakeSizing;
    impl SizeAccounting for FakeSizing {
        fn bytes_for(&self, meta: &Metadata) -> Option<u64> {
            Some(std::os::unix::fs::MetadataExt::size(meta))
        }
    }

    /// Sums plain files under the root with `std::fs`. Does not follow
    /// symlinks. Fails when a directory cannot be read.
    struct FakeScanner;
    impl FakeScanner {
        fn walk(dir: &Path, sizes: &mut Vec<u64>) -> io::Result<()> {
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let meta = fs::symlink_metadata(entry.path())?;
                if meta.is_dir() {
                    Self::walk(&entry.path(), sizes)?;
                } else if meta.is_file() {
                    sizes.push(std::os::unix::fs::MetadataExt::size(&meta));
                }
            }
            Ok(())
        }
    }
    impl ScannerTrait for FakeScanner {
        fn scan(
            &self,
            root: &Path,
            _options: &ScanOptions,
            cancel: &CancelToken,
            _on_progress: &(dyn Fn(crate::core::scan::ScanProgress) + Sync),
        ) -> io::Result<ScanOutput> {
            if cancel.is_cancelled() {
                return Err(io::Error::new(ErrorKind::Interrupted, "cancelled"));
            }
            let mut sizes = Vec::new();
            Self::walk(root, &mut sizes)?;
            let mut tree = Tree::new(&root.to_string_lossy());
            for (i, size) in sizes.into_iter().enumerate() {
                tree.add_file(NodeId::ROOT, &format!("f{i}"), size);
            }
            tree.rollup();
            Ok(ScanOutput {
                tree,
                issues: Vec::new(),
                skipped_cloud_folders: Vec::new(),
            })
        }
    }

    fn temp_home(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ctd-disc-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn write(path: &Path, len: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![1u8; len]).unwrap();
    }

    fn rules() -> NameRules {
        NameRules::new(["com.apple."], ["com.carbon.treemap"])
    }

    /// Everything counts as old, because "now" is days after creation.
    fn long_after() -> SystemTime {
        SystemTime::now() + Duration::from_secs(5 * 24 * 3600)
    }

    fn run(
        roots: &[CatalogRoot],
        now: SystemTime,
        window: Duration,
        cancel: &CancelToken,
    ) -> Result<Discovery, Cancelled> {
        let rules = rules();
        let ctx = DiscoveryContext {
            roots,
            rules: &rules,
            now,
            recent_window: window,
            scanner: &FakeScanner,
            sizing: &FakeSizing,
        };
        discover(&ctx, cancel, &|_| {})
    }

    fn find(d: &Discovery, name: &str) -> Option<Candidate> {
        d.candidates.iter().find(|c| c.name == name).cloned()
    }

    fn reason_of(d: &Discovery, name: &str) -> Option<SkipReason> {
        d.skipped.iter().find(|s| s.name == name).map(|s| s.reason)
    }

    fn old(roots: &[CatalogRoot]) -> Discovery {
        run(roots, long_after(), NEVER_RECENT, &CancelToken::default()).unwrap()
    }

    #[test]
    fn children_are_offered_with_sizes_and_the_root_is_not() {
        let home = temp_home("children");
        let caches = home.join("Caches");
        write(&caches.join("Homebrew/a.bin"), 3000);
        write(&caches.join("Homebrew/sub/b.bin"), 2000);
        write(&caches.join("loose.txt"), 100);
        let d = old(&[CatalogRoot::children(&caches, Category::AppCaches)]);

        let brew = find(&d, "Homebrew").unwrap();
        assert_eq!((brew.size, brew.files, brew.kind), (5000, 2, ItemKind::Dir));
        let loose = find(&d, "loose.txt").unwrap();
        assert_eq!(
            (loose.size, loose.files, loose.kind),
            (100, 1, ItemKind::File)
        );
        assert_eq!(d.candidates.len(), 2, "the root itself is not offered");
        assert_eq!(d.total_bytes(), 5100);
    }

    #[test]
    fn candidates_are_ordered_largest_first_with_unique_ids_from_one() {
        let home = temp_home("order");
        let caches = home.join("Caches");
        write(&caches.join("small/x"), 10);
        write(&caches.join("big/x"), 9000);
        write(&caches.join("mid/x"), 500);
        let d = old(&[CatalogRoot::children(&caches, Category::AppCaches)]);
        let names: Vec<_> = d.candidates.iter().map(|c| c.name.as_str()).collect();
        let ids: Vec<_> = d.candidates.iter().map(|c| c.id).collect();
        assert_eq!(names, ["big", "mid", "small"]);
        assert_eq!(ids, [1, 2, 3]);
    }

    #[test]
    fn excluded_names_are_counted_and_not_offered_or_listed() {
        let home = temp_home("excluded");
        let caches = home.join("Caches");
        write(&caches.join("com.apple.Music/x"), 10);
        write(&caches.join("COM.APPLE.Safari/x"), 10);
        write(&caches.join("com.carbon.treemap/x"), 10);
        write(&caches.join("Homebrew/x"), 10);
        let d = old(&[CatalogRoot::children(&caches, Category::AppCaches)]);
        assert_eq!(d.candidates.len(), 1);
        assert_eq!(d.candidates[0].name, "Homebrew");
        assert_eq!(d.excluded, 3);
        assert!(d.skipped.is_empty());
    }

    #[test]
    fn recent_items_are_skipped_and_old_ones_are_kept() {
        let home = temp_home("recent");
        let caches = home.join("Caches");
        write(&caches.join("Fresh/x"), 10);
        let roots = [CatalogRoot::children(&caches, Category::AppCaches)];

        let now = run(
            &roots,
            SystemTime::now(),
            Duration::from_secs(3600),
            &CancelToken::default(),
        )
        .unwrap();
        assert!(now.candidates.is_empty());
        assert_eq!(reason_of(&now, "Fresh"), Some(SkipReason::Recent));

        assert!(find(&old(&roots), "Fresh").is_some());
    }

    #[test]
    fn a_symlink_child_is_skipped_and_its_target_is_never_offered() {
        let home = temp_home("symchild");
        let caches = home.join("Caches");
        let secret = home.join("Secret");
        write(&secret.join("key"), 50);
        fs::create_dir_all(&caches).unwrap();
        symlink(&secret, caches.join("link")).unwrap();
        let d = old(&[CatalogRoot::children(&caches, Category::AppCaches)]);
        assert!(d.candidates.is_empty());
        assert_eq!(reason_of(&d, "link"), Some(SkipReason::Symlink));
    }

    #[test]
    fn a_symlinked_root_is_skipped_whole() {
        let home = temp_home("symroot");
        let real = home.join("Real");
        write(&real.join("a/x"), 10);
        let link = home.join("Caches");
        symlink(&real, &link).unwrap();
        let d = old(&[CatalogRoot::children(&link, Category::AppCaches)]);
        assert!(d.candidates.is_empty());
        assert_eq!(reason_of(&d, "Caches"), Some(SkipReason::Symlink));
    }

    #[test]
    fn a_missing_root_is_ignored_without_a_trace() {
        let home = temp_home("missing");
        let d = old(&[CatalogRoot::children(home.join("Nope"), Category::AppLogs)]);
        assert!(d.candidates.is_empty() && d.skipped.is_empty());
    }

    #[test]
    fn a_whole_root_is_offered_once_as_itself() {
        let home = temp_home("whole");
        let npm = home.join(".npm/_cacache");
        write(&npm.join("content/a"), 700);
        write(&npm.join("index/b"), 300);
        let d = old(&[CatalogRoot::whole(&npm, Category::NpmCache)]);
        assert_eq!(d.candidates.len(), 1);
        let c = &d.candidates[0];
        assert_eq!((c.name.as_str(), c.size, c.files), ("_cacache", 1000, 2));
        assert_eq!(c.category, Category::NpmCache);
    }

    #[test]
    fn an_unreadable_folder_is_skipped_and_the_rest_continue() {
        let home = temp_home("unreadable");
        let caches = home.join("Caches");
        write(&caches.join("Locked/x"), 10);
        write(&caches.join("Open/x"), 10);
        let locked = caches.join("Locked");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        let d = old(&[CatalogRoot::children(&caches, Category::AppCaches)]);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(find(&d, "Open").is_some());
        assert_eq!(reason_of(&d, "Locked"), Some(SkipReason::Unreadable));
    }

    #[test]
    fn cancelling_before_or_during_discovery_stops_it() {
        let home = temp_home("cancel");
        let caches = home.join("Caches");
        write(&caches.join("A/x"), 10);
        let cancel = CancelToken::default();
        cancel.cancel();
        let roots = [CatalogRoot::children(&caches, Category::AppCaches)];
        assert_eq!(
            run(&roots, long_after(), NEVER_RECENT, &cancel).unwrap_err(),
            Cancelled
        );
    }

    #[test]
    fn discovery_changes_nothing_on_disk() {
        let home = temp_home("readonly");
        let caches = home.join("Caches");
        write(&caches.join("A/x"), 10);
        write(&caches.join("B.txt"), 10);
        old(&[CatalogRoot::children(&caches, Category::AppCaches)]);
        assert!(caches.join("A/x").exists() && caches.join("B.txt").exists());
    }

    #[test]
    fn progress_reports_each_path() {
        let home = temp_home("progress");
        let caches = home.join("Caches");
        write(&caches.join("A/x"), 10);
        write(&caches.join("B/x"), 10);
        let rules = rules();
        let roots = [CatalogRoot::children(&caches, Category::AppCaches)];
        let ctx = DiscoveryContext {
            roots: &roots,
            rules: &rules,
            now: long_after(),
            recent_window: NEVER_RECENT,
            scanner: &FakeScanner,
            sizing: &FakeSizing,
        };
        let seen = std::sync::Mutex::new(Vec::new());
        discover(&ctx, &CancelToken::default(), &|p| {
            seen.lock().unwrap().push(p.current.clone());
        })
        .unwrap();
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    #[test]
    fn candidates_serialize_in_camel_case() {
        let home = temp_home("json");
        let caches = home.join("Caches");
        write(&caches.join("A/x"), 10);
        let d = old(&[CatalogRoot::children(&caches, Category::AppCaches)]);
        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(json["candidates"][0]["category"], "appCaches");
        assert!(json["candidates"][0]["modifiedSecs"].is_number());
        assert_eq!(json["candidates"][0]["kind"], "dir");
        assert_eq!(json["excluded"], 0);
    }
}
