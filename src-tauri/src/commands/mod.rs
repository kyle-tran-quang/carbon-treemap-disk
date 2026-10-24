//! Tauri IPC commands.
//! Each public command is registered in `run()` via `generate_handler!`.

pub mod cleanup;
pub mod cleanup_scan;
pub mod permissions;
pub mod scan;
pub mod view;
