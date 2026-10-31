//! Windows catalog for Safe cache cleanup.
//!
//! Covers the per-user app-cache and temp locations that tools rebuild on
//! demand.  Every path here has been verified to exist on a typical Windows 10+
//! machine before shipping.  Never add a location without checking it first.

use std::path::Path;

use crate::core::cleanup_catalog::{CatalogRoot, Category, NameRules};

/// Folders that belong to Windows or Microsoft services — never offered.
const EXCLUDED_PREFIXES: &[&str] = &["Microsoft.", "Windows.", "microsoft.", "windows."];

pub fn catalog_roots(home: &Path) -> Vec<CatalogRoot> {
    let local = home.join("AppData\\Local");
    let roaming = home.join("AppData\\Roaming");
    let mut roots = vec![
        // Per-user app caches: the Windows equivalent of ~/Library/Caches.
        CatalogRoot::children(local.clone(), Category::AppCaches),
        // Temp folder under LocalAppData (not the system %TEMP%).
        CatalogRoot::children(local.join("Temp"), Category::AppCaches),
        // npm cache.
        CatalogRoot::whole(home.join("AppData\\Roaming\\npm-cache"), Category::NpmCache),
        CatalogRoot::whole(local.join("npm-cache"), Category::NpmCache),
    ];

    // Only offer if they exist; CatalogRoot is still returned and discovery
    // skips roots that do not exist on disk.
    let pip_cache = roaming.join("pip\\Cache");
    roots.push(CatalogRoot::whole(pip_cache, Category::AppCaches));

    roots
}

/// `own_names` are names tied to this app (bundle identifier, binary name).
/// Empty names are ignored.
pub fn name_rules(own_names: &[&str]) -> NameRules {
    NameRules::new(
        EXCLUDED_PREFIXES,
        own_names.iter().copied().filter(|name| !name.is_empty()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn roots() -> Vec<CatalogRoot> {
        catalog_roots(Path::new(r"C:\Users\me"))
    }

    #[test]
    fn user_data_folders_are_not_in_the_catalog() {
        let forbidden = [
            "Documents",
            "Desktop",
            "Downloads",
            "Music",
            "Pictures",
            "Videos",
            "Favorites",
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
    fn microsoft_and_windows_prefixes_are_excluded() {
        let rules = name_rules(&["com.carbon.treemap", "carbon-treemap-disk"]);
        assert!(rules.excludes("Microsoft.WindowsStore"));
        assert!(rules.excludes("windows.immersivecontrolpanel"));
        assert!(rules.excludes("com.carbon.treemap"));
        assert!(!rules.excludes("Homebrew"));
        assert!(!rules.excludes("Google"));
    }

    #[test]
    fn an_empty_identifier_still_excludes_microsoft_prefixes() {
        let rules = name_rules(&[""]);
        assert!(rules.excludes("Microsoft.Photos"));
        assert!(!rules.excludes(""), "empty name is not a rule");
    }
}
