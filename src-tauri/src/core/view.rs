//! Bounded views over the arena `Tree`.
//!
//! The UI never receives the full tree. `build_view` returns a small subtree
//! for one node:
//!
//! - each expanded directory lists its `top_k` largest children (by size,
//!   ties broken by name) followed by at most one `Other` node;
//! - `Other` folds together the omitted children and the directory's
//!   aggregated small files;
//! - therefore, for every expanded directory, the sizes of its listed
//!   children sum exactly to the directory's size, and so do the file counts;
//! - the number of nodes never exceeds `max_nodes`. Expansion is breadth
//!   first, and a directory is expanded only if all of its children fit in
//!   the remaining budget. A directory left unexpanded keeps its true size
//!   and is marked `expandable` so the UI can request it separately.
//!
//! Payload bound: at most `max_nodes` nodes, each carrying one name. At the
//! default of 5,000 nodes that is a few hundred KB of JSON; the measured
//! figure is recorded in the plan.

use std::path::Path;

use serde::Serialize;

use super::config::{DEFAULT_TOP_K, DEFAULT_VIEW_DEPTH, MAX_VIEW_NODES};
use super::tree::{NodeId, NodeKind, Tree};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ViewKind {
    Dir,
    File,
    Inaccessible,
    /// Aggregate of omitted children and small files. Has no tree id.
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewNode {
    /// Raw tree index; absent for `Other`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u32>,
    /// Empty for `Other`; the UI supplies its own label.
    pub name: String,
    pub kind: ViewKind,
    pub size: u64,
    /// Files in the subtree. For `Other`: files folded into it.
    pub files: u64,
    /// A directory that has content which this view did not expand.
    pub expandable: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<ViewNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewOptions {
    pub depth: usize,
    pub top_k: usize,
    pub max_nodes: usize,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self {
            depth: DEFAULT_VIEW_DEPTH,
            top_k: DEFAULT_TOP_K,
            max_nodes: MAX_VIEW_NODES,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Crumb {
    pub id: u32,
    pub name: String,
}

struct Slot {
    tree_id: NodeId,
    depth: usize,
    node: Option<ViewNode>,
    children: Vec<usize>,
}

fn view_kind(kind: NodeKind) -> ViewKind {
    match kind {
        NodeKind::Dir => ViewKind::Dir,
        NodeKind::File => ViewKind::File,
        NodeKind::Inaccessible => ViewKind::Inaccessible,
    }
}

fn leaf(tree: &Tree, id: NodeId) -> ViewNode {
    let has_content = tree.children(id).next().is_some() || tree.small_files(id).count > 0;
    ViewNode {
        id: Some(id.raw()),
        name: tree.name(id).to_owned(),
        kind: view_kind(tree.kind(id)),
        size: tree.size(id),
        files: tree.file_count(id),
        expandable: tree.kind(id) == NodeKind::Dir && has_content,
        children: Vec::new(),
    }
}

/// Children to list and the `Other` node covering everything else.
fn plan_children(tree: &Tree, dir: NodeId, top_k: usize) -> (Vec<NodeId>, Option<ViewNode>) {
    let by_size_then_name = |a: &NodeId, b: &NodeId| {
        tree.size(*b)
            .cmp(&tree.size(*a))
            .then_with(|| tree.name(*a).cmp(tree.name(*b)))
    };
    let mut kept: Vec<NodeId> = tree.children(dir).collect();
    let omitted = if kept.len() > top_k {
        kept.select_nth_unstable_by(top_k, by_size_then_name);
        kept.split_off(top_k)
    } else {
        Vec::new()
    };
    kept.sort_unstable_by(by_size_then_name);

    let small = tree.small_files(dir);
    let size = omitted.iter().map(|&id| tree.size(id)).sum::<u64>() + small.bytes;
    let files = omitted.iter().map(|&id| tree.file_count(id)).sum::<u64>() + small.count;
    let other = (!omitted.is_empty() || small.count > 0).then_some(ViewNode {
        id: None,
        name: String::new(),
        kind: ViewKind::Other,
        size,
        files,
        expandable: false,
        children: Vec::new(),
    });
    (kept, other)
}

/// Builds the bounded view of `root`. See the module docs for the guarantees.
pub fn build_view(tree: &Tree, root: NodeId, options: &ViewOptions) -> ViewNode {
    let mut slots = vec![Slot {
        tree_id: root,
        depth: 0,
        node: Some(leaf(tree, root)),
        children: Vec::new(),
    }];

    // Slots are appended in breadth-first order, so a plain index walk over a
    // growing vector is a BFS queue.
    let mut next = 0;
    while next < slots.len() {
        let (tree_id, depth) = (slots[next].tree_id, slots[next].depth);
        let current = next;
        next += 1;
        if depth >= options.depth || tree.kind(tree_id) != NodeKind::Dir {
            continue;
        }
        let (kept, other) = plan_children(tree, tree_id, options.top_k);
        let needed = kept.len() + usize::from(other.is_some());
        if needed == 0 || slots.len() + needed > options.max_nodes {
            continue;
        }
        let mut child_slots = Vec::with_capacity(needed);
        for id in kept {
            child_slots.push(slots.len());
            slots.push(Slot {
                tree_id: id,
                depth: depth + 1,
                node: Some(leaf(tree, id)),
                children: Vec::new(),
            });
        }
        if let Some(other) = other {
            child_slots.push(slots.len());
            slots.push(Slot {
                tree_id,
                depth: usize::MAX,
                node: Some(other),
                children: Vec::new(),
            });
        }
        slots[current].children = child_slots;
        if let Some(node) = slots[current].node.as_mut() {
            node.expandable = false;
        }
    }

    // Children always sit after their parent, so a reverse pass assembles the
    // nested tree without recursion.
    for index in (0..slots.len()).rev() {
        let child_indexes = std::mem::take(&mut slots[index].children);
        let children: Vec<ViewNode> = child_indexes
            .into_iter()
            .map(|c| slots[c].node.take().expect("child assembled once"))
            .collect();
        if let Some(node) = slots[index].node.as_mut() {
            node.children = children;
        }
    }
    slots[0].node.take().expect("root present")
}

/// Root-first chain of ancestors ending at `id`.
pub fn breadcrumbs(tree: &Tree, id: NodeId) -> Vec<Crumb> {
    let mut chain = Vec::new();
    let mut current = Some(id);
    while let Some(node) = current {
        chain.push(Crumb {
            id: node.raw(),
            name: tree.name(node).to_owned(),
        });
        current = tree.parent(node);
    }
    chain.reverse();
    chain
}

/// Finds the node for an absolute path under the scanned root. Used to
/// restore the user's position after a rescan, when raw ids are no longer
/// valid. Returns `None` for paths outside the root, paths that no longer
/// exist, and files folded into a small-file aggregate (they have no node).
pub fn find_by_path(tree: &Tree, path: &Path) -> Option<NodeId> {
    let relative = path.strip_prefix(Path::new(tree.name(NodeId::ROOT))).ok()?;
    let mut current = NodeId::ROOT;
    for component in relative.components() {
        let name = component.as_os_str().to_str()?;
        current = tree.children(current).find(|&c| tree.name(c) == name)?;
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;

    /// root: a=10, b=8, c=6, d=4, e=2 (MiB files) + 2 small files (100 + 200 B)
    fn flat_tree() -> Tree {
        let mut tree = Tree::new("/root");
        for (name, mib) in [("c", 6), ("a", 10), ("e", 2), ("b", 8), ("d", 4)] {
            tree.add_file(NodeId::ROOT, name, mib * MIB);
        }
        tree.add_small_file(NodeId::ROOT, 100);
        tree.add_small_file(NodeId::ROOT, 200);
        tree.rollup();
        tree
    }

    fn options(depth: usize, top_k: usize) -> ViewOptions {
        ViewOptions {
            depth,
            top_k,
            max_nodes: usize::MAX,
        }
    }

    fn names(node: &ViewNode) -> Vec<&str> {
        node.children.iter().map(|c| c.name.as_str()).collect()
    }

    /// Every expanded node's children sum exactly to the node itself.
    fn assert_sums_hold(node: &ViewNode) {
        if !node.children.is_empty() {
            assert_eq!(node.children.iter().map(|c| c.size).sum::<u64>(), node.size);
            assert_eq!(
                node.children.iter().map(|c| c.files).sum::<u64>(),
                node.files
            );
            for child in &node.children {
                assert_sums_hold(child);
            }
        }
    }

    fn count(node: &ViewNode) -> usize {
        1 + node.children.iter().map(count).sum::<usize>()
    }

    #[test]
    fn keeps_top_k_largest_sorted_then_other() {
        let tree = flat_tree();
        let view = build_view(&tree, NodeId::ROOT, &options(1, 3));
        assert_eq!(names(&view), ["a", "b", "c", ""]);
        let other = view.children.last().unwrap();
        assert_eq!(other.kind, ViewKind::Other);
        assert_eq!(other.id, None);
        assert_eq!(other.size, 6 * MIB + 300);
        assert_eq!(other.files, 4);
        assert_sums_hold(&view);
        assert_eq!(view.size, tree.size(NodeId::ROOT));
    }

    #[test]
    fn small_files_alone_produce_other() {
        let tree = flat_tree();
        let view = build_view(&tree, NodeId::ROOT, &options(1, 10));
        assert_eq!(names(&view), ["a", "b", "c", "d", "e", ""]);
        let other = view.children.last().unwrap();
        assert_eq!((other.size, other.files), (300, 2));
        assert_sums_hold(&view);
    }

    #[test]
    fn no_other_when_nothing_is_omitted() {
        let mut tree = Tree::new("/root");
        tree.add_file(NodeId::ROOT, "x", MIB);
        tree.rollup();
        let view = build_view(&tree, NodeId::ROOT, &options(1, 5));
        assert_eq!(names(&view), ["x"]);
    }

    #[test]
    fn top_k_zero_collapses_everything_into_other() {
        let tree = flat_tree();
        let view = build_view(&tree, NodeId::ROOT, &options(1, 0));
        assert_eq!(view.children.len(), 1);
        assert_eq!(view.children[0].kind, ViewKind::Other);
        assert_sums_hold(&view);
    }

    #[test]
    fn equal_sizes_are_ordered_by_name() {
        let mut tree = Tree::new("/root");
        for name in ["z", "m", "a"] {
            tree.add_file(NodeId::ROOT, name, MIB);
        }
        tree.rollup();
        let view = build_view(&tree, NodeId::ROOT, &options(1, 2));
        assert_eq!(names(&view), ["a", "m", ""]);
    }

    /// root/{big/{inner/{leaf}}, file}
    fn nested_tree() -> Tree {
        let mut tree = Tree::new("/root");
        let big = tree.add_dir(NodeId::ROOT, "big");
        let inner = tree.add_dir(big, "inner");
        tree.add_file(inner, "leaf", 5 * MIB);
        tree.add_file(NodeId::ROOT, "file", MIB);
        tree.rollup();
        tree
    }

    #[test]
    fn depth_limit_keeps_true_sizes_and_marks_expandable() {
        let tree = nested_tree();
        let view = build_view(&tree, NodeId::ROOT, &options(1, 10));
        let big = &view.children[0];
        assert_eq!(big.name, "big");
        assert_eq!(big.size, 5 * MIB);
        assert!(big.children.is_empty());
        assert!(big.expandable);
        assert!(!view.expandable);
        assert_sums_hold(&view);
    }

    #[test]
    fn depth_zero_returns_only_the_node() {
        let tree = nested_tree();
        let view = build_view(&tree, NodeId::ROOT, &options(0, 10));
        assert!(view.children.is_empty());
        assert!(view.expandable);
        assert_eq!(view.size, 6 * MIB);
    }

    #[test]
    fn deeper_depth_expands_further() {
        let tree = nested_tree();
        let view = build_view(&tree, NodeId::ROOT, &options(3, 10));
        let inner = &view.children[0].children[0];
        assert_eq!(inner.name, "inner");
        assert_eq!(inner.children[0].name, "leaf");
        assert_sums_hold(&view);
    }

    #[test]
    fn viewing_a_subtree_starts_at_that_node() {
        let tree = nested_tree();
        let big = tree
            .children(NodeId::ROOT)
            .find(|&c| tree.name(c) == "big")
            .unwrap();
        let view = build_view(&tree, big, &options(1, 10));
        assert_eq!(view.name, "big");
        assert_eq!(names(&view), ["inner"]);
    }

    #[test]
    fn file_and_empty_dir_are_not_expandable() {
        let mut tree = Tree::new("/root");
        tree.add_dir(NodeId::ROOT, "empty");
        tree.add_file(NodeId::ROOT, "f", MIB);
        tree.rollup();
        let view = build_view(&tree, NodeId::ROOT, &options(1, 10));
        assert!(view.children.iter().all(|c| !c.expandable));
        let file = tree
            .children(NodeId::ROOT)
            .find(|&c| tree.name(c) == "f")
            .unwrap();
        assert!(build_view(&tree, file, &options(3, 10)).children.is_empty());
    }

    #[test]
    fn directory_with_only_small_files_is_expandable_and_shows_other() {
        let mut tree = Tree::new("/root");
        let dir = tree.add_dir(NodeId::ROOT, "only-small");
        tree.add_small_file(dir, 10);
        tree.rollup();
        let view = build_view(&tree, NodeId::ROOT, &options(1, 10));
        assert!(view.children[0].expandable);
        let drill = build_view(&tree, dir, &options(1, 10));
        assert_eq!(drill.children.len(), 1);
        assert_eq!(drill.children[0].kind, ViewKind::Other);
    }

    #[test]
    fn inaccessible_nodes_are_listed_with_zero_size() {
        let mut tree = Tree::new("/root");
        tree.add_inaccessible(NodeId::ROOT, "denied");
        tree.add_file(NodeId::ROOT, "f", MIB);
        tree.rollup();
        let view = build_view(&tree, NodeId::ROOT, &options(1, 10));
        assert_eq!(names(&view), ["f", "denied"]);
        assert_eq!(view.children[1].kind, ViewKind::Inaccessible);
        assert_sums_hold(&view);
    }

    /// Deterministic pseudo-random tree: ~`dirs` directories with varied
    /// fan-out, file sizes and small-file counts.
    fn random_tree(dirs: usize, seed: u64) -> Tree {
        let mut state = seed;
        let mut next = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            state >> 33
        };
        let mut tree = Tree::new("/root");
        let mut dir_ids = vec![NodeId::ROOT];
        for i in 0..dirs {
            let parent = dir_ids[(next() as usize) % dir_ids.len()];
            dir_ids.push(tree.add_dir(parent, &format!("d{i}")));
        }
        for (i, &dir) in dir_ids.iter().enumerate() {
            for j in 0..(next() % 40) {
                tree.add_file(dir, &format!("f{i}-{j}"), next() % (50 * MIB));
            }
            for _ in 0..(next() % 5) {
                tree.add_small_file(dir, next() % 4096);
            }
        }
        tree.rollup();
        tree
    }

    #[test]
    fn sums_hold_across_random_trees_and_options() {
        for seed in 1..=5 {
            let tree = random_tree(300, seed);
            for (depth, top_k) in [(1, 1), (2, 3), (3, 8), (6, 64), (4, 0)] {
                let view = build_view(&tree, NodeId::ROOT, &options(depth, top_k));
                assert_eq!(view.size, tree.size(NodeId::ROOT));
                assert_eq!(view.files, tree.file_count(NodeId::ROOT));
                assert_sums_hold(&view);
            }
        }
    }

    #[test]
    fn node_budget_is_respected_and_sums_still_hold() {
        let tree = random_tree(500, 9);
        for max_nodes in [1, 2, 10, 50, 200] {
            let view = build_view(
                &tree,
                NodeId::ROOT,
                &ViewOptions {
                    depth: 6,
                    top_k: 20,
                    max_nodes,
                },
            );
            assert!(count(&view) <= max_nodes, "budget {max_nodes}");
            assert_eq!(view.size, tree.size(NodeId::ROOT));
            assert_sums_hold(&view);
        }
    }

    #[test]
    fn unexpanded_directories_report_expandable_when_budget_runs_out() {
        let tree = nested_tree();
        let view = build_view(
            &tree,
            NodeId::ROOT,
            &ViewOptions {
                depth: 3,
                top_k: 10,
                max_nodes: 3,
            },
        );
        // Root + its two children fit; "big" could not be expanded further.
        assert_eq!(count(&view), 3);
        let big = view.children.iter().find(|c| c.name == "big").unwrap();
        assert!(big.expandable && big.children.is_empty());
    }

    #[test]
    fn breadcrumbs_run_root_first() {
        let tree = nested_tree();
        let big = tree
            .children(NodeId::ROOT)
            .find(|&c| tree.name(c) == "big")
            .unwrap();
        let inner = tree.children(big).next().unwrap();
        let names: Vec<_> = breadcrumbs(&tree, inner)
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, ["/root", "big", "inner"]);
        assert_eq!(breadcrumbs(&tree, NodeId::ROOT).len(), 1);
    }

    #[test]
    fn find_by_path_resolves_nested_nodes() {
        let tree = nested_tree();
        assert_eq!(find_by_path(&tree, Path::new("/root")), Some(NodeId::ROOT));
        let inner = find_by_path(&tree, Path::new("/root/big/inner")).unwrap();
        assert_eq!(tree.name(inner), "inner");
        let leaf = find_by_path(&tree, Path::new("/root/big/inner/leaf")).unwrap();
        assert_eq!(tree.size(leaf), 5 * MIB);
    }

    #[test]
    fn find_by_path_rejects_unknown_paths() {
        let tree = nested_tree();
        assert_eq!(find_by_path(&tree, Path::new("/root/nope")), None);
        assert_eq!(find_by_path(&tree, Path::new("/elsewhere/big")), None);
        assert_eq!(find_by_path(&tree, Path::new("/rootbeer")), None);
    }

    #[test]
    fn find_by_path_skips_aggregated_small_files() {
        let mut tree = Tree::new("/root");
        tree.add_file_or_aggregate(NodeId::ROOT, "tiny", 10, 1000);
        tree.rollup();
        assert_eq!(find_by_path(&tree, Path::new("/root/tiny")), None);
    }

    #[test]
    fn find_by_path_works_for_filesystem_root() {
        let mut tree = Tree::new("/");
        let usr = tree.add_dir(NodeId::ROOT, "usr");
        tree.rollup();
        assert_eq!(find_by_path(&tree, Path::new("/usr")), Some(usr));
    }

    #[test]
    fn node_id_validates_raw_indexes() {
        let tree = nested_tree();
        assert_eq!(tree.node_id(0), Some(NodeId::ROOT));
        assert!(tree.node_id(u32::try_from(tree.len()).unwrap()).is_none());
        assert!(tree.node_id(u32::MAX).is_none());
    }

    #[test]
    fn serialization_is_compact_and_camel_case() {
        let tree = flat_tree();
        let view = build_view(&tree, NodeId::ROOT, &options(1, 1));
        let json = serde_json::to_value(&view).unwrap();
        let first = &json["children"][0];
        assert_eq!(first["kind"], "file");
        assert!(first.get("children").is_none());
        assert!(first.get("id").is_some());
        let other = &json["children"][1];
        assert_eq!(other["kind"], "other");
        assert!(other.get("id").is_none());
    }
}
