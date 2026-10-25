//! Turns a choice of duplicate copies into a dry-run plan for the Trash, and
//! moves them.
//!
//! The UI only names a group id and file ids from the last duplicate scan.
//! Everything else comes from that scan, never from the caller:
//!
//! * The last-copy rule (`check_selection`) is applied to every group, after
//!   merging all picks that name the same group. Two picks that each leave a
//!   different copy would otherwise move all of them.
//! * Each file to move is re-checked against the disk: same place, still a
//!   regular file, same size, same modification time, and the guardrails.
//! * At least one copy that stays must pass the same check, before the plan
//!   is made and again before every single move, so a copy that disappears
//!   half-way stops the rest of its group.
//!
//! The plan reuses `PlanItem`, `ItemResult` and `execute` from `cleanup`. In
//! this flow `PlanItem::node` and `ItemResult::node` hold the file id.
//!
//! What the checks cannot see: a file edited within the same second without
//! changing its size passes them, because only whole seconds are recorded.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, ErrorKind};
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use serde::Serialize;

use super::cleanup::{
    ExcludedItem, Guardrails, ItemKind, ItemResult, Outcome, PlanItem, Reason, ReasonKind, Verdict,
    execute, resolve, serialize_path,
};
use super::config::MAX_DUPLICATE_MOVES_PER_PLAN;
use super::duplicate_keep::{Selection, SelectionError, check_selection};
use super::duplicates::{DuplicateFile, DuplicateGroup};
use crate::platform::TrashProvider;

/// What the caller wants moved out of one group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    pub group: u32,
    pub files: Vec<u32>,
}

/// A copy that stays, shown in the confirmation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeptCopy {
    pub id: u32,
    #[serde(serialize_with = "serialize_path")]
    pub path: PathBuf,
    pub name: String,
    pub protected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedGroup {
    pub group: u32,
    /// Size of each copy.
    pub size: u64,
    /// Copies that stay and passed the disk check. Never empty.
    pub kept: Vec<KeptCopy>,
    /// Copies to move. Never empty. `PlanItem::node` is the file id.
    pub moves: Vec<PlanItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicatePlan {
    /// Most bytes first.
    pub groups: Vec<PlannedGroup>,
    /// Copies left out, with the reason. `node` is the file id.
    pub excluded: Vec<ExcludedItem>,
    pub total_bytes: u64,
    pub total_files: u64,
}

impl DuplicatePlan {
    pub fn needs_acknowledgement(&self) -> bool {
        self.groups
            .iter()
            .flat_map(|g| &g.moves)
            .any(|item| item.warning.is_some())
    }

    pub fn move_count(&self) -> usize {
        self.groups.iter().map(|g| g.moves.len()).sum()
    }
}

/// The caller asked for something the scan does not allow. The whole plan is
/// refused, because the interface never offers such a choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanError {
    UnknownGroup(u32),
    Selection { group: u32, error: SelectionError },
    TooManyMoves,
}

fn changed(message: &str) -> Reason {
    Reason::new(ReasonKind::Changed, message)
}

const KEPT_CHANGED: &str =
    "The copy that would stay was changed or removed. Find duplicates again.";

/// Checks one file against what the scan recorded. `Ok` carries the guardrail
/// warning, if any. Pass `None` for a copy that stays: it is never moved, so
/// the guardrails do not apply to it.
fn recheck(
    file: &DuplicateFile,
    size: u64,
    guardrails: Option<&Guardrails>,
) -> Result<Option<Reason>, Reason> {
    let lookup = |e: io::Error| {
        if e.kind() == ErrorKind::NotFound {
            Reason::new(
                ReasonKind::Missing,
                "No longer exists on disk. Find duplicates again.",
            )
        } else {
            Reason::new(ReasonKind::Inaccessible, e.to_string())
        }
    };
    let resolved = resolve(&file.path).map_err(lookup)?;
    if resolved != file.path {
        return Err(changed(
            "Its location changed since the scan. Find duplicates again.",
        ));
    }
    let meta = fs::symlink_metadata(&resolved).map_err(lookup)?;
    if !meta.is_file() || meta.len() != size {
        return Err(changed("It changed since the scan. Find duplicates again."));
    }
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|age| age.as_secs());
    if modified != Some(file.modified_secs) {
        return Err(changed(
            "It was modified since the scan, so it may be in use. Find duplicates again.",
        ));
    }
    match guardrails.map(|g| g.verdict(&resolved)) {
        None | Some(Verdict::Allowed) => Ok(None),
        Some(Verdict::Warn(reason)) => Ok(Some(reason)),
        Some(Verdict::Blocked(reason)) => Err(reason),
    }
}

