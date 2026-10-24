//! Turns discovered candidates into a dry-run plan for the Trash.
//!
//! The UI only names candidate ids. Each one is looked up in what discovery
//! found and then re-checked against the disk, because time has passed since
//! it was found: the path must resolve to the same place, still be the same
//! kind of item, be unchanged since discovery, and pass the guardrails.
//! Anything that fails is listed as excluded with a reason, never moved.
//!
//! The plan reuses `PlanItem` and `execute` from `cleanup`. In this flow
//! `PlanItem::node` holds the candidate id, not a tree node id.

use std::collections::HashSet;
use std::fs;
use std::io::ErrorKind;
use std::time::UNIX_EPOCH;

use super::cleanup::{
    CleanupPlan, ExcludedItem, Guardrails, ItemKind, ItemResult, Outcome, PlanItem, Reason,
    ReasonKind, Verdict, execute, resolve,
};
use super::cleanup_discovery::Candidate;
use crate::platform::TrashProvider;

#[derive(Debug, PartialEq, Eq)]
pub struct UnknownCandidate(pub u32);

fn excluded(candidate: &Candidate, reason: Reason) -> ExcludedItem {
    ExcludedItem {
        node: candidate.id,
        path: candidate.path.clone(),
        name: candidate.name.clone(),
        reason,
    }
}

fn changed(message: &str) -> Reason {
    Reason::new(ReasonKind::Changed, message)
}

/// Checks one candidate against the disk. `Ok` carries the warning, if any.
fn recheck(candidate: &Candidate, guardrails: &Guardrails) -> Result<Option<Reason>, Reason> {
    let lookup = |e: std::io::Error| {
        if e.kind() == ErrorKind::NotFound {
            Reason::new(
                ReasonKind::Missing,
                "No longer exists on disk. Find items again.",
            )
        } else {
            Reason::new(ReasonKind::Inaccessible, e.to_string())
        }
    };
    let resolved = resolve(&candidate.path).map_err(lookup)?;
    if resolved != candidate.path {
        return Err(changed(
            "Its location changed since it was found. Find items again.",
        ));
    }
    let meta = fs::symlink_metadata(&resolved).map_err(lookup)?;
    if meta.file_type().is_symlink() || meta.is_dir() != (candidate.kind == ItemKind::Dir) {
        return Err(changed(
            "It changed type since it was found. Find items again.",
        ));
    }
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|age| age.as_secs());
    if modified != Some(candidate.modified_secs) {
        return Err(changed(
            "It was modified since it was found, so it may be in use. Find items again.",
        ));
    }
    match guardrails.verdict(&resolved) {
        Verdict::Blocked(reason) => Err(reason),
        Verdict::Warn(reason) => Ok(Some(reason)),
        Verdict::Allowed => Ok(None),
    }
}

/// Builds the dry-run result for the chosen candidate ids. Performs no
/// writes. Fails only on an id that discovery did not produce.
pub fn plan_candidates(
    candidates: &[Candidate],
    ids: &[u32],
    guardrails: &Guardrails,
) -> Result<CleanupPlan, UnknownCandidate> {
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    let mut left_out = Vec::new();

    for &id in ids {
        let candidate = candidates
            .iter()
            .find(|c| c.id == id)
            .ok_or(UnknownCandidate(id))?;
        if !seen.insert(id) {
            continue;
        }
        match recheck(candidate, guardrails) {
            Ok(warning) => items.push(PlanItem {
                node: candidate.id,
                path: candidate.path.clone(),
                name: candidate.name.clone(),
                kind: candidate.kind,
                size: candidate.size,
                files: candidate.files,
                warning,
            }),
            Err(reason) => left_out.push(excluded(candidate, reason)),
        }
    }

    items.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
    let total_bytes = items.iter().map(|item| item.size).sum();
    let total_files = items.iter().map(|item| item.files).sum();
    Ok(CleanupPlan {
        items,
        excluded: left_out,
        merged: 0,
        total_bytes,
        total_files,
    })
}

