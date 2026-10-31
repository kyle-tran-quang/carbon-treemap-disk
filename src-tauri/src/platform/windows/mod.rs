//! Windows platform adapter.
//! Implements the platform traits from `platform/` using Windows APIs.

mod catalog;
mod content_hasher;
mod file_walker;
mod guardrails;
mod permissions;
mod scanner;
mod size_accounting;
mod trash;

pub use catalog::{catalog_roots, name_rules};
#[allow(unused_imports)]
pub use content_hasher::WinContentHasher;
#[allow(unused_imports)]
pub use file_walker::WinFileWalker;
pub use guardrails::{AppPaths, protected_paths};
#[allow(unused_imports)]
pub use permissions::WinPermissionChecker;
#[allow(unused_imports)]
pub use scanner::WinScanner;
pub use size_accounting::WinSizeAccounting;
pub use trash::WinTrashProvider;
