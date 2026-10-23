mod commands;
// Remove the dead_code allowances as sub-tasks 3 and 5 start consuming these modules.
#[allow(dead_code)]
mod core;
#[allow(dead_code)]
mod platform;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(commands::scan::ScanState::default())
        .manage(commands::cleanup::CleanupState::default())
        .invoke_handler(tauri::generate_handler![
            commands::scan::start_scan,
            commands::scan::cancel_scan,
            commands::view::get_view,
            commands::view::find_node_by_path,
            commands::cleanup::plan_cleanup_items,
            commands::cleanup::execute_cleanup,
            commands::permissions::check_full_disk_access,
            commands::permissions::open_full_disk_access_settings
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
