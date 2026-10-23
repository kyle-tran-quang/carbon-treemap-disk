//! Cleanup planning and execution. OS-agnostic.
//!
//! The flow is two steps so that nothing is moved without a preview:
//!
//! 1. `plan_cleanup` turns selected node ids into a `CleanupPlan`: the exact
//!    paths that would be moved, their sizes, and the items left out with a
//!    reason. It performs no writes.
//! 2. `execute` moves the items of a plan to the Trash through a
//!    `TrashProvider`. Nothing in this module deletes permanently.
//!
//! Guardrails are applied to the *resolved* path (parent directory
//! canonicalised, final component kept), which is the path the Trash call
//! acts on. They are checked again at execution time.
//!
//! Sizes in a plan come from the scan, not from disk. Moving to the Trash
//! does not free space until the Trash is emptied, and shared storage (hard
//! links, APFS clones) can make the space actually freed differ from the
//! scanned size.

use std::collections::HashSet;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Component, Path, PathBuf};

use serde::{Serialize, Serializer};

use super::tree::{NodeId, NodeKind, Tree};
use super::view::breadcrumbs;
use crate::platform::TrashProvider;

#[allow(clippy::ptr_arg)] // serde's `serialize_with` passes `&PathBuf`.
fn serialize_path<S: Serializer>(path: &PathBuf, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&path.to_string_lossy())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ReasonKind {
    InvalidPath,
    ScanRoot,
    Inaccessible,
    Missing,
    SystemPath,
    HomeFolder,
    Trash,
    AppData,
    OutsideHome,
}

/// Structured so the UI can branch on `kind` instead of parsing text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reason {
    pub kind: ReasonKind,
    pub message: String,
}

impl Reason {
    fn new(kind: ReasonKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The path and everything below it.
    Tree,
    /// Only the path itself. Its contents may be moved.
    Dir,
}

/// A location that must not be moved. Anything that contains one is blocked
/// too, because moving the container would move the protected path with it.
#[derive(Debug, Clone)]
pub struct Protected {
    pub path: PathBuf,
    pub scope: Scope,
    pub kind: ReasonKind,
}

impl Protected {
    pub fn tree(path: impl Into<PathBuf>, kind: ReasonKind) -> Self {
        Self {
            path: path.into(),
            scope: Scope::Tree,
            kind,
        }
    }

    pub fn dir(path: impl Into<PathBuf>, kind: ReasonKind) -> Self {
        Self {
            path: path.into(),
            scope: Scope::Dir,
            kind,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allowed,
    /// Allowed, but the user must acknowledge the warning.
    Warn(Reason),
    Blocked(Reason),
}

/// Comparison key. Lower-cased because default macOS volumes are
/// case-insensitive. On a case-sensitive volume this can only over-block.
fn fold(path: &Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().to_lowercase())
}

#[derive(Debug)]
struct Rule {
    original: PathBuf,
    folded: PathBuf,
    scope: Scope,
    kind: ReasonKind,
}

#[derive(Debug)]
pub struct Guardrails {
    home: Option<PathBuf>,
    rules: Vec<Rule>,
}

fn describe(kind: ReasonKind) -> &'static str {
    match kind {
        ReasonKind::SystemPath => "a protected system location",
        ReasonKind::HomeFolder => "a protected folder",
        ReasonKind::Trash => "the Trash",
        ReasonKind::AppData => "the app's own data",
        _ => "a protected location",
    }
}

impl Guardrails {
    /// `home` is the user's home folder. When it is unknown, every path
    /// counts as outside it and needs acknowledgement.
    pub fn new(home: Option<PathBuf>, protected: Vec<Protected>) -> Self {
        let rules = protected
            .into_iter()
            .map(|p| Rule {
                folded: fold(&p.path),
                original: p.path,
                scope: p.scope,
                kind: p.kind,
            })
            .collect();
        Self {
            home: home.as_deref().map(fold),
            rules,
        }
    }

