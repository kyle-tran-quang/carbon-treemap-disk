//! Cleanup commands: preview what would be moved, then move it to the Trash.
//!
//! `plan_cleanup` is a dry run and stores the plan in Rust. `execute_cleanup`
//! only accepts the id of that stored plan, so the UI cannot ask for a path
//! the user never saw. A plan is single use and is tied to the scan it was
//! made from: a rescan, a newer plan, or a second execute all invalidate it.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use super::scan::ScanState;
use crate::core::cleanup::{
    CleanupPlan, Guardrails, ItemResult, Outcome, PlanItem, apply_results, execute, plan_cleanup,
};
use crate::core::tree::NodeId;
use crate::platform::TrashProvider;

/// Cap on items in one request, so a runaway selection cannot stall the app.
pub const MAX_ITEMS_PER_PLAN: usize = 1_000;

struct PendingPlan {
    id: u64,
    generation: u64,
    needs_acknowledgement: bool,
    items: Vec<PlanItem>,
}

#[derive(Default)]
pub struct CleanupState {
    pending: Mutex<Option<PendingPlan>>,
    next_id: AtomicU64,
}

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CleanupErrorKind {
    NoScan,
    UnknownNode,
    TooMany,
    /// No stored plan, or the stored plan is a different one.
    StalePlan,
    NeedsAcknowledgement,
    ScanRunning,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CleanupError {
    pub kind: CleanupErrorKind,
    pub message: String,
}

impl CleanupError {
    fn new(kind: CleanupErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PlanResponse {
    pub plan_id: u64,
    #[serde(flatten)]
    pub plan: CleanupPlan,
    pub needs_acknowledgement: bool,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteResponse {
    pub results: Vec<ItemResult>,
    pub moved: usize,
    pub already_gone: usize,
    pub failed: usize,
    /// Scanned bytes of the items that were moved (not of those already gone).
    pub moved_bytes: u64,
    /// Whether the in-memory tree was updated. False if a new scan replaced it.
    pub tree_updated: bool,
    pub total_bytes: u64,
    pub file_count: u64,
}

pub fn make_plan(
    scan: &ScanState,
    cleanup: &CleanupState,
    guardrails: &Guardrails,
    nodes: &[u32],
) -> Result<PlanResponse, CleanupError> {
    if nodes.len() > MAX_ITEMS_PER_PLAN {
        return Err(CleanupError::new(
            CleanupErrorKind::TooMany,
            format!("Select at most {MAX_ITEMS_PER_PLAN} items at a time."),
        ));
    }
    let guard = scan.tree.lock().expect("scan state poisoned");
    let tree = guard
        .as_ref()
        .ok_or_else(|| CleanupError::new(CleanupErrorKind::NoScan, "No scan has completed yet"))?;
    let generation = scan.generation.load(Ordering::SeqCst);
    let plan = plan_cleanup(tree, nodes, guardrails).map_err(|e| {
        CleanupError::new(
            CleanupErrorKind::UnknownNode,
            format!(
                "Node {} is not in the current scan. Rescan and try again.",
                e.0
            ),
        )
    })?;
    drop(guard);

    let needs_acknowledgement = plan.needs_acknowledgement();
    let plan_id = cleanup.next_id.fetch_add(1, Ordering::SeqCst) + 1;
    *cleanup.pending.lock().expect("cleanup state poisoned") = Some(PendingPlan {
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
    scan: &ScanState,
    cleanup: &CleanupState,
    guardrails: &Guardrails,
    provider: &dyn TrashProvider,
    plan_id: u64,
    acknowledged: bool,
) -> Result<ExecuteResponse, CleanupError> {
    if scan.active.lock().expect("scan state poisoned").is_some() {
        return Err(CleanupError::new(
            CleanupErrorKind::ScanRunning,
            "Wait for the scan to finish, or cancel it, before moving items.",
        ));
    }

    // Take the plan while holding the lock only briefly. From here on it is
    // consumed, whatever happens, so it can never run twice.
    let pending = {
        let mut slot = cleanup.pending.lock().expect("cleanup state poisoned");
        match slot.as_ref() {
            Some(plan) if plan.id == plan_id => slot.take(),
            _ => None,
        }
    }
    .ok_or_else(|| {
        CleanupError::new(
            CleanupErrorKind::StalePlan,
            "This preview is out of date. Preview the selection again.",
        )
    })?;

    if pending.generation != scan.generation.load(Ordering::SeqCst) {
        return Err(CleanupError::new(
            CleanupErrorKind::StalePlan,
            "The folder was rescanned after this preview. Preview the selection again.",
        ));
    }
    if pending.needs_acknowledgement && !acknowledged {
        // Put the plan back so the UI can retry with the acknowledgement.
        *cleanup.pending.lock().expect("cleanup state poisoned") = Some(pending);
        return Err(CleanupError::new(
            CleanupErrorKind::NeedsAcknowledgement,
            "Confirm that you want to move items from outside your home folder.",
        ));
    }

    let results = execute(&pending.items, guardrails, provider);

    let mut guard = scan.tree.lock().expect("scan state poisoned");
    let tree_updated =
        pending.generation == scan.generation.load(Ordering::SeqCst) && guard.is_some();
    let (total_bytes, file_count) = match guard.as_mut() {
        Some(tree) if tree_updated => {
            apply_results(tree, &results);
            (tree.size(NodeId::ROOT), tree.file_count(NodeId::ROOT))
        }
        _ => (0, 0),
    };
    drop(guard);

    let count = |outcome: Outcome| results.iter().filter(|r| r.outcome == outcome).count();
    Ok(ExecuteResponse {
        moved: count(Outcome::Moved),
        already_gone: count(Outcome::AlreadyGone),
        failed: count(Outcome::Failed),
        moved_bytes: results
            .iter()
            .filter(|r| r.outcome == Outcome::Moved)
            .map(|r| r.size)
            .sum(),
        results,
        tree_updated,
        total_bytes,
        file_count,
    })
}

#[cfg(target_os = "macos")]
fn make_provider() -> impl TrashProvider {
    crate::platform::macos::MacTrashProvider::new()
}

#[cfg(target_os = "macos")]
fn make_guardrails(app: &AppHandle) -> Guardrails {
    use crate::platform::macos::{AppPaths, app_bundle_or_exe, protected_paths};

    let canonical = |path: PathBuf| path.canonicalize().unwrap_or(path);
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .map(canonical);
    let paths = app.path();
    let data_dirs = [
        paths.app_data_dir(),
        paths.app_local_data_dir(),
        paths.app_cache_dir(),
        paths.app_config_dir(),
        paths.app_log_dir(),
    ]
    .into_iter()
    .filter_map(Result::ok)
    .map(canonical)
    .collect();
    let app_paths = AppPaths {
        data_dirs,
        executable: std::env::current_exe()
            .ok()
            .map(|exe| app_bundle_or_exe(&canonical(exe))),
        identifier: app.config().identifier.clone(),
    };
    Guardrails::new(home.clone(), protected_paths(home.as_deref(), &app_paths))
}

#[tauri::command(async)]
pub fn plan_cleanup_items(
    app: AppHandle,
    scan: State<'_, ScanState>,
    cleanup: State<'_, CleanupState>,
    nodes: Vec<u32>,
) -> Result<PlanResponse, CleanupError> {
    make_plan(&scan, &cleanup, &make_guardrails(&app), &nodes)
}

#[tauri::command(async)]
pub fn execute_cleanup(
    app: AppHandle,
    scan: State<'_, ScanState>,
    cleanup: State<'_, CleanupState>,
    plan_id: u64,
    acknowledged: bool,
) -> Result<ExecuteResponse, CleanupError> {
    run_plan(
        &scan,
        &cleanup,
        &make_guardrails(&app),
        &make_provider(),
        plan_id,
        acknowledged,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::cleanup::{Protected, ReasonKind};
    use crate::core::tree::Tree;
    use std::fs;
    use std::io::{self, ErrorKind};
    use std::path::Path;

    const MIB: u64 = 1024 * 1024;

    #[derive(Default)]
    struct FakeTrash {
        moved: Mutex<Vec<PathBuf>>,
    }

    impl TrashProvider for FakeTrash {
        fn move_to_trash(&self, path: &Path) -> io::Result<()> {
            if path.file_name().is_some_and(|n| n == "locked.bin") {
                return Err(io::Error::new(ErrorKind::PermissionDenied, "locked"));
            }
            self.moved.lock().unwrap().push(path.to_path_buf());
            Ok(())
        }
    }

    struct World {
        root: PathBuf,
        scan: ScanState,
        cleanup: CleanupState,
        a: u32,
        b: u32,
        locked: u32,
    }

    /// root/{a.bin 4 MiB, b.bin 6 MiB, locked.bin 1 MiB}
    fn world(name: &str) -> World {
        let root = std::env::temp_dir().join(format!("ctd-cmd-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let mut tree = Tree::new(&root.to_string_lossy());
        let mut ids = Vec::new();
        for (file, mib) in [("a.bin", 4), ("b.bin", 6), ("locked.bin", 1)] {
            fs::write(root.join(file), b"x").unwrap();
            ids.push(tree.add_file(NodeId::ROOT, file, mib * MIB).raw());
        }
        tree.rollup();
        let scan = ScanState::default();
        *scan.tree.lock().unwrap() = Some(tree);
        scan.generation.fetch_add(1, Ordering::SeqCst);
        World {
            root,
            scan,
            cleanup: CleanupState::default(),
            a: ids[0],
            b: ids[1],
            locked: ids[2],
        }
    }

    fn open(root: &Path) -> Guardrails {
        Guardrails::new(Some(root.to_path_buf()), Vec::new())
    }

    fn run(w: &World, id: u64, ack: bool) -> Result<ExecuteResponse, CleanupError> {
        run_plan(
            &w.scan,
            &w.cleanup,
            &open(&w.root),
            &FakeTrash::default(),
            id,
            ack,
        )
    }

    fn kind<T: std::fmt::Debug>(r: Result<T, CleanupError>) -> CleanupErrorKind {
        r.unwrap_err().kind
    }

    #[test]
    fn a_plan_then_execute_moves_items_and_updates_the_tree() {
        let w = world("happy");
        let plan = make_plan(&w.scan, &w.cleanup, &open(&w.root), &[w.a, w.b]).unwrap();
        assert_eq!(plan.plan.total_bytes, 10 * MIB);
        let done = run(&w, plan.plan_id, false).unwrap();
        assert_eq!((done.moved, done.failed, done.already_gone), (2, 0, 0));
        assert_eq!(done.moved_bytes, 10 * MIB);
        assert!(done.tree_updated);
        assert_eq!(done.total_bytes, MIB);
        assert_eq!(done.file_count, 1);
    }

    #[test]
    fn nothing_is_moved_by_planning() {
        let w = world("dry");
        make_plan(&w.scan, &w.cleanup, &open(&w.root), &[w.a]).unwrap();
        assert!(w.root.join("a.bin").exists());
        let tree = w.scan.tree.lock().unwrap();
        assert_eq!(tree.as_ref().unwrap().size(NodeId::ROOT), 11 * MIB);
    }

    #[test]
    fn a_plan_can_only_run_once() {
        let w = world("once");
        let plan = make_plan(&w.scan, &w.cleanup, &open(&w.root), &[w.a]).unwrap();
        run(&w, plan.plan_id, false).unwrap();
        assert_eq!(
            kind(run(&w, plan.plan_id, false)),
            CleanupErrorKind::StalePlan
        );
    }

    #[test]
    fn an_unknown_plan_id_is_rejected_and_keeps_the_real_plan() {
        let w = world("wrong-id");
        let plan = make_plan(&w.scan, &w.cleanup, &open(&w.root), &[w.a]).unwrap();
        assert_eq!(
            kind(run(&w, plan.plan_id + 5, false)),
            CleanupErrorKind::StalePlan
        );
        assert!(
            run(&w, plan.plan_id, false).is_ok(),
            "a wrong id must not consume the plan"
        );
    }

    #[test]
    fn a_newer_plan_replaces_the_older_one() {
        let w = world("replace");
        let first = make_plan(&w.scan, &w.cleanup, &open(&w.root), &[w.a]).unwrap();
        let second = make_plan(&w.scan, &w.cleanup, &open(&w.root), &[w.b]).unwrap();
        assert_ne!(first.plan_id, second.plan_id);
        assert_eq!(
            kind(run(&w, first.plan_id, false)),
            CleanupErrorKind::StalePlan
        );
    }

    #[test]
    fn a_rescan_invalidates_the_plan() {
        let w = world("rescan");
        let plan = make_plan(&w.scan, &w.cleanup, &open(&w.root), &[w.a]).unwrap();
        w.scan.generation.fetch_add(1, Ordering::SeqCst);
        assert_eq!(
            kind(run(&w, plan.plan_id, false)),
            CleanupErrorKind::StalePlan
        );
        assert!(w.root.join("a.bin").exists());
    }

    #[test]
    fn execution_is_refused_while_a_scan_runs() {
        let w = world("scanning");
        let plan = make_plan(&w.scan, &w.cleanup, &open(&w.root), &[w.a]).unwrap();
        *w.scan.active.lock().unwrap() = Some(Default::default());
        assert_eq!(
            kind(run(&w, plan.plan_id, false)),
            CleanupErrorKind::ScanRunning
        );
        *w.scan.active.lock().unwrap() = None;
        assert!(run(&w, plan.plan_id, false).is_ok());
    }

    #[test]
    fn items_outside_home_need_acknowledgement_and_the_plan_survives_the_refusal() {
        let w = world("ack");
        let elsewhere = Guardrails::new(Some("/nowhere".into()), Vec::new());
        let plan = make_plan(&w.scan, &w.cleanup, &elsewhere, &[w.a]).unwrap();
        assert!(plan.needs_acknowledgement);

        let refused = run_plan(
            &w.scan,
            &w.cleanup,
            &elsewhere,
            &FakeTrash::default(),
            plan.plan_id,
            false,
        );
        assert_eq!(kind(refused), CleanupErrorKind::NeedsAcknowledgement);
        assert!(w.root.join("a.bin").exists());

        let accepted = run_plan(
            &w.scan,
            &w.cleanup,
            &elsewhere,
            &FakeTrash::default(),
            plan.plan_id,
            true,
        );
        assert_eq!(accepted.unwrap().moved, 1);
    }

    #[test]
    fn a_failed_item_is_reported_and_stays_in_the_tree() {
        let w = world("partial");
        let plan = make_plan(&w.scan, &w.cleanup, &open(&w.root), &[w.a, w.locked]).unwrap();
        let done = run(&w, plan.plan_id, false).unwrap();
        assert_eq!((done.moved, done.failed), (1, 1));
        assert_eq!(done.moved_bytes, 4 * MIB);
        assert_eq!(done.total_bytes, 7 * MIB);
        let failed = done
            .results
            .iter()
            .find(|r| r.outcome == Outcome::Failed)
            .unwrap();
        assert_eq!(failed.message.as_deref(), Some("locked"));
    }

    #[test]
    fn protected_items_never_reach_the_plan() {
        let w = world("blocked");
        let guardrails = Guardrails::new(
            Some(w.root.clone()),
            vec![Protected::tree(w.root.join("a.bin"), ReasonKind::AppData)],
        );
        let plan = make_plan(&w.scan, &w.cleanup, &guardrails, &[w.a, w.b]).unwrap();
        assert_eq!(plan.plan.items.len(), 1);
        assert_eq!(plan.plan.excluded.len(), 1);
        let done = run_plan(
            &w.scan,
            &w.cleanup,
            &guardrails,
            &FakeTrash::default(),
            plan.plan_id,
            false,
        )
        .unwrap();
        assert!(w.root.join("a.bin").exists());
        assert_eq!(done.moved, 1);
    }

    #[test]
    fn errors_for_missing_scan_unknown_nodes_and_oversized_requests() {
        let empty = ScanState::default();
        let cleanup = CleanupState::default();
        let g = open(Path::new("/"));
        assert_eq!(
            kind(make_plan(&empty, &cleanup, &g, &[1])),
            CleanupErrorKind::NoScan
        );

        let w = world("errors");
        assert_eq!(
            kind(make_plan(&w.scan, &w.cleanup, &open(&w.root), &[9999])),
            CleanupErrorKind::UnknownNode
        );
        let many = vec![w.a; MAX_ITEMS_PER_PLAN + 1];
        assert_eq!(
            kind(make_plan(&w.scan, &w.cleanup, &open(&w.root), &many)),
            CleanupErrorKind::TooMany
        );
    }

    #[test]
    fn responses_use_camel_case() {
        let w = world("json");
        let plan = make_plan(&w.scan, &w.cleanup, &open(&w.root), &[w.a]).unwrap();
        let json = serde_json::to_value(&plan).unwrap();
        assert!(json["planId"].is_number());
        assert_eq!(json["needsAcknowledgement"], false);
        assert_eq!(
            json["totalBytes"],
            4 * MIB,
            "plan fields are flattened into the response"
        );
        let done = serde_json::to_value(run(&w, plan.plan_id, false).unwrap()).unwrap();
        assert_eq!(done["results"][0]["outcome"], "moved");
        assert!(done["treeUpdated"].as_bool().unwrap());
        assert_eq!(
            serde_json::to_value(CleanupErrorKind::NeedsAcknowledgement).unwrap(),
            "needsAcknowledgement"
        );
    }
}