fn file_name(file: &DuplicateFile) -> String {
    file.path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn find(group: &DuplicateGroup, id: u32) -> Option<&DuplicateFile> {
    group.files.iter().find(|f| f.id == id)
}

fn excluded(file: &DuplicateFile, reason: Reason) -> ExcludedItem {
    ExcludedItem {
        node: file.id,
        path: file.path.clone(),
        name: file_name(file),
        reason,
    }
}

/// Collects the picks per group, in the order the groups first appear.
fn merge_picks(picks: &[Pick]) -> Vec<(u32, Vec<u32>)> {
    let mut order: Vec<u32> = Vec::new();
    let mut wanted: HashMap<u32, Vec<u32>> = HashMap::new();
    for pick in picks {
        if !wanted.contains_key(&pick.group) {
            order.push(pick.group);
        }
        wanted
            .entry(pick.group)
            .or_default()
            .extend_from_slice(&pick.files);
    }
    order
        .into_iter()
        .map(|group| {
            let files = wanted.remove(&group).unwrap_or_default();
            (group, files)
        })
        .collect()
}

/// Builds the dry-run result for the chosen copies. Performs no writes.
pub fn plan_duplicates(
    groups: &[DuplicateGroup],
    picks: &[Pick],
    guardrails: &Guardrails,
) -> Result<DuplicatePlan, PlanError> {
    let by_id: HashMap<u32, &DuplicateGroup> = groups.iter().map(|g| (g.id, g)).collect();

    let mut checked: Vec<(&DuplicateGroup, Selection)> = Vec::new();
    for (group_id, files) in merge_picks(picks) {
        let group = by_id
            .get(&group_id)
            .copied()
            .ok_or(PlanError::UnknownGroup(group_id))?;
        let selection = check_selection(group, &files).map_err(|error| PlanError::Selection {
            group: group_id,
            error,
        })?;
        if !selection.moves.is_empty() {
            checked.push((group, selection));
        }
    }
    let wanted: usize = checked.iter().map(|(_, s)| s.moves.len()).sum();
    if wanted > MAX_DUPLICATE_MOVES_PER_PLAN {
        return Err(PlanError::TooManyMoves);
    }

    let mut planned = Vec::new();
    let mut left_out = Vec::new();
    for (group, selection) in checked {
        let kept: Vec<KeptCopy> = selection
            .kept
            .iter()
            .filter_map(|&id| find(group, id))
            .filter(|file| recheck(file, group.size, None).is_ok())
            .map(|file| KeptCopy {
                id: file.id,
                path: file.path.clone(),
                name: file_name(file),
                protected: file.protected,
            })
            .collect();

        let mut moves = Vec::new();
        for file in selection.moves.iter().filter_map(|&id| find(group, id)) {
            let result = if kept.is_empty() {
                Err(changed(KEPT_CHANGED))
            } else {
                recheck(file, group.size, Some(guardrails))
            };
            match result {
                Ok(warning) => moves.push(PlanItem {
                    node: file.id,
                    path: file.path.clone(),
                    name: file_name(file),
                    kind: ItemKind::File,
                    size: group.size,
                    files: 1,
                    warning,
                }),
                Err(reason) => left_out.push(excluded(file, reason)),
            }
        }
        if !moves.is_empty() {
            moves.sort_by(|a, b| a.path.cmp(&b.path));
            planned.push(PlannedGroup {
                group: group.id,
                size: group.size,
                kept,
                moves,
            });
        }
    }

    let bytes_of = |g: &PlannedGroup| g.size * g.moves.len() as u64;
    planned.sort_by(|a, b| bytes_of(b).cmp(&bytes_of(a)).then(a.group.cmp(&b.group)));
    let total_bytes = planned.iter().map(bytes_of).sum();
    let total_files = planned.iter().map(|g| g.moves.len() as u64).sum();
    Ok(DuplicatePlan {
        groups: planned,
        excluded: left_out,
        total_bytes,
        total_files,
    })
}

/// Drops files that left the disk and then the groups that are no longer
/// worth showing: fewer than two copies, or nothing left that may be moved.
/// File ids are unique within a scan, so one set covers every group.
pub fn remove_files(groups: &mut Vec<DuplicateGroup>, gone: &HashSet<u32>) {
    for group in groups.iter_mut() {
        group.files.retain(|file| !gone.contains(&file.id));
    }
    groups.retain(|group| group.files.len() >= 2 && group.files.iter().any(|f| !f.protected));
}

fn result(item: &PlanItem, outcome: Outcome, message: Option<String>) -> ItemResult {
    ItemResult {
        node: item.node,
        path: item.path.clone(),
        name: item.name.clone(),
        size: item.size,
        files: item.files,
        outcome,
        message,
    }
}

fn failed(item: &PlanItem, message: &str) -> ItemResult {
    result(item, Outcome::Failed, Some(message.to_owned()))
}

/// A group of the plan, matched against the scan and re-validated.
struct Prepared<'a> {
    group: &'a DuplicateGroup,
    selection: Selection,
    kept_ids: Vec<u32>,
}

