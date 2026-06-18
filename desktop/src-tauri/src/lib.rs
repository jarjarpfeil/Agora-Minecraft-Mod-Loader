pub mod commands;
pub mod auth;
pub mod crash_diagnostics;
pub mod db;
pub mod download;
pub mod error;
pub mod instances;
pub mod launcher_profiles;
pub mod loader_manifests;
pub mod models;
pub mod mojang;
pub mod override_sanitizer;
pub mod paths;
pub mod registry;
pub mod registry_sync;
pub mod state;

use state::LauncherState;

/// Run the Tauri application.
pub fn run() {
    tauri::Builder::default()
        .manage(LauncherState::default())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_sql::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            commands::greet,
            commands::browse_items,
            commands::get_registry_item,
            commands::list_categories,
            commands::check_registry_update,
            commands::get_registry_status,
            commands::extract_overrides,
            commands::list_instances,
            commands::get_instance_detail,
            commands::create_instance,
            commands::delete_instance,
            commands::launch_instance,
            commands::list_loader_versions,
            commands::get_setting,
            commands::set_setting,
            commands::github_login,
            commands::github_login_poll,
            commands::github_logout,
            commands::get_auth_status,
            commands::get_github_profile,
            commands::check_instance_crash,
            commands::triage_crash_report,
            commands::list_crash_reports_cmd,
            commands::read_crash_log_cmd
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            tauri::async_runtime::block_on(async move {
                if let Err(e) = db::init_local_state_db(&handle) {
                    eprintln!("Failed to initialize local state: {}", e);
                }
                if let Err(e) = crate::registry_sync::seed_from_local_build(&handle) {
                    eprintln!("Failed to seed registry: {}", e);
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
