//! Arena-based file tree.
//!
//! All nodes live in one `Vec` and refer to each other by `NodeId` (a `u32`
//! index). Names are stored back-to-back in a single string buffer. Children
//! are kept as a singly linked list (`first_child` / `next_sibling`) so nodes
//! can be inserted in any order while a scan is running; new children are
//! prepended, so child order is the reverse of insertion order.
//!
//! A child is always inserted after its parent, so every node's id is greater
//! than its parent's id. `rollup` relies on this to aggregate bottom-up with a
//! single reverse pass and no recursion.
//!
//! Files below the scan threshold get no node. They are counted in the
//! parent directory's `SmallFileAggregate` instead.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(u32);

impl NodeId {
    pub const ROOT: NodeId = NodeId(0);

    fn index(self) -> usize {
        self.0 as usize
    }

    /// Raw index for crossing the IPC boundary. Only meaningful for the tree
    /// that produced it; convert back with `Tree::node_id`.
    pub fn raw(self) -> u32 {
        self.0
    }
}

const NONE: u32 = u32::MAX;

fn link(raw: u32) -> Option<NodeId> {
    (raw != NONE).then_some(NodeId(raw))
}

fn to_u32(value: usize) -> u32 {
    u32::try_from(value).expect("tree exceeds u32 capacity")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Dir,
    File,
    /// A directory that could not be read. Has no children and size 0.
    Inaccessible,
}

/// Count and total bytes of files folded into a directory.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SmallFileAggregate {
    pub count: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone)]
struct Node {
    parent: u32,
    first_child: u32,
    next_sibling: u32,
    name_offset: u32,
    name_len: u32,
    /// Index into `Tree::aggregates`; `NONE` for non-directories.
    aggregate: u32,
    kind: NodeKind,
    /// Set on a node and its whole subtree by `detach`. Such nodes keep their
    /// slot (so other ids stay valid) but are no longer part of the tree.
    removed: bool,
    /// File: its own bytes. Directory: subtree total after `rollup`.
    size: u64,
    /// Number of files in the subtree, including aggregated small files.
    files: u64,
}

#[derive(Debug)]
pub struct Tree {
    nodes: Vec<Node>,
    names: String,
    aggregates: Vec<SmallFileAggregate>,
}

impl Tree {
    /// Creates a tree whose root (`NodeId::ROOT`) is a directory.
    pub fn new(root_name: &str) -> Self {
        let mut tree = Tree {
            nodes: Vec::new(),
            names: String::new(),
            aggregates: Vec::new(),
        };
        tree.push(NONE, root_name, NodeKind::Dir, 0);
        tree
    }

