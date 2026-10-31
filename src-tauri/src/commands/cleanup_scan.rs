//! Safe cache cleanup commands: find candidates, preview, then move to Trash.
//!
//! Discovery keeps its results in Rust. The UI receives them for display but
//! later names only candidate ids, so it can never ask for a path it was not
//! shown. A plan is single use and is invalidated by a newer discovery, a
//! newer plan, or a second execute. Execution re-checks every item on disk.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use std::time::SystemTime;

use tauri::{AppHandle, Emitter, Manager, State};

use super::cleanup::{CleanupError, CleanupErrorKind, PlanResponse};
use crate::core::cleanup::{Guardrails, ItemResult, Outcome, PlanItem};
use crate::core::cleanup_discovery::{Candidate, Discovery};
use crate::core::cleanup_log::log_moved;
use crate::core::cleanup_plan::{execute_candidates, plan_candidates};
use crate::core::scan::CancelToken;
use crate::platform::TrashProvider;

pub const PROGRESS_EVENT: &str = "cleanup-progress";

/// Cap on items in one request, matching the treemap cleanup.
pub const MAX_ITEMS_PER_PLAN: usize = super::cleanup::MAX_ITEMS_PER_PLAN;

struct PendingPlan {
    id: u64,
    generation: u64,
    needs_acknowledgement: bool,
    items: Vec<PlanItem>,
}

#[derive(Default)]
pub struct CleanupScanState {
    active: Mutex<Option<CancelToken>>,
    found: Mutex<Vec<Candidate>>,
    pending: Mutex<Option<PendingPlan>>,
    /// Bumped, under the `found` lock, each time a discovery is stored.
    generation: AtomicU64,
    next_plan_id: AtomicU64,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryResponse {
    #[serde(flatten)]
    pub discovery: Discovery,
    pub total_bytes: u64,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteCandidatesResponse {
    pub results: Vec<ItemResult>,
    pub moved: usize,
    pub already_gone: usize,
    pub failed: usize,
    /// Discovered bytes of the items that were moved, not of those already gone.
    pub moved_bytes: u64,
    /// Whether the moved items were written to the cleanup log. A logging
    /// failure never undoes or hides a move; it only sets this to false.
    pub log_saved: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProgressPayload {
    found: usize,
    current_path: String,
}

fn error(kind: CleanupErrorKind, message: &str) -> CleanupError {
    CleanupError::new(kind, message)
}

/// Clears the active-discovery slot on every exit path.
struct ActiveGuard<'a>(&'a Mutex<Option<CancelToken>>);

impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        *self.0.lock().expect("cleanup scan state poisoned") = None;
    }
}

fn register(state: &CleanupScanState) -> Result<CancelToken, CleanupError> {
    let mut active = state.active.lock().expect("cleanup scan state poisoned");
    if active.is_some() {
        return Err(error(
            CleanupErrorKind::DiscoveryRunning,
            "Already looking for items to clean up.",
        ));
    }
    let token = CancelToken::default();
    *active = Some(token.clone());
    Ok(token)
}

/// Stores a finished discovery and drops any plan made from an older one.
pub fn store_discovery(state: &CleanupScanState, discovery: Discovery) -> DiscoveryResponse {
    let mut found = state.found.lock().expect("cleanup scan state poisoned");
    *found = discovery.candidates.clone();
    state.generation.fetch_add(1, Ordering::SeqCst);
    *state.pending.lock().expect("cleanup scan state poisoned") = None;
    drop(found);
    let total_bytes = discovery.total_bytes();
    DiscoveryResponse {
        discovery,
        total_bytes,
    }
}

pub fn make_plan(
    state: &CleanupScanState,
    guardrails: &Guardrails,
    ids: &[u32],
) -> Result<PlanResponse, CleanupError> {
    if ids.len() > MAX_ITEMS_PER_PLAN {
        return Err(error(
            CleanupErrorKind::TooMany,
            &format!("Select at most {MAX_ITEMS_PER_PLAN} items at a time."),
        ));
    }
    let found = state.found.lock().expect("cleanup scan state poisoned");
    if found.is_empty() {
        return Err(error(
            CleanupErrorKind::NoDiscovery,
            "Nothing has been found yet. Find items to clean up first.",
        ));
    }
    let generation = state.generation.load(Ordering::SeqCst);
    let plan = plan_candidates(&found, ids, guardrails).map_err(|e| {
        error(
            CleanupErrorKind::UnknownCandidate,
            &format!("Item {} is not in the current list. Find items again.", e.0),
        )
    })?;
    drop(found);

    let needs_acknowledgement = plan.needs_acknowledgement();
    let plan_id = state.next_plan_id.fetch_add(1, Ordering::SeqCst) + 1;
    *state.pending.lock().expect("cleanup scan state poisoned") = Some(PendingPlan {
        id: plan_id,
        generation,
        needs_acknowledgement,
        items: plan.items.clone(),
    });
    Ok(PlanResponse {
        plan_id,
        plan,
        needs_acknowledgement,
    })
}

