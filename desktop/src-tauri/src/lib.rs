pub mod ai_assistant;
pub mod auth;
pub mod commands;
pub mod crash_diagnostics;
pub mod crash_investigator;
pub mod db;
pub mod dependency_ops;
pub use agora_core::{download, error, loader_manifests, models};

pub mod governance;
pub mod install_pipeline;
pub mod instances;
pub mod launcher_profiles;
pub mod mod_install;
pub mod modrinth_raw;
pub mod mojang;
pub use agora_core::override_sanitizer;
pub mod mcp;
pub mod paths;
pub mod registry;
pub mod registry_sync;
pub use agora_core::state;
pub mod version_cache;

use state::LauncherState;
use tauri::Manager;

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
        .manage(mcp::McpServerManager::default())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_sql::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            commands::browse_items,
            commands::for_you_items,
            commands::get_registry_item,
            commands::list_categories,
            commands::list_pack_mods,
            commands::list_audit_log,
            commands::check_registry_update,
            commands::get_registry_status,
            commands::extract_overrides,
            commands::list_instances,
            commands::get_instance_detail,
            commands::create_instance,
            commands::delete_instance,
            commands::unlock_instance,
            commands::lock_instance,
            commands::rename_instance,
            commands::revert_instance,
            commands::launch_instance,
            commands::launch_instance_direct,
            commands::query_launch_state,
            commands::resolve_install_plan,
            commands::apply_install_plan,
            commands::cancel_install,
            commands::check_instance_updates,
            commands::get_lkg_marker,
            commands::export_lockfile,
            commands::import_lockfile,
            commands::detect_drift,
            commands::verify_lockfile,
            commands::repair_lockfile,
            commands::list_snapshots,
            commands::create_snapshot,
            commands::restore_snapshot,
            commands::delete_snapshot,
            commands::list_loadout_profiles,
            commands::create_loadout_profile,
            commands::apply_loadout_profile,
            commands::delete_loadout_profile,
            commands::import_instance,
            commands::detect_launchers,
            commands::clone_instance_cmd,
            commands::check_instance_health,
            commands::list_loader_versions,
            commands::list_manifest_loaders,
            commands::list_manifest_mc_versions,
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
            commands::list_mod_versions_load_more,
            commands::check_mod_compat,
            commands::batch_check_compat,
            commands::pick_open_file,
            commands::explain_crash,
            commands::export_instance_pack,
            commands::import_instance_pack,
            commands::is_modrinth_enabled,
            commands::search_modrinth,
            commands::list_modrinth_categories,
            commands::list_modrinth_loaders,
            commands::list_modrinth_game_versions,
            commands::list_raw_modrinth_versions,
            commands::fetch_modrinth_project,
            commands::list_under_review_items,
            commands::list_recent_resolutions,
            commands::list_mod_reviews,
            commands::fetch_triage_poll,
            commands::flag_review,
            commands::get_flag_rate_limit,
            commands::investigate_crash,
            commands::investigate_manual,
            commands::disable_mod_for_test,
            commands::enable_mod_for_test,
            commands::disable_instance_mod,
            commands::enable_instance_mod,
            commands::confirm_crash_fix,
            commands::report_still_crashing,
            commands::get_disable_plan,
            commands::get_removal_plan,
            commands::get_install_plan,
            commands::enable_mod_with_auto_deps,
            commands::start_mcp_server,
            commands::stop_mcp_server,
            commands::get_mcp_status,
            commands::get_mcp_token,
            commands::regenerate_mcp_token,
            commands::get_mcp_skill_content,
            commands::set_mcp_approval,
            commands::copilot_login,
            commands::copilot_try_governance_token,
            commands::copilot_login_poll,
            commands::copilot_status,
            commands::copilot_logout,
            commands::ai_chat,
            commands::msa_login,
            commands::msa_get_status,
            commands::msa_refresh,
            commands::msa_logout,
            commands::compute_gc_args,
            commands::browse_search,
            commands::browse_load_more,
            commands::browse_page,
            commands::export_server_environment,
            commands::kill_process,
            commands::install_pack,
            commands::import_modrinth_pack_by_url,
            commands::get_curated_annotation,
            commands::get_windows_accent_color,
            commands::detect_mojang_launcher,
            commands::test_launcher_path,
            commands::repair_instance_loader,
            commands::list_java_runtimes,
            commands::ensure_java_runtime,
            commands::remove_unused_java_runtimes,
            commands::inspect_java_executable,
            commands::update_instance_java,
            commands::cancel_java_runtime,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            tauri::async_runtime::block_on(async move {
                if let Err(e) = db::init_local_state_db(&handle) {
                    eprintln!("Failed to initialize local state: {}", e);
                }
                // Dev-only: seed registry.db from a local compiler build when
                // running `tauri:dev`. The re-seed path copies an unverified
                // local db+sig pair (acceptable in debug builds, which relax
                // signature checks) â€” must NEVER run in release binaries, where
                // it could overwrite the CI-signed registry from any
                // registry.db found in the cwd parent walk.
                #[cfg(debug_assertions)]
                if let Err(e) = crate::registry_sync::seed_from_local_build(&handle) {
                    eprintln!("Failed to seed registry: {}", e);
                }
                let purge_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let _ = tokio::task::spawn_blocking(move || {
                        if let Ok(conn) = db::local_state_connection(&purge_handle) {
                            if let Err(e) = db::purge_stale_crash_telemetry(&conn) {
                                eprintln!("Failed to purge stale crash telemetry: {}", e);
                            }
                        }
                    })
                    .await;
                });
                // Start MCP server if enabled.
                match db::local_state_connection(&handle) {
                    Ok(conn) => {
                        if matches!(
                            db::get_setting(&conn, "ai_mcp_enabled"),
                            Ok(Some(serde_json::Value::Bool(true)))
                        ) {
                            let mcp_app = handle.clone();
                            tauri::async_runtime::spawn(async move {
                                let manager = mcp_app.state::<crate::mcp::McpServerManager>();
                                if let Err(e) = manager.start(mcp_app.clone()).await {
                                    eprintln!("Failed to start MCP server: {}", e);
                                }
                            });
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to open local state for MCP startup: {}", e);
                    }
                }
            });
            Ok(())
        })
        .on_window_event(|app, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                if let Some(manager) = app.try_state::<crate::mcp::McpServerManager>() {
                    manager.request_shutdown();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