impl Prepared<'_> {
    /// True when at least one copy that stays is still as the scan found it.
    fn keeper_intact(&self) -> bool {
        self.kept_ids.iter().any(|id| {
            self.selection.kept.contains(id)
                && find(self.group, *id)
                    .is_some_and(|file| recheck(file, self.group.size, None).is_ok())
        })
    }
}

fn prepare<'a>(
    by_id: &HashMap<u32, &'a DuplicateGroup>,
    planned: &PlannedGroup,
) -> Result<Prepared<'a>, &'static str> {
    let group = by_id
        .get(&planned.group)
        .copied()
        .ok_or("This group is no longer in the scan. Find duplicates again.")?;
    let ids: Vec<u32> = planned.moves.iter().map(|item| item.node).collect();
    let selection = check_selection(group, &ids)
        .map_err(|_| "Moving these copies would leave none of this file. Nothing was moved.")?;
    Ok(Prepared {
        group,
        selection,
        kept_ids: planned.kept.iter().map(|k| k.id).collect(),
    })
}

fn move_checked(
    prepared: &Prepared,
    item: &PlanItem,
    guardrails: &Guardrails,
    provider: &dyn TrashProvider,
) -> ItemResult {
    let Some(file) = find(prepared.group, item.node) else {
        return failed(item, "It is no longer in the scan. Find duplicates again.");
    };
    if file.path != item.path || !prepared.selection.moves.contains(&item.node) {
        return failed(item, "It does not match the scan. Find duplicates again.");
    }
    if !prepared.keeper_intact() {
        return failed(item, KEPT_CHANGED);
    }
    match recheck(file, prepared.group.size, Some(guardrails)) {
        Ok(_) => execute(std::slice::from_ref(item), guardrails, provider).remove(0),
        Err(reason) if reason.kind == ReasonKind::Missing => {
            result(item, Outcome::AlreadyGone, None)
        }
        Err(reason) => failed(item, &reason.message),
    }
}

