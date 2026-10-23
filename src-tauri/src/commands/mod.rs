//! Tauri IPC commands.
//! Each public command is registered in `run()` via `generate_handler!`.

pub mod cleanup;
pub mod permissions;
pub mod scan;
pub mod view;
