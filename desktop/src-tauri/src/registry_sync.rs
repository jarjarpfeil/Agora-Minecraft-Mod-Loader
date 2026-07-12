//! Thin compat shim: preserves the original `&tauri::AppHandle` signatures
//! so no caller across the desktop crate needs to change.
//!
//! Internally this module resolves Tauri-specific paths from the handle
//! once, then delegates to `agora_core::registry_sync` for actual logic.

pub use agora_core::registry_sync::RegistryStatus;

/// Check GitHub Releases for a newer registry.db and download + verify it.
pub async fn check_and_download_update<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    force: bool,
) -> agora_core::error::LauncherResult<RegistryStatus> {
    let base =
        crate::paths::app_data_dir(app).map_err(|e| agora_core::error::LauncherError::Generic {
            code: "ERR_APP_DATA_DIR".to_string(),
            message: e.to_string(),
        })?;
    let ls_path = crate::paths::local_state_db_path(app).map_err(|e| {
        agora_core::error::LauncherError::Generic {
            code: "ERR_LOCAL_STATE_PATH".to_string(),
            message: e.to_string(),
        }
    })?;
    let token = crate::auth::get_token(app);
    agora_core::registry_sync::check_and_download_update(&base, &ls_path, force, token).await
}

/// Return the current registry status without performing a network check.
pub fn get_status<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> RegistryStatus {
    let base = crate::paths::app_data_dir(app).ok();
    let ls_path = crate::paths::local_state_db_path(app).ok();
    match (base, ls_path) {
        (Some(b), Some(l)) => agora_core::registry_sync::get_status(&b, &l),
        _ => RegistryStatus {
            has_cached_db: false,
            cached_tag: None,
            cached_schema_version: None,
            latest_tag: None,
            update_available: false,
            checked: false,
            message: "No registry database found. Click Check for Updates.".to_string(),
        },
    }
}

/// On first run with no cached DB, copy the local registry.db from the repo
/// if it exists. Development convenience for local testing.
#[cfg(debug_assertions)]
pub fn seed_from_local_build<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> agora_core::error::LauncherResult<bool> {
    let base =
        crate::paths::app_data_dir(app).map_err(|e| agora_core::error::LauncherError::Generic {
            code: "ERR_APP_DATA_DIR".to_string(),
            message: e.to_string(),
        })?;
    agora_core::registry_sync::seed_from_local_build(&base)
}
