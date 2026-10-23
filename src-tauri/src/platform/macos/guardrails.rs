//! The macOS list of paths that cleanup must never move.
//!
//! `Tree` entries block the path and everything below it. `Dir` entries block
//! only the folder itself, so its contents can still be cleaned. Anything that
//! contains an entry is blocked too (see `Guardrails::verdict`).

use std::path::{Path, PathBuf};

use crate::core::cleanup::{Protected, ReasonKind};

/// Operating-system locations. Moving these breaks the machine.
const SYSTEM_TREES: &[&str] = &[
    "/System",
    "/bin",
    "/sbin",
    "/usr",
    "/dev",
    "/cores",
    "/private/etc",
    "/private/var/db",
];

/// Top-level folders that exist for the system. Their contents are the
/// user's or other apps' business, so only the folder itself is protected.
const SYSTEM_DIRS: &[&str] = &[
    "/",
    "/Users",
    "/Applications",
    "/Library",
    "/Volumes",
    "/private",
    "/private/var",
    "/private/tmp",
];

/// Standard folders directly inside the home folder.
const HOME_DIRS: &[&str] = &[
    "Applications",
    "Desktop",
    "Documents",
    "Downloads",
    "Library",
    "Movies",
    "Music",
    "Pictures",
    "Public",
];

/// Credentials. Losing these is hard to undo even from the Trash.
const HOME_SECRET_TREES: &[&str] = &[".ssh", ".gnupg", "Library/Keychains"];

/// Paths of the running app that cleanup must not touch.
#[derive(Debug, Default)]
pub struct AppPaths {
    /// Directories holding the app's data, caches, logs and config.
    pub data_dirs: Vec<PathBuf>,
    /// The `.app` bundle when running from one, otherwise the executable.
    pub executable: Option<PathBuf>,
    /// The bundle identifier, used to find per-app files in `~/Library`.
    pub identifier: String,
}

/// The bundle containing `exe`, or `exe` itself when it is not inside one.
pub fn app_bundle_or_exe(exe: &Path) -> PathBuf {
    exe.ancestors()
        .find(|p| p.extension().is_some_and(|ext| ext == "app"))
        .unwrap_or(exe)
        .to_path_buf()
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
            HOME_DIRS
                .iter()
                .map(|d| Protected::dir(home.join(d), ReasonKind::HomeFolder)),
        );
        rules.extend(
            HOME_SECRET_TREES
                .iter()
                .map(|d| Protected::tree(home.join(d), ReasonKind::HomeFolder)),
        );
        rules.push(Protected::tree(home.join(".Trash"), ReasonKind::Trash));

        if !app.identifier.is_empty() {
            let library = home.join("Library");
            for folder in [
                "Caches",
                "WebKit",
                "Application Support",
                "Logs",
                "Saved Application State",
                "HTTPStorages",
            ] {
                rules.push(Protected::tree(
                    library.join(folder).join(&app.identifier),
                    ReasonKind::AppData,
                ));
            }
            rules.push(Protected::tree(
                library
                    .join("Preferences")
                    .join(format!("{}.plist", app.identifier)),
                ReasonKind::AppData,
            ));
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
        let home = PathBuf::from("/Users/me");
        let app = AppPaths {
            data_dirs: vec!["/Users/me/Library/Application Support/com.x.app".into()],
            executable: Some("/Applications/Carbon Disk Treemap.app".into()),
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
            "/",
            "/System",
            "/System/Library",
            "/usr/bin/ls",
            "/bin",
            "/Users",
            "/Applications",
            "/Library",
            "/private/etc/hosts",
            "/private/var/db/x",
        ] {
            assert_eq!(kind(path), Some(ReasonKind::SystemPath), "{path}");
        }
    }

    #[test]
    fn user_content_next_to_system_folders_is_not_blocked() {
        let g = guardrails();
        assert!(matches!(
            g.verdict(Path::new("/Applications/Foo.app")),
            Verdict::Warn(_)
        ));
        assert!(matches!(
            g.verdict(Path::new("/Library/Caches/foo")),
            Verdict::Warn(_)
        ));
        assert!(matches!(
            g.verdict(Path::new("/Volumes/Backup/old")),
            Verdict::Warn(_)
        ));
    }

    #[test]
    fn the_home_folder_and_its_standard_folders_are_blocked_but_not_their_contents() {
        for path in [
            "/Users/me",
            "/Users/me/Documents",
            "/Users/me/Library",
            "/Users/me/Downloads",
        ] {
            assert_eq!(kind(path), Some(ReasonKind::HomeFolder), "{path}");
        }
        for path in [
            "/Users/me/Downloads/big.iso",
            "/Users/me/Library/Caches/com.other.app",
            "/Users/me/Movies/film.mov",
        ] {
            assert_eq!(kind(path), None, "{path}");
        }
    }

    #[test]
    fn credentials_and_the_trash_are_blocked() {
        assert_eq!(kind("/Users/me/.ssh"), Some(ReasonKind::HomeFolder));
        assert_eq!(
            kind("/Users/me/.ssh/id_ed25519"),
            Some(ReasonKind::HomeFolder)
        );
        assert_eq!(
            kind("/Users/me/Library/Keychains/login.keychain-db"),
            Some(ReasonKind::HomeFolder)
        );
        assert_eq!(kind("/Users/me/.Trash/old.zip"), Some(ReasonKind::Trash));
    }

    #[test]
    fn the_apps_own_data_and_executable_are_blocked() {
        for path in [
            "/Users/me/Library/Caches/com.x.app",
            "/Users/me/Library/Caches/com.x.app/blob",
            "/Users/me/Library/WebKit/com.x.app",
            "/Users/me/Library/Preferences/com.x.app.plist",
            "/Users/me/Library/Application Support/com.x.app/db",
            "/Applications/Carbon Disk Treemap.app",
            "/Applications/Carbon Disk Treemap.app/Contents/MacOS/x",
        ] {
            assert_eq!(kind(path), Some(ReasonKind::AppData), "{path}");
        }
    }

    #[test]
    fn a_bundle_is_found_from_its_executable_path() {
        assert_eq!(
            app_bundle_or_exe(Path::new(
                "/Applications/Carbon Disk Treemap.app/Contents/MacOS/bin"
            )),
            PathBuf::from("/Applications/Carbon Disk Treemap.app")
        );
        assert_eq!(
            app_bundle_or_exe(Path::new("/repo/target/debug/app")),
            PathBuf::from("/repo/target/debug/app")
        );
    }

    #[test]
    fn rules_still_cover_the_system_when_home_is_unknown() {
        let g = Guardrails::new(None, protected_paths(None, &AppPaths::default()));
        assert!(matches!(
            g.verdict(Path::new("/System/x")),
            Verdict::Blocked(_)
        ));
        assert!(matches!(
            g.verdict(Path::new("/Users/me/x")),
            Verdict::Warn(_)
        ));
    }
}
