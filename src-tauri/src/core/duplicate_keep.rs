//! Which copy of a duplicate group stays, and the last-copy rule.
//!
//! Pure functions over the groups `find_duplicates` returns. Nothing here
//! touches the disk, so the rule that no group is ever emptied can be tested
//! exhaustively. Later steps (plan and execute) call `check_selection` and
//! still re-check the disk.
//!
//! Ranking, best keeper first. The first rule that tells two copies apart
//! decides:
//! 1. a copy under a protected folder,
//! 2. a copy under a folder the user pinned ("always keep copies here"),
//! 3. a copy outside Downloads, Desktop and the Trash,
//! 4. a copy without a copy-style name such as `report (1)` or `report copy`,
//! 5. the oldest modification time (an unknown time counts as newest),
//! 6. the shortest path,
//! 7. the file name, then the full path, sorted first.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::duplicates::{DuplicateFile, DuplicateGroup, fold};

/// Highest number accepted in "name (N)", so "Report (2024)" is not a copy.
const MAX_COPY_COUNTER_DIGITS: usize = 3;

/// User choices that feed the ranking.
#[derive(Debug, Clone, Default)]
pub struct KeepPreferences {
    pinned: Vec<PathBuf>,
    poor_locations: Vec<PathBuf>,
}

impl KeepPreferences {
    /// `home` enables the Downloads and Desktop rule; without it only the
    /// Trash counts as a poor location. `pinned` are folders whose copies the
    /// user wants kept.
    pub fn new(home: Option<&Path>, pinned: &[PathBuf]) -> Self {
        Self {
            pinned: pinned.iter().map(|p| fold(p)).collect(),
            poor_locations: home
                .into_iter()
                .flat_map(|h| [fold(&h.join("Downloads")), fold(&h.join("Desktop"))])
                .collect(),
        }
    }
}

/// The rule that put the keeper ahead of the next best copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum KeepReason {
    Protected,
    Pinned,
    BetterLocation,
    OriginalName,
    Oldest,
    ShortestPath,
    NameOrder,
}

impl KeepReason {
    /// Plain words for the interface.
    pub fn description(self) -> &'static str {
        match self {
            Self::Protected => "It is in a protected folder",
            Self::Pinned => "It is in a folder you chose to keep",
            Self::BetterLocation => "The other copies are in Downloads, Desktop or the Trash",
            Self::OriginalName => "The other copies have a copy-style name",
            Self::Oldest => "It is the oldest copy",
            Self::ShortestPath => "It has the shortest path",
            Self::NameOrder => "It comes first by name",
        }
    }
}

/// Lower is better, field by field in declaration order.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key<'a> {
    not_protected: bool,
    not_pinned: bool,
    poor_location: bool,
    copy_name: bool,
    modified: u64,
    path_len: usize,
    name: &'a OsStr,
    path: &'a Path,
}

fn is_trash_folder(component: &OsStr) -> bool {
    component == ".trash" || component == ".trashes"
}

/// `stem` is already lower-cased.
fn is_copy_name(stem: &str) -> bool {
    if stem.starts_with("copy of ") {
        return true;
    }
    let stem = stem.trim_end();
    // "report (2)"
    if let Some(inner) = stem
        .strip_suffix(')')
        .and_then(|s| s.rsplit_once(" ("))
        .map(|(_, inner)| inner)
        && (1..=MAX_COPY_COUNTER_DIGITS).contains(&inner.len())
        && inner.bytes().all(|b| b.is_ascii_digit())
    {
        return true;
    }
    // "report copy" and "report copy 2"
    let without_counter = stem.trim_end_matches(|c: char| c.is_ascii_digit());
    let base = if without_counter.len() < stem.len() {
        without_counter.trim_end()
    } else {
        stem
    };
    base.ends_with(" copy")
}

