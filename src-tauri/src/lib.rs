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
        .manage(commands::cleanup_scan::CleanupScanState::default())
        .manage(commands::duplicates::DuplicateState::default())
        .invoke_handler(tauri::generate_handler![
            commands::scan::start_scan,
            commands::scan::cancel_scan,
            commands::view::get_view,
            commands::view::find_node_by_path,
            commands::cleanup::plan_cleanup_items,
            commands::cleanup::execute_cleanup,
            commands::cleanup_scan::discover_cleanup,
            commands::cleanup_scan::cancel_discovery,
            commands::cleanup_scan::plan_cleanup_candidates,
            commands::cleanup_scan::execute_cleanup_candidates,
            commands::duplicates::start_duplicate_scan,
            commands::duplicates::cancel_duplicate_scan,
            commands::duplicates::get_duplicate_groups,
            commands::duplicates::suggest_duplicate_picks,
            commands::duplicates::plan_duplicate_cleanup,
            commands::duplicates::execute_duplicate_cleanup,
            commands::permissions::check_full_disk_access,
            commands::permissions::open_full_disk_access_settings,
            commands::permissions::preview_file
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