/// Moves the planned copies to the Trash. The last-copy rule and the disk
/// checks run again for every file, so something that changed after the
/// preview is reported and left alone. A failure on one file does not stop
/// the others.
pub fn execute_duplicates(
    groups: &[DuplicateGroup],
    plan: &DuplicatePlan,
    guardrails: &Guardrails,
    provider: &dyn TrashProvider,
) -> Vec<ItemResult> {
    let by_id: HashMap<u32, &DuplicateGroup> = groups.iter().map(|g| (g.id, g)).collect();
    let mut results = Vec::new();
    for planned in &plan.groups {
        match prepare(&by_id, planned) {
            Err(message) => results.extend(planned.moves.iter().map(|item| failed(item, message))),
            Ok(prepared) => results.extend(
                planned
                    .moves
                    .iter()
                    .map(|item| move_checked(&prepared, item, guardrails, provider)),
            ),
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::cleanup::Protected;
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::UNIX_EPOCH;

    const CONTENT: &[u8] = b"same content";
    const GROUP: u32 = 7;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ctd-dplan-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn open(root: &Path) -> Guardrails {
        Guardrails::new(Some(root.to_path_buf()), Vec::new())
    }

    struct World {
        root: PathBuf,
        group: DuplicateGroup,
    }

    impl World {
        fn groups(&self) -> Vec<DuplicateGroup> {
            vec![self.group.clone()]
        }

        fn path(&self, id: u32) -> PathBuf {
            find(&self.group, id).unwrap().path.clone()
        }
    }

    /// `copies` identical files `root/d<i>/doc.bin`, with ids 1 to `copies`.
    fn world(name: &str, copies: u32) -> World {
        let root = temp_root(name);
        let files = (1..=copies)
            .map(|id| {
                let dir = root.join(format!("d{id}"));
                fs::create_dir_all(&dir).unwrap();
                let path = dir.join("doc.bin");
                fs::write(&path, CONTENT).unwrap();
                let modified = fs::metadata(&path)
                    .unwrap()
                    .modified()
                    .unwrap()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                DuplicateFile {
                    id,
                    path,
                    modified_secs: modified,
                    protected: false,
                }
            })
            .collect();
        World {
            root,
            group: DuplicateGroup {
                id: GROUP,
                size: CONTENT.len() as u64,
                files,
            },
        }
    }

    fn pick(files: &[u32]) -> Pick {
        Pick {
            group: GROUP,
            files: files.to_vec(),
        }
    }

    fn reason_of(plan: &DuplicatePlan, id: u32) -> Option<ReasonKind> {
        plan.excluded
            .iter()
            .find(|e| e.node == id)
            .map(|e| e.reason.kind)
    }

    fn move_ids(plan: &DuplicatePlan) -> Vec<u32> {
        plan.groups
            .iter()
            .flat_map(|g| g.moves.iter().map(|i| i.node))
            .collect()
    }

    #[derive(Default)]
    struct FakeTrash {
        moved: Mutex<Vec<PathBuf>>,
        fail_on: Option<PathBuf>,
        on_move: Option<Box<dyn Fn() + Send + Sync>>,
    }

    impl TrashProvider for FakeTrash {
        fn move_to_trash(&self, path: &Path) -> io::Result<()> {
            if self.fail_on.as_deref() == Some(path) {
                return Err(io::Error::other("denied"));
            }
            self.moved.lock().unwrap().push(path.to_path_buf());
            if let Some(hook) = &self.on_move {
                hook();
            }
            Ok(())
        }
    }

    fn moved(trash: &FakeTrash) -> Vec<PathBuf> {
        trash.moved.lock().unwrap().clone()
    }

    // --- planning ---

    #[test]
    fn chosen_copies_become_items_and_the_rest_are_kept() {
        let w = world("ok", 3);
        let plan = plan_duplicates(&w.groups(), &[pick(&[3, 1])], &open(&w.root)).unwrap();
        assert_eq!(plan.groups.len(), 1);
        let group = &plan.groups[0];
        assert_eq!(group.group, GROUP);
        assert_eq!(move_ids(&plan), [1, 3]);
        assert_eq!(group.kept.len(), 1);
        assert_eq!(group.kept[0].id, 2);
        assert_eq!(group.kept[0].name, "doc.bin");
        assert_eq!((plan.total_bytes, plan.total_files), (2 * 12, 2));
        assert!(plan.excluded.is_empty() && !plan.needs_acknowledgement());
        assert_eq!(plan.move_count(), 2);
    }

    #[test]
    fn planning_changes_nothing_on_disk() {
        let w = world("dry", 3);
        plan_duplicates(&w.groups(), &[pick(&[1, 2])], &open(&w.root)).unwrap();
        for id in 1..=3 {
            assert!(w.path(id).exists());
        }
    }

    #[test]
    fn an_empty_choice_makes_an_empty_plan() {
        let w = world("empty", 2);
        let plan = plan_duplicates(&w.groups(), &[], &open(&w.root)).unwrap();
        assert!(plan.groups.is_empty() && plan.excluded.is_empty());
        let plan = plan_duplicates(&w.groups(), &[pick(&[])], &open(&w.root)).unwrap();
        assert!(plan.groups.is_empty());
        assert_eq!((plan.total_bytes, plan.total_files), (0, 0));
    }

    #[test]
    fn a_repeated_id_counts_once() {
        let w = world("repeat", 2);
        let plan = plan_duplicates(&w.groups(), &[pick(&[1, 1, 1])], &open(&w.root)).unwrap();
        assert_eq!(move_ids(&plan), [1]);
    }

    #[test]
    fn choosing_every_copy_is_refused() {
        let w = world("last", 3);
        assert_eq!(
            plan_duplicates(&w.groups(), &[pick(&[1, 2, 3])], &open(&w.root)).unwrap_err(),
            PlanError::Selection {
                group: GROUP,
                error: SelectionError::LastCopy
            }
        );
    }

    #[test]
    fn two_picks_for_one_group_cannot_add_up_to_every_copy() {
        let w = world("split", 2);
        let err =
            plan_duplicates(&w.groups(), &[pick(&[1]), pick(&[2])], &open(&w.root)).unwrap_err();
        assert_eq!(
            err,
            PlanError::Selection {
                group: GROUP,
                error: SelectionError::LastCopy
            }
        );
    }

    #[test]
    fn unknown_group_unknown_file_and_protected_file_are_refused() {
        let mut w = world("bad", 3);
        let guardrails = open(&w.root);
        let other = Pick {
            group: 99,
            files: vec![1],
        };
        assert_eq!(
            plan_duplicates(&w.groups(), &[other], &guardrails).unwrap_err(),
            PlanError::UnknownGroup(99)
        );
        assert_eq!(
            plan_duplicates(&w.groups(), &[pick(&[1, 42])], &guardrails).unwrap_err(),
            PlanError::Selection {
                group: GROUP,
                error: SelectionError::UnknownFile(42)
            }
        );
        w.group.files[0].protected = true;
        assert_eq!(
            plan_duplicates(&w.groups(), &[pick(&[1])], &guardrails).unwrap_err(),
            PlanError::Selection {
                group: GROUP,
                error: SelectionError::ProtectedFile(1)
            }
        );
    }

    #[test]
    fn a_protected_copy_counts_as_the_one_that_stays() {
        let mut w = world("protected", 2);
        w.group.files[0].protected = true;
        let plan = plan_duplicates(&w.groups(), &[pick(&[2])], &open(&w.root)).unwrap();
        assert_eq!(move_ids(&plan), [2]);
        assert!(plan.groups[0].kept[0].protected);
    }

    #[test]
    fn too_many_moves_are_refused_before_the_disk_is_touched() {
        let files = (1..=MAX_DUPLICATE_MOVES_PER_PLAN as u32 + 2)
            .map(|id| DuplicateFile {
                id,
                path: PathBuf::from(format!("/nonexistent/{id}")),
                modified_secs: 1,
                protected: false,
            })
            .collect::<Vec<_>>();
        let ids: Vec<u32> = (1..=MAX_DUPLICATE_MOVES_PER_PLAN as u32 + 1).collect();
        let group = DuplicateGroup {
            id: GROUP,
            size: 1,
            files,
        };
        assert_eq!(
            plan_duplicates(&[group], &[pick(&ids)], &open(Path::new("/nonexistent"))).unwrap_err(),
            PlanError::TooManyMoves
        );
    }

    #[test]
    fn a_copy_that_vanished_is_excluded_as_missing() {
        let w = world("gone", 3);
        fs::remove_file(w.path(1)).unwrap();
        let plan = plan_duplicates(&w.groups(), &[pick(&[1, 2])], &open(&w.root)).unwrap();
        assert_eq!(reason_of(&plan, 1), Some(ReasonKind::Missing));
        assert_eq!(move_ids(&plan), [2]);
    }

    #[test]
    fn a_copy_modified_since_the_scan_is_excluded_as_changed() {
        let mut w = world("modified", 3);
        w.group.files[0].modified_secs -= 1000;
        let plan = plan_duplicates(&w.groups(), &[pick(&[1, 2])], &open(&w.root)).unwrap();
        assert_eq!(reason_of(&plan, 1), Some(ReasonKind::Changed));
        assert_eq!(move_ids(&plan), [2]);
    }

    #[test]
    fn a_copy_whose_size_changed_is_excluded() {
        let mut w = world("size", 3);
        fs::write(w.path(1), b"longer content than before").unwrap();
        // Keep the recorded time equal to the new time so only the size differs.
        w.group.files[0].modified_secs = fs::metadata(w.path(1))
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let plan = plan_duplicates(&w.groups(), &[pick(&[1])], &open(&w.root)).unwrap();
        assert_eq!(reason_of(&plan, 1), Some(ReasonKind::Changed));
        assert!(plan.groups.is_empty());
    }

    #[test]
    fn a_copy_replaced_by_a_symlink_is_excluded() {
        let w = world("symlink", 3);
        let path = w.path(1);
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(w.path(2), &path).unwrap();
        let plan = plan_duplicates(&w.groups(), &[pick(&[1])], &open(&w.root)).unwrap();
        assert_eq!(reason_of(&plan, 1), Some(ReasonKind::Changed));
    }

    #[test]
    fn a_copy_replaced_by_a_folder_is_excluded() {
        let w = world("folder", 3);
        let path = w.path(1);
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        let plan = plan_duplicates(&w.groups(), &[pick(&[1])], &open(&w.root)).unwrap();
        assert_eq!(reason_of(&plan, 1), Some(ReasonKind::Changed));
    }

    #[test]
    fn a_copy_reached_through_a_symlinked_parent_is_excluded() {
        let mut w = world("parent", 3);
        let alias = w.root.join("alias");
        std::os::unix::fs::symlink(&w.root, &alias).unwrap();
        w.group.files[0].path = alias.join("d1").join("doc.bin");
        let plan = plan_duplicates(&w.groups(), &[pick(&[1])], &open(&w.root)).unwrap();
        assert_eq!(reason_of(&plan, 1), Some(ReasonKind::Changed));
    }

    #[test]
    fn a_missing_kept_copy_is_left_out_of_the_kept_list() {
        let w = world("kept-gone", 3);
        fs::remove_file(w.path(2)).unwrap();
        let plan = plan_duplicates(&w.groups(), &[pick(&[1])], &open(&w.root)).unwrap();
        assert_eq!(move_ids(&plan), [1]);
        let kept: Vec<u32> = plan.groups[0].kept.iter().map(|k| k.id).collect();
        assert_eq!(kept, [3]);
    }

    #[test]
    fn when_no_kept_copy_is_left_nothing_in_the_group_is_planned() {
        let w = world("kept-none", 3);
        fs::remove_file(w.path(3)).unwrap();
        let mut groups = w.groups();
        groups[0].files[2].modified_secs -= 1;
        // Copy 3 is gone and copy 2 changed, so only copy 1 would remain wanted.
        groups[0].files[1].modified_secs -= 1;
        let plan = plan_duplicates(&groups, &[pick(&[1])], &open(&w.root)).unwrap();
        assert!(plan.groups.is_empty());
        assert_eq!(reason_of(&plan, 1), Some(ReasonKind::Changed));
        assert!(
            plan.excluded[0]
                .reason
                .message
                .contains("would stay was changed")
        );
        assert!(w.path(1).exists());
    }

    #[test]
    fn guardrails_still_block_a_protected_location() {
        let w = world("guard", 3);
        let guardrails = Guardrails::new(
            Some(w.root.clone()),
            vec![Protected::tree(w.root.join("d1"), ReasonKind::AppData)],
        );
        let plan = plan_duplicates(&w.groups(), &[pick(&[1, 2])], &guardrails).unwrap();
        assert_eq!(reason_of(&plan, 1), Some(ReasonKind::AppData));
        assert_eq!(move_ids(&plan), [2]);
    }

    #[test]
    fn a_copy_outside_home_carries_a_warning() {
        let w = world("warn", 2);
        let elsewhere = Guardrails::new(Some("/nowhere".into()), Vec::new());
        let plan = plan_duplicates(&w.groups(), &[pick(&[1])], &elsewhere).unwrap();
        assert!(plan.needs_acknowledgement());
    }

    #[test]
    fn groups_are_listed_by_bytes_then_id() {
        let small = world("order-a", 2);
        let mut big = world("order-b", 3);
        big.group.id = 3;
        let mut groups = vec![small.group.clone(), big.group.clone()];
        groups[1].id = 3;
        let picks = [
            pick(&[1]),
            Pick {
                group: 3,
                files: vec![1, 2],
            },
        ];
        // Both worlds live under different temp roots; allow both.
        let guardrails = Guardrails::new(None, Vec::new());
        let plan = plan_duplicates(&groups, &picks, &guardrails).unwrap();
        let order: Vec<u32> = plan.groups.iter().map(|g| g.group).collect();
        assert_eq!(order, [3, GROUP]);
    }

    // --- groups after a move ---

    fn bare(id: u32, protected: bool) -> DuplicateFile {
        DuplicateFile {
            id,
            path: PathBuf::from(format!("/x/{id}")),
            modified_secs: 1,
            protected,
        }
    }

    fn bare_group(id: u32, files: Vec<DuplicateFile>) -> DuplicateGroup {
        DuplicateGroup { id, size: 1, files }
    }

    fn ids_of(groups: &[DuplicateGroup]) -> Vec<(u32, Vec<u32>)> {
        groups
            .iter()
            .map(|g| (g.id, g.files.iter().map(|f| f.id).collect()))
            .collect()
    }

    #[test]
    fn a_group_shrinks_in_place_and_others_are_untouched() {
        let mut groups = vec![
            bare_group(1, vec![bare(1, false), bare(2, false), bare(3, false)]),
            bare_group(2, vec![bare(4, false), bare(5, false)]),
        ];
        remove_files(&mut groups, &HashSet::from([1]));
        assert_eq!(ids_of(&groups), [(1, vec![2, 3]), (2, vec![4, 5])]);
    }

    #[test]
    fn a_group_left_with_one_copy_disappears() {
        let mut groups = vec![bare_group(1, vec![bare(1, false), bare(2, false)])];
        remove_files(&mut groups, &HashSet::from([2]));
        assert!(groups.is_empty());
    }

    #[test]
    fn a_group_left_with_only_protected_copies_disappears() {
        let mut groups = vec![bare_group(
            1,
            vec![bare(1, true), bare(2, true), bare(3, false)],
        )];
        remove_files(&mut groups, &HashSet::from([3]));
        assert!(groups.is_empty());
    }

    #[test]
    fn nothing_removed_changes_nothing() {
        let mut groups = vec![bare_group(1, vec![bare(1, true), bare(2, false)])];
        remove_files(&mut groups, &HashSet::new());
        assert_eq!(ids_of(&groups), [(1, vec![1, 2])]);
    }

    // --- execution ---

    fn run(w: &World, picks: &[Pick]) -> (DuplicatePlan, Vec<ItemResult>, FakeTrash) {
        let guardrails = open(&w.root);
        let plan = plan_duplicates(&w.groups(), picks, &guardrails).unwrap();
        let trash = FakeTrash::default();
        let results = execute_duplicates(&w.groups(), &plan, &guardrails, &trash);
        (plan, results, trash)
    }

    #[test]
    fn execution_moves_the_planned_copies_and_nothing_else() {
        let w = world("exec", 3);
        let (_, results, trash) = run(&w, &[pick(&[1, 3])]);
        assert!(results.iter().all(|r| r.outcome == Outcome::Moved));
        assert_eq!(moved(&trash), [w.path(1), w.path(3)]);
    }

    #[test]
    fn a_copy_modified_after_the_preview_is_not_moved() {
        let mut w = world("late-change", 3);
        let guardrails = open(&w.root);
        let plan = plan_duplicates(&w.groups(), &[pick(&[1, 2])], &guardrails).unwrap();
        w.group.files[0].modified_secs -= 5000;
        let trash = FakeTrash::default();
        let results = execute_duplicates(&w.groups(), &plan, &guardrails, &trash);

        let by_id = |id| results.iter().find(|r| r.node == id).unwrap();
        assert_eq!(by_id(1).outcome, Outcome::Failed);
        assert!(by_id(1).message.as_deref().unwrap().contains("modified"));
        assert_eq!(by_id(2).outcome, Outcome::Moved);
        assert_eq!(moved(&trash), [w.path(2)]);
    }

    #[test]
    fn a_copy_that_vanished_after_the_preview_is_reported_as_already_gone() {
        let w = world("late-gone", 3);
        let guardrails = open(&w.root);
        let plan = plan_duplicates(&w.groups(), &[pick(&[1])], &guardrails).unwrap();
        fs::remove_file(w.path(1)).unwrap();
        let trash = FakeTrash::default();
        let results = execute_duplicates(&w.groups(), &plan, &guardrails, &trash);
        assert_eq!(results[0].outcome, Outcome::AlreadyGone);
        assert!(moved(&trash).is_empty());
    }

    #[test]
    fn nothing_moves_when_the_kept_copy_is_removed_after_the_preview() {
        let w = world("late-kept-gone", 2);
        let guardrails = open(&w.root);
        let plan = plan_duplicates(&w.groups(), &[pick(&[1])], &guardrails).unwrap();
        fs::remove_file(w.path(2)).unwrap();
        let trash = FakeTrash::default();
        let results = execute_duplicates(&w.groups(), &plan, &guardrails, &trash);
        assert_eq!(results[0].outcome, Outcome::Failed);
        assert!(moved(&trash).is_empty());
        assert!(w.path(1).exists(), "the only remaining copy stays");
    }

    #[test]
    fn nothing_moves_when_the_kept_copy_changes_after_the_preview() {
        let mut w = world("late-kept-modified", 2);
        let guardrails = open(&w.root);
        let plan = plan_duplicates(&w.groups(), &[pick(&[1])], &guardrails).unwrap();
        w.group.files[1].modified_secs -= 5000;
        let trash = FakeTrash::default();
        let results = execute_duplicates(&w.groups(), &plan, &guardrails, &trash);
        assert_eq!(results[0].outcome, Outcome::Failed);
        assert!(moved(&trash).is_empty());
    }

    #[test]
    fn a_kept_copy_that_disappears_part_way_stops_the_rest_of_the_group() {
        let w = world("midway", 3);
        let guardrails = open(&w.root);
        let plan = plan_duplicates(&w.groups(), &[pick(&[1, 2])], &guardrails).unwrap();
        let keeper = w.path(3);
        let trash = FakeTrash {
            on_move: Some(Box::new(move || {
                let _ = fs::remove_file(&keeper);
            })),
            ..FakeTrash::default()
        };
        let results = execute_duplicates(&w.groups(), &plan, &guardrails, &trash);
        assert_eq!(results[0].outcome, Outcome::Moved);
        assert_eq!(results[1].outcome, Outcome::Failed);
        assert_eq!(moved(&trash).len(), 1, "the second move was not attempted");
    }

    #[test]
    fn a_plan_that_covers_every_copy_is_refused_at_execute_time() {
        let w = world("tampered", 2);
        let guardrails = open(&w.root);
        let plan = plan_duplicates(&w.groups(), &[pick(&[1])], &guardrails).unwrap();
        let mut forged = plan.clone();
        let mut extra = forged.groups[0].moves[0].clone();
        extra.node = 2;
        extra.path = w.path(2);
        extra.name = "doc.bin".into();
        forged.groups[0].moves.push(extra);
        let trash = FakeTrash::default();
        let results = execute_duplicates(&w.groups(), &forged, &guardrails, &trash);
        assert!(results.iter().all(|r| r.outcome == Outcome::Failed));
        assert!(moved(&trash).is_empty());
        assert!(w.path(1).exists() && w.path(2).exists());
    }

    #[test]
    fn a_forged_plan_cannot_move_a_protected_copy() {
        let mut w = world("forged-protected", 3);
        let guardrails = open(&w.root);
        let plan = plan_duplicates(&w.groups(), &[pick(&[2])], &guardrails).unwrap();
        w.group.files[1].protected = true;
        let trash = FakeTrash::default();
        let results = execute_duplicates(&w.groups(), &plan, &guardrails, &trash);
        assert_eq!(results[0].outcome, Outcome::Failed);
        assert!(moved(&trash).is_empty());
    }

    #[test]
    fn a_plan_for_a_group_that_is_not_in_the_scan_moves_nothing() {
        let w = world("no-group", 2);
        let guardrails = open(&w.root);
        let plan = plan_duplicates(&w.groups(), &[pick(&[1])], &guardrails).unwrap();
        let trash = FakeTrash::default();
        let results = execute_duplicates(&[], &plan, &guardrails, &trash);
        assert_eq!(results[0].outcome, Outcome::Failed);
        assert!(moved(&trash).is_empty());
    }

    #[test]
    fn a_plan_item_whose_path_differs_from_the_scan_is_not_moved() {
        let w = world("path-mismatch", 3);
        let guardrails = open(&w.root);
        let plan = plan_duplicates(&w.groups(), &[pick(&[1])], &guardrails).unwrap();
        let mut forged = plan.clone();
        forged.groups[0].moves[0].path = w.path(2);
        let trash = FakeTrash::default();
        let results = execute_duplicates(&w.groups(), &forged, &guardrails, &trash);
        assert_eq!(results[0].outcome, Outcome::Failed);
        assert!(moved(&trash).is_empty());
    }

    #[test]
    fn a_plan_item_that_is_not_a_member_of_the_group_is_not_moved() {
        let w = world("stranger", 3);
        let guardrails = open(&w.root);
        let plan = plan_duplicates(&w.groups(), &[pick(&[1])], &guardrails).unwrap();
        let mut forged = plan.clone();
        forged.groups[0].moves[0].node = 500;
        let trash = FakeTrash::default();
        let results = execute_duplicates(&w.groups(), &forged, &guardrails, &trash);
        assert_eq!(results[0].outcome, Outcome::Failed);
        assert!(moved(&trash).is_empty());
    }

    #[test]
    fn one_failure_does_not_stop_the_other_copies() {
        let w = world("partial", 4);
        let guardrails = open(&w.root);
        let plan = plan_duplicates(&w.groups(), &[pick(&[1, 2, 3])], &guardrails).unwrap();
        let trash = FakeTrash {
            fail_on: Some(w.path(2)),
            ..FakeTrash::default()
        };
        let results = execute_duplicates(&w.groups(), &plan, &guardrails, &trash);
        let outcome = |id| results.iter().find(|r| r.node == id).unwrap().outcome;
        assert_eq!(outcome(1), Outcome::Moved);
        assert_eq!(outcome(2), Outcome::Failed);
        assert_eq!(outcome(3), Outcome::Moved);
        assert_eq!(moved(&trash), [w.path(1), w.path(3)]);
    }

    #[test]
    fn guardrails_are_applied_again_at_execute_time() {
        let w = world("guard-late", 3);
        let plan = plan_duplicates(&w.groups(), &[pick(&[1])], &open(&w.root)).unwrap();
        let strict = Guardrails::new(
            Some(w.root.clone()),
            vec![Protected::tree(w.root.join("d1"), ReasonKind::AppData)],
        );
        let trash = FakeTrash::default();
        let results = execute_duplicates(&w.groups(), &plan, &strict, &trash);
        assert_eq!(results[0].outcome, Outcome::Failed);
        assert!(moved(&trash).is_empty());
    }

    #[test]
    fn a_second_group_is_still_processed_after_the_first_is_refused() {
        let w1 = world("multi-a", 2);
        let w2 = world("multi-b", 2);
        let mut g2 = w2.group.clone();
        g2.id = 8;
        let groups = vec![w1.group.clone(), g2];
        let guardrails = Guardrails::new(None, Vec::new());
        let picks = [
            pick(&[1]),
            Pick {
                group: 8,
                files: vec![1],
            },
        ];
        let plan = plan_duplicates(&groups, &picks, &guardrails).unwrap();
        fs::remove_file(w1.path(2)).unwrap(); // group 7 loses its kept copy
        let trash = FakeTrash::default();
        let results = execute_duplicates(&groups, &plan, &guardrails, &trash);
        let for_group = |g: u32| {
            let planned = plan.groups.iter().find(|p| p.group == g).unwrap();
            results
                .iter()
                .find(|r| r.node == planned.moves[0].node && r.path == planned.moves[0].path)
                .unwrap()
                .outcome
        };
        assert_eq!(for_group(GROUP), Outcome::Failed);
        assert_eq!(for_group(8), Outcome::Moved);
    }
}