fn key_of<'a>(file: &'a DuplicateFile, prefs: &KeepPreferences) -> Key<'a> {
    let folded = fold(&file.path);
    let stem = file
        .path
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let poor_location = folded.components().any(|c| is_trash_folder(c.as_os_str()))
        || prefs
            .poor_locations
            .iter()
            .any(|root| folded.starts_with(root));
    Key {
        not_protected: !file.protected,
        not_pinned: !prefs.pinned.iter().any(|root| folded.starts_with(root)),
        poor_location,
        copy_name: is_copy_name(&stem),
        modified: if file.modified_secs == 0 {
            u64::MAX
        } else {
            file.modified_secs
        },
        path_len: file.path.as_os_str().len(),
        name: file.path.file_name().unwrap_or_default(),
        path: &file.path,
    }
}

fn first_difference(best: &Key, other: &Key) -> KeepReason {
    if best.not_protected != other.not_protected {
        KeepReason::Protected
    } else if best.not_pinned != other.not_pinned {
        KeepReason::Pinned
    } else if best.poor_location != other.poor_location {
        KeepReason::BetterLocation
    } else if best.copy_name != other.copy_name {
        KeepReason::OriginalName
    } else if best.modified != other.modified {
        KeepReason::Oldest
    } else if best.path_len != other.path_len {
        KeepReason::ShortestPath
    } else {
        KeepReason::NameOrder
    }
}

/// The suggested keeper of one group and the copies that go with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub keeper: u32,
    /// Every other copy that may be moved, in group order. A protected copy
    /// is never listed.
    pub moves: Vec<u32>,
    pub reason: KeepReason,
}

/// Picks the keeper. `None` only for a group with no files.
pub fn suggest(group: &DuplicateGroup, prefs: &KeepPreferences) -> Option<Suggestion> {
    let mut ranked: Vec<_> = group
        .files
        .iter()
        .map(|file| (file, key_of(file, prefs)))
        .collect();
    ranked.sort_by(|a, b| a.1.cmp(&b.1));
    let (best, best_key) = ranked.first()?;
    // A group has at least two files; a missing runner-up just falls back to
    // the last tie-break.
    let reason = ranked.get(1).map_or(KeepReason::NameOrder, |(_, key)| {
        first_difference(best_key, key)
    });
    Some(Suggestion {
        keeper: best.id,
        moves: movable_except(group, best.id),
        reason,
    })
}

fn movable_except(group: &DuplicateGroup, keeper: u32) -> Vec<u32> {
    group
        .files
        .iter()
        .filter(|f| !f.protected && f.id != keeper)
        .map(|f| f.id)
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionError {
    /// The id is not a member of the group.
    UnknownFile(u32),
    /// The file is under a protected folder.
    ProtectedFile(u32),
    /// The selection would move every copy.
    LastCopy,
}

/// Moves for a keeper the user chose instead of the suggestion.
pub fn moves_keeping(group: &DuplicateGroup, keeper: u32) -> Result<Vec<u32>, SelectionError> {
    if group.files.iter().any(|f| f.id == keeper) {
        Ok(movable_except(group, keeper))
    } else {
        Err(SelectionError::UnknownFile(keeper))
    }
}

/// A checked choice of files to move from one group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// Distinct, in group order. Never contains a protected file.
    pub moves: Vec<u32>,
    /// What stays. Never empty.
    pub kept: Vec<u32>,
}