/// Moves the planned items to the Trash. Each item is checked against the
/// disk again first, so something that changed after the preview is reported
/// and left alone. A failure on one item does not stop the others.
pub fn execute_candidates(
    candidates: &[Candidate],
    items: &[PlanItem],
    guardrails: &Guardrails,
    provider: &dyn TrashProvider,
) -> Vec<ItemResult> {
    items
        .iter()
        .map(|item| {
            let checked = candidates.iter().find(|c| c.id == item.node).map_or_else(
                || Err(changed("It is no longer in the list. Find items again.")),
                |candidate| recheck(candidate, guardrails),
            );
            match checked {
                Ok(_) => execute(std::slice::from_ref(item), guardrails, provider).remove(0),
                Err(reason) => {
                    let outcome = if reason.kind == ReasonKind::Missing {
                        Outcome::AlreadyGone
                    } else {
                        Outcome::Failed
                    };
                    ItemResult {
                        node: item.node,
                        path: item.path.clone(),
                        name: item.name.clone(),
                        size: item.size,
                        files: item.files,
                        outcome,
                        message: (outcome == Outcome::Failed).then_some(reason.message),
                    }
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::cleanup::Protected;
    use crate::core::cleanup_catalog::Category;
    use std::path::{Path, PathBuf};

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ctd-cplan-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn open(root: &Path) -> Guardrails {
        Guardrails::new(Some(root.to_path_buf()), Vec::new())
    }

    /// A candidate that matches what is on disk right now.
    fn candidate(id: u32, path: PathBuf, size: u64) -> Candidate {
        let meta = fs::symlink_metadata(&path).unwrap();
        Candidate {
            id,
            category: Category::AppCaches,
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            kind: if meta.is_dir() {
                ItemKind::Dir
            } else {
                ItemKind::File
            },
            size,
            files: 1,
            modified_secs: meta
                .modified()
                .unwrap()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            incomplete: false,
            path,
        }
    }

    fn world(name: &str) -> (PathBuf, Vec<Candidate>) {
        let root = temp_root(name);
        fs::create_dir_all(root.join("big")).unwrap();
        fs::write(root.join("big/x"), b"x").unwrap();
        fs::write(root.join("small.txt"), b"x").unwrap();
        let candidates = vec![
            candidate(1, root.join("big"), 9000),
            candidate(2, root.join("small.txt"), 10),
        ];
        (root, candidates)
    }

    fn reason_of(plan: &CleanupPlan, id: u32) -> Option<ReasonKind> {
        plan.excluded
            .iter()
            .find(|e| e.node == id)
            .map(|e| e.reason.kind)
    }

    #[test]
    fn chosen_candidates_become_items_largest_first_with_totals() {
        let (root, candidates) = world("ok");
        let plan = plan_candidates(&candidates, &[2, 1], &open(&root)).unwrap();
        let ids: Vec<_> = plan.items.iter().map(|i| i.node).collect();
        assert_eq!(ids, [1, 2]);
        assert_eq!((plan.total_bytes, plan.total_files), (9010, 2));
        assert!(plan.excluded.is_empty() && !plan.needs_acknowledgement());
    }

    #[test]
    fn only_the_chosen_ids_are_planned() {
        let (root, candidates) = world("subset");
        let plan = plan_candidates(&candidates, &[2], &open(&root)).unwrap();
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.items[0].node, 2);
    }

    #[test]
    fn an_unknown_id_is_an_error_and_a_repeated_id_counts_once() {
        let (root, candidates) = world("ids");
        assert_eq!(
            plan_candidates(&candidates, &[1, 99], &open(&root)).unwrap_err(),
            UnknownCandidate(99)
        );
        let plan = plan_candidates(&candidates, &[1, 1, 1], &open(&root)).unwrap();
        assert_eq!(plan.items.len(), 1);
    }

    #[test]
    fn planning_changes_nothing_on_disk() {
        let (root, candidates) = world("dry");
        plan_candidates(&candidates, &[1, 2], &open(&root)).unwrap();
        assert!(root.join("big/x").exists() && root.join("small.txt").exists());
    }

    #[test]
    fn an_item_that_vanished_is_excluded_as_missing() {
        let (root, candidates) = world("gone");
        fs::remove_file(root.join("small.txt")).unwrap();
        let plan = plan_candidates(&candidates, &[1, 2], &open(&root)).unwrap();
        assert_eq!(reason_of(&plan, 2), Some(ReasonKind::Missing));
        assert_eq!(plan.items.len(), 1);
    }

    #[test]
    fn an_item_modified_since_discovery_is_excluded_as_changed() {
        let (root, mut candidates) = world("modified");
        candidates[1].modified_secs -= 1000;
        let plan = plan_candidates(&candidates, &[1, 2], &open(&root)).unwrap();
        assert_eq!(reason_of(&plan, 2), Some(ReasonKind::Changed));
    }

    #[test]
    fn an_item_that_changed_kind_or_became_a_symlink_is_excluded() {
        let (root, mut candidates) = world("kind");
        candidates[0].kind = ItemKind::File;
        let plan = plan_candidates(&candidates, &[1], &open(&root)).unwrap();
        assert_eq!(reason_of(&plan, 1), Some(ReasonKind::Changed));

        let (root, candidates) = world("symlink");
        fs::remove_file(root.join("small.txt")).unwrap();
        std::os::unix::fs::symlink(root.join("big"), root.join("small.txt")).unwrap();
        let plan = plan_candidates(&candidates, &[2], &open(&root)).unwrap();
        assert_eq!(reason_of(&plan, 2), Some(ReasonKind::Changed));
    }

    #[test]
    fn a_candidate_that_moved_through_a_symlinked_parent_is_excluded() {
        let (root, mut candidates) = world("parent");
        let alias = root.join("alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        candidates[0].path = alias.join("big");
        let plan = plan_candidates(&candidates, &[1], &open(&root)).unwrap();
        assert_eq!(reason_of(&plan, 1), Some(ReasonKind::Changed));
    }

    #[test]
    fn guardrails_still_block_a_protected_candidate() {
        let (root, candidates) = world("guard");
        let guardrails = Guardrails::new(
            Some(root.clone()),
            vec![Protected::tree(root.join("big"), ReasonKind::AppData)],
        );
        let plan = plan_candidates(&candidates, &[1, 2], &guardrails).unwrap();
        assert_eq!(reason_of(&plan, 1), Some(ReasonKind::AppData));
        assert_eq!(plan.items.len(), 1);
    }

    #[derive(Default)]
    struct FakeTrash {
        moved: std::sync::Mutex<Vec<PathBuf>>,
    }

    impl TrashProvider for FakeTrash {
        fn move_to_trash(&self, path: &Path) -> std::io::Result<()> {
            self.moved.lock().unwrap().push(path.to_path_buf());
            Ok(())
        }
    }

    fn run(root: &Path, candidates: &[Candidate], ids: &[u32]) -> (Vec<ItemResult>, FakeTrash) {
        let guardrails = open(root);
        let plan = plan_candidates(candidates, ids, &guardrails).unwrap();
        let trash = FakeTrash::default();
        let results = execute_candidates(candidates, &plan.items, &guardrails, &trash);
        (results, trash)
    }

    #[test]
    fn execution_moves_the_planned_items() {
        let (root, candidates) = world("exec");
        let (results, trash) = run(&root, &candidates, &[1, 2]);
        assert!(results.iter().all(|r| r.outcome == Outcome::Moved));
        assert_eq!(trash.moved.lock().unwrap().len(), 2);
    }

    #[test]
    fn an_item_modified_after_the_preview_is_not_moved() {
        let (root, mut candidates) = world("late-change");
        let guardrails = open(&root);
        let plan = plan_candidates(&candidates, &[1, 2], &guardrails).unwrap();
        candidates[0].modified_secs -= 5000;

        let trash = FakeTrash::default();
        let results = execute_candidates(&candidates, &plan.items, &guardrails, &trash);

        let by_id = |id| results.iter().find(|r| r.node == id).unwrap();
        assert_eq!(by_id(1).outcome, Outcome::Failed);
        assert!(by_id(1).message.as_deref().unwrap().contains("modified"));
        assert_eq!(by_id(2).outcome, Outcome::Moved);
        assert_eq!(
            trash.moved.lock().unwrap().len(),
            1,
            "the changed item stays"
        );
    }

    #[test]
    fn an_item_that_vanished_after_the_preview_is_reported_as_already_gone() {
        let (root, candidates) = world("late-gone");
        let guardrails = open(&root);
        let plan = plan_candidates(&candidates, &[2], &guardrails).unwrap();
        fs::remove_file(root.join("small.txt")).unwrap();
        let trash = FakeTrash::default();
        let results = execute_candidates(&candidates, &plan.items, &guardrails, &trash);
        assert_eq!(results[0].outcome, Outcome::AlreadyGone);
        assert!(trash.moved.lock().unwrap().is_empty());
    }

    #[test]
    fn an_item_missing_from_the_candidate_list_is_never_moved() {
        let (root, candidates) = world("not-listed");
        let guardrails = open(&root);
        let plan = plan_candidates(&candidates, &[1], &guardrails).unwrap();
        let trash = FakeTrash::default();
        let results = execute_candidates(&candidates[1..], &plan.items, &guardrails, &trash);
        assert_eq!(results[0].outcome, Outcome::Failed);
        assert!(trash.moved.lock().unwrap().is_empty());
    }

    #[test]
    fn a_candidate_outside_home_carries_a_warning() {
        let (_root, candidates) = world("warn");
        let elsewhere = Guardrails::new(Some("/nowhere".into()), Vec::new());
        let plan = plan_candidates(&candidates, &[1], &elsewhere).unwrap();
        assert!(plan.needs_acknowledgement());
    }
}