pub fn run_plan(
    state: &CleanupScanState,
    guardrails: &Guardrails,
    provider: &dyn TrashProvider,
    plan_id: u64,
    acknowledged: bool,
) -> Result<ExecuteCandidatesResponse, CleanupError> {
    if state
        .active
        .lock()
        .expect("cleanup scan state poisoned")
        .is_some()
    {
        return Err(error(
            CleanupErrorKind::DiscoveryRunning,
            "Wait for the search to finish, or cancel it, before moving items.",
        ));
    }

    // Consumed here whatever happens next, so a plan can never run twice.
    let pending = {
        let mut slot = state.pending.lock().expect("cleanup scan state poisoned");
        match slot.as_ref() {
            Some(plan) if plan.id == plan_id => slot.take(),
            _ => None,
        }
    }
    .ok_or_else(|| {
        error(
            CleanupErrorKind::StalePlan,
            "This preview is out of date. Preview the selection again.",
        )
    })?;

    if pending.generation != state.generation.load(Ordering::SeqCst) {
        return Err(error(
            CleanupErrorKind::StalePlan,
            "The list was refreshed after this preview. Preview the selection again.",
        ));
    }
    if pending.needs_acknowledgement && !acknowledged {
        *state.pending.lock().expect("cleanup scan state poisoned") = Some(pending);
        return Err(error(
            CleanupErrorKind::NeedsAcknowledgement,
            "Confirm that you want to move items from outside your home folder.",
        ));
    }

    let candidates = state
        .found
        .lock()
        .expect("cleanup scan state poisoned")
        .clone();
    let results = execute_candidates(&candidates, &pending.items, guardrails, provider);

    // Items that left the disk can't be offered again. Failed ones stay.
    let gone: Vec<u32> = results
        .iter()
        .filter(|r| r.outcome != Outcome::Failed)
        .map(|r| r.node)
        .collect();
    if pending.generation == state.generation.load(Ordering::SeqCst) {
        state
            .found
            .lock()
            .expect("cleanup scan state poisoned")
            .retain(|c| !gone.contains(&c.id));
    }

    let count = |outcome: Outcome| results.iter().filter(|r| r.outcome == outcome).count();
    Ok(ExecuteCandidatesResponse {
        moved: count(Outcome::Moved),
        already_gone: count(Outcome::AlreadyGone),
        failed: count(Outcome::Failed),
        moved_bytes: results
            .iter()
            .filter(|r| r.outcome == Outcome::Moved)
            .map(|r| r.size)
            .sum(),
        results,
        log_saved: false,
    })
}

#[cfg(target_os = "macos")]
fn run_discovery(
    app: &AppHandle,
    token: &CancelToken,
) -> Result<Discovery, crate::core::cleanup_discovery::Cancelled> {
    use crate::core::cleanup_catalog::RECENT_WINDOW;
    use crate::core::cleanup_discovery::{DiscoveryContext, discover};
    use crate::platform::macos::{MacScanner, MacSizeAccounting, catalog_roots, name_rules};
    use std::path::PathBuf;
    use std::time::SystemTime;

    let Some(home) = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .and_then(|p| p.canonicalize().ok())
    else {
        return Ok(Discovery::default());
    };
    let roots = catalog_roots(&home);
    let rules = name_rules(&[&app.config().identifier, &app.package_info().name]);
    let scanner = MacScanner::new();
    let sizing = MacSizeAccounting::new();
    let ctx = DiscoveryContext {
        roots: &roots,
        rules: &rules,
        now: SystemTime::now(),
        recent_window: RECENT_WINDOW,
        scanner: &scanner,
        sizing: &sizing,
    };
    discover(&ctx, token, &|progress| {
        let _ = app.emit(
            PROGRESS_EVENT,
            ProgressPayload {
                found: progress.found,
                current_path: progress.current.to_string_lossy().into_owned(),
            },
        );
    })
}