    /// Validates a raw index received from outside. `None` if out of range or
    /// if the node was removed by `detach`.
    pub fn node_id(&self, raw: u32) -> Option<NodeId> {
        self.nodes
            .get(raw as usize)
            .filter(|node| !node.removed)
            .map(|_| NodeId(raw))
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn add_dir(&mut self, parent: NodeId, name: &str) -> NodeId {
        self.add(parent, name, NodeKind::Dir, 0)
    }

    pub fn add_file(&mut self, parent: NodeId, name: &str, size: u64) -> NodeId {
        self.add(parent, name, NodeKind::File, size)
    }

    pub fn add_inaccessible(&mut self, parent: NodeId, name: &str) -> NodeId {
        self.add(parent, name, NodeKind::Inaccessible, 0)
    }

    /// Reclassifies an empty directory that turned out to be unreadable.
    pub fn mark_inaccessible(&mut self, id: NodeId) {
        let node = &mut self.nodes[id.index()];
        assert!(
            node.kind == NodeKind::Dir && node.first_child == NONE,
            "only an empty directory can be marked inaccessible"
        );
        node.kind = NodeKind::Inaccessible;
    }

    /// Adds a file node when `size >= threshold`, otherwise folds it into the
    /// parent's aggregate and returns `None`.
    pub fn add_file_or_aggregate(
        &mut self,
        parent: NodeId,
        name: &str,
        size: u64,
        threshold: u64,
    ) -> Option<NodeId> {
        if size >= threshold {
            Some(self.add_file(parent, name, size))
        } else {
            self.add_small_file(parent, size);
            None
        }
    }

    pub fn add_small_file(&mut self, parent: NodeId, size: u64) {
        let aggregate = self.aggregate_index(parent);
        let slot = &mut self.aggregates[aggregate];
        slot.count += 1;
        slot.bytes += size;
    }

    /// Recomputes `size` and `files` for every directory from its aggregate
    /// and children. Safe to call repeatedly.
    pub fn rollup(&mut self) {
        for node in &mut self.nodes {
            match node.kind {
                NodeKind::Dir => {
                    let aggregate = self.aggregates[node.aggregate as usize];
                    node.size = aggregate.bytes;
                    node.files = aggregate.count;
                }
                NodeKind::File => node.files = 1,
                NodeKind::Inaccessible => {
                    node.size = 0;
                    node.files = 0;
                }
            }
        }
        for index in (1..self.nodes.len()).rev() {
            let (parent, size, files) = {
                let node = &self.nodes[index];
                if node.removed {
                    continue;
                }
                (node.parent as usize, node.size, node.files)
            };
            let parent = &mut self.nodes[parent];
            parent.size += size;
            parent.files += files;
        }
    }

    /// Removes `id` and everything below it from the tree, then subtracts its
    /// size and file count from every ancestor. Runs in O(siblings + depth +
    /// subtree size) and leaves all other ids valid, so the result matches a
    /// fresh `rollup` without walking the whole tree.
    ///
    /// Returns the removed `(size, files)`, or `None` for the root and for a
    /// node that is already removed.
    pub fn detach(&mut self, id: NodeId) -> Option<(u64, u64)> {
        let node = &self.nodes[id.index()];
        if id == NodeId::ROOT || node.removed {
            return None;
        }
        let (parent, next, size, files) = (node.parent, node.next_sibling, node.size, node.files);

        let parent_node = &mut self.nodes[parent as usize];
        if parent_node.first_child == id.0 {
            parent_node.first_child = next;
        } else {
            let mut cursor = parent_node.first_child;
            while cursor != NONE {
                let sibling = &mut self.nodes[cursor as usize];
                if sibling.next_sibling == id.0 {
                    sibling.next_sibling = next;
                    break;
                }
                cursor = sibling.next_sibling;
            }
        }
        self.nodes[id.index()].next_sibling = NONE;

        let mut ancestor = parent;
        while ancestor != NONE {
            let node = &mut self.nodes[ancestor as usize];
            node.size = node.size.saturating_sub(size);
            node.files = node.files.saturating_sub(files);
            ancestor = node.parent;
        }

        let mut stack = vec![id];
        while let Some(current) = stack.pop() {
            self.nodes[current.index()].removed = true;
            let mut child = self.nodes[current.index()].first_child;
            while child != NONE {
                stack.push(NodeId(child));
                child = self.nodes[child as usize].next_sibling;
            }
        }
        Some((size, files))
    }

    pub fn name(&self, id: NodeId) -> &str {
        let node = &self.nodes[id.index()];
        let start = node.name_offset as usize;
        &self.names[start..start + node.name_len as usize]
    }

    pub fn kind(&self, id: NodeId) -> NodeKind {
        self.nodes[id.index()].kind
    }

    /// Own bytes for files; subtree total for directories (after `rollup`).
    pub fn size(&self, id: NodeId) -> u64 {
        self.nodes[id.index()].size
    }

    /// Files in the subtree, including aggregated small files (after `rollup`).
    pub fn file_count(&self, id: NodeId) -> u64 {
        self.nodes[id.index()].files
    }

    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        link(self.nodes[id.index()].parent)
    }

