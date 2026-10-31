//! Windows guardrails: paths that cleanup must never move.
//!
//! The structure mirrors `platform/macos/guardrails.rs`.  Windows paths are
//! case-insensitive, which is handled by `Guardrails::new` using the home
//! parameter; individual path comparisons inside `core/cleanup.rs` do NOT
//! normalise case themselves, so the paths here must be canonical
//! (lower-case drive letters, forward slashes only in tests where needed).
//!
//! `Tree` entries block the path and all descendants.
//! `Dir` entries block only the folder itself; its contents can still be moved.

use std::path::{Path, PathBuf};

use crate::core::cleanup::{Protected, ReasonKind};

/// Operating-system trees.  Moving anything here breaks the system.
const SYSTEM_TREES: &[&str] = &[
    "C:\\Windows",
    "C:\\Windows\\System32",
    "C:\\Windows\\SysWOW64",
    "C:\\ProgramData\\Microsoft",
];

/// Top-level folders whose contents belong to the user or other apps, so only
/// the folder itself is protected.
const SYSTEM_DIRS: &[&str] = &[
    "C:\\",
    "C:\\Users",
    "C:\\Program Files",
    "C:\\Program Files (x86)",
    "C:\\ProgramData",
];

/// Standard folders directly inside the user's profile.
const PROFILE_DIRS: &[&str] = &[
    "Desktop",
    "Documents",
    "Downloads",
    "Music",
    "Pictures",
    "Videos",
    "AppData",
    "Favorites",
    "Links",
    "Saved Games",
    "Searches",
];

/// Credential and key stores inside the profile.
const PROFILE_SECRET_TREES: &[&str] = &[
    ".ssh",
    "AppData\\Roaming\\Microsoft\\Credentials",
    "AppData\\Roaming\\Microsoft\\Protect",
    "AppData\\Local\\Microsoft\\Credentials",
];

/// Data about the running app (inside AppData).
#[derive(Debug, Default)]
pub struct AppPaths {
    pub data_dirs: Vec<PathBuf>,
    pub executable: Option<PathBuf>,
    pub identifier: String,
}

pub fn protected_paths(home: Option<&Path>, app: &AppPaths) -> Vec<Protected> {
    let mut rules = Vec::new();

    rules.extend(
        SYSTEM_TREES
            .iter()
            .map(|p| Protected::tree(*p, ReasonKind::SystemPath)),
    );
    rules.extend(
        SYSTEM_DIRS
            .iter()
            .map(|p| Protected::dir(*p, ReasonKind::SystemPath)),
    );

    if let Some(home) = home {
        rules.push(Protected::dir(home, ReasonKind::HomeFolder));
        rules.extend(
            PROFILE_DIRS
                .iter()
                .map(|d| Protected::dir(home.join(d), ReasonKind::HomeFolder)),
        );
        rules.extend(
            PROFILE_SECRET_TREES
                .iter()
                .map(|d| Protected::tree(home.join(d), ReasonKind::HomeFolder)),
        );
        // Recycle Bin lives on every drive as $Recycle.Bin.
        // Block the per-user subfolder when we can identify it; also block
        // the root-level Recycle.Bin entries.
        rules.push(Protected::tree(
            PathBuf::from("C:\\$Recycle.Bin"),
            ReasonKind::Trash,
        ));

        if !app.identifier.is_empty() {
            let roaming = home.join("AppData\\Roaming");
            let local = home.join("AppData\\Local");
            for base in [&roaming, &local] {
                rules.push(Protected::tree(
                    base.join(&app.identifier),
                    ReasonKind::AppData,
                ));
            }
        }
    }

    rules.extend(
        app.data_dirs
            .iter()
            .map(|p| Protected::tree(p, ReasonKind::AppData)),
    );
    if let Some(exe) = &app.executable {
        rules.push(Protected::tree(exe, ReasonKind::AppData));
    }
    rules
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::cleanup::{Guardrails, Verdict};

    fn guardrails() -> Guardrails {
        let home = PathBuf::from(r"C:\Users\me");
        let app = AppPaths {
            data_dirs: vec![r"C:\Users\me\AppData\Local\com.x.app".into()],
            executable: Some(r"C:\Program Files\CarbonDiskTreemap\carbon-treemap-disk.exe".into()),
            identifier: "com.x.app".into(),
        };
        Guardrails::new(Some(home.clone()), protected_paths(Some(&home), &app))
    }

    fn kind(path: &str) -> Option<ReasonKind> {
        match guardrails().verdict(Path::new(path)) {
            Verdict::Blocked(reason) => Some(reason.kind),
            _ => None,
        }
    }

    #[test]
    fn system_locations_are_blocked() {
        for path in [
            r"C:\",
            r"C:\Windows",
            r"C:\Windows\System32\cmd.exe",
            r"C:\Users",
            r"C:\Program Files",
            r"C:\ProgramData",
        ] {
            assert_eq!(kind(path), Some(ReasonKind::SystemPath), "{path}");
        }
    }

    #[test]
    fn home_and_standard_profile_folders_are_blocked_but_not_their_contents() {
        for path in [
            r"C:\Users\me",
            r"C:\Users\me\Documents",
            r"C:\Users\me\Downloads",
            r"C:\Users\me\AppData",
        ] {
            assert_eq!(kind(path), Some(ReasonKind::HomeFolder), "{path}");
        }
        // Contents are not blocked by default.
        for path in [
            r"C:\Users\me\Downloads\big.iso",
            r"C:\Users\me\Documents\report.docx",
        ] {
            assert_eq!(kind(path), None, "{path}");
        }
    }

    #[test]
    fn credentials_and_recycle_bin_are_blocked() {
        assert_eq!(kind(r"C:\Users\me\.ssh"), Some(ReasonKind::HomeFolder));
        assert_eq!(
            kind(r"C:\Users\me\.ssh\id_ed25519"),
            Some(ReasonKind::HomeFolder)
        );
        assert_eq!(kind(r"C:\$Recycle.Bin\whatever"), Some(ReasonKind::Trash));
    }

    #[test]
    fn app_data_and_executable_are_blocked() {
        for path in [
            r"C:\Users\me\AppData\Local\com.x.app",
            r"C:\Users\me\AppData\Local\com.x.app\cache\blob",
            r"C:\Users\me\AppData\Roaming\com.x.app",
            r"C:\Program Files\CarbonDiskTreemap\carbon-treemap-disk.exe",
        ] {
            assert_eq!(kind(path), Some(ReasonKind::AppData), "{path}");
        }
    }

    #[test]
    fn rules_cover_the_system_when_home_is_unknown() {
        let g = Guardrails::new(None, protected_paths(None, &AppPaths::default()));
        assert!(matches!(
            g.verdict(Path::new(r"C:\Windows\explorer.exe")),
            Verdict::Blocked(_)
        ));
    }
}
