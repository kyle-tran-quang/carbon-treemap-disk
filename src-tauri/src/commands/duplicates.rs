//! Duplicate finder commands: scan, browse the groups, preview, then move
//! the extra copies to the Trash.
//!
//! The groups stay in Rust. The UI receives them for display, but it only
//! ever names a group id and file ids, so it cannot ask for a path it was not
//! shown. A plan is single use and is invalidated by a newer scan, a newer
//! plan, or a second execute. Execution checks every file on disk again, and
//! the rule that one copy always stays is enforced in `core::duplicate_plan`,
//! not here.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use super::cleanup::{CleanupError, CleanupErrorKind};
use super::scan::ScanIssueDto;
use crate::core::cleanup::{Guardrails, ItemResult, Outcome};
use crate::core::cleanup_log::log_moved;
use crate::core::config::{
    DEFAULT_DUPLICATE_MIN_SIZE_BYTES, MAX_DUPLICATE_GROUPS_PER_PAGE, MAX_DUPLICATE_ISSUES_SHOWN,
    MAX_DUPLICATE_MOVES_PER_PLAN, MIN_DUPLICATE_MIN_SIZE_BYTES,
};
use crate::core::duplicate_exclusions::Exclusions;
use crate::core::duplicate_keep::SelectionError;
use crate::core::duplicate_keep::{KeepPreferences, KeepReason, suggest};
use crate::core::duplicate_plan::{
    DuplicatePlan, Pick, PlanError, execute_duplicates, plan_duplicates, remove_files,
};
use crate::core::duplicates::{
    ContentHasher, DuplicateError, DuplicateGroup, DuplicateOptions, DuplicateProgress,
    DuplicateRequest, DuplicateScan, Stage, find_duplicates,
};
use crate::core::scan::CancelToken;
use crate::platform::{FileWalker, TrashProvider};

pub const PROGRESS_EVENT: &str = "duplicate-progress";

const DEFAULT_GROUPS_PER_PAGE: usize = 50;

struct Found {
    groups: Vec<DuplicateGroup>,
    home: Option<PathBuf>,
}

struct PendingPlan {
    id: u64,
    generation: u64,
    needs_acknowledgement: bool,
    plan: DuplicatePlan,
}

#[derive(Default)]
pub struct DuplicateState {
    active: Mutex<Option<CancelToken>>,
    /// `None` until a scan has finished. A scan that found nothing is
    /// `Some` with no groups, so "no scan yet" and "no duplicates" differ.
    found: Mutex<Option<Found>>,
    pending: Mutex<Option<PendingPlan>>,
    /// Bumped, under the `found` lock, each time a scan is stored.
    generation: AtomicU64,
    next_plan_id: AtomicU64,
}

// --- requests ---

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct ExclusionsRequest {
    pub packages: bool,
    pub node_modules: bool,
    pub git: bool,
    pub library: bool,
    pub trash: bool,
}

impl Default for ExclusionsRequest {
    fn default() -> Self {
        let on = Exclusions::default();
        Self {
            packages: on.packages,
            node_modules: on.node_modules,
            git: on.git,
            library: on.library,
            trash: on.trash,
        }
    }
}