    /// Small files folded directly into this directory. Zero for non-directories.
    pub fn small_files(&self, id: NodeId) -> SmallFileAggregate {
        match self.nodes[id.index()].aggregate {
            NONE => SmallFileAggregate::default(),
            index => self.aggregates[index as usize],
        }
    }

    pub fn children(&self, id: NodeId) -> Children<'_> {
        Children {
            tree: self,
            next: link(self.nodes[id.index()].first_child),
        }
    }

    fn add(&mut self, parent: NodeId, name: &str, kind: NodeKind, size: u64) -> NodeId {
        assert_eq!(
            self.nodes[parent.index()].kind,
            NodeKind::Dir,
            "parent must be a directory"
        );
        let id = self.push(parent.0, name, kind, size);
        let parent_node = &mut self.nodes[parent.index()];
        let previous_first = parent_node.first_child;
        parent_node.first_child = id.0;
        self.nodes[id.index()].next_sibling = previous_first;
        id
    }

    fn push(&mut self, parent: u32, name: &str, kind: NodeKind, size: u64) -> NodeId {
        let id = NodeId(to_u32(self.nodes.len()));
        let aggregate = if kind == NodeKind::Dir {
            self.aggregates.push(SmallFileAggregate::default());
            to_u32(self.aggregates.len() - 1)
        } else {
            NONE
        };
        let node = Node {
            parent,
            first_child: NONE,
            next_sibling: NONE,
            name_offset: to_u32(self.names.len()),
            name_len: to_u32(name.len()),
            aggregate,
            kind,
            removed: false,
            size,
            files: 0,
        };
        self.names.push_str(name);
        self.nodes.push(node);
        id
    }

    fn aggregate_index(&self, dir: NodeId) -> usize {
        let node = &self.nodes[dir.index()];
        assert_eq!(node.kind, NodeKind::Dir, "parent must be a directory");
        node.aggregate as usize
    }
}

pub struct Children<'a> {
    tree: &'a Tree,
    next: Option<NodeId>,
}

