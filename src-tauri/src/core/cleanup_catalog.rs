//! Rules for the Safe cache cleanup. OS-agnostic, and free of I/O.
//!
//! The catalog is an allowlist: a fixed set of roots supplied by the
//! platform layer. A path that is not under a root can never become a
//! candidate. This module holds only the pure decisions (which names are
//! left out, what counts as recently used, whether a resolved path stayed
//! inside its root); discovery that touches the disk builds on them.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Serialize;

/// Anything modified within this window is skipped as probably in use.
/// A starting point to tune against real data. Directory times only change
/// when direct entries change, so this is a partial safeguard.
pub const RECENT_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Category {
    AppCaches,
    AppLogs,
    XcodeBuildCache,
    NpmCache,
}

/// What a root offers as candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Offer {
    /// Each direct child of the root. The root itself is never offered.
    Children,
    /// The root itself, as a single candidate.
    Whole,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogRoot {
    pub path: PathBuf,
    pub category: Category,
    pub offer: Offer,
}

impl CatalogRoot {
    pub fn children(path: impl Into<PathBuf>, category: Category) -> Self {
        Self {
            path: path.into(),
            category,
            offer: Offer::Children,
        }
    }

    pub fn whole(path: impl Into<PathBuf>, category: Category) -> Self {
        Self {
            path: path.into(),
            category,
            offer: Offer::Whole,
        }
    }
}

/// Names that are never offered, matched case-insensitively because default
/// macOS volumes are.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NameRules {
    prefixes: Vec<String>,
    names: Vec<String>,
}

impl NameRules {
    pub fn new<P, N>(prefixes: P, names: N) -> Self
    where
        P: IntoIterator,
        P::Item: AsRef<str>,
        N: IntoIterator,
        N::Item: AsRef<str>,
    {
        let lower = |s: &str| s.to_lowercase();
        Self {
            prefixes: prefixes.into_iter().map(|s| lower(s.as_ref())).collect(),
            names: names.into_iter().map(|s| lower(s.as_ref())).collect(),
        }
    }

    pub fn excludes(&self, name: &str) -> bool {
        let name = name.to_lowercase();
        self.names.contains(&name) || self.prefixes.iter().any(|p| name.starts_with(p))
    }
}

/// Whether `modified` falls inside `window` before `now`. A time in the
/// future (clock change) counts as recent: when unsure, do not offer.
pub fn is_recent(modified: SystemTime, now: SystemTime, window: Duration) -> bool {
    match now.duration_since(modified) {
        Ok(age) => age < window,
        Err(_) => true,
    }
}

/// Whether a candidate, after resolving symlinks, is still where its root
/// says it should be. Both paths must already be canonical.
///
/// `Children` roots require a strict descendant, so the root cannot be its
/// own candidate. `Whole` roots require the root itself. A child that is a
/// symlink pointing elsewhere resolves outside the root and fails this check.
pub fn stays_inside(root: &Path, candidate: &Path, offer: Offer) -> bool {
    match offer {
        Offer::Children => candidate != root && candidate.starts_with(root),
        Offer::Whole => candidate == root,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> NameRules {
        NameRules::new(["com.apple."], ["com.carbon.treemap"])
    }

    #[test]
    fn apple_prefixed_names_are_excluded_whatever_the_case() {
        let r = rules();
        assert!(r.excludes("com.apple.Music"));
        assert!(r.excludes("COM.APPLE.Safari"));
        assert!(r.excludes("com.apple."));
    }

    #[test]
    fn the_apps_own_name_is_excluded_exactly() {
        let r = rules();
        assert!(r.excludes("com.carbon.treemap"));
        assert!(r.excludes("Com.Carbon.Treemap"));
        assert!(!r.excludes("com.carbon.treemap2"));
    }

    #[test]
    fn other_names_are_kept() {
        let r = rules();
        for name in ["Homebrew", "Google", "org.swift.swiftpm", "applecom", "x"] {
            assert!(!r.excludes(name), "{name}");
        }
    }

    #[test]
    fn empty_rules_exclude_nothing() {
        assert!(!NameRules::default().excludes("anything"));
    }

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn recent_means_strictly_inside_the_window() {
        let window = Duration::from_secs(100);
        let now = at(1_000);
        assert!(is_recent(at(1_000), now, window), "modified just now");
        assert!(is_recent(at(901), now, window), "99 seconds old");
        assert!(!is_recent(at(900), now, window), "exactly the window");
        assert!(!is_recent(at(10), now, window), "old");
    }

    #[test]
    fn a_modification_time_in_the_future_counts_as_recent() {
        assert!(is_recent(at(2_000), at(1_000), Duration::from_secs(100)));
    }

    #[test]
    fn children_roots_accept_descendants_but_not_the_root_or_outsiders() {
        let root = Path::new("/h/Library/Caches");
        let check = |p: &str| stays_inside(root, Path::new(p), Offer::Children);
        assert!(check("/h/Library/Caches/Homebrew"));
        assert!(check("/h/Library/Caches/Homebrew/downloads"));
        assert!(!check("/h/Library/Caches"), "the root is never a candidate");
        assert!(!check("/h/Library"), "the parent");
        assert!(
            !check("/h/Library/Caches2/x"),
            "a sibling with a shared prefix"
        );
        assert!(!check("/etc/passwd"), "a symlink resolved elsewhere");
    }

    #[test]
    fn whole_roots_accept_only_the_root() {
        let root = Path::new("/h/.npm/_cacache");
        let check = |p: &str| stays_inside(root, Path::new(p), Offer::Whole);
        assert!(check("/h/.npm/_cacache"));
        assert!(!check("/h/.npm/_cacache/content-v2"));
        assert!(!check("/h/.npm"));
        assert!(!check("/elsewhere/_cacache"));
    }

    #[test]
    fn constructors_set_the_offer() {
        assert_eq!(
            CatalogRoot::children("/a", Category::AppCaches).offer,
            Offer::Children
        );
        assert_eq!(
            CatalogRoot::whole("/a", Category::NpmCache).offer,
            Offer::Whole
        );
    }

    #[test]
    fn categories_serialize_in_camel_case() {
        assert_eq!(
            serde_json::to_value(Category::XcodeBuildCache).unwrap(),
            "xcodeBuildCache"
        );
    }
}
