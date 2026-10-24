//! macOS platform adapter.
//! Implements the platform traits from `platform/` using macOS APIs.

mod bulk;
mod catalog;
mod cloud;
mod guardrails;
mod permissions;
mod scanner;
mod size_accounting;
mod trash;

pub use catalog::{catalog_roots, name_rules};
pub use guardrails::{AppPaths, app_bundle_or_exe, protected_paths};
#[allow(unused_imports)]
pub use permissions::MacPermissionChecker;
#[allow(unused_imports)]
pub use scanner::{MacScanner, ReadMode};
pub use size_accounting::MacSizeAccounting;
pub use trash::MacTrashProvider;
