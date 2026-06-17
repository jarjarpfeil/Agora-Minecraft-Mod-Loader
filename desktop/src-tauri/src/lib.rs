pub mod commands;
pub mod db;
pub mod download;
pub mod error;
pub mod instances;
pub mod launcher_profiles;
pub mod loader_manifests;
pub mod models;
pub mod mojang;
pub mod paths;
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
            commands::query_registry,
            commands::list_instances,
            commands::get_instance_detail,
            commands::create_instance,
            commands::delete_instance,
            commands::launch_instance,
            commands::list_loader_versions,
            commands::get_setting,
            commands::set_setting
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            tauri::async_runtime::block_on(async move {
                if let Err(e) = db::init_local_state_db(&handle) {
                    eprintln!("Failed to initialize local state: {}", e);
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
