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
pub mod mod_install;
pub mod modrinth_raw;
pub mod mojang;
pub mod override_sanitizer;
pub mod paths;
pub mod registry;
pub mod registry_sync;
pub mod state;

use state::LauncherState;

/// Run the Tauri application.
pub fn run() {
    // Log startup so the user can verify from the log file that they are
    // actually running the freshly-compiled binary (not a stale one). When
    // diagnosing OAuth issues, the absence of this line means the running
    // app predates the latest cargo build.
    crate::auth::log_line(&format!(
        "AGORA BIN STARTED build_nonce={}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    ));
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
            commands::list_pack_mods,
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
            commands::read_crash_log_cmd,
            commands::list_mod_versions,
            commands::install_mod_version,
            commands::remove_mod_from_instance,
            commands::is_modrinth_enabled,
            commands::search_modrinth,
            commands::list_modrinth_categories,
            commands::list_modrinth_loaders,
            commands::list_modrinth_game_versions,
            commands::list_raw_modrinth_versions,
            commands::install_raw_modrinth
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