impl From<ExclusionsRequest> for Exclusions {
    fn from(request: ExclusionsRequest) -> Self {
        Self {
            packages: request.packages,
            node_modules: request.node_modules,
            git: request.git,
            library: request.library,
            trash: request.trash,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StartRequest {
    /// The folder to scan. `None` means the home folder.
    pub root: Option<String>,
    /// Folders whose files may be kept but never moved.
    pub protected_roots: Vec<String>,
    /// Raised to the smallest allowed size if lower.
    pub min_size_bytes: Option<u64>,
    pub include_cloud_folders: Option<bool>,
    pub exclusions: Option<ExclusionsRequest>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SortKey {
    /// Most space that could be reclaimed first.
    #[default]
    WastedSpace,
    FileSize,
    CopyCount,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GroupsRequest {
    pub sort: SortKey,
    pub offset: usize,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PickRequest {
    pub group: u32,
    pub files: Vec<u32>,
}

// --- responses ---

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateSummary {
    pub root: String,
    pub protected_roots: Vec<String>,
    pub min_size_bytes: u64,
    pub group_count: usize,
    pub reclaimable_bytes: u64,
    pub files_listed: u64,
    pub bytes_hashed: u64,
    /// Paths folded into another path to the same file.
    pub hard_links_collapsed: usize,
    pub issue_count: usize,
    /// At most `MAX_DUPLICATE_ISSUES_SHOWN`.
    pub issues: Vec<ScanIssueDto>,
    pub skipped_cloud_folders: Vec<String>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FileDto {
    /// Valid only for the scan that produced it.
    pub id: u32,
    pub path: String,
    pub name: String,
    pub folder: String,
    pub modified_secs: u64,
    pub protected: bool,
    pub suggested_keep: bool,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GroupDto {
    /// Valid only for the scan that produced it.
    pub id: u32,
    /// Size of each copy.
    pub size: u64,
    pub reclaimable_bytes: u64,
    pub reclaimable_files: usize,
    pub keep_reason: Option<KeepReason>,
    pub keep_reason_text: Option<&'static str>,
    pub files: Vec<FileDto>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GroupsPage {
    pub groups: Vec<GroupDto>,
    pub total_groups: usize,
    pub offset: usize,
    /// Over all groups, not just this page.
    pub reclaimable_bytes: u64,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PickDto {
    pub group: u32,
    pub files: Vec<u32>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SuggestedPicks {
    pub picks: Vec<PickDto>,
    pub move_count: usize,
    pub bytes: u64,
    /// More could be suggested, but one plan is capped.
    pub truncated: bool,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PlanResponse {
    pub plan_id: u64,
    #[serde(flatten)]
    pub plan: DuplicatePlan,
    pub needs_acknowledgement: bool,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteResponse {
    pub results: Vec<ItemResult>,
    pub moved: usize,
    pub already_gone: usize,
    pub failed: usize,
    pub moved_bytes: u64,
    /// Whether the moved files were written to the cleanup log. A logging
    /// failure never undoes or hides a move; it only sets this to false.
    pub log_saved: bool,
    /// Groups still worth showing after the move.
    pub remaining_groups: usize,
    pub remaining_reclaimable_bytes: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProgressPayload {
    stage: &'static str,
    done: u64,
    /// Zero while the total is not known.
    total: u64,
    bytes: u64,
    current_path: String,
}

impl From<DuplicateProgress> for ProgressPayload {
    fn from(progress: DuplicateProgress) -> Self {
        Self {
            stage: match progress.stage {
                Stage::Listing => "listing",
                Stage::Fingerprinting => "fingerprinting",
                Stage::Hashing => "hashing",
            },
            done: progress.done,
            total: progress.total,
            bytes: progress.bytes,
            current_path: progress.current_path.to_string_lossy().into_owned(),
        }
    }
}

fn error(kind: CleanupErrorKind, message: &str) -> CleanupError {
    CleanupError::new(kind, message)
}

// --- scan ---

/// A validated scan request.
#[derive(Debug)]
pub struct ScanParams {
    pub scan_root: PathBuf,
    pub protected_roots: Vec<PathBuf>,
    pub options: DuplicateOptions,
    pub home: Option<PathBuf>,
}

fn existing_folder(raw: &str) -> Result<PathBuf, CleanupError> {
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return Err(error(
            CleanupErrorKind::InvalidRoot,
            "Choose a folder by its full path.",
        ));
    }
    let canonical = path.canonicalize().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => {
            error(CleanupErrorKind::InvalidRoot, "That folder does not exist.")
        }
        std::io::ErrorKind::PermissionDenied => error(
            CleanupErrorKind::PermissionDenied,
            "This app is not allowed to open that folder.",
        ),
        _ => error(CleanupErrorKind::InvalidRoot, &e.to_string()),
    })?;
    if canonical.is_dir() {
        Ok(canonical)
    } else {
        Err(error(
            CleanupErrorKind::InvalidRoot,
            "That is not a folder.",
        ))
    }
}

/// Checks the request and fills in the defaults. `home` is the user's
/// canonical home folder, if known.
pub fn resolve_params(
    request: StartRequest,
    home: Option<PathBuf>,
) -> Result<ScanParams, CleanupError> {
    let scan_root = match request.root.as_deref() {
        Some(raw) => existing_folder(raw)?,
        None => home.clone().ok_or_else(|| {
            error(
                CleanupErrorKind::InvalidRoot,
                "The home folder could not be found. Choose a folder instead.",
            )
        })?,
    };

    let mut protected_roots: Vec<PathBuf> = Vec::new();
    for raw in &request.protected_roots {
        let folder = existing_folder(raw)?;
        if !protected_roots.contains(&folder) {
            protected_roots.push(folder);
        }
    }
    if protected_roots
        .iter()
        .any(|protected| scan_root.starts_with(protected))
    {
        return Err(error(
            CleanupErrorKind::InvalidRoot,
            "The folder to scan is the protected folder or inside it, so nothing in it could be moved.",
        ));
    }

    let options = DuplicateOptions {
        min_size: request
            .min_size_bytes
            .map_or(DEFAULT_DUPLICATE_MIN_SIZE_BYTES, |size| {
                size.max(MIN_DUPLICATE_MIN_SIZE_BYTES)
            }),
        include_cloud_folders: request.include_cloud_folders.unwrap_or(false),
        exclusions: request.exclusions.unwrap_or_default().into(),
        ..DuplicateOptions::default()
    };
    Ok(ScanParams {
        scan_root,
        protected_roots,
        options,
        home,
    })
}

fn scan_error(failure: DuplicateError) -> CleanupError {
    match failure {
        DuplicateError::Cancelled => error(CleanupErrorKind::Cancelled, "Search cancelled."),
        DuplicateError::RootUnreadable { path, error: cause } => {
            let kind = if cause.kind() == std::io::ErrorKind::PermissionDenied {
                CleanupErrorKind::PermissionDenied
            } else {
                CleanupErrorKind::Other
            };
            error(kind, &format!("Could not read {}: {cause}", path.display()))
        }
    }
}

/// Runs the search. Read-only.
pub fn run_scan(
    walker: &dyn FileWalker,
    hasher: &dyn ContentHasher,
    params: &ScanParams,
    cancel: &CancelToken,
    on_progress: &(dyn Fn(DuplicateProgress) + Sync),
) -> Result<DuplicateScan, CleanupError> {
    let request = DuplicateRequest {
        scan_roots: std::slice::from_ref(&params.scan_root),
        protected_roots: &params.protected_roots,
        options: &params.options,
    };
    find_duplicates(walker, hasher, &request, cancel, on_progress).map_err(scan_error)
}

fn total_reclaimable(groups: &[DuplicateGroup]) -> u64 {
    groups.iter().map(DuplicateGroup::reclaimable_bytes).sum()
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Stores a finished scan and drops any plan made from an older one.
pub fn store_scan(
    state: &DuplicateState,
    scan: DuplicateScan,
    params: &ScanParams,
) -> DuplicateSummary {
    let summary = DuplicateSummary {
        root: path_text(&params.scan_root),
        protected_roots: params
            .protected_roots
            .iter()
            .map(|p| path_text(p))
            .collect(),
        min_size_bytes: params.options.min_size,
        group_count: scan.groups.len(),
        reclaimable_bytes: scan.reclaimable_bytes(),
        files_listed: scan.files_listed,
        bytes_hashed: scan.bytes_hashed,
        hard_links_collapsed: scan.hard_links_collapsed,
        issue_count: scan.issues.len(),
        issues: scan
            .issues
            .iter()
            .take(MAX_DUPLICATE_ISSUES_SHOWN)
            .map(ScanIssueDto::from)
            .collect(),
        skipped_cloud_folders: scan
            .skipped_cloud_folders
            .iter()
            .map(|p| path_text(p))
            .collect(),
    };
    let mut found = state.found.lock().expect("duplicate state poisoned");
    *found = Some(Found {
        groups: scan.groups,
        home: params.home.clone(),
    });
    state.generation.fetch_add(1, Ordering::SeqCst);
    *state.pending.lock().expect("duplicate state poisoned") = None;
    drop(found);
    summary
}

/// Clears the active-scan slot on every exit path.
struct ActiveGuard<'a>(&'a Mutex<Option<CancelToken>>);

impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        *self.0.lock().expect("duplicate state poisoned") = None;
    }
}

fn register(state: &DuplicateState) -> Result<CancelToken, CleanupError> {
    let mut active = state.active.lock().expect("duplicate state poisoned");
    if active.is_some() {
        return Err(error(
            CleanupErrorKind::DuplicateScanRunning,
            "Already looking for duplicates.",
        ));
    }
    let token = CancelToken::default();
    *active = Some(token.clone());
    Ok(token)
}

// --- browsing ---

fn file_dto(file: &crate::core::duplicates::DuplicateFile, keeper: Option<u32>) -> FileDto {
    FileDto {
        id: file.id,
        path: path_text(&file.path),
        name: file
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        folder: file.path.parent().map(path_text).unwrap_or_default(),
        modified_secs: file.modified_secs,
        protected: file.protected,
        suggested_keep: keeper == Some(file.id),
    }
}

fn group_dto(group: &DuplicateGroup, prefs: &KeepPreferences) -> GroupDto {
    let suggestion = suggest(group, prefs);
    let keeper = suggestion.as_ref().map(|s| s.keeper);
    GroupDto {
        id: group.id,
        size: group.size,
        reclaimable_bytes: group.reclaimable_bytes(),
        reclaimable_files: group.reclaimable_files(),
        keep_reason: suggestion.as_ref().map(|s| s.reason),
        keep_reason_text: suggestion.as_ref().map(|s| s.reason.description()),
        files: group.files.iter().map(|f| file_dto(f, keeper)).collect(),
    }
}

fn sorted(groups: &[DuplicateGroup], key: SortKey) -> Vec<&DuplicateGroup> {
    let mut order: Vec<&DuplicateGroup> = groups.iter().collect();
    order.sort_by(|a, b| {
        let primary = match key {
            SortKey::WastedSpace => b.reclaimable_bytes().cmp(&a.reclaimable_bytes()),
            SortKey::FileSize => b.size.cmp(&a.size),
            SortKey::CopyCount => b.files.len().cmp(&a.files.len()),
        };
        primary.then(a.id.cmp(&b.id))
    });
    order
}

fn no_scan() -> CleanupError {
    error(
        CleanupErrorKind::NoDuplicateScan,
        "No duplicate search has finished yet. Find duplicates first.",
    )
}

/// One page of groups, never the whole list.
pub fn groups_page(
    state: &DuplicateState,
    request: &GroupsRequest,
) -> Result<GroupsPage, CleanupError> {
    let found = state.found.lock().expect("duplicate state poisoned");
    let found = found.as_ref().ok_or_else(no_scan)?;
    let limit = request
        .limit
        .unwrap_or(DEFAULT_GROUPS_PER_PAGE)
        .clamp(1, MAX_DUPLICATE_GROUPS_PER_PAGE);
    let prefs = KeepPreferences::new(found.home.as_deref(), &[]);
    let groups = sorted(&found.groups, request.sort)
        .into_iter()
        .skip(request.offset)
        .take(limit)
        .map(|group| group_dto(group, &prefs))
        .collect();
    Ok(GroupsPage {
        groups,
        total_groups: found.groups.len(),
        offset: request.offset,
        reclaimable_bytes: total_reclaimable(&found.groups),
    })
}

/// What "Select suggested duplicates" would pick: the suggested moves of the
/// groups that waste the most space, up to the size of one plan.
pub fn suggested_picks(state: &DuplicateState) -> Result<SuggestedPicks, CleanupError> {
    let found = state.found.lock().expect("duplicate state poisoned");
    let found = found.as_ref().ok_or_else(no_scan)?;
    let prefs = KeepPreferences::new(found.home.as_deref(), &[]);

    let mut picks = Vec::new();
    let mut move_count = 0usize;
    let mut bytes = 0u64;
    let mut truncated = false;
    for group in sorted(&found.groups, SortKey::WastedSpace) {
        let Some(suggestion) = suggest(group, &prefs) else {
            continue;
        };
        let room = MAX_DUPLICATE_MOVES_PER_PLAN - move_count;
        if suggestion.moves.len() > room {
            truncated = true;
        }
        let files: Vec<u32> = suggestion.moves.into_iter().take(room).collect();
        if files.is_empty() {
            if room == 0 {
                break;
            }
            continue;
        }
        move_count += files.len();
        bytes += group.size * files.len() as u64;
        picks.push(PickDto {
            group: group.id,
            files,
        });
        if truncated {
            break;
        }
    }
    Ok(SuggestedPicks {
        picks,
        move_count,
        bytes,
        truncated,
    })
}

// --- plan and execute ---

fn plan_error(failure: PlanError) -> CleanupError {
    match failure {
        PlanError::UnknownGroup(id) => error(
            CleanupErrorKind::UnknownCandidate,
            &format!("Group {id} is not in the current results. Find duplicates again."),
        ),
        PlanError::Selection { group, error: why } => match why {
            SelectionError::UnknownFile(id) => error(
                CleanupErrorKind::UnknownCandidate,
                &format!("File {id} is not in group {group}. Find duplicates again."),
            ),
            SelectionError::ProtectedFile(_) => error(
                CleanupErrorKind::ProtectedFile,
                "A file in a protected folder can be kept but not moved.",
            ),
            SelectionError::LastCopy => error(
                CleanupErrorKind::LastCopy,
                "Keep at least one copy of each file. Deselect one copy in every group.",
            ),
        },
        PlanError::TooManyMoves => too_many(),
    }
}

fn too_many() -> CleanupError {
    error(
        CleanupErrorKind::TooMany,
        &format!("Select at most {MAX_DUPLICATE_MOVES_PER_PLAN} files at a time."),
    )
}

pub fn make_plan(
    state: &DuplicateState,
    guardrails: &Guardrails,
    picks: &[PickRequest],
) -> Result<PlanResponse, CleanupError> {
    if picks.iter().map(|p| p.files.len()).sum::<usize>() > MAX_DUPLICATE_MOVES_PER_PLAN {
        return Err(too_many());
    }
    let picks: Vec<Pick> = picks
        .iter()
        .map(|p| Pick {
            group: p.group,
            files: p.files.clone(),
        })
        .collect();

    let found = state.found.lock().expect("duplicate state poisoned");
    let found = found.as_ref().ok_or_else(no_scan)?;
    let generation = state.generation.load(Ordering::SeqCst);
    let plan = plan_duplicates(&found.groups, &picks, guardrails).map_err(plan_error)?;

    let needs_acknowledgement = plan.needs_acknowledgement();
    let plan_id = state.next_plan_id.fetch_add(1, Ordering::SeqCst) + 1;
    *state.pending.lock().expect("duplicate state poisoned") = Some(PendingPlan {
        id: plan_id,
        generation,
        needs_acknowledgement,
        plan: plan.clone(),
    });
    Ok(PlanResponse {
        plan_id,
        plan,
        needs_acknowledgement,
    })
}

pub fn run_plan(
    state: &DuplicateState,
    guardrails: &Guardrails,
    provider: &dyn TrashProvider,
    plan_id: u64,
    acknowledged: bool,
) -> Result<ExecuteResponse, CleanupError> {
    if state
        .active
        .lock()
        .expect("duplicate state poisoned")
        .is_some()
    {
        return Err(error(
            CleanupErrorKind::DuplicateScanRunning,
            "Wait for the search to finish, or cancel it, before moving files.",
        ));
    }

    // Consumed here whatever happens next, so a plan can never run twice.
    let pending = {
        let mut slot = state.pending.lock().expect("duplicate state poisoned");
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
            "The results were refreshed after this preview. Preview the selection again.",
        ));
    }
    if pending.needs_acknowledgement && !acknowledged {
        *state.pending.lock().expect("duplicate state poisoned") = Some(pending);
        return Err(error(
            CleanupErrorKind::NeedsAcknowledgement,
            "Confirm that you want to move files from outside your home folder.",
        ));
    }

    let groups = state
        .found
        .lock()
        .expect("duplicate state poisoned")
        .as_ref()
        .map(|found| found.groups.clone())
        .ok_or_else(no_scan)?;
    let results = execute_duplicates(&groups, &pending.plan, guardrails, provider);

    // Files that left the disk can't be offered again. Failed ones stay.
    let gone: HashSet<u32> = results
        .iter()
        .filter(|r| r.outcome != Outcome::Failed)
        .map(|r| r.node)
        .collect();
    let (remaining_groups, remaining_reclaimable_bytes) = {
        let mut found = state.found.lock().expect("duplicate state poisoned");
        match found.as_mut() {
            Some(found) if pending.generation == state.generation.load(Ordering::SeqCst) => {
                remove_files(&mut found.groups, &gone);
                (found.groups.len(), total_reclaimable(&found.groups))
            }
            Some(found) => (found.groups.len(), total_reclaimable(&found.groups)),
            None => (0, 0),
        }
    };

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
        log_saved: false,
        remaining_groups,
        remaining_reclaimable_bytes,
    })
}

// --- Tauri glue ---

#[cfg(target_os = "macos")]
fn make_walker() -> impl FileWalker {
    crate::platform::macos::MacFileWalker::new()
}

#[cfg(target_os = "windows")]
fn make_walker() -> impl FileWalker {
    crate::platform::windows::WinFileWalker::new()
}

#[cfg(target_os = "macos")]
fn make_hasher() -> impl ContentHasher {
    crate::platform::macos::MacContentHasher
}

#[cfg(target_os = "windows")]
fn make_hasher() -> impl ContentHasher {
    crate::platform::windows::WinContentHasher
}

#[cfg(target_os = "macos")]
fn home_folder() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .and_then(|p| p.canonicalize().ok())
}

#[cfg(target_os = "windows")]
fn home_folder() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .and_then(|p| p.canonicalize().ok())
}

#[tauri::command]
pub async fn start_duplicate_scan(
    app: AppHandle,
    state: State<'_, DuplicateState>,
    request: StartRequest,
) -> Result<DuplicateSummary, CleanupError> {
    let params = resolve_params(request, home_folder())?;
    let token = register(&state)?;
    let _guard = ActiveGuard(&state.active);

    let (params, scan) = tauri::async_runtime::spawn_blocking(move || {
        let scan = run_scan(
            &make_walker(),
            &make_hasher(),
            &params,
            &token,
            &|progress| {
                let _ = app.emit(PROGRESS_EVENT, ProgressPayload::from(progress));
            },
        )?;
        Ok::<_, CleanupError>((params, scan))
    })
    .await
    .map_err(|e| error(CleanupErrorKind::Other, &e.to_string()))??;
    Ok(store_scan(&state, scan, &params))
}

#[tauri::command]
pub fn cancel_duplicate_scan(state: State<'_, DuplicateState>) {
    if let Some(token) = state
        .active
        .lock()
        .expect("duplicate state poisoned")
        .as_ref()
    {
        token.cancel();
    }
}

#[tauri::command]
pub fn get_duplicate_groups(
    state: State<'_, DuplicateState>,
    request: GroupsRequest,
) -> Result<GroupsPage, CleanupError> {
    groups_page(&state, &request)
}

#[tauri::command]
pub fn suggest_duplicate_picks(
    state: State<'_, DuplicateState>,
) -> Result<SuggestedPicks, CleanupError> {
    suggested_picks(&state)
}

#[tauri::command(async)]
pub fn plan_duplicate_cleanup(
    app: AppHandle,
    state: State<'_, DuplicateState>,
    picks: Vec<PickRequest>,
) -> Result<PlanResponse, CleanupError> {
    make_plan(&state, &super::cleanup::make_guardrails(&app), &picks)
}

/// Records the moved files. Returns whether it worked; the caller reports a
/// failure to the user but does not treat it as a failed cleanup.
fn write_log(app: &AppHandle, results: &[ItemResult]) -> bool {
    let Ok(dir) = app.path().app_data_dir() else {
        return false;
    };
    log_moved(
        &dir.join(super::cleanup_scan::LOG_FILE),
        results,
        SystemTime::now(),
    )
    .is_ok()
}

#[tauri::command(async)]
pub fn execute_duplicate_cleanup(
    app: AppHandle,
    state: State<'_, DuplicateState>,
    plan_id: u64,
    acknowledged: bool,
) -> Result<ExecuteResponse, CleanupError> {
    let mut response = run_plan(
        &state,
        &super::cleanup::make_guardrails(&app),
        &super::cleanup::make_provider(),
        plan_id,
        acknowledged,
    )?;
    response.log_saved = response.moved == 0 || write_log(&app, &response.results);
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::cleanup::ReasonKind;
    use crate::core::duplicates::DuplicateFile;
    use crate::core::scan::{ScanIssue, ScanIssueKind};
    use std::fs;
    use std::io;
    use std::time::UNIX_EPOCH;

    const CONTENT: &[u8] = b"same content";
    const GROUP: u32 = 7;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ctd-dcmd-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn open(root: &Path) -> Guardrails {
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
                return Err(io::Error::other("denied"));
            }
            self.moved.lock().unwrap().push(path.to_path_buf());
            Ok(())
        }
    }

    fn moved(trash: &FakeTrash) -> Vec<PathBuf> {
        trash.moved.lock().unwrap().clone()
    }

    fn store(state: &DuplicateState, groups: Vec<DuplicateGroup>, home: Option<PathBuf>) {
        *state.found.lock().unwrap() = Some(Found { groups, home });
        state.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// A group of `copies` identical files on disk: root/d<i>/doc.bin, ids 1..
    fn real_group(root: &Path, id: u32, copies: u32) -> DuplicateGroup {
        let files = (1..=copies)
            .map(|n| {
                let dir = root.join(format!("g{id}-d{n}"));
                fs::create_dir_all(&dir).unwrap();
                let path = dir.join("doc.bin");
                fs::write(&path, CONTENT).unwrap();
                let modified_secs = fs::metadata(&path)
                    .unwrap()
                    .modified()
                    .unwrap()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                DuplicateFile {
                    id: id * 100 + n,
                    path,
                    modified_secs,
                    protected: false,
                }
            })
            .collect();
        DuplicateGroup {
            id,
            size: CONTENT.len() as u64,
            files,
        }
    }

    struct World {
        root: PathBuf,
        state: DuplicateState,
    }

    impl World {
        fn path(&self, group: u32, file: u32) -> PathBuf {
            let found = self.state.found.lock().unwrap();
            let group = found
                .as_ref()
                .unwrap()
                .groups
                .iter()
                .find(|g| g.id == group)
                .unwrap();
            group
                .files
                .iter()
                .find(|f| f.id == file)
                .unwrap()
                .path
                .clone()
        }

        fn group_count(&self) -> usize {
            self.state
                .found
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .groups
                .len()
        }
    }

    /// One group, id 7, with `copies` files whose ids are 701, 702, ...
    fn world(name: &str, copies: u32) -> World {
        let root = temp_root(name);
        let state = DuplicateState::default();
        store(
            &state,
            vec![real_group(&root, GROUP, copies)],
            Some(root.clone()),
        );
        World { root, state }
    }

    fn pick(files: &[u32]) -> Vec<PickRequest> {
        vec![PickRequest {
            group: GROUP,
            files: files.to_vec(),
        }]
    }

    fn kind(result: Result<impl std::fmt::Debug, CleanupError>) -> CleanupErrorKind {
        result.unwrap_err().kind
    }

    fn bare(id: u32, path: &str, protected: bool) -> DuplicateFile {
        DuplicateFile {
            id,
            path: PathBuf::from(path),
            modified_secs: 1,
            protected,
        }
    }

    fn bare_group(id: u32, size: u64, copies: u32) -> DuplicateGroup {
        DuplicateGroup {
            id,
            size,
            files: (1..=copies)
                .map(|n| bare(id * 100 + n, &format!("/data/g{id}/f{n}.bin"), false))
                .collect(),
        }
    }

    fn params(root: &Path) -> ScanParams {
        ScanParams {
            scan_root: root.to_path_buf(),
            protected_roots: Vec::new(),
            options: DuplicateOptions::default(),
            home: Some(root.to_path_buf()),
        }
    }

    // --- request validation ---

    #[test]
    fn an_empty_request_means_the_home_folder_with_the_defaults() {
        let request: StartRequest = serde_json::from_str("{}").unwrap();
        let home = temp_root("home-default");
        let resolved = resolve_params(request, Some(home.clone())).unwrap();
        assert_eq!(resolved.scan_root, home);
        assert!(resolved.protected_roots.is_empty());
        assert_eq!(resolved.options.min_size, DEFAULT_DUPLICATE_MIN_SIZE_BYTES);
        assert!(!resolved.options.include_cloud_folders);
        assert_eq!(resolved.options.exclusions, Exclusions::default());
    }

    #[test]
    fn a_partial_exclusions_object_keeps_the_other_defaults() {
        let request: StartRequest =
            serde_json::from_str(r#"{"exclusions":{"git":false}}"#).unwrap();
        let exclusions: Exclusions = request.exclusions.unwrap().into();
        assert!(!exclusions.git);
        assert!(exclusions.packages && exclusions.node_modules);
        assert!(exclusions.library && exclusions.trash);
    }

    #[test]
    fn without_a_root_or_a_home_folder_the_request_is_refused() {
        let err = resolve_params(StartRequest::default(), None).unwrap_err();
        assert_eq!(err.kind, CleanupErrorKind::InvalidRoot);
    }

    #[test]
    fn a_chosen_folder_must_be_an_absolute_existing_folder() {
        let dir = temp_root("root-checks");
        let file = dir.join("plain.txt");
        fs::write(&file, b"x").unwrap();
        let with_root = |root: &str| StartRequest {
            root: Some(root.to_owned()),
            ..StartRequest::default()
        };
        let run = |root: &str| resolve_params(with_root(root), None).map(|p| p.scan_root);

        assert_eq!(kind(run("relative/folder")), CleanupErrorKind::InvalidRoot);
        assert_eq!(
            kind(run(&dir.join("missing").to_string_lossy())),
            CleanupErrorKind::InvalidRoot
        );
        assert_eq!(
            kind(run(&file.to_string_lossy())),
            CleanupErrorKind::InvalidRoot
        );
        assert_eq!(run(&dir.to_string_lossy()).unwrap(), dir);
    }

    #[test]
    fn a_symlinked_root_is_resolved_to_the_real_folder() {
        let dir = temp_root("root-link");
        let real = dir.join("real");
        fs::create_dir(&real).unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let request = StartRequest {
            root: Some(link.to_string_lossy().into_owned()),
            ..StartRequest::default()
        };
        assert_eq!(resolve_params(request, None).unwrap().scan_root, real);
    }

    #[test]
    fn the_minimum_size_has_a_floor_and_an_override() {
        let home = temp_root("min-size");
        let with = |size: Option<u64>| StartRequest {
            min_size_bytes: size,
            ..StartRequest::default()
        };
        let min = |size| {
            resolve_params(with(size), Some(home.clone()))
                .unwrap()
                .options
                .min_size
        };
        assert_eq!(min(None), DEFAULT_DUPLICATE_MIN_SIZE_BYTES);
        assert_eq!(min(Some(0)), MIN_DUPLICATE_MIN_SIZE_BYTES);
        assert_eq!(min(Some(1)), MIN_DUPLICATE_MIN_SIZE_BYTES);
        assert_eq!(min(Some(10 * 1024 * 1024)), 10 * 1024 * 1024);
    }

    #[test]
    fn protected_folders_are_resolved_and_listed_once() {
        let root = temp_root("protected-ok");
        let scan = root.join("scan");
        let keep = root.join("keep");
        fs::create_dir_all(&scan).unwrap();
        fs::create_dir_all(&keep).unwrap();
        let text = |p: &Path| p.to_string_lossy().into_owned();
        let request = StartRequest {
            root: Some(text(&scan)),
            protected_roots: vec![text(&keep), text(&keep), text(&keep.join("."))],
            ..StartRequest::default()
        };
        let resolved = resolve_params(request, None).unwrap();
        assert_eq!(resolved.protected_roots, [keep]);
    }

    #[test]
    fn a_protected_folder_that_contains_the_scan_folder_is_refused() {
        let root = temp_root("protected-bad");
        let inner = root.join("inner");
        fs::create_dir_all(&inner).unwrap();
        let text = |p: &Path| p.to_string_lossy().into_owned();
        let request = |scan: &Path, protect: &Path| StartRequest {
            root: Some(text(scan)),
            protected_roots: vec![text(protect)],
            ..StartRequest::default()
        };
        for (scan, protect) in [(&inner, &root), (&root, &root)] {
            let err = resolve_params(request(scan, protect), None).unwrap_err();
            assert_eq!(err.kind, CleanupErrorKind::InvalidRoot);
        }
        // A protected folder inside the scan folder is fine.
        assert!(resolve_params(request(&root, &inner), None).is_ok());
    }

    #[test]
    fn an_unreadable_protected_folder_is_refused_not_ignored() {
        let root = temp_root("protected-missing");
        let request = StartRequest {
            root: Some(root.to_string_lossy().into_owned()),
            protected_roots: vec![root.join("gone").to_string_lossy().into_owned()],
            ..StartRequest::default()
        };
        assert_eq!(
            resolve_params(request, None).unwrap_err().kind,
            CleanupErrorKind::InvalidRoot
        );
    }

    #[test]
    fn engine_failures_map_to_structured_errors() {
        assert_eq!(
            scan_error(DuplicateError::Cancelled).kind,
            CleanupErrorKind::Cancelled
        );
        let unreadable = |kind| {
            scan_error(DuplicateError::RootUnreadable {
                path: "/p".into(),
                error: io::Error::from(kind),
            })
            .kind
        };
        assert_eq!(
            unreadable(io::ErrorKind::PermissionDenied),
            CleanupErrorKind::PermissionDenied
        );
        assert_eq!(unreadable(io::ErrorKind::NotFound), CleanupErrorKind::Other);
    }

    // --- storing a scan ---

    #[test]
    fn storing_a_scan_replaces_the_groups_and_drops_a_pending_plan() {
        let w = world("store", 2);
        let plan = make_plan(&w.state, &open(&w.root), &pick(&[701])).unwrap();
        let before = w.state.generation.load(Ordering::SeqCst);

        let scan = DuplicateScan {
            groups: vec![bare_group(1, 100, 2)],
            files_listed: 9,
            bytes_hashed: 5,
            hard_links_collapsed: 3,
            ..DuplicateScan::default()
        };
        let summary = store_scan(&w.state, scan, &params(&w.root));
        assert_eq!(summary.group_count, 1);
        assert_eq!(summary.reclaimable_bytes, 100);
        assert_eq!(
            (
                summary.files_listed,
                summary.bytes_hashed,
                summary.hard_links_collapsed
            ),
            (9, 5, 3)
        );
        assert!(w.state.generation.load(Ordering::SeqCst) > before);
        assert!(w.state.pending.lock().unwrap().is_none());
        assert_eq!(
            kind(run_plan(
                &w.state,
                &open(&w.root),
                &FakeTrash::default(),
                plan.plan_id,
                true
            )),
            CleanupErrorKind::StalePlan
        );
    }

    #[test]
    fn only_the_first_issues_are_sent_but_all_are_counted() {
        let state = DuplicateState::default();
        let issues = (0..MAX_DUPLICATE_ISSUES_SHOWN + 5)
            .map(|n| ScanIssue {
                path: format!("/p/{n}").into(),
                kind: ScanIssueKind::PermissionDenied,
            })
            .collect();
        let scan = DuplicateScan {
            issues,
            ..DuplicateScan::default()
        };
        let summary = store_scan(&state, scan, &params(Path::new("/p")));
        assert_eq!(summary.issue_count, MAX_DUPLICATE_ISSUES_SHOWN + 5);
        assert_eq!(summary.issues.len(), MAX_DUPLICATE_ISSUES_SHOWN);
    }

    #[test]
    fn a_scan_that_found_nothing_is_not_the_same_as_no_scan() {
        let state = DuplicateState::default();
        let request = GroupsRequest::default();
        assert_eq!(
            kind(groups_page(&state, &request)),
            CleanupErrorKind::NoDuplicateScan
        );
        store_scan(&state, DuplicateScan::default(), &params(Path::new("/p")));
        let page = groups_page(&state, &request).unwrap();
        assert!(page.groups.is_empty());
        assert_eq!(page.total_groups, 0);
    }

    #[test]
    fn only_one_scan_can_run_at_a_time() {
        let state = DuplicateState::default();
        let _token = register(&state).unwrap();
        assert_eq!(
            kind(register(&state)),
            CleanupErrorKind::DuplicateScanRunning
        );
        drop(ActiveGuard(&state.active));
        assert!(register(&state).is_ok());
    }

    // --- browsing ---

    fn sorted_state() -> DuplicateState {
        // Reclaimable: g1 100, g2 40, g3 100.
        let state = DuplicateState::default();
        store(
            &state,
            vec![
                bare_group(1, 100, 2),
                bare_group(2, 10, 5),
                bare_group(3, 50, 3),
            ],
            None,
        );
        state
    }

    fn ids(page: &GroupsPage) -> Vec<u32> {
        page.groups.iter().map(|g| g.id).collect()
    }

    fn page_with(state: &DuplicateState, sort: SortKey) -> GroupsPage {
        let request = GroupsRequest {
            sort,
            ..GroupsRequest::default()
        };
        groups_page(state, &request).unwrap()
    }

    #[test]
    fn groups_sort_by_wasted_space_file_size_or_copy_count() {
        let state = sorted_state();
        assert_eq!(ids(&page_with(&state, SortKey::WastedSpace)), [1, 3, 2]);
        assert_eq!(ids(&page_with(&state, SortKey::FileSize)), [1, 3, 2]);
        assert_eq!(ids(&page_with(&state, SortKey::CopyCount)), [2, 3, 1]);
    }

    #[test]
    fn a_page_is_a_slice_and_the_totals_cover_every_group() {
        let state = sorted_state();
        let request = GroupsRequest {
            offset: 1,
            limit: Some(1),
            ..GroupsRequest::default()
        };
        let page = groups_page(&state, &request).unwrap();
        assert_eq!(ids(&page), [3]);
        assert_eq!((page.total_groups, page.offset), (3, 1));
        assert_eq!(page.reclaimable_bytes, 240);

        let beyond = GroupsRequest {
            offset: 10,
            ..GroupsRequest::default()
        };
        assert!(groups_page(&state, &beyond).unwrap().groups.is_empty());
    }

    #[test]
    fn the_page_size_is_bounded() {
        let state = DuplicateState::default();
        let groups = (1..=MAX_DUPLICATE_GROUPS_PER_PAGE as u32 + 20)
            .map(|id| bare_group(id, 10, 2))
            .collect();
        store(&state, groups, None);
        let with_limit = |limit| {
            let request = GroupsRequest {
                limit,
                ..GroupsRequest::default()
            };
            groups_page(&state, &request).unwrap().groups.len()
        };
        assert_eq!(with_limit(Some(usize::MAX)), MAX_DUPLICATE_GROUPS_PER_PAGE);
        assert_eq!(with_limit(Some(0)), 1);
        assert_eq!(with_limit(None), DEFAULT_GROUPS_PER_PAGE);
    }

    #[test]
    fn each_group_marks_one_suggested_copy_and_gives_the_reason() {
        let state = DuplicateState::default();
        let group = DuplicateGroup {
            id: 1,
            size: 10,
            files: vec![
                bare(11, "/Users/me/Downloads/a.pdf", false),
                bare(12, "/Users/me/Documents/a.pdf", false),
            ],
        };
        store(&state, vec![group], Some("/Users/me".into()));
        let page = page_with(&state, SortKey::WastedSpace);
        let dto = &page.groups[0];
        let suggested: Vec<u32> = dto
            .files
            .iter()
            .filter(|f| f.suggested_keep)
            .map(|f| f.id)
            .collect();
        assert_eq!(suggested, [12]);
        assert_eq!(dto.keep_reason, Some(KeepReason::BetterLocation));
        assert!(dto.keep_reason_text.is_some_and(|t| !t.is_empty()));
        let file = &dto.files[1];
        assert_eq!(file.name, "a.pdf");
        assert_eq!(file.folder, "/Users/me/Documents");
        assert_eq!(file.path, "/Users/me/Documents/a.pdf");
    }

    // --- suggested picks ---

    #[test]
    fn suggested_picks_need_a_scan() {
        assert_eq!(
            kind(suggested_picks(&DuplicateState::default())),
            CleanupErrorKind::NoDuplicateScan
        );
    }

    #[test]
    fn suggested_picks_leave_one_copy_in_every_group() {
        let state = sorted_state();
        let suggestion = suggested_picks(&state).unwrap();
        assert!(!suggestion.truncated);
        assert_eq!(suggestion.move_count, 1 + 4 + 2);
        assert_eq!(suggestion.bytes, 100 + 4 * 10 + 2 * 50);
        let found = state.found.lock().unwrap();
        for pick in &suggestion.picks {
            let group = found
                .as_ref()
                .unwrap()
                .groups
                .iter()
                .find(|g| g.id == pick.group)
                .unwrap();
            assert_eq!(pick.files.len(), group.files.len() - 1);
        }
    }

    #[test]
    fn suggested_picks_stop_at_the_size_of_one_plan() {
        let build = |groups: usize| {
            let state = DuplicateState::default();
            store(
                &state,
                (1..=groups as u32)
                    .map(|id| bare_group(id, 10, 2))
                    .collect(),
                None,
            );
            suggested_picks(&state).unwrap()
        };
        let exact = build(MAX_DUPLICATE_MOVES_PER_PLAN);
        assert_eq!(exact.move_count, MAX_DUPLICATE_MOVES_PER_PLAN);
        assert!(!exact.truncated);

        let over = build(MAX_DUPLICATE_MOVES_PER_PLAN + 1);
        assert_eq!(over.move_count, MAX_DUPLICATE_MOVES_PER_PLAN);
        assert!(over.truncated);
    }

    #[test]
    fn suggested_picks_never_name_a_protected_copy() {
        let state = DuplicateState::default();
        let group = DuplicateGroup {
            id: 1,
            size: 10,
            files: vec![
                bare(11, "/keep/a.bin", true),
                bare(12, "/scan/a.bin", false),
                bare(13, "/scan/b/a.bin", false),
            ],
        };
        store(&state, vec![group], None);
        let suggestion = suggested_picks(&state).unwrap();
        assert_eq!(
            suggestion.picks,
            [PickDto {
                group: 1,
                files: vec![12, 13]
            }]
        );
    }

    // --- planning ---

    #[test]
    fn planning_needs_a_scan() {
        let state = DuplicateState::default();
        assert_eq!(
            kind(make_plan(&state, &open(Path::new("/p")), &pick(&[701]))),
            CleanupErrorKind::NoDuplicateScan
        );
    }

    #[test]
    fn a_selection_over_the_cap_is_refused_before_anything_else() {
        let files: Vec<u32> = (0..=MAX_DUPLICATE_MOVES_PER_PLAN as u32).collect();
        let state = DuplicateState::default();
        assert_eq!(
            kind(make_plan(&state, &open(Path::new("/p")), &pick(&files))),
            CleanupErrorKind::TooMany
        );
    }

    #[test]
    fn selections_the_rules_forbid_get_their_own_error_kinds() {
        let w = world("plan-errors", 2);
        let guardrails = open(&w.root);
        let plan = |picks: &[PickRequest]| kind(make_plan(&w.state, &guardrails, picks));

        assert_eq!(plan(&pick(&[701, 702])), CleanupErrorKind::LastCopy);
        assert_eq!(plan(&pick(&[999])), CleanupErrorKind::UnknownCandidate);
        let other_group = vec![PickRequest {
            group: 99,
            files: vec![701],
        }];
        assert_eq!(plan(&other_group), CleanupErrorKind::UnknownCandidate);

        w.state.found.lock().unwrap().as_mut().unwrap().groups[0].files[0].protected = true;
        assert_eq!(plan(&pick(&[701])), CleanupErrorKind::ProtectedFile);
    }

    #[test]
    fn two_picks_for_one_group_cannot_add_up_to_every_copy() {
        let w = world("plan-split", 2);
        let picks = vec![
            PickRequest {
                group: GROUP,
                files: vec![701],
            },
            PickRequest {
                group: GROUP,
                files: vec![702],
            },
        ];
        assert_eq!(
            kind(make_plan(&w.state, &open(&w.root), &picks)),
            CleanupErrorKind::LastCopy
        );
    }

    #[test]
    fn a_new_plan_replaces_the_old_one() {
        let w = world("plan-replace", 3);
        let guardrails = open(&w.root);
        let first = make_plan(&w.state, &guardrails, &pick(&[701])).unwrap();
        let second = make_plan(&w.state, &guardrails, &pick(&[702])).unwrap();
        assert!(second.plan_id > first.plan_id);
        let trash = FakeTrash::default();
        assert_eq!(
            kind(run_plan(
                &w.state,
                &guardrails,
                &trash,
                first.plan_id,
                false
            )),
            CleanupErrorKind::StalePlan
        );
        assert!(moved(&trash).is_empty());
        assert!(run_plan(&w.state, &guardrails, &trash, second.plan_id, false).is_ok());
    }

    // --- executing ---

    #[test]
    fn executing_moves_the_planned_copies_and_the_groups_shrink() {
        let w = world("exec", 3);
        let guardrails = open(&w.root);
        let plan = make_plan(&w.state, &guardrails, &pick(&[701, 703])).unwrap();
        // Looked up now, because the group leaves the state once it is done.
        let expected = [w.path(GROUP, 701), w.path(GROUP, 703)];
        let trash = FakeTrash::default();
        let done = run_plan(&w.state, &guardrails, &trash, plan.plan_id, false).unwrap();

        assert_eq!((done.moved, done.already_gone, done.failed), (2, 0, 0));
        assert_eq!(done.moved_bytes, 2 * CONTENT.len() as u64);
        assert_eq!(moved(&trash), expected);
        // One copy is left, so the group is gone.
        assert_eq!(
            (done.remaining_groups, done.remaining_reclaimable_bytes),
            (0, 0)
        );
        assert_eq!(w.group_count(), 0);
    }

    #[test]
    fn a_group_with_copies_left_stays_with_fewer_files() {
        let w = world("exec-shrink", 4);
        let guardrails = open(&w.root);
        let plan = make_plan(&w.state, &guardrails, &pick(&[701])).unwrap();
        let done = run_plan(
            &w.state,
            &guardrails,
            &FakeTrash::default(),
            plan.plan_id,
            false,
        )
        .unwrap();
        assert_eq!(done.remaining_groups, 1);
        assert_eq!(done.remaining_reclaimable_bytes, 2 * CONTENT.len() as u64);
        let page = page_with(&w.state, SortKey::WastedSpace);
        let remaining: Vec<u32> = page.groups[0].files.iter().map(|f| f.id).collect();
        assert_eq!(remaining, [702, 703, 704]);
    }

    #[test]
    fn a_plan_runs_only_once() {
        let w = world("exec-once", 3);
        let guardrails = open(&w.root);
        let plan = make_plan(&w.state, &guardrails, &pick(&[701])).unwrap();
        let trash = FakeTrash::default();
        run_plan(&w.state, &guardrails, &trash, plan.plan_id, false).unwrap();
        assert_eq!(
            kind(run_plan(&w.state, &guardrails, &trash, plan.plan_id, false)),
            CleanupErrorKind::StalePlan
        );
        assert_eq!(moved(&trash).len(), 1);
    }

    #[test]
    fn a_wrong_plan_id_is_refused_and_does_not_use_up_the_real_plan() {
        let w = world("exec-wrong-id", 3);
        let guardrails = open(&w.root);
        let plan = make_plan(&w.state, &guardrails, &pick(&[701])).unwrap();
        let trash = FakeTrash::default();
        assert_eq!(
            kind(run_plan(
                &w.state,
                &guardrails,
                &trash,
                plan.plan_id + 5,
                false
            )),
            CleanupErrorKind::StalePlan
        );
        assert!(run_plan(&w.state, &guardrails, &trash, plan.plan_id, false).is_ok());
    }

    #[test]
    fn without_any_plan_nothing_runs() {
        let w = world("exec-none", 2);
        let trash = FakeTrash::default();
        assert_eq!(
            kind(run_plan(&w.state, &open(&w.root), &trash, 1, true)),
            CleanupErrorKind::StalePlan
        );
        assert!(moved(&trash).is_empty());
    }

    #[test]
    fn a_plan_from_before_a_refresh_is_refused() {
        let w = world("exec-generation", 3);
        let guardrails = open(&w.root);
        let plan = make_plan(&w.state, &guardrails, &pick(&[701])).unwrap();
        // A refresh that did not clear the pending plan, as a race could leave.
        w.state.generation.fetch_add(1, Ordering::SeqCst);
        let trash = FakeTrash::default();
        let err = run_plan(&w.state, &guardrails, &trash, plan.plan_id, true).unwrap_err();
        assert_eq!(err.kind, CleanupErrorKind::StalePlan);
        assert!(err.message.contains("refreshed"));
        assert!(moved(&trash).is_empty());
        assert!(w.path(GROUP, 701).exists());
    }

    #[test]
    fn a_plan_outside_home_needs_an_acknowledgement_and_is_kept_until_given() {
        let w = world("exec-ack", 3);
        let elsewhere = Guardrails::new(Some("/nowhere".into()), Vec::new());
        let plan = make_plan(&w.state, &elsewhere, &pick(&[701])).unwrap();
        assert!(plan.needs_acknowledgement);

        let trash = FakeTrash::default();
        assert_eq!(
            kind(run_plan(&w.state, &elsewhere, &trash, plan.plan_id, false)),
            CleanupErrorKind::NeedsAcknowledgement
        );
        assert!(moved(&trash).is_empty());
        let done = run_plan(&w.state, &elsewhere, &trash, plan.plan_id, true).unwrap();
        assert_eq!(done.moved, 1);
    }

    #[test]
    fn nothing_moves_while_a_scan_is_running() {
        let w = world("exec-running", 3);
        let guardrails = open(&w.root);
        let plan = make_plan(&w.state, &guardrails, &pick(&[701])).unwrap();
        *w.state.active.lock().unwrap() = Some(CancelToken::default());
        let trash = FakeTrash::default();
        assert_eq!(
            kind(run_plan(&w.state, &guardrails, &trash, plan.plan_id, true)),
            CleanupErrorKind::DuplicateScanRunning
        );
        assert!(moved(&trash).is_empty());
        // The plan was not used up.
        *w.state.active.lock().unwrap() = None;
        assert!(run_plan(&w.state, &guardrails, &trash, plan.plan_id, true).is_ok());
    }

    #[test]
    fn a_failed_move_stays_in_the_list_and_the_others_are_removed() {
        let w = world("exec-partial", 4);
        let guardrails = open(&w.root);
        let plan = make_plan(&w.state, &guardrails, &pick(&[701, 702, 703])).unwrap();
        let trash = FakeTrash {
            fail_on: Some(w.path(GROUP, 702)),
            ..FakeTrash::default()
        };
        let done = run_plan(&w.state, &guardrails, &trash, plan.plan_id, false).unwrap();
        assert_eq!((done.moved, done.failed), (2, 1));
        let page = page_with(&w.state, SortKey::WastedSpace);
        let remaining: Vec<u32> = page.groups[0].files.iter().map(|f| f.id).collect();
        assert_eq!(remaining, [702, 704]);
    }

    #[test]
    fn nothing_moves_if_the_kept_copy_disappears_after_the_preview() {
        let w = world("exec-kept-gone", 2);
        let guardrails = open(&w.root);
        let plan = make_plan(&w.state, &guardrails, &pick(&[701])).unwrap();
        fs::remove_file(w.path(GROUP, 702)).unwrap();
        let trash = FakeTrash::default();
        let done = run_plan(&w.state, &guardrails, &trash, plan.plan_id, false).unwrap();
        assert_eq!((done.moved, done.failed), (0, 1));
        assert!(moved(&trash).is_empty());
        assert!(w.path(GROUP, 701).exists());
    }

    // --- shapes the interface depends on ---

    #[test]
    fn error_kinds_serialize_in_camel_case() {
        let json = |k| serde_json::to_value(error(k, "m")).unwrap()["kind"].clone();
        assert_eq!(json(CleanupErrorKind::LastCopy), "lastCopy");
        assert_eq!(json(CleanupErrorKind::NoDuplicateScan), "noDuplicateScan");
        assert_eq!(
            json(CleanupErrorKind::DuplicateScanRunning),
            "duplicateScanRunning"
        );
        assert_eq!(json(CleanupErrorKind::ProtectedFile), "protectedFile");
        assert_eq!(json(CleanupErrorKind::InvalidRoot), "invalidRoot");
        assert_eq!(json(CleanupErrorKind::PermissionDenied), "permissionDenied");
    }

    #[test]
    fn progress_events_name_the_stage() {
        let payload = |stage| {
            serde_json::to_value(ProgressPayload::from(DuplicateProgress {
                stage,
                done: 3,
                total: 9,
                bytes: 5,
                current_path: "/a/b".into(),
            }))
            .unwrap()
        };
        let listing = payload(Stage::Listing);
        assert_eq!(listing["stage"], "listing");
        assert_eq!(listing["done"], 3);
        assert_eq!(listing["total"], 9);
        assert_eq!(listing["bytes"], 5);
        assert_eq!(listing["currentPath"], "/a/b");
        assert_eq!(payload(Stage::Fingerprinting)["stage"], "fingerprinting");
        assert_eq!(payload(Stage::Hashing)["stage"], "hashing");
    }

    #[test]
    fn a_plan_response_flattens_the_plan_next_to_its_id() {
        let w = world("shape-plan", 3);
        let plan = make_plan(&w.state, &open(&w.root), &pick(&[701])).unwrap();
        let json = serde_json::to_value(&plan).unwrap();
        assert_eq!(json["planId"], plan.plan_id);
        assert_eq!(json["needsAcknowledgement"], false);
        assert_eq!(json["totalFiles"], 1);
        assert_eq!(json["totalBytes"], CONTENT.len());
        assert!(json["excluded"].as_array().unwrap().is_empty());
        let group = &json["groups"][0];
        assert_eq!(group["group"], GROUP);
        assert_eq!(group["kept"].as_array().unwrap().len(), 2);
        assert_eq!(group["moves"][0]["node"], 701);
        assert_eq!(group["moves"][0]["name"], "doc.bin");
    }

    #[test]
    fn group_and_summary_responses_use_camel_case_keys() {
        let state = sorted_state();
        let page = serde_json::to_value(page_with(&state, SortKey::WastedSpace)).unwrap();
        for key in ["groups", "totalGroups", "offset", "reclaimableBytes"] {
            assert!(page.get(key).is_some(), "page is missing {key}");
        }
        let group = &page["groups"][0];
        for key in [
            "id",
            "size",
            "reclaimableBytes",
            "reclaimableFiles",
            "keepReason",
            "files",
        ] {
            assert!(group.get(key).is_some(), "group is missing {key}");
        }
        assert_eq!(group["keepReason"], "nameOrder");
        let file = &group["files"][0];
        for key in [
            "id",
            "path",
            "name",
            "folder",
            "modifiedSecs",
            "protected",
            "suggestedKeep",
        ] {
            assert!(file.get(key).is_some(), "file is missing {key}");
        }

        let summary = store_scan(&state, DuplicateScan::default(), &params(Path::new("/p")));
        let json = serde_json::to_value(summary).unwrap();
        for key in [
            "root",
            "protectedRoots",
            "minSizeBytes",
            "groupCount",
            "reclaimableBytes",
            "filesListed",
            "bytesHashed",
            "hardLinksCollapsed",
            "issueCount",
            "issues",
            "skippedCloudFolders",
        ] {
            assert!(json.get(key).is_some(), "summary is missing {key}");
        }
    }

    #[test]
    fn the_start_request_reads_camel_case_keys() {
        let request: StartRequest = serde_json::from_str(
            r#"{"root":"/a","protectedRoots":["/b"],"minSizeBytes":9000,
                "includeCloudFolders":true,
                "exclusions":{"nodeModules":false,"packages":true}}"#,
        )
        .unwrap();
        assert_eq!(request.root.as_deref(), Some("/a"));
        assert_eq!(request.protected_roots, ["/b"]);
        assert_eq!(request.min_size_bytes, Some(9000));
        assert_eq!(request.include_cloud_folders, Some(true));
        assert!(!request.exclusions.unwrap().node_modules);

        let groups: GroupsRequest =
            serde_json::from_str(r#"{"sort":"copyCount","offset":2,"limit":5}"#).unwrap();
        assert_eq!(
            (groups.sort, groups.offset, groups.limit),
            (SortKey::CopyCount, 2, Some(5))
        );
        let picks: Vec<PickRequest> =
            serde_json::from_str(r#"[{"group":1,"files":[2,3]}]"#).unwrap();
        assert_eq!((picks[0].group, picks[0].files.clone()), (1, vec![2, 3]));
    }

    // --- the whole flow on real files ---

    #[cfg(target_os = "macos")]
    #[test]
    fn scan_browse_plan_and_move_on_real_files() {
        let root = temp_root("e2e");
        let write = |rel: &str, bytes: &[u8]| {
            let path = root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        };
        let twin = vec![b'a'; 8 * 1024];
        let other = vec![b'b'; 8 * 1024];
        write("Documents/report.bin", &twin);
        write("Downloads/report (1).bin", &twin);
        write("Archive/report.bin", &twin);
        write("Documents/different.bin", &other);
        write("Documents/tiny.bin", b"x");

        let request = StartRequest {
            root: Some(root.to_string_lossy().into_owned()),
            min_size_bytes: Some(MIN_DUPLICATE_MIN_SIZE_BYTES),
            ..StartRequest::default()
        };
        let mut scan_params = resolve_params(request, Some(root.clone())).unwrap();
        scan_params.home = Some(root.clone());
        let scan = run_scan(
            &make_walker(),
            &make_hasher(),
            &scan_params,
            &CancelToken::default(),
            &|_| {},
        )
        .unwrap();
        let state = DuplicateState::default();
        let summary = store_scan(&state, scan, &scan_params);
        assert_eq!(summary.group_count, 1);
        assert_eq!(summary.reclaimable_bytes, 2 * 8 * 1024);

        let page = groups_page(&state, &GroupsRequest::default()).unwrap();
        assert_eq!(page.groups[0].files.len(), 3);
        let suggestion = suggested_picks(&state).unwrap();
        assert_eq!(suggestion.move_count, 2);

        let picks: Vec<PickRequest> = suggestion
            .picks
            .iter()
            .map(|p| PickRequest {
                group: p.group,
                files: p.files.clone(),
            })
            .collect();
        let guardrails = open(&root);
        let plan = make_plan(&state, &guardrails, &picks).unwrap();
        assert_eq!(plan.plan.total_files, 2);
        let trash = FakeTrash::default();
        let done = run_plan(&state, &guardrails, &trash, plan.plan_id, false).unwrap();
        assert_eq!((done.moved, done.failed), (2, 0));

        // Nothing was really deleted: the fake trash only recorded the paths.
        for path in moved(&trash) {
            assert!(path.exists());
        }
        // Which of Documents and Archive is kept depends on file times and
        // path length, so it is not asserted. Only this is: the Downloads copy
        // with the copy-style name is never the one kept.
        let keeper = page.groups[0]
            .files
            .iter()
            .find(|f| f.suggested_keep)
            .unwrap();
        assert!(!keeper.path.contains("/Downloads/"));
        let moved_paths: Vec<String> = moved(&trash)
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert!(moved_paths.iter().all(|p| *p != keeper.path));
        assert!(moved_paths.iter().any(|p| p.contains("/Downloads/")));
        assert_eq!(done.remaining_groups, 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_cancelled_scan_reports_cancelled() {
        let root = temp_root("e2e-cancel");
        let cancel = CancelToken::default();
        cancel.cancel();
        let err = run_scan(
            &make_walker(),
            &make_hasher(),
            &params(&root),
            &cancel,
            &|_| {},
        )
        .unwrap_err();
        assert_eq!(err.kind, CleanupErrorKind::Cancelled);
    }

    #[test]
    fn reason_kinds_used_for_exclusions_are_unchanged_by_this_module() {
        // The plan reuses the cleanup reasons; a rename there would break the UI.
        let json = serde_json::to_value(ReasonKind::Changed).unwrap();
        assert_eq!(json, "changed");
    }
}