/// The last-copy rule: a selection is valid only if every id belongs to the
/// group, none is protected, and at least one copy stays. Repeated ids count
/// once.
pub fn check_selection(
    group: &DuplicateGroup,
    move_ids: &[u32],
) -> Result<Selection, SelectionError> {
    let mut wanted = HashSet::new();
    for &id in move_ids {
        let file = group
            .files
            .iter()
            .find(|f| f.id == id)
            .ok_or(SelectionError::UnknownFile(id))?;
        if file.protected {
            return Err(SelectionError::ProtectedFile(id));
        }
        wanted.insert(id);
    }
    let (moves, kept): (Vec<u32>, Vec<u32>) = group
        .files
        .iter()
        .map(|f| f.id)
        .partition(|id| wanted.contains(id));
    if kept.is_empty() {
        Err(SelectionError::LastCopy)
    } else {
        Ok(Selection { moves, kept })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/Users/me";

    fn file(id: u32, path: &str, modified_secs: u64) -> DuplicateFile {
        DuplicateFile {
            id,
            path: PathBuf::from(path),
            modified_secs,
            protected: false,
        }
    }

    fn protected(id: u32, path: &str, modified_secs: u64) -> DuplicateFile {
        DuplicateFile {
            protected: true,
            ..file(id, path, modified_secs)
        }
    }

    fn group(files: Vec<DuplicateFile>) -> DuplicateGroup {
        DuplicateGroup {
            id: 1,
            size: 1024,
            files,
        }
    }

    fn prefs() -> KeepPreferences {
        KeepPreferences::new(Some(Path::new(HOME)), &[])
    }

    fn keeper(files: Vec<DuplicateFile>) -> u32 {
        suggest(&group(files), &prefs()).unwrap().keeper
    }

    // --- copy-style names ---

    #[test]
    fn recognises_copy_style_names() {
        for name in [
            "report (1)",
            "report (12)",
            "report copy",
            "report copy 2",
            "report - copy",
            "report - copy (2)",
            "copy of report",
            "report copy  ",
        ] {
            assert!(is_copy_name(name), "{name:?} should be a copy name");
        }
    }

    #[test]
    fn ordinary_names_are_not_copy_names() {
        for name in [
            "report",
            "report 2",
            "report (2024)",
            "report ()",
            "report(1)",
            "copy",
            "copyright",
            "photocopy",
            "report copy final",
            "",
        ] {
            assert!(!is_copy_name(name), "{name:?} should not be a copy name");
        }
    }

    // --- ranking ---

    #[test]
    fn a_protected_copy_outranks_everything() {
        let files = vec![
            file(1, "/Users/me/Documents/a.pdf", 10),
            protected(2, "/Users/me/Downloads/a (1).pdf", 999),
        ];
        assert_eq!(keeper(files), 2);
    }

    #[test]
    fn a_pinned_folder_outranks_location_name_and_age() {
        let pinned =
            KeepPreferences::new(Some(Path::new(HOME)), &[PathBuf::from("/Users/me/Desktop")]);
        let g = group(vec![
            file(1, "/Users/me/Documents/a.pdf", 10),
            file(2, "/Users/me/Desktop/a (1).pdf", 999),
        ]);
        let s = suggest(&g, &pinned).unwrap();
        assert_eq!((s.keeper, s.reason), (2, KeepReason::Pinned));
    }

    #[test]
    fn a_pinned_folder_does_not_override_a_protected_copy() {
        let pinned = KeepPreferences::new(None, &[PathBuf::from("/Users/me/Desktop")]);
        let g = group(vec![
            file(1, "/Users/me/Desktop/a.pdf", 10),
            protected(2, "/Users/me/Documents/a.pdf", 10),
        ]);
        let s = suggest(&g, &pinned).unwrap();
        assert_eq!((s.keeper, s.moves), (2, vec![1]));
    }

    #[test]
    fn pin_matches_whole_folders_not_name_prefixes() {
        let pinned = KeepPreferences::new(None, &[PathBuf::from("/Users/me/Photos")]);
        let g = group(vec![
            file(1, "/Users/me/Photos-old/a.jpg", 10),
            file(2, "/Users/me/Photos/a.jpg", 50),
        ]);
        assert_eq!(suggest(&g, &pinned).unwrap().keeper, 2);
    }

    #[test]
    fn downloads_desktop_and_trash_lose_to_a_real_location() {
        for poor in [
            "/Users/me/Downloads/a.pdf",
            "/Users/me/Desktop/a.pdf",
            "/Users/me/.Trash/a.pdf",
            "/Volumes/Disk/.Trashes/501/a.pdf",
            "/Users/me/downloads/a.pdf",
        ] {
            // The poor copy is older and has the shorter path, so only the
            // location rule can make the other one win.
            let g = group(vec![
                file(1, poor, 1),
                file(2, "/Users/me/Documents/projects/a.pdf", 500),
            ]);
            let s = suggest(&g, &prefs()).unwrap();
            assert_eq!(s.keeper, 2, "{poor}");
            assert_eq!(s.reason, KeepReason::BetterLocation, "{poor}");
        }
    }

    #[test]
    fn without_a_home_folder_only_the_trash_is_a_poor_location() {
        let none = KeepPreferences::new(None, &[]);
        let g = group(vec![
            file(1, "/Users/me/Downloads/a.pdf", 1),
            file(2, "/Users/me/Documents/projects/a.pdf", 500),
        ]);
        assert_eq!(suggest(&g, &none).unwrap().keeper, 1);
    }

    #[test]
    fn nested_downloads_folder_counts_but_a_sibling_name_does_not() {
        let g = group(vec![
            file(1, "/Users/me/Downloads/old/a.pdf", 1),
            file(2, "/Users/me/Downloads-archive/a.pdf", 500),
        ]);
        assert_eq!(suggest(&g, &prefs()).unwrap().keeper, 2);
    }

    #[test]
    fn location_beats_a_copy_style_name() {
        let g = group(vec![
            file(1, "/Users/me/Downloads/report.pdf", 1),
            file(2, "/Users/me/Documents/report (1).pdf", 500),
        ]);
        assert_eq!(suggest(&g, &prefs()).unwrap().keeper, 2);
    }

    #[test]
    fn within_one_folder_the_original_name_wins() {
        let g = group(vec![
            file(1, "/Users/me/Documents/report (1).pdf", 1),
            file(2, "/Users/me/Documents/report.pdf", 500),
        ]);
        let s = suggest(&g, &prefs()).unwrap();
        assert_eq!((s.keeper, s.reason), (2, KeepReason::OriginalName));
    }

    #[test]
    fn the_oldest_copy_wins_after_location_and_name() {
        let g = group(vec![
            file(1, "/Users/me/Documents/a.pdf", 500),
            file(2, "/Users/me/Documents/b.pdf", 100),
        ]);
        let s = suggest(&g, &prefs()).unwrap();
        assert_eq!((s.keeper, s.reason), (2, KeepReason::Oldest));
    }

    #[test]
    fn an_unknown_modification_time_counts_as_newest() {
        let g = group(vec![
            file(1, "/Users/me/Documents/a.pdf", 0),
            file(2, "/Users/me/Documents/b.pdf", 100),
        ]);
        assert_eq!(suggest(&g, &prefs()).unwrap().keeper, 2);
    }

    #[test]
    fn equal_age_prefers_the_shorter_path() {
        let g = group(vec![
            file(1, "/Users/me/Documents/deeper/folder/a.pdf", 100),
            file(2, "/Users/me/Documents/a.pdf", 100),
        ]);
        let s = suggest(&g, &prefs()).unwrap();
        assert_eq!((s.keeper, s.reason), (2, KeepReason::ShortestPath));
    }

    #[test]
    fn a_full_tie_is_broken_by_name_then_path() {
        let g = group(vec![
            file(1, "/Users/me/Documents/b.pdf", 100),
            file(2, "/Users/me/Documents/a.pdf", 100),
        ]);
        let s = suggest(&g, &prefs()).unwrap();
        assert_eq!((s.keeper, s.reason), (2, KeepReason::NameOrder));

        let g = group(vec![
            file(1, "/Users/me/Documents/y/a.pdf", 100),
            file(2, "/Users/me/Documents/x/a.pdf", 100),
        ]);
        assert_eq!(suggest(&g, &prefs()).unwrap().keeper, 2);
    }

    #[test]
    fn the_keeper_does_not_depend_on_the_order_of_the_files() {
        let files = vec![
            file(1, "/Users/me/Downloads/a.pdf", 5),
            file(2, "/Users/me/Documents/a (1).pdf", 7),
            file(3, "/Users/me/Documents/a.pdf", 7),
            file(4, "/Users/me/Documents/x/a.pdf", 7),
            file(5, "/Users/me/.Trash/a.pdf", 1),
        ];
        let expected = suggest(&group(files.clone()), &prefs()).unwrap().keeper;
        assert_eq!(expected, 3);
        for rotation in 0..files.len() {
            let mut rotated = files.clone();
            rotated.rotate_left(rotation);
            assert_eq!(keeper(rotated.clone()), expected);
            rotated.reverse();
            assert_eq!(keeper(rotated), expected);
        }
    }

    // --- suggestion ---

    #[test]
    fn the_suggestion_moves_every_other_copy() {
        let g = group(vec![
            file(1, "/Users/me/Documents/a.pdf", 1),
            file(2, "/Users/me/Downloads/a.pdf", 2),
            file(3, "/Users/me/Desktop/a.pdf", 3),
        ]);
        let s = suggest(&g, &prefs()).unwrap();
        assert_eq!(s.keeper, 1);
        assert_eq!(s.moves, vec![2, 3]);
    }

    #[test]
    fn a_protected_copy_is_never_suggested_for_moving() {
        let g = group(vec![
            protected(1, "/Users/me/Library-copy/a.pdf", 1),
            protected(2, "/Users/me/Archive/a.pdf", 2),
            file(3, "/Users/me/Downloads/a.pdf", 3),
            file(4, "/Users/me/Desktop/a.pdf", 4),
        ]);
        let s = suggest(&g, &prefs()).unwrap();
        assert_eq!(s.keeper, 1);
        assert_eq!(s.moves, vec![3, 4]);
    }

    #[test]
    fn an_empty_group_has_no_suggestion() {
        assert_eq!(suggest(&group(vec![]), &prefs()), None);
    }

    #[test]
    fn every_reason_has_words() {
        for reason in [
            KeepReason::Protected,
            KeepReason::Pinned,
            KeepReason::BetterLocation,
            KeepReason::OriginalName,
            KeepReason::Oldest,
            KeepReason::ShortestPath,
            KeepReason::NameOrder,
        ] {
            assert!(!reason.description().is_empty());
        }
    }

    #[test]
    fn a_suggestion_always_passes_the_last_copy_check() {
        let groups = [
            group(vec![file(1, "/a/x", 1), file(2, "/b/x", 2)]),
            group(vec![
                protected(1, "/p/x", 1),
                file(2, "/b/x", 2),
                file(3, "/c/x", 3),
            ]),
            group(vec![
                file(1, "/Users/me/Downloads/x", 0),
                file(2, "/Users/me/.Trash/x", 0),
                file(3, "/Users/me/Desktop/x", 0),
            ]),
        ];
        for g in &groups {
            let s = suggest(g, &prefs()).unwrap();
            let checked = check_selection(g, &s.moves).unwrap();
            assert!(checked.kept.contains(&s.keeper));
            assert!(!s.moves.contains(&s.keeper));
        }
    }

    // --- user-chosen keeper ---

    #[test]
    fn choosing_a_keeper_moves_the_other_unprotected_copies() {
        let g = group(vec![
            protected(1, "/p/x", 1),
            file(2, "/b/x", 2),
            file(3, "/c/x", 3),
        ]);
        assert_eq!(moves_keeping(&g, 3), Ok(vec![2]));
        assert_eq!(moves_keeping(&g, 1), Ok(vec![2, 3]));
    }

    #[test]
    fn choosing_a_keeper_outside_the_group_is_an_error() {
        let g = group(vec![file(1, "/a/x", 1), file(2, "/b/x", 2)]);
        assert_eq!(moves_keeping(&g, 9), Err(SelectionError::UnknownFile(9)));
    }

    // --- last-copy rule ---

    #[test]
    fn a_selection_that_leaves_a_copy_is_accepted_in_group_order() {
        let g = group(vec![
            file(1, "/a/x", 1),
            file(2, "/b/x", 2),
            file(3, "/c/x", 3),
        ]);
        let checked = check_selection(&g, &[3, 1]).unwrap();
        assert_eq!(checked.moves, vec![1, 3]);
        assert_eq!(checked.kept, vec![2]);
    }

    #[test]
    fn repeated_ids_count_once() {
        let g = group(vec![file(1, "/a/x", 1), file(2, "/b/x", 2)]);
        let checked = check_selection(&g, &[1, 1, 1]).unwrap();
        assert_eq!(checked.moves, vec![1]);
        assert_eq!(checked.kept, vec![2]);
    }

    #[test]
    fn moving_every_copy_is_rejected() {
        let g = group(vec![
            file(1, "/a/x", 1),
            file(2, "/b/x", 2),
            file(3, "/c/x", 3),
        ]);
        assert_eq!(
            check_selection(&g, &[1, 2, 3]),
            Err(SelectionError::LastCopy)
        );
        assert_eq!(
            check_selection(&g, &[3, 2, 1, 2]),
            Err(SelectionError::LastCopy)
        );
    }

    #[test]
    fn moving_both_copies_of_a_pair_is_rejected() {
        let g = group(vec![file(1, "/a/x", 1), file(2, "/b/x", 2)]);
        assert_eq!(check_selection(&g, &[1, 2]), Err(SelectionError::LastCopy));
    }

    #[test]
    fn an_unknown_or_foreign_id_is_rejected() {
        let g = group(vec![file(1, "/a/x", 1), file(2, "/b/x", 2)]);
        assert_eq!(
            check_selection(&g, &[1, 77]),
            Err(SelectionError::UnknownFile(77))
        );
        assert_eq!(
            check_selection(&g, &[0]),
            Err(SelectionError::UnknownFile(0))
        );
    }

    #[test]
    fn a_protected_file_cannot_be_selected() {
        let g = group(vec![protected(1, "/p/x", 1), file(2, "/b/x", 2)]);
        assert_eq!(
            check_selection(&g, &[1]),
            Err(SelectionError::ProtectedFile(1))
        );
    }

    #[test]
    fn an_empty_selection_is_accepted_and_moves_nothing() {
        let g = group(vec![file(1, "/a/x", 1), file(2, "/b/x", 2)]);
        let checked = check_selection(&g, &[]).unwrap();
        assert!(checked.moves.is_empty());
        assert_eq!(checked.kept, vec![1, 2]);
    }

    #[test]
    fn no_subset_of_a_group_can_empty_it_or_move_a_protected_file() {
        let g = group(vec![
            protected(1, "/p/x", 1),
            file(2, "/b/x", 2),
            file(3, "/c/x", 3),
            file(4, "/d/x", 4),
        ]);
        let ids: Vec<u32> = g.files.iter().map(|f| f.id).collect();
        for mask in 0u32..(1 << ids.len()) {
            let chosen: Vec<u32> = ids
                .iter()
                .enumerate()
                .filter(|(i, _)| mask & (1 << i) != 0)
                .map(|(_, id)| *id)
                .collect();
            match check_selection(&g, &chosen) {
                Ok(selection) => {
                    assert!(!selection.kept.is_empty(), "mask {mask:b}");
                    assert!(!selection.moves.contains(&1), "mask {mask:b}");
                    assert_eq!(selection.moves.len() + selection.kept.len(), ids.len());
                }
                Err(err) => {
                    // The protected copy always stays, so the only way to be
                    // rejected is to name it.
                    assert!(chosen.contains(&1), "mask {mask:b} rejected for {err:?}");
                    assert_eq!(err, SelectionError::ProtectedFile(1));
                }
            }
        }
    }
}
