//! OS-agnostic data model and scan orchestration.
//! No platform-specific code lives here.

pub mod cleanup;
pub mod cleanup_catalog;
pub mod cleanup_discovery;
pub mod cleanup_log;
pub mod cleanup_plan;
pub mod config;
pub mod duplicate_exclusions;
pub mod duplicate_keep;
pub mod duplicate_plan;
pub mod duplicates;
pub mod scan;
pub mod tree;
pub mod view;