#[cfg(target_os = "windows")]
fn run_discovery(
    app: &AppHandle,
    token: &CancelToken,
) -> Result<Discovery, crate::core::cleanup_discovery::Cancelled> {
    use crate::core::cleanup_catalog::RECENT_WINDOW;
    use crate::core::cleanup_discovery::{DiscoveryContext, discover};
    use crate::platform::windows::{WinScanner, WinSizeAccounting, catalog_roots, name_rules};
    use std::path::PathBuf;
    use std::time::SystemTime;

    let Some(home) = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .and_then(|p| p.canonicalize().ok())
    else {
        return Ok(Discovery::default());
    };
    let roots = catalog_roots(&home);
    let rules = name_rules(&[&app.config().identifier, &app.package_info().name]);
    let scanner = WinScanner::new();
    let sizing = WinSizeAccounting::new();
    let ctx = DiscoveryContext {
        roots: &roots,
        rules: &rules,
        now: SystemTime::now(),
        recent_window: RECENT_WINDOW,
        scanner: &scanner,
        sizing: &sizing,
    };
    discover(&ctx, token, &|progress| {
        let _ = app.emit(
            PROGRESS_EVENT,
            ProgressPayload {
                found: progress.found,
                current_path: progress.current.to_string_lossy().into_owned(),
            },
        );
    })
}

#[tauri::command]
pub async fn discover_cleanup(
    app: AppHandle,
    state: State<'_, CleanupScanState>,
) -> Result<DiscoveryResponse, CleanupError> {
    let token = register(&state)?;
    let _guard = ActiveGuard(&state.active);

    let discovery = tauri::async_runtime::spawn_blocking(move || run_discovery(&app, &token))
        .await
        .map_err(|e| error(CleanupErrorKind::Other, &e.to_string()))?
        .map_err(|_| error(CleanupErrorKind::Cancelled, "Search cancelled."))?;
    Ok(store_discovery(&state, discovery))
}

#[tauri::command]
pub fn cancel_discovery(state: State<'_, CleanupScanState>) {
    if let Some(token) = state
        .active
        .lock()
        .expect("cleanup scan state poisoned")
        .as_ref()
    {
        token.cancel();
    }
}

#[tauri::command(async)]
pub fn plan_cleanup_candidates(
    app: AppHandle,
    state: State<'_, CleanupScanState>,
    ids: Vec<u32>,
) -> Result<PlanResponse, CleanupError> {
    make_plan(&state, &super::cleanup::make_guardrails(&app), &ids)
}

/// File name of the cleanup log inside the app's data directory.
pub(super) const LOG_FILE: &str = "cleanup-log.jsonl";

/// Records the moved items. Returns whether it worked; the caller reports a
/// failure to the user but does not treat it as a failed cleanup.
fn write_log(app: &AppHandle, response: &ExecuteCandidatesResponse) -> bool {
    let Ok(dir) = app.path().app_data_dir() else {
        return false;
    };
    log_moved(&dir.join(LOG_FILE), &response.results, SystemTime::now()).is_ok()
}

