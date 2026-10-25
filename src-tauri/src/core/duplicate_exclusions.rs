//! Folders the duplicate finder leaves out by default. Pure: no filesystem
//! access, so the rules are tested without touching a disk.
//!
//! Duplicates inside these are almost always intentional or managed by an
//! app, and removing one can break it:
//!
//! - **Packages** such as `.app` and `.photoslibrary`, which the OS shows as a
//!   single item.
//! - **`node_modules`** and **`.git`**, which tools rebuild and rely on.
//! - **`~/Library`**, whose contents belong to apps.
//! - **`~/.Trash`**, which is already waiting to be emptied.
//!
//! The list is judgement-based. A rule applies only to folders *below* the
//! root the user chose, never to the root itself: choosing `~/Library/Caches`
//! scans it as asked.

use std::ffi::OsStr;
use std::path::Path;

/// Directory extensions that macOS treats as one item. Compared lower-case.
const PACKAGE_EXTENSIONS: &[&str] = &[
    "app",
    "bundle",
    "framework",
    "fcpbundle",
    "imovielibrary",
    "musiclibrary",
    "photoslibrary",
    "playground",
    "plugin",
    "sparsebundle",
    "tvlibrary",
    "xcarchive",
    "xcodeproj",
    "xcworkspace",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Excluded {
    Package,
    NodeModules,
    Git,
    Library,
    Trash,
}

/// Each switch, when true, leaves that kind of folder out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exclusions {
    pub packages: bool,
    pub node_modules: bool,
    pub git: bool,
    pub library: bool,
    pub trash: bool,
}

impl Default for Exclusions {
    fn default() -> Self {
        Self {
            packages: true,
            node_modules: true,
            git: true,
            library: true,
            trash: true,
        }
    }
}

impl Exclusions {
    pub fn none() -> Self {
        Self {
            packages: false,
            node_modules: false,
            git: false,
            library: false,
            trash: false,
        }
    }

    /// Whether the folder `parent/name` is left out. `home` is the user's home
    /// folder, if known; without it the home-relative rules do not apply.
    pub fn check(&self, home: Option<&Path>, parent: &Path, name: &OsStr) -> Option<Excluded> {
        let name_lower = name.to_string_lossy().to_lowercase();
        let in_home = home.is_some_and(|home| same_path(home, parent));

        if self.library && in_home && name_lower == "library" {
            return Some(Excluded::Library);
        }
        if self.trash && in_home && name_lower == ".trash" {
            return Some(Excluded::Trash);
        }
        if self.node_modules && name_lower == "node_modules" {
            return Some(Excluded::NodeModules);
        }
        if self.git && name_lower == ".git" {
            return Some(Excluded::Git);
        }
        if self.packages
            && Path::new(&name_lower)
                .extension()
                .and_then(OsStr::to_str)
                .is_some_and(|ext| PACKAGE_EXTENSIONS.contains(&ext))
        {
            return Some(Excluded::Package);
        }
        None
    }
}