impl Iterator for Children<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        let current = self.next?;
        self.next = link(self.tree.nodes[current.index()].next_sibling);
        Some(current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;

    /// root
    /// ├── a/        (big.bin 10 MiB, 2 small files: 100 + 200 bytes)
    /// │   └── b/    (mid.bin 2 MiB)
    /// ├── c/        (empty)
    /// ├── denied/   (inaccessible)
    /// └── root.bin  (3 MiB)
    fn sample() -> (Tree, NodeId, NodeId, NodeId) {
        let mut tree = Tree::new("root");
        let a = tree.add_dir(NodeId::ROOT, "a");
        let b = tree.add_dir(a, "b");
        let c = tree.add_dir(NodeId::ROOT, "c");
        tree.add_inaccessible(NodeId::ROOT, "denied");
        tree.add_file(NodeId::ROOT, "root.bin", 3 * MIB);
        tree.add_file(a, "big.bin", 10 * MIB);
        tree.add_small_file(a, 100);
        tree.add_small_file(a, 200);
        tree.add_file(b, "mid.bin", 2 * MIB);
        tree.rollup();
        (tree, a, b, c)
    }

    #[test]
    fn rollup_sums_sizes_bottom_up() {
        let (tree, a, b, c) = sample();
        assert_eq!(tree.size(b), 2 * MIB);
        assert_eq!(tree.size(a), 12 * MIB + 300);
        assert_eq!(tree.size(c), 0);
        assert_eq!(tree.size(NodeId::ROOT), 15 * MIB + 300);
    }

    #[test]
    fn rollup_counts_files_including_aggregated() {
        let (tree, a, b, c) = sample();
        assert_eq!(tree.file_count(b), 1);
        assert_eq!(tree.file_count(a), 4);
        assert_eq!(tree.file_count(c), 0);
        assert_eq!(tree.file_count(NodeId::ROOT), 5);
    }

    #[test]
    fn rollup_is_idempotent() {
        let (mut tree, ..) = sample();
        let before = (tree.size(NodeId::ROOT), tree.file_count(NodeId::ROOT));
        tree.rollup();
        assert_eq!(
            before,
            (tree.size(NodeId::ROOT), tree.file_count(NodeId::ROOT))
        );
    }

    #[test]
    fn rollup_reflects_later_additions() {
        let (mut tree, _, b, _) = sample();
        tree.add_file(b, "late.bin", MIB);
        tree.add_small_file(b, 5);
        tree.rollup();
        assert_eq!(tree.size(NodeId::ROOT), 16 * MIB + 305);
        assert_eq!(tree.file_count(NodeId::ROOT), 7);
    }

    #[test]
    fn inaccessible_dir_has_zero_size_and_no_files() {
        let (tree, ..) = sample();
        let denied = tree
            .children(NodeId::ROOT)
            .find(|&id| tree.kind(id) == NodeKind::Inaccessible)
            .unwrap();
        assert_eq!(tree.name(denied), "denied");
        assert_eq!(tree.size(denied), 0);
        assert_eq!(tree.file_count(denied), 0);
    }

    #[test]
    fn threshold_splits_nodes_from_aggregate() {
        let mut tree = Tree::new("root");
        let threshold = 1000;
        assert!(
            tree.add_file_or_aggregate(NodeId::ROOT, "small", 999, threshold)
                .is_none()
        );
        let kept = tree.add_file_or_aggregate(NodeId::ROOT, "exact", 1000, threshold);
        assert!(kept.is_some());
        tree.rollup();
        assert_eq!(tree.len(), 2);
        assert_eq!(
            tree.small_files(NodeId::ROOT),
            SmallFileAggregate {
                count: 1,
                bytes: 999
            }
        );
        assert_eq!(tree.size(NodeId::ROOT), 1999);
        assert_eq!(tree.file_count(NodeId::ROOT), 2);
    }

    #[test]
    fn names_and_parents_round_trip() {
        let (tree, a, b, _) = sample();
        assert_eq!(tree.name(NodeId::ROOT), "root");
        assert_eq!(tree.name(a), "a");
        assert_eq!(tree.name(b), "b");
        assert_eq!(tree.parent(b), Some(a));
        assert_eq!(tree.parent(a), Some(NodeId::ROOT));
        assert_eq!(tree.parent(NodeId::ROOT), None);
    }

    #[test]
    fn children_lists_only_direct_children() {
        let (tree, a, ..) = sample();
        let mut names: Vec<_> = tree
            .children(NodeId::ROOT)
            .map(|id| tree.name(id))
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["a", "c", "denied", "root.bin"]);
        let mut a_names: Vec<_> = tree.children(a).map(|id| tree.name(id)).collect();
        a_names.sort_unstable();
        assert_eq!(a_names, ["b", "big.bin"]);
    }

    #[test]
    fn non_ascii_names_are_preserved() {
        let mut tree = Tree::new("/");
        let id = tree.add_file(NodeId::ROOT, "résumé 日本語.txt", 1);
        assert_eq!(tree.name(id), "résumé 日本語.txt");
    }

    #[test]
    fn deep_chain_rolls_up_without_recursion() {
        let mut tree = Tree::new("root");
        let mut current = NodeId::ROOT;
        for _ in 0..100_000 {
            current = tree.add_dir(current, "d");
        }
        tree.add_file(current, "leaf", 7);
        tree.rollup();
        assert_eq!(tree.size(NodeId::ROOT), 7);
        assert_eq!(tree.file_count(NodeId::ROOT), 1);
    }

    /// Sizes and counts of every live node, in id order.
    fn totals(tree: &Tree) -> Vec<(u64, u64)> {
        (0..tree.len() as u32)
            .filter_map(|raw| tree.node_id(raw))
            .map(|id| (tree.size(id), tree.file_count(id)))
            .collect()
    }

    #[test]
    fn detach_matches_a_fresh_rollup() {
        let (mut tree, a, b, _) = sample();
        assert_eq!(tree.detach(b), Some((2 * MIB, 1)));
        let incremental = totals(&tree);
        tree.rollup();
        assert_eq!(incremental, totals(&tree));
        assert_eq!(tree.size(a), 10 * MIB + 300);
        assert_eq!(tree.size(NodeId::ROOT), 13 * MIB + 300);
        assert_eq!(tree.file_count(NodeId::ROOT), 4);
    }

    #[test]
    fn detach_unlinks_first_middle_and_last_children() {
        for victim in ["first", "second", "third"] {
            let mut tree = Tree::new("root");
            for name in ["first", "second", "third"] {
                tree.add_file(NodeId::ROOT, name, MIB);
            }
            tree.rollup();
            let id = tree
                .children(NodeId::ROOT)
                .find(|&c| tree.name(c) == victim)
                .unwrap();
            tree.detach(id).unwrap();

            let mut left: Vec<_> = tree.children(NodeId::ROOT).map(|c| tree.name(c)).collect();
            left.sort_unstable();
            let mut expected = vec!["first", "second", "third"];
            expected.retain(|n| *n != victim);
            assert_eq!(left, expected);
            assert_eq!(tree.size(NodeId::ROOT), 2 * MIB);
        }
    }

    #[test]
    fn detach_removes_the_whole_subtree_but_keeps_other_ids_valid() {
        let (mut tree, a, b, c) = sample();
        let big = tree
            .children(a)
            .find(|&x| tree.name(x) == "big.bin")
            .unwrap();
        let mid = tree.children(b).next().unwrap();
        tree.detach(a).unwrap();

        for removed in [a, b, big, mid] {
            assert!(tree.node_id(removed.raw()).is_none());
        }
        assert_eq!(tree.node_id(c.raw()), Some(c));
        assert_eq!(tree.name(c), "c");
        assert_eq!(tree.size(NodeId::ROOT), 3 * MIB);
        assert_eq!(tree.file_count(NodeId::ROOT), 1);
    }

    #[test]
    fn rollup_after_detach_does_not_bring_removed_nodes_back() {
        let (mut tree, a, ..) = sample();
        tree.detach(a).unwrap();
        tree.rollup();
        tree.rollup();
        assert_eq!(tree.size(NodeId::ROOT), 3 * MIB);
        assert_eq!(tree.file_count(NodeId::ROOT), 1);
    }

    #[test]
    fn detach_refuses_the_root_and_repeats() {
        let (mut tree, a, ..) = sample();
        assert_eq!(tree.detach(NodeId::ROOT), None);
        assert!(tree.detach(a).is_some());
        assert_eq!(tree.detach(a), None);
        assert_eq!(tree.size(NodeId::ROOT), 3 * MIB);
    }

    #[test]
    fn detach_in_a_deep_chain_updates_every_ancestor() {
        let mut tree = Tree::new("root");
        let mut current = NodeId::ROOT;
        let mut middle = current;
        for depth in 0..10_000 {
            current = tree.add_dir(current, "d");
            if depth == 5_000 {
                middle = current;
            }
        }
        tree.add_file(current, "leaf", 7);
        tree.rollup();
        assert_eq!(tree.detach(middle), Some((7, 1)));
        assert_eq!(tree.size(NodeId::ROOT), 0);
        assert_eq!(tree.file_count(NodeId::ROOT), 0);
    }

    #[test]
    #[should_panic(expected = "parent must be a directory")]
    fn adding_child_to_file_panics() {
        let mut tree = Tree::new("root");
        let file = tree.add_file(NodeId::ROOT, "f", 1);
        tree.add_file(file, "g", 1);
    }

    #[test]
    #[should_panic(expected = "parent must be a directory")]
    fn aggregating_into_file_panics() {
        let mut tree = Tree::new("root");
        let file = tree.add_file(NodeId::ROOT, "f", 1);
        tree.add_small_file(file, 1);
    }
}