#[tauri::command(async)]
pub fn execute_cleanup_candidates(
    app: AppHandle,
    state: State<'_, CleanupScanState>,
    plan_id: u64,
    acknowledged: bool,
) -> Result<ExecuteCandidatesResponse, CleanupError> {
    let mut response = run_plan(
        &state,
        &super::cleanup::make_guardrails(&app),
        &super::cleanup::make_provider(),
        plan_id,
        acknowledged,
    )?;
    response.log_saved = response.moved == 0 || write_log(&app, &response);
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::cleanup::{ItemKind, Protected, ReasonKind};
    use crate::core::cleanup_catalog::Category;
    use std::fs;
    use std::io::{self, ErrorKind};
    use std::path::{Path, PathBuf};
    use std::time::UNIX_EPOCH;

    #[derive(Default)]
    struct FakeTrash {
        moved: Mutex<Vec<PathBuf>>,
    }

    impl TrashProvider for FakeTrash {
        fn move_to_trash(&self, path: &Path) -> io::Result<()> {
            if path.file_name().is_some_and(|n| n == "locked") {
                return Err(io::Error::new(ErrorKind::PermissionDenied, "locked"));
            }
            self.moved.lock().unwrap().push(path.to_path_buf());
            Ok(())
        }
    }

    struct World {
        root: PathBuf,
        state: CleanupScanState,
    }

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

    /// root/{a 4000, b 6000, locked 1000}, all plain files, stored as the
    /// result of a discovery.
    fn world(name: &str) -> World {
        let root = std::env::temp_dir().join(format!("ctd-ccmd-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let mut candidates = Vec::new();
        for (index, (file, size)) in [("a", 4000), ("b", 6000), ("locked", 1000)]
            .into_iter()
            .enumerate()
        {
            fs::write(root.join(file), b"x").unwrap();
            candidates.push(candidate(
                u32::try_from(index).unwrap() + 1,
                root.join(file),
                size,
            ));
        }
        let state = CleanupScanState::default();
        store_discovery(
            &state,
            Discovery {
                candidates,
                skipped: Vec::new(),
                excluded: 0,
            },
        );
        World { root, state }
    }

    fn open(root: &Path) -> Guardrails {
        Guardrails::new(Some(root.to_path_buf()), Vec::new())
    }

    fn run(w: &World, id: u64, ack: bool) -> Result<ExecuteCandidatesResponse, CleanupError> {
        run_plan(&w.state, &open(&w.root), &FakeTrash::default(), id, ack)
    }

    fn kind<T: std::fmt::Debug>(r: Result<T, CleanupError>) -> CleanupErrorKind {
        r.unwrap_err().kind
    }

    const A: u32 = 1;
    const B: u32 = 2;
    const LOCKED: u32 = 3;

    #[test]
    fn plan_then_execute_moves_items_and_removes_them_from_the_list() {
        let w = world("happy");
        let plan = make_plan(&w.state, &open(&w.root), &[A, B]).unwrap();
        assert_eq!(plan.plan.total_bytes, 10_000);
        let done = run(&w, plan.plan_id, false).unwrap();
        assert_eq!((done.moved, done.failed, done.already_gone), (2, 0, 0));
        assert_eq!(done.moved_bytes, 10_000);

        let left: Vec<u32> = w.state.found.lock().unwrap().iter().map(|c| c.id).collect();
        assert_eq!(left, [LOCKED]);
        assert_eq!(
            kind(make_plan(&w.state, &open(&w.root), &[A])),
            CleanupErrorKind::UnknownCandidate,
            "a moved item can't be planned again"
        );
    }

    #[test]
    fn planning_moves_nothing() {
        let w = world("dry");
        make_plan(&w.state, &open(&w.root), &[A]).unwrap();
        assert!(w.root.join("a").exists());
        assert_eq!(w.state.found.lock().unwrap().len(), 3);
    }

    #[test]
    fn a_plan_can_only_run_once() {
        let w = world("once");
        let plan = make_plan(&w.state, &open(&w.root), &[A]).unwrap();
        run(&w, plan.plan_id, false).unwrap();
        assert_eq!(
            kind(run(&w, plan.plan_id, false)),
            CleanupErrorKind::StalePlan
        );
    }

    #[test]
    fn a_wrong_plan_id_is_rejected_and_keeps_the_real_plan() {
        let w = world("wrong-id");
        let plan = make_plan(&w.state, &open(&w.root), &[A]).unwrap();
        assert_eq!(
            kind(run(&w, plan.plan_id + 5, false)),
            CleanupErrorKind::StalePlan
        );
        assert!(run(&w, plan.plan_id, false).is_ok());
    }

    #[test]
    fn a_newer_plan_replaces_the_older_one() {
        let w = world("replace");
        let first = make_plan(&w.state, &open(&w.root), &[A]).unwrap();
        let second = make_plan(&w.state, &open(&w.root), &[B]).unwrap();
        assert_eq!(
            kind(run(&w, first.plan_id, false)),
            CleanupErrorKind::StalePlan
        );
        assert!(run(&w, second.plan_id, false).is_ok());
    }

    #[test]
    fn a_new_discovery_invalidates_the_plan() {
        let w = world("rediscover");
        let plan = make_plan(&w.state, &open(&w.root), &[A]).unwrap();
        let again = w.state.found.lock().unwrap().clone();
        store_discovery(
            &w.state,
            Discovery {
                candidates: again,
                skipped: Vec::new(),
                excluded: 0,
            },
        );
        assert_eq!(
            kind(run(&w, plan.plan_id, false)),
            CleanupErrorKind::StalePlan
        );
        assert!(w.root.join("a").exists());
    }

    #[test]
    fn execution_is_refused_while_a_search_runs() {
        let w = world("searching");
        let plan = make_plan(&w.state, &open(&w.root), &[A]).unwrap();
        let token = register(&w.state).unwrap();
        assert_eq!(
            kind(run(&w, plan.plan_id, false)),
            CleanupErrorKind::DiscoveryRunning
        );
        drop(token);
        drop(ActiveGuard(&w.state.active));
        assert!(run(&w, plan.plan_id, false).is_ok());
    }

    #[test]
    fn a_second_search_is_rejected_until_the_first_ends() {
        let state = CleanupScanState::default();
        let _token = register(&state).unwrap();
        assert!(register(&state).is_err());
        drop(ActiveGuard(&state.active));
        assert!(register(&state).is_ok());
    }

    #[test]
    fn outside_home_items_need_acknowledgement_and_the_plan_survives_the_refusal() {
        let w = world("ack");
        let elsewhere = Guardrails::new(Some("/nowhere".into()), Vec::new());
        let plan = make_plan(&w.state, &elsewhere, &[A]).unwrap();
        assert!(plan.needs_acknowledgement);

        let refused = run_plan(
            &w.state,
            &elsewhere,
            &FakeTrash::default(),
            plan.plan_id,
            false,
        );
        assert_eq!(kind(refused), CleanupErrorKind::NeedsAcknowledgement);
        assert!(w.root.join("a").exists());

        let accepted = run_plan(
            &w.state,
            &elsewhere,
            &FakeTrash::default(),
            plan.plan_id,
            true,
        );
        assert_eq!(accepted.unwrap().moved, 1);
    }

    #[test]
    fn a_failed_item_is_reported_and_stays_in_the_list() {
        let w = world("partial");
        let plan = make_plan(&w.state, &open(&w.root), &[A, LOCKED]).unwrap();
        let done = run(&w, plan.plan_id, false).unwrap();
        assert_eq!((done.moved, done.failed), (1, 1));
        assert_eq!(done.moved_bytes, 4000);
        let failed = done
            .results
            .iter()
            .find(|r| r.outcome == Outcome::Failed)
            .unwrap();
        assert_eq!(failed.message.as_deref(), Some("locked"));
        let left: Vec<u32> = w.state.found.lock().unwrap().iter().map(|c| c.id).collect();
        assert!(left.contains(&LOCKED) && !left.contains(&A));
    }

    #[test]
    fn protected_items_never_reach_the_plan() {
        let w = world("blocked");
        let guardrails = Guardrails::new(
            Some(w.root.clone()),
            vec![Protected::tree(w.root.join("a"), ReasonKind::AppData)],
        );
        let plan = make_plan(&w.state, &guardrails, &[A, B]).unwrap();
        assert_eq!((plan.plan.items.len(), plan.plan.excluded.len()), (1, 1));
        run_plan(
            &w.state,
            &guardrails,
            &FakeTrash::default(),
            plan.plan_id,
            false,
        )
        .unwrap();
        assert!(w.root.join("a").exists());
    }

    #[test]
    fn a_file_changed_after_the_preview_is_left_alone() {
        let w = world("late");
        let plan = make_plan(&w.state, &open(&w.root), &[A, B]).unwrap();
        w.state.found.lock().unwrap()[0].modified_secs -= 9999;
        let trash = FakeTrash::default();
        let done = run_plan(&w.state, &open(&w.root), &trash, plan.plan_id, false).unwrap();
        assert_eq!((done.moved, done.failed), (1, 1));
        assert_eq!(trash.moved.lock().unwrap().len(), 1);
    }

    #[test]
    fn errors_for_no_discovery_unknown_ids_and_oversized_requests() {
        let empty = CleanupScanState::default();
        let g = open(Path::new("/"));
        assert_eq!(
            kind(make_plan(&empty, &g, &[1])),
            CleanupErrorKind::NoDiscovery
        );
        let w = world("errors");
        assert_eq!(
            kind(make_plan(&w.state, &open(&w.root), &[9999])),
            CleanupErrorKind::UnknownCandidate
        );
        let many = vec![A; MAX_ITEMS_PER_PLAN + 1];
        assert_eq!(
            kind(make_plan(&w.state, &open(&w.root), &many)),
            CleanupErrorKind::TooMany
        );
    }

    #[test]
    fn responses_use_camel_case() {
        let w = world("json");
        // Clone first: a guard held in the argument would still be alive when
        // `store_discovery` takes the same lock, and the test would deadlock.
        let candidates = w.state.found.lock().unwrap().clone();
        let found = serde_json::to_value(store_discovery(
            &w.state,
            Discovery {
                candidates,
                skipped: Vec::new(),
                excluded: 2,
            },
        ))
        .unwrap();
        assert_eq!(found["totalBytes"], 11_000);
        assert_eq!(found["excluded"], 2);
        assert_eq!(found["candidates"][0]["category"], "appCaches");

        let plan = make_plan(&w.state, &open(&w.root), &[A]).unwrap();
        let json = serde_json::to_value(&plan).unwrap();
        assert!(json["planId"].is_number());
        assert_eq!(json["totalBytes"], 4000);
        let done = serde_json::to_value(run(&w, plan.plan_id, false).unwrap()).unwrap();
        assert_eq!(done["results"][0]["outcome"], "moved");
        assert_eq!(done["movedBytes"], 4000);
        assert_eq!(
            serde_json::to_value(CleanupErrorKind::NoDiscovery).unwrap(),
            "noDiscovery"
        );
    }
}