/// Lower-cased because default macOS volumes are case-insensitive. On a
/// case-sensitive volume this can only exclude more, never less.
fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/Users/me";

    fn check(rules: &Exclusions, parent: &str, name: &str) -> Option<Excluded> {
        rules.check(Some(Path::new(HOME)), Path::new(parent), OsStr::new(name))
    }

    #[test]
    fn packages_are_excluded_by_extension_in_any_case() {
        let rules = Exclusions::default();
        for name in [
            "Safari.app",
            "Pictures.photoslibrary",
            "Old.XCARCHIVE",
            "Lib.framework",
            "x.musiclibrary",
        ] {
            assert_eq!(
                check(&rules, "/Users/me/Documents", name),
                Some(Excluded::Package),
                "{name}"
            );
        }
    }

    #[test]
    fn similar_names_are_not_packages() {
        let rules = Exclusions::default();
        for name in [
            "app",
            ".app",
            "my.apple",
            "photos",
            "Documents",
            "a.application",
        ] {
            assert_eq!(check(&rules, "/Users/me/Documents", name), None, "{name}");
        }
    }

    #[test]
    fn node_modules_and_git_are_excluded_anywhere() {
        let rules = Exclusions::default();
        assert_eq!(
            check(&rules, "/Users/me/code/app", "node_modules"),
            Some(Excluded::NodeModules)
        );
        assert_eq!(
            check(&rules, "/Users/me/code/app", ".git"),
            Some(Excluded::Git)
        );
        assert_eq!(check(&rules, "/Users/me/code/app", ".github"), None);
        assert_eq!(check(&rules, "/Users/me/code/app", "my_node_modules"), None);
    }

    #[test]
    fn library_and_trash_are_excluded_only_directly_under_home() {
        let rules = Exclusions::default();
        assert_eq!(check(&rules, HOME, "Library"), Some(Excluded::Library));
        assert_eq!(check(&rules, HOME, ".Trash"), Some(Excluded::Trash));
        assert_eq!(check(&rules, "/Users/me/Documents", "Library"), None);
        assert_eq!(check(&rules, "/Users/me/Documents", ".Trash"), None);
        assert_eq!(check(&rules, "/Users/other", "Library"), None);
    }

    #[test]
    fn home_rules_ignore_letter_case() {
        let rules = Exclusions::default();
        assert_eq!(
            check(&rules, "/users/ME", "library"),
            Some(Excluded::Library)
        );
    }

    #[test]
    fn home_rules_do_not_apply_without_a_home_folder() {
        let rules = Exclusions::default();
        assert_eq!(
            rules.check(None, Path::new(HOME), OsStr::new("Library")),
            None
        );
        // Rules that need no home still apply.
        assert_eq!(
            rules.check(None, Path::new(HOME), OsStr::new(".git")),
            Some(Excluded::Git)
        );
    }

    #[test]
    fn a_folder_inside_library_is_not_excluded_by_the_library_rule() {
        // Choosing ~/Library/Caches as the root scans it as asked.
        let rules = Exclusions::default();
        assert_eq!(check(&rules, "/Users/me/Library", "Caches"), None);
        assert_eq!(check(&rules, "/Users/me/Library/Caches", "Homebrew"), None);
    }

    #[test]
    fn each_switch_controls_only_its_own_rule() {
        let only = |f: fn(&mut Exclusions)| {
            let mut rules = Exclusions::none();
            f(&mut rules);
            rules
        };
        let cases: [(Exclusions, &str, &str, Excluded); 5] = [
            (
                only(|r| r.packages = true),
                "/x",
                "A.app",
                Excluded::Package,
            ),
            (
                only(|r| r.node_modules = true),
                "/x",
                "node_modules",
                Excluded::NodeModules,
            ),
            (only(|r| r.git = true), "/x", ".git", Excluded::Git),
            (
                only(|r| r.library = true),
                HOME,
                "Library",
                Excluded::Library,
            ),
            (only(|r| r.trash = true), HOME, ".Trash", Excluded::Trash),
        ];
        let all = [
            ("/x", "A.app"),
            ("/x", "node_modules"),
            ("/x", ".git"),
            (HOME, "Library"),
            (HOME, ".Trash"),
        ];
        for (rules, parent, name, expected) in cases {
            for (p, n) in all {
                let want = (p == parent && n == name).then_some(expected);
                assert_eq!(check(&rules, p, n), want, "{n} with only {expected:?} on");
            }
        }
    }

    #[test]
    fn none_excludes_nothing_and_default_excludes_everything_listed() {
        let none = Exclusions::none();
        let all = Exclusions::default();
        for (parent, name) in [
            ("/x", "A.app"),
            ("/x", "node_modules"),
            ("/x", ".git"),
            (HOME, "Library"),
            (HOME, ".Trash"),
        ] {
            assert_eq!(check(&none, parent, name), None, "{name}");
            assert!(check(&all, parent, name).is_some(), "{name}");
        }
    }
}