    pub fn verdict(&self, path: &Path) -> Verdict {
        // `Path` already drops an interior `.`, which names the same location.
        let has_parent_ref = path.components().any(|c| matches!(c, Component::ParentDir));
        if !path.is_absolute() || has_parent_ref {
            return Verdict::Blocked(Reason::new(
                ReasonKind::InvalidPath,
                "Not an absolute path.",
            ));
        }
        let folded = fold(path);
        for rule in &self.rules {
            let equal = folded == rule.folded;
            let inside = match rule.scope {
                Scope::Tree => folded.starts_with(&rule.folded),
                Scope::Dir => equal,
            };
            if inside {
                let message = if equal {
                    format!("This is {}.", describe(rule.kind))
                } else {
                    format!("This is inside {}.", describe(rule.kind))
                };
                return Verdict::Blocked(Reason::new(rule.kind, message));
            }
            if rule.folded.starts_with(&folded) {
                return Verdict::Blocked(Reason::new(
                    rule.kind,
                    format!(
                        "This contains {}: {}.",
                        describe(rule.kind),
                        rule.original.display()
                    ),
                ));
            }
        }
        let in_home = self
            .home
            .as_ref()
            .is_some_and(|home| folded.starts_with(home));
        if in_home {
            Verdict::Allowed
        } else {
            Verdict::Warn(Reason::new(
                ReasonKind::OutsideHome,
                "Outside your home folder. Make sure you want to remove it.",
            ))
        }
    }
}

/// Canonicalises the parent and keeps the final component, which is what the
/// Trash call acts on. A final component that is a symlink is not followed.
pub fn resolve(path: &Path) -> io::Result<PathBuf> {
    let invalid = || io::Error::new(ErrorKind::InvalidInput, "path has no parent or name");
    let parent = path.parent().ok_or_else(invalid)?;
    let name = path.file_name().ok_or_else(invalid)?;
    Ok(parent.canonicalize()?.join(name))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemKind {
    Dir,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanItem {
    /// Raw tree id, valid for the scan the plan was made from.
    pub node: u32,
    #[serde(serialize_with = "serialize_path")]
    pub path: PathBuf,
    pub name: String,
    pub kind: ItemKind,
    pub size: u64,
    pub files: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<Reason>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExcludedItem {
    pub node: u32,
    #[serde(serialize_with = "serialize_path")]
    pub path: PathBuf,
    pub name: String,
    pub reason: Reason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupPlan {
    /// Largest first.
    pub items: Vec<PlanItem>,
    pub excluded: Vec<ExcludedItem>,
    /// Selected items left out because a selected folder already contains them.
    pub merged: usize,
    pub total_bytes: u64,
    pub total_files: u64,
}

impl CleanupPlan {
    pub fn needs_acknowledgement(&self) -> bool {
        self.items.iter().any(|item| item.warning.is_some())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct UnknownNode(pub u32);

fn node_path(tree: &Tree, id: NodeId) -> PathBuf {
    let mut crumbs = breadcrumbs(tree, id).into_iter();
    let mut path = PathBuf::from(crumbs.next().map(|c| c.name).unwrap_or_default());
    path.extend(crumbs.map(|c| c.name));
    path
}

fn lookup_error(error: &io::Error) -> Reason {
    if error.kind() == ErrorKind::NotFound {
        Reason::new(
            ReasonKind::Missing,
            "No longer exists on disk. Rescan to refresh.",
        )
    } else {
        Reason::new(ReasonKind::Inaccessible, error.to_string())
    }
}

fn classify(
    tree: &Tree,
    id: NodeId,
    path: &Path,
    guardrails: &Guardrails,
) -> Result<(ItemKind, PathBuf, Option<Reason>), Reason> {
    if id == NodeId::ROOT {
        return Err(Reason::new(
            ReasonKind::ScanRoot,
            "The scanned folder itself can't be moved. Select something inside it.",
        ));
    }
    let kind = match tree.kind(id) {
        NodeKind::Dir => ItemKind::Dir,
        NodeKind::File => ItemKind::File,
        NodeKind::Inaccessible => {
            return Err(Reason::new(
                ReasonKind::Inaccessible,
                "Not accessible, so its contents are unknown.",
            ));
        }
    };
    let resolved = resolve(path).map_err(|e| lookup_error(&e))?;
    fs::symlink_metadata(&resolved).map_err(|e| lookup_error(&e))?;
    match guardrails.verdict(&resolved) {
        Verdict::Blocked(reason) => Err(reason),
        Verdict::Warn(reason) => Ok((kind, resolved, Some(reason))),
        Verdict::Allowed => Ok((kind, resolved, None)),
    }
}

/// Builds the dry-run result for `nodes`. Performs no writes.
///
/// Fails only when a node id is not in the tree, which means the caller is
/// working from an older scan.
pub fn plan_cleanup(
    tree: &Tree,
    nodes: &[u32],
    guardrails: &Guardrails,
) -> Result<CleanupPlan, UnknownNode> {
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    let mut excluded = Vec::new();

    for &raw in nodes {
        let id = tree.node_id(raw).ok_or(UnknownNode(raw))?;
        if !seen.insert(raw) {
            continue;
        }
        let path = node_path(tree, id);
        let name = tree.name(id).to_owned();
        match classify(tree, id, &path, guardrails) {
            Ok((kind, resolved, warning)) => items.push(PlanItem {
                node: raw,
                path: resolved,
                name,
                kind,
                size: tree.size(id),
                files: tree.file_count(id),
                warning,
            }),
            Err(reason) => excluded.push(ExcludedItem {
                node: raw,
                path,
                name,
                reason,
            }),
        }
    }

    let paths: HashSet<PathBuf> = items.iter().map(|item| fold(&item.path)).collect();
    let before = items.len();
    items.retain(|item| {
        !item
            .path
            .ancestors()
            .skip(1)
            .any(|ancestor| paths.contains(&fold(ancestor)))
    });
    let merged = before - items.len();

    items.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
    let total_bytes = items.iter().map(|item| item.size).sum();
    let total_files = items.iter().map(|item| item.files).sum();
    Ok(CleanupPlan {
        items,
        excluded,
        merged,
        total_bytes,
        total_files,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    Moved,
    /// The path no longer existed, so there was nothing to move.
    AlreadyGone,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemResult {
    pub node: u32,
    #[serde(serialize_with = "serialize_path")]
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub files: u64,
    pub outcome: Outcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

fn move_one(
    item: &PlanItem,
    guardrails: &Guardrails,
    provider: &dyn TrashProvider,
) -> (Outcome, Option<String>) {
    let failed = |message: String| (Outcome::Failed, Some(message));
    match resolve(&item.path) {
        Ok(resolved) if resolved == item.path => {}
        Ok(_) => {
            return failed("The location changed since the preview. Preview again.".into());
        }
        Err(e) if e.kind() == ErrorKind::NotFound => return (Outcome::AlreadyGone, None),
        Err(e) => return failed(e.to_string()),
    }
    if let Verdict::Blocked(reason) = guardrails.verdict(&item.path) {
        return failed(reason.message);
    }
    match fs::symlink_metadata(&item.path) {
        Ok(meta) if meta.is_dir() != (item.kind == ItemKind::Dir) => {
            return failed("It changed type since the preview. Preview again.".into());
        }
        Ok(_) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => return (Outcome::AlreadyGone, None),
        Err(e) => return failed(e.to_string()),
    }
    match provider.move_to_trash(&item.path) {
        Ok(()) => (Outcome::Moved, None),
        Err(e) if e.kind() == ErrorKind::NotFound => (Outcome::AlreadyGone, None),
        Err(e) => failed(e.to_string()),
    }
}

/// Moves each item to the Trash. A failure on one item does not stop the
/// others. Every item is re-checked against the guardrails first.
pub fn execute(
    items: &[PlanItem],
    guardrails: &Guardrails,
    provider: &dyn TrashProvider,
) -> Vec<ItemResult> {
    items
        .iter()
        .map(|item| {
            let (outcome, message) = move_one(item, guardrails, provider);
            ItemResult {
                node: item.node,
                path: item.path.clone(),
                name: item.name.clone(),
                size: item.size,
                files: item.files,
                outcome,
                message,
            }
        })
        .collect()
}

/// Removes moved items from the tree and updates every ancestor's size and
/// file count, without a rescan. Items that failed stay in the tree.
pub fn apply_results(tree: &mut Tree, results: &[ItemResult]) {
    for result in results {
        if result.outcome == Outcome::Failed {
            continue;
        }
        if let Some(id) = tree.node_id(result.node) {
            tree.detach(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const MIB: u64 = 1024 * 1024;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ctd-cleanup-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    /// root/a/{big.bin 10 MiB, b/mid.bin 2 MiB}, root/c.bin 3 MiB — on disk
    /// and in the tree. Returns the tree and the ids (a, b, big, mid, c).
    struct Fixture {
        root: PathBuf,
        tree: Tree,
        a: u32,
        b: u32,
        big: u32,
        mid: u32,
        c: u32,
    }

    fn fixture(name: &str) -> Fixture {
        let root = temp_root(name);
        fs::create_dir_all(root.join("a/b")).unwrap();
        for file in ["a/big.bin", "a/b/mid.bin", "c.bin"] {
            fs::write(root.join(file), b"x").unwrap();
        }
        let mut tree = Tree::new(&root.to_string_lossy());
        let a = tree.add_dir(NodeId::ROOT, "a");
        let b = tree.add_dir(a, "b");
        let big = tree.add_file(a, "big.bin", 10 * MIB);
        let mid = tree.add_file(b, "mid.bin", 2 * MIB);
        let c = tree.add_file(NodeId::ROOT, "c.bin", 3 * MIB);
        tree.rollup();
        Fixture {
            root,
            tree,
            a: a.raw(),
            b: b.raw(),
            big: big.raw(),
            mid: mid.raw(),
            c: c.raw(),
        }
    }

    fn open_guardrails(root: &Path) -> Guardrails {
        Guardrails::new(Some(root.to_path_buf()), Vec::new())
    }

    #[derive(Default)]
    struct FakeTrash {
        moved: Mutex<Vec<PathBuf>>,
        fail_on: Option<PathBuf>,
    }

    impl TrashProvider for FakeTrash {
        fn move_to_trash(&self, path: &Path) -> io::Result<()> {
            if self.fail_on.as_deref() == Some(path) {
                return Err(io::Error::new(
                    ErrorKind::PermissionDenied,
                    "permission denied",
                ));
            }
            self.moved.lock().unwrap().push(path.to_path_buf());
            Ok(())
        }
    }

    fn system_rules() -> Vec<Protected> {
        vec![
            Protected::tree("/System", ReasonKind::SystemPath),
            Protected::dir("/Users", ReasonKind::SystemPath),
            Protected::dir("/Users/me", ReasonKind::HomeFolder),
            Protected::dir("/Users/me/Documents", ReasonKind::HomeFolder),
            Protected::tree("/Users/me/.Trash", ReasonKind::Trash),
            Protected::tree("/Users/me/Library/Caches/com.app", ReasonKind::AppData),
        ]
    }

    fn rules() -> Guardrails {
        Guardrails::new(Some("/Users/me".into()), system_rules())
    }

    fn blocked_kind(verdict: Verdict) -> ReasonKind {
        match verdict {
            Verdict::Blocked(reason) => reason.kind,
            other => panic!("expected blocked, got {other:?}"),
        }
    }

    // ---- guardrails ----

    #[test]
    fn protected_tree_blocks_itself_and_everything_below() {
        let g = rules();
        for path in ["/System", "/System/Library/Foo", "/Users/me/.Trash/x"] {
            assert!(
                matches!(g.verdict(Path::new(path)), Verdict::Blocked(_)),
                "{path}"
            );
        }
        assert_eq!(
            blocked_kind(g.verdict(Path::new("/System/x"))),
            ReasonKind::SystemPath
        );
        assert_eq!(
            blocked_kind(g.verdict(Path::new("/Users/me/.Trash"))),
            ReasonKind::Trash
        );
    }

    #[test]
    fn protected_dir_blocks_itself_but_not_its_contents() {
        let g = rules();
        assert_eq!(
            blocked_kind(g.verdict(Path::new("/Users/me/Documents"))),
            ReasonKind::HomeFolder
        );
        assert_eq!(
            g.verdict(Path::new("/Users/me/Documents/report.pdf")),
            Verdict::Allowed
        );
    }

    #[test]
    fn anything_containing_a_protected_path_is_blocked() {
        let g = rules();
        assert_eq!(
            blocked_kind(g.verdict(Path::new("/"))),
            ReasonKind::SystemPath
        );
        assert_eq!(
            blocked_kind(g.verdict(Path::new("/Users"))),
            ReasonKind::SystemPath
        );
        // The home folder is protected, and so is the app's data below it.
        assert_eq!(
            blocked_kind(g.verdict(Path::new("/Users/me/Library/Caches"))),
            ReasonKind::AppData
        );
        let Verdict::Blocked(reason) = g.verdict(Path::new("/Users/me/Library")) else {
            panic!("expected blocked");
        };
        assert!(
            reason.message.contains("This contains"),
            "{}",
            reason.message
        );
    }

    #[test]
    fn app_data_is_blocked() {
        let g = rules();
        assert_eq!(
            blocked_kind(g.verdict(Path::new("/Users/me/Library/Caches/com.app/cache.db"))),
            ReasonKind::AppData
        );
    }

    #[test]
    fn a_sibling_sharing_a_name_prefix_is_not_blocked() {
        let g = rules();
        assert_eq!(
            g.verdict(Path::new("/Users/me/Documents2")),
            Verdict::Allowed
        );
        assert_eq!(
            g.verdict(Path::new("/Users/me/Library/Caches/com.app2")),
            Verdict::Allowed
        );
    }

    #[test]
    fn matching_ignores_case() {
        let g = rules();
        assert!(matches!(
            g.verdict(Path::new("/system/library")),
            Verdict::Blocked(_)
        ));
        assert!(matches!(
            g.verdict(Path::new("/USERS/ME/documents")),
            Verdict::Blocked(_)
        ));
    }

    #[test]
    fn relative_and_dotdot_paths_are_blocked() {
        let g = rules();
        for path in ["relative/path", "./x", "/Users/me/../../System"] {
            assert_eq!(
                blocked_kind(g.verdict(Path::new(path))),
                ReasonKind::InvalidPath,
                "{path}"
            );
        }
    }

    #[test]
    fn an_interior_dot_is_judged_as_the_path_it_names() {
        let g = rules();
        assert_eq!(
            blocked_kind(g.verdict(Path::new("/Users/./me/Documents"))),
            ReasonKind::HomeFolder
        );
        assert_eq!(
            g.verdict(Path::new("/Users/./me/Downloads/x")),
            Verdict::Allowed
        );
    }

    #[test]
    fn paths_outside_home_warn_and_inside_home_do_not() {
        let g = rules();
        assert_eq!(
            g.verdict(Path::new("/Users/me/Downloads/big.iso")),
            Verdict::Allowed
        );
        let Verdict::Warn(reason) = g.verdict(Path::new("/Applications/Foo.app")) else {
            panic!("expected a warning");
        };
        assert_eq!(reason.kind, ReasonKind::OutsideHome);
        assert!(matches!(
            g.verdict(Path::new("/Users/meow/x")),
            Verdict::Warn(_)
        ));
    }

    #[test]
    fn everything_warns_when_home_is_unknown() {
        let g = Guardrails::new(None, Vec::new());
        assert!(matches!(
            g.verdict(Path::new("/Users/me/x")),
            Verdict::Warn(_)
        ));
    }

    // ---- planning ----

    #[test]
    fn plans_files_and_folders_with_their_scanned_sizes() {
        let f = fixture("plan");
        let plan = plan_cleanup(&f.tree, &[f.c, f.a], &open_guardrails(&f.root)).unwrap();
        assert_eq!(plan.items.len(), 2);
        assert_eq!(plan.items[0].path, f.root.join("a"));
        assert_eq!(plan.items[0].kind, ItemKind::Dir);
        assert_eq!(plan.items[0].size, 12 * MIB);
        assert_eq!(plan.items[0].files, 2);
        assert_eq!(plan.items[1].path, f.root.join("c.bin"));
        assert_eq!(plan.total_bytes, 15 * MIB);
        assert_eq!(plan.total_files, 3);
        assert!(plan.excluded.is_empty());
        assert!(!plan.needs_acknowledgement());
    }

    #[test]
    fn planning_writes_nothing() {
        let f = fixture("dryrun");
        plan_cleanup(&f.tree, &[f.a, f.c], &open_guardrails(&f.root)).unwrap();
        assert!(f.root.join("a/big.bin").exists());
        assert!(f.root.join("c.bin").exists());
    }

    #[test]
    fn a_selected_folder_absorbs_selected_items_inside_it() {
        let f = fixture("merge");
        let plan = plan_cleanup(
            &f.tree,
            &[f.mid, f.a, f.big, f.b],
            &open_guardrails(&f.root),
        )
        .unwrap();
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.items[0].path, f.root.join("a"));
        assert_eq!(plan.merged, 3);
        assert_eq!(
            plan.total_bytes,
            12 * MIB,
            "inner items must not be counted twice"
        );
    }

    #[test]
    fn duplicate_ids_are_counted_once() {
        let f = fixture("dupe");
        let plan = plan_cleanup(&f.tree, &[f.c, f.c], &open_guardrails(&f.root)).unwrap();
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.merged, 0);
    }

    #[test]
    fn an_unknown_node_is_an_error() {
        let f = fixture("unknown");
        assert_eq!(
            plan_cleanup(&f.tree, &[9999], &open_guardrails(&f.root)),
            Err(UnknownNode(9999))
        );
    }

    #[test]
    fn the_scan_root_is_excluded() {
        let f = fixture("root");
        let plan = plan_cleanup(&f.tree, &[0], &open_guardrails(&f.root)).unwrap();
        assert!(plan.items.is_empty());
        assert_eq!(plan.excluded[0].reason.kind, ReasonKind::ScanRoot);
    }

    #[test]
    fn inaccessible_entries_are_excluded() {
        let mut f = fixture("denied");
        let denied = f.tree.add_inaccessible(NodeId::ROOT, "denied").raw();
        f.tree.rollup();
        let plan = plan_cleanup(&f.tree, &[denied], &open_guardrails(&f.root)).unwrap();
        assert_eq!(plan.excluded[0].reason.kind, ReasonKind::Inaccessible);
    }

    #[test]
    fn items_missing_on_disk_are_excluded() {
        let f = fixture("missing");
        fs::remove_file(f.root.join("c.bin")).unwrap();
        let plan = plan_cleanup(&f.tree, &[f.c, f.a], &open_guardrails(&f.root)).unwrap();
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.excluded.len(), 1);
        assert_eq!(plan.excluded[0].reason.kind, ReasonKind::Missing);
        assert_eq!(plan.excluded[0].name, "c.bin");
    }

    #[test]
    fn protected_items_are_excluded_with_their_reason() {
        let f = fixture("protected");
        let guardrails = Guardrails::new(
            Some(f.root.clone()),
            vec![Protected::tree(f.root.join("a"), ReasonKind::AppData)],
        );
        let plan = plan_cleanup(&f.tree, &[f.a, f.b, f.c], &guardrails).unwrap();
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.items[0].name, "c.bin");
        assert_eq!(plan.excluded.len(), 2);
        assert!(
            plan.excluded
                .iter()
                .all(|e| e.reason.kind == ReasonKind::AppData)
        );
    }

    #[test]
    fn items_outside_home_carry_a_warning_that_needs_acknowledgement() {
        let f = fixture("warn");
        let guardrails = Guardrails::new(Some(PathBuf::from("/somewhere/else")), Vec::new());
        let plan = plan_cleanup(&f.tree, &[f.c], &guardrails).unwrap();
        assert_eq!(
            plan.items[0].warning.as_ref().unwrap().kind,
            ReasonKind::OutsideHome
        );
        assert!(plan.needs_acknowledgement());
    }

    #[test]
    fn plan_serializes_in_camel_case_with_string_paths() {
        let f = fixture("json");
        let plan = plan_cleanup(&f.tree, &[f.c, 0], &open_guardrails(&f.root)).unwrap();
        let json = serde_json::to_value(&plan).unwrap();
        assert_eq!(json["totalBytes"], 3 * MIB);
        assert_eq!(json["items"][0]["kind"], "file");
        assert!(json["items"][0]["path"].is_string());
        assert_eq!(json["excluded"][0]["reason"]["kind"], "scanRoot");
        assert!(json["items"][0].get("warning").is_none());
    }

    // ---- execution ----

    #[test]
    fn moves_every_item_through_the_provider() {
        let f = fixture("exec");
        let guardrails = open_guardrails(&f.root);
        let plan = plan_cleanup(&f.tree, &[f.a, f.c], &guardrails).unwrap();
        let trash = FakeTrash::default();
        let results = execute(&plan.items, &guardrails, &trash);
        assert!(results.iter().all(|r| r.outcome == Outcome::Moved));
        assert_eq!(
            *trash.moved.lock().unwrap(),
            vec![f.root.join("a"), f.root.join("c.bin")]
        );
    }

    #[test]
    fn one_failure_does_not_stop_the_rest() {
        let f = fixture("partial");
        let guardrails = open_guardrails(&f.root);
        let plan = plan_cleanup(&f.tree, &[f.a, f.c], &guardrails).unwrap();
        let trash = FakeTrash {
            fail_on: Some(f.root.join("a")),
            ..FakeTrash::default()
        };
        let results = execute(&plan.items, &guardrails, &trash);
        assert_eq!(results[0].outcome, Outcome::Failed);
        assert_eq!(results[0].message.as_deref(), Some("permission denied"));
        assert_eq!(results[1].outcome, Outcome::Moved);
    }

    #[test]
    fn an_item_removed_after_the_preview_is_reported_as_already_gone() {
        let f = fixture("gone");
        let guardrails = open_guardrails(&f.root);
        let plan = plan_cleanup(&f.tree, &[f.c], &guardrails).unwrap();
        fs::remove_file(f.root.join("c.bin")).unwrap();
        let trash = FakeTrash::default();
        let results = execute(&plan.items, &guardrails, &trash);
        assert_eq!(results[0].outcome, Outcome::AlreadyGone);
        assert!(trash.moved.lock().unwrap().is_empty());
    }

    #[test]
    fn guardrails_are_checked_again_at_execution() {
        let f = fixture("recheck");
        let plan = plan_cleanup(&f.tree, &[f.c], &open_guardrails(&f.root)).unwrap();
        let stricter = Guardrails::new(
            Some(f.root.clone()),
            vec![Protected::tree(
                f.root.join("c.bin"),
                ReasonKind::SystemPath,
            )],
        );
        let trash = FakeTrash::default();
        let results = execute(&plan.items, &stricter, &trash);
        assert_eq!(results[0].outcome, Outcome::Failed);
        assert!(
            trash.moved.lock().unwrap().is_empty(),
            "the provider must not be called"
        );
    }

    #[test]
    fn a_changed_item_type_is_not_moved() {
        let f = fixture("retyped");
        let guardrails = open_guardrails(&f.root);
        let plan = plan_cleanup(&f.tree, &[f.c], &guardrails).unwrap();
        fs::remove_file(f.root.join("c.bin")).unwrap();
        fs::create_dir(f.root.join("c.bin")).unwrap();
        let trash = FakeTrash::default();
        let results = execute(&plan.items, &guardrails, &trash);
        assert_eq!(results[0].outcome, Outcome::Failed);
        assert!(trash.moved.lock().unwrap().is_empty());
    }

    #[test]
    fn a_path_that_resolves_elsewhere_is_not_moved() {
        let f = fixture("moved-parent");
        let guardrails = open_guardrails(&f.root);
        let item = PlanItem {
            node: f.c,
            // `a/../c.bin` is never what resolve() returns, so it counts as changed.
            path: f.root.join("a").join("..").join("c.bin"),
            name: "c.bin".into(),
            kind: ItemKind::File,
            size: 1,
            files: 1,
            warning: None,
        };
        let trash = FakeTrash::default();
        let results = execute(&[item], &guardrails, &trash);
        assert_eq!(results[0].outcome, Outcome::Failed);
        assert!(trash.moved.lock().unwrap().is_empty());
    }

    // ---- applying results ----

    #[test]
    fn applying_results_matches_a_fresh_rollup() {
        let mut f = fixture("apply");
        let guardrails = open_guardrails(&f.root);
        let plan = plan_cleanup(&f.tree, &[f.b, f.c], &guardrails).unwrap();
        let results = execute(&plan.items, &guardrails, &FakeTrash::default());
        apply_results(&mut f.tree, &results);

        assert_eq!(f.tree.size(NodeId::ROOT), 10 * MIB);
        assert_eq!(f.tree.file_count(NodeId::ROOT), 1);
        let incremental = (f.tree.size(NodeId::ROOT), f.tree.file_count(NodeId::ROOT));
        f.tree.rollup();
        assert_eq!(
            incremental,
            (f.tree.size(NodeId::ROOT), f.tree.file_count(NodeId::ROOT))
        );
        assert!(f.tree.node_id(f.b).is_none());
        assert!(f.tree.node_id(f.mid).is_none());
        assert!(f.tree.node_id(f.big).is_some(), "unrelated ids stay valid");
    }

    #[test]
    fn failed_items_stay_in_the_tree_and_already_gone_items_leave_it() {
        let mut f = fixture("apply-partial");
        let guardrails = open_guardrails(&f.root);
        let plan = plan_cleanup(&f.tree, &[f.a, f.c], &guardrails).unwrap();
        fs::remove_file(f.root.join("c.bin")).unwrap();
        let trash = FakeTrash {
            fail_on: Some(f.root.join("a")),
            ..FakeTrash::default()
        };
        let results = execute(&plan.items, &guardrails, &trash);
        apply_results(&mut f.tree, &results);

        assert!(f.tree.node_id(f.a).is_some());
        assert!(f.tree.node_id(f.c).is_none());
        assert_eq!(f.tree.size(NodeId::ROOT), 12 * MIB);
    }
}
