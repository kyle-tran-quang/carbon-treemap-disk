//! Scan commands: start, cancel, and progress events.
//!
//! The finished tree stays in Rust (`ScanState`); the UI only receives a
//! small summary. Sub-task 5 adds the bounded view query over this state.

use std::io::{self, ErrorKind};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::core::config::ScanOptions;
use crate::core::scan::{CancelToken, ScanIssueKind, ScanProgress};
use crate::core::tree::{NodeId, Tree};
use crate::platform::Scanner;

pub const PROGRESS_EVENT: &str = "scan-progress";

#[derive(Default)]
pub struct ScanState {
    pub(super) active: Mutex<Option<CancelToken>>,
    pub(super) tree: Mutex<Option<Tree>>,
    /// Bumped, under the `tree` lock, each time a new tree is stored. Raw node
    /// ids and cleanup plans are only valid for the generation that made them.
    pub(super) generation: AtomicU64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProgressPayload {
    items_scanned: u64,
    bytes: u64,
    current_path: String,
}

impl From<ScanProgress> for ProgressPayload {
    fn from(p: ScanProgress) -> Self {
        Self {
            items_scanned: p.items_scanned,
            bytes: p.bytes,
            current_path: p.current_path.to_string_lossy().into_owned(),
        }
    }
}

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ScanErrorKind {
    Cancelled,
    PermissionDenied,
    NotFound,
    AlreadyRunning,
    Other,
}

/// Structured so the UI can branch on `kind` instead of parsing text.
#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScanError {
    kind: ScanErrorKind,
    message: String,
}

impl ScanError {
    fn new(kind: ScanErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl From<io::Error> for ScanError {
    fn from(error: io::Error) -> Self {
        let kind = match error.kind() {
            ErrorKind::Interrupted => ScanErrorKind::Cancelled,
            ErrorKind::PermissionDenied => ScanErrorKind::PermissionDenied,
            ErrorKind::NotFound => ScanErrorKind::NotFound,
            _ => ScanErrorKind::Other,
        };
        Self::new(kind, error.to_string())
    }
}

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ScanIssueDtoKind {
    PermissionDenied,
    Io,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScanIssueDto {
    path: String,
    kind: ScanIssueDtoKind,
    message: String,
}

impl From<&crate::core::scan::ScanIssue> for ScanIssueDto {
    fn from(issue: &crate::core::scan::ScanIssue) -> Self {
        let (kind, message) = match &issue.kind {
            ScanIssueKind::PermissionDenied => (
                ScanIssueDtoKind::PermissionDenied,
                "Permission denied".into(),
            ),
            ScanIssueKind::Io(message) => (ScanIssueDtoKind::Io, message.clone()),
        };
        Self {
            path: issue.path.to_string_lossy().into_owned(),
            kind,
            message,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSummary {
    total_bytes: u64,
    file_count: u64,
    node_count: usize,
    issues: Vec<ScanIssueDto>,
    /// Cloud-sync folders left out of the totals because the scan skips them.
    skipped_cloud_folders: Vec<String>,
}

fn scan_options(include_cloud_folders: Option<bool>) -> ScanOptions {
    ScanOptions {
        include_cloud_folders: include_cloud_folders.unwrap_or(false),
        ..ScanOptions::default()
    }
}

#[cfg(target_os = "macos")]
fn make_scanner() -> impl Scanner {
    crate::platform::macos::MacScanner::new()
}

/// Clears the active-scan slot on every exit path.
struct ActiveGuard<'a>(&'a Mutex<Option<CancelToken>>);

impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        *self.0.lock().expect("scan state poisoned") = None;
    }
}

fn register(state: &ScanState) -> Result<CancelToken, ScanError> {
    let mut active = state.active.lock().expect("scan state poisoned");
    if active.is_some() {
        return Err(ScanError::new(
            ScanErrorKind::AlreadyRunning,
            "A scan is already running",
        ));
    }
    let token = CancelToken::default();
    *active = Some(token.clone());
    Ok(token)
}

#[tauri::command]
pub async fn start_scan(
    app: AppHandle,
    state: State<'_, ScanState>,
    root: String,
    include_cloud_folders: Option<bool>,
) -> Result<ScanSummary, ScanError> {
    let token = register(&state)?;
    let _guard = ActiveGuard(&state.active);
    let options = scan_options(include_cloud_folders);

    let output = tauri::async_runtime::spawn_blocking(move || {
        make_scanner().scan(&PathBuf::from(root), &options, &token, &|progress| {
            let _ = app.emit(PROGRESS_EVENT, ProgressPayload::from(progress));
        })
    })
    .await
    .map_err(|e| ScanError::new(ScanErrorKind::Other, e.to_string()))??;

    let summary = ScanSummary {
        total_bytes: output.tree.size(NodeId::ROOT),
        file_count: output.tree.file_count(NodeId::ROOT),
        node_count: output.tree.len(),
        issues: output.issues.iter().map(ScanIssueDto::from).collect(),
        skipped_cloud_folders: output
            .skipped_cloud_folders
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
    };
    {
        let mut tree = state.tree.lock().expect("scan state poisoned");
        *tree = Some(output.tree);
        state.generation.fetch_add(1, Ordering::SeqCst);
    }
    Ok(summary)
}

#[tauri::command]
pub fn cancel_scan(state: State<'_, ScanState>) {
    if let Some(token) = state.active.lock().expect("scan state poisoned").as_ref() {
        token.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_errors_map_to_structured_kinds() {
        let kind = |k| ScanError::from(io::Error::new(k, "x")).kind;
        assert_eq!(kind(ErrorKind::Interrupted), ScanErrorKind::Cancelled);
        assert_eq!(
            kind(ErrorKind::PermissionDenied),
            ScanErrorKind::PermissionDenied
        );
        assert_eq!(kind(ErrorKind::NotFound), ScanErrorKind::NotFound);
        assert_eq!(kind(ErrorKind::InvalidInput), ScanErrorKind::Other);
    }

    #[test]
    fn issues_serialize_with_camel_case_kinds() {
        use crate::core::scan::ScanIssue;
        let issue = ScanIssue {
            path: "/p".into(),
            kind: ScanIssueKind::PermissionDenied,
        };
        let json = serde_json::to_value(ScanIssueDto::from(&issue)).unwrap();
        assert_eq!(json["kind"], "permissionDenied");
        assert_eq!(json["path"], "/p");
        let err = serde_json::to_value(ScanError::new(ScanErrorKind::AlreadyRunning, "m")).unwrap();
        assert_eq!(err["kind"], "alreadyRunning");
    }

    #[test]
    fn cloud_folders_are_skipped_unless_asked_for() {
        assert!(!scan_options(None).include_cloud_folders);
        assert!(!scan_options(Some(false)).include_cloud_folders);
        assert!(scan_options(Some(true)).include_cloud_folders);
    }

    #[test]
    fn second_registration_is_rejected_until_guard_drops() {
        let state = ScanState::default();
        let _token = register(&state).unwrap();
        assert!(register(&state).is_err());
        drop(ActiveGuard(&state.active));
        assert!(register(&state).is_ok());
    }
}
