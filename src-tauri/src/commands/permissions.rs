//! Permission commands: Full Disk Access status, a shortcut to the
//! System Settings pane where the user grants it, and Quick Look preview.

use std::process::Command;

use serde::Serialize;

use crate::platform::{FullDiskAccess, PermissionChecker};

const FULL_DISK_ACCESS_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles";

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FullDiskAccessStatus {
    Granted,
    Missing,
    Unknown,
}

impl From<FullDiskAccess> for FullDiskAccessStatus {
    fn from(access: FullDiskAccess) -> Self {
        match access {
            FullDiskAccess::Granted => Self::Granted,
            FullDiskAccess::Missing => Self::Missing,
            FullDiskAccess::Unknown => Self::Unknown,
        }
    }
}

#[cfg(target_os = "macos")]
fn make_checker() -> impl PermissionChecker {
    crate::platform::macos::MacPermissionChecker::new()
}

#[tauri::command]
pub fn check_full_disk_access() -> FullDiskAccessStatus {
    make_checker().full_disk_access().into()
}

#[tauri::command]
pub fn open_full_disk_access_settings() -> Result<(), String> {
    Command::new("open")
        .arg(FULL_DISK_ACCESS_SETTINGS_URL)
        .status()
        .map_err(|e| e.to_string())
        .and_then(|status| {
            status
                .success()
                .then_some(())
                .ok_or_else(|| format!("`open` exited with {status}"))
        })
}

/// Open a Quick Look panel for a file. Fire-and-forget: `qlmanage` is spawned
/// as a detached process; we do not wait for it. Returns an error only if the
/// process could not be spawned at all (e.g. `qlmanage` not found).
#[tauri::command]
pub fn preview_file(path: String) -> Result<(), String> {
    Command::new("qlmanage")
        .args(["-p", &path])
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_serialize_in_camel_case() {
        for (status, expected) in [
            (FullDiskAccess::Granted, "granted"),
            (FullDiskAccess::Missing, "missing"),
            (FullDiskAccess::Unknown, "unknown"),
        ] {
            let json = serde_json::to_value(FullDiskAccessStatus::from(status)).unwrap();
            assert_eq!(json, expected);
        }
    }
}
