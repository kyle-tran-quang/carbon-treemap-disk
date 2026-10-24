//! The macOS catalog for Safe cache cleanup.
//!
//! Every root here is a place apps rebuild on demand. Adding a root is a
//! deliberate act: check it on a real machine first (sub-task 9).

use std::path::Path;

use crate::core::cleanup_catalog::{CatalogRoot, Category, NameRules};

/// `com.apple.*` folders belong to system services (iCloud sync, Spotlight)
/// and removing them can trigger resyncs, so they are never offered.
const EXCLUDED_PREFIXES: &[&str] = &["com.apple."];

pub fn catalog_roots(home: &Path) -> Vec<CatalogRoot> {
    let library = home.join("Library");
    vec![
        CatalogRoot::children(library.join("Caches"), Category::AppCaches),
        CatalogRoot::children(library.join("Logs"), Category::AppLogs),
        CatalogRoot::children(
            library.join("Developer/Xcode/DerivedData"),
            Category::XcodeBuildCache,
        ),
        CatalogRoot::whole(home.join(".npm/_cacache"), Category::NpmCache),
    ]
}

/// `own_names` are names tied to this app, such as its bundle identifier and
/// binary name. Entries with these names are never offered. Empty names are
/// ignored.
pub fn name_rules(own_names: &[&str]) -> NameRules {
    NameRules::new(
        EXCLUDED_PREFIXES,
        own_names.iter().copied().filter(|name| !name.is_empty()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::cleanup_catalog::Offer;
    use std::path::PathBuf;

    fn roots() -> Vec<CatalogRoot> {
        catalog_roots(Path::new("/Users/me"))
    }

    #[test]
    fn the_catalog_has_exactly_the_four_safe_locations() {
        let paths: Vec<PathBuf> = roots().into_iter().map(|r| r.path).collect();
        assert_eq!(
            paths,
            [
                "/Users/me/Library/Caches",
                "/Users/me/Library/Logs",
                "/Users/me/Library/Developer/Xcode/DerivedData",
                "/Users/me/.npm/_cacache",
            ]
            .map(PathBuf::from)
        );
    }

    #[test]
    fn folders_that_hold_user_data_are_not_in_the_catalog() {
        let forbidden = [
            "Application Support",
            "Preferences",
            "Keychains",
            "Containers",
            "Group Containers",
            "Mail",
            "Photos",
            "Documents",
            "Desktop",
            "Downloads",
        ];
        for root in roots() {
            for name in forbidden {
                assert!(
                    !root.path.components().any(|c| c.as_os_str() == name),
                    "{} must not be inside the catalog: {}",
                    name,
                    root.path.display()
                );
            }
        }
    }

    #[test]
    fn only_the_npm_cache_is_offered_whole() {
        for root in roots() {
            let whole = root.offer == Offer::Whole;
            assert_eq!(whole, root.category == Category::NpmCache);
        }
    }

    #[test]
    fn apple_folders_and_the_apps_own_folder_are_excluded() {
        let rules = name_rules(&["com.carbon.treemap", "carbon-treemap-disk"]);
        assert!(rules.excludes("com.apple.Music"));
        assert!(rules.excludes("com.carbon.treemap"));
        assert!(rules.excludes("carbon-treemap-disk"), "the binary name too");
        assert!(!rules.excludes("Homebrew"));
    }

    /// Read-only run against the real home folder. Run with:
    /// `cargo test real_home -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_home_discovery_report() {
        use crate::core::cleanup_catalog::RECENT_WINDOW;
        use crate::core::cleanup_discovery::{DiscoveryContext, discover};
        use crate::core::scan::CancelToken;
        use crate::platform::macos::{MacScanner, MacSizeAccounting};
        use std::time::{Instant, SystemTime};

        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        let roots = catalog_roots(&home);
        let rules = name_rules(&["com.carbon.treemap", "carbon-treemap-disk"]);
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
        let started = Instant::now();
        let found = discover(&ctx, &CancelToken::default(), &|_| {}).unwrap();
        println!("elapsed: {:?}", started.elapsed());
        println!(
            "candidates: {}  total: {:.2} GB  excluded by name: {}  skipped: {}",
            found.candidates.len(),
            found.total_bytes() as f64 / 1e9,
            found.excluded,
            found.skipped.len()
        );
        for c in found.candidates.iter().take(12) {
            println!(
                "{:>9.1} MB  {:?}  {}{}",
                c.size as f64 / 1e6,
                c.category,
                c.path.display(),
                if c.incomplete { "  (incomplete)" } else { "" }
            );
        }
        for s in &found.skipped {
            println!("skipped {:?}: {}", s.reason, s.path.display());
        }
    }

    #[test]
    fn an_empty_identifier_still_excludes_apple_folders() {
        let rules = name_rules(&[""]);
        assert!(rules.excludes("com.apple.Safari"));
        assert!(!rules.excludes(""), "an empty name is not a rule");
    }
}
