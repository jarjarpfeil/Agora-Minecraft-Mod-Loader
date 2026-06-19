use crate::auth::{DeviceFlowResponse, GithubProfile};
use crate::crash_diagnostics::{self, CrashReportInfo, CrashTriageResult};
use crate::db;
use crate::error::{LauncherError, LauncherResult};
use crate::instances::{self, CreateInstanceRequest, InstanceDetail, LoaderVersionSummary};
use crate::mod_install;
use crate::models::{InstanceRow, InstalledMod, ModVersionCandidate};
use crate::modrinth_raw;
use crate::registry::{self, CategoryInfo, PackModRow, RegistryItem, SortOption};
use crate::state::LauncherState;

#[tauri::command]
pub async fn greet(name: String) -> String {
    format!("Hello, {}!", name)
}

/// Browse registry items with typed filters (replaces raw-SQL queryRegistry).
///
/// When `modrinth_enabled` is false, items with `download_strategy = 'modrinth_id'`
/// are excluded from results.
#[tauri::command]
pub async fn browse_items(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    content_type: Option<String>,
    category: Option<String>,
    sort: Option<SortOption>,
    modrinth_enabled: Option<bool>,
    mc_version: Option<String>,
    loader: Option<String>,
    limit: Option<i64>,
) -> LauncherResult<Vec<RegistryItem>> {
    tokio::task::spawn_blocking(move || {
        let conn = registry::open_registry(&app)?;
        registry::browse_items(
            &conn,
            content_type.as_deref(),
            category.as_deref(),
            &sort.unwrap_or_default(),
            modrinth_enabled.unwrap_or(false),
            mc_version.as_deref(),
            loader.as_deref(),
            limit.unwrap_or(100),
        )
    })
    .await
    .map_err(|_| LauncherError::Generic {
        code: "ERR_REGISTRY_QUERY".to_string(),
        message: "Registry query task failed.".to_string(),
    })?
}

/// Fetch a single registry item by ID.
#[tauri::command]
pub async fn get_registry_item(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    item_id: String,
) -> LauncherResult<Option<RegistryItem>> {
    tokio::task::spawn_blocking(move || {
        let conn = registry::open_registry(&app)?;
        registry::get_item_by_id(&conn, &item_id)
    })
    .await
    .map_err(|_| LauncherError::Generic {
        code: "ERR_REGISTRY_QUERY".to_string(),
        message: "Registry query task failed.".to_string(),
    })?
}

/// List all categories from the registry.
#[tauri::command]
pub async fn list_categories(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
) -> LauncherResult<Vec<CategoryInfo>> {
    tokio::task::spawn_blocking(move || {
        let conn = registry::open_registry(&app)?;
        registry::list_categories(&conn)
    })
    .await
    .map_err(|_| LauncherError::Generic {
        code: "ERR_REGISTRY_QUERY".to_string(),
        message: "Registry query task failed.".to_string(),
    })?
}

/// List all mods in a pack.
#[tauri::command]
pub async fn list_pack_mods(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    pack_id: String,
) -> LauncherResult<Vec<PackModRow>> {
    tokio::task::spawn_blocking(move || {
        let conn = registry::open_registry(&app)?;
        registry::pack_mods_for_pack(&conn, &pack_id)
    })
    .await
    .map_err(|_| LauncherError::Generic {
        code: "ERR_REGISTRY_QUERY".to_string(),
        message: "Pack mods query task failed.".to_string(),
    })?
}

/// List all user instances from `local_state.db`.
#[tauri::command]
pub async fn list_instances(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
) -> LauncherResult<Vec<InstanceRow>> {
    tokio::task::spawn_blocking(move || instances::list_instances(&app))
        .await
        .map_err(|_| LauncherError::LocalStateFailed)?
}

/// Fetch a single instance plus its on-disk manifest.
#[tauri::command]
pub async fn get_instance_detail(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    instance_id: String,
) -> LauncherResult<Option<InstanceDetail>> {
    tokio::task::spawn_blocking(move || instances::get_instance_detail(&app, &instance_id))
        .await
        .map_err(|_| LauncherError::LocalStateFailed)?
}

/// Create a custom instance and inject its modloader.
#[tauri::command]
pub async fn create_instance(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    request: CreateInstanceRequest,
) -> LauncherResult<InstanceRow> {
    instances::create_instance(app, request).await
}

/// Delete an instance, moving its directory to the OS trash.
#[tauri::command]
pub async fn delete_instance(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    instance_id: String,
) -> LauncherResult<()> {
    tokio::task::spawn_blocking(move || instances::delete_instance(&app, &instance_id))
        .await
        .map_err(|_| LauncherError::LocalStateFailed)?
}

/// Launch an instance via the official Mojang launcher delegation.
#[tauri::command]
pub async fn launch_instance(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    instance_id: String,
) -> LauncherResult<()> {
    tokio::task::spawn_blocking(move || instances::launch_instance(&app, &instance_id))
        .await
        .map_err(|_| LauncherError::LocalStateFailed)?
}

/// List pinned loader versions for a loader + Minecraft version.
#[tauri::command]
pub async fn list_loader_versions(
    _state: tauri::State<'_, LauncherState>,
    loader: String,
    mc_version: String,
) -> LauncherResult<Vec<LoaderVersionSummary>> {
    Ok(instances::list_loader_versions(&loader, &mc_version))
}

/// Read a JSON-encoded setting from `local_state.db`.
#[tauri::command]
pub async fn get_setting(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    key: String,
) -> LauncherResult<Option<serde_json::Value>> {
    tokio::task::spawn_blocking(move || {
        let conn = db::local_state_connection(&app).map_err(|_| LauncherError::LocalStateFailed)?;
        db::get_setting(&conn, &key).map_err(|_| LauncherError::LocalStateFailed)
    })
    .await
    .map_err(|_| LauncherError::LocalStateFailed)?
}

/// Upsert a JSON-encoded setting into `local_state.db`.
#[tauri::command]
pub async fn set_setting(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    key: String,
    value: serde_json::Value,
) -> LauncherResult<()> {
    tokio::task::spawn_blocking(move || {
        let conn = db::local_state_connection(&app).map_err(|_| LauncherError::LocalStateFailed)?;
        db::set_setting(&conn, &key, &value).map_err(|_| LauncherError::LocalStateFailed)
    })
    .await
    .map_err(|_| LauncherError::LocalStateFailed)?
}

/// Check GitHub Releases for a registry.db update and download + verify it.
#[tauri::command]
pub async fn check_registry_update(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    force: Option<bool>,
) -> LauncherResult<crate::registry_sync::RegistryStatus> {
    crate::registry_sync::check_and_download_update(&app, force.unwrap_or(false)).await
}

/// Return current registry status without network check.
#[tauri::command]
pub async fn get_registry_status(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
) -> LauncherResult<crate::registry_sync::RegistryStatus> {
    Ok(crate::registry_sync::get_status(&app))
}

/// Extract a pack override zip into an instance directory with full sanitization.
///
/// Implements §7.2: directory whitelist, zip-bomb limits, banned extensions,
/// and Zip Slip protection.
#[tauri::command]
pub async fn extract_overrides(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    zip_path: String,
    instance_id: String,
) -> LauncherResult<crate::override_sanitizer::ExtractionResult> {
    let zip = std::path::PathBuf::from(zip_path);
    let dest = crate::paths::instance_dir(&app, &instance_id)
        .map_err(|_| LauncherError::InstanceCreateFailed)?;
    tokio::task::spawn_blocking(move || {
        crate::override_sanitizer::extract_overrides(&zip, &dest)
    })
    .await
    .map_err(|_| LauncherError::Generic {
        code: "ERR_OVERRIDE_FAILED".to_string(),
        message: "Extraction task failed.".to_string(),
    })?
}

/// Begin the GitHub OAuth Device Flow and return the code the user must enter.
#[tauri::command]
pub async fn github_login() -> LauncherResult<DeviceFlowResponse> {
    crate::auth::start_device_flow().await
}

/// Poll the GitHub token endpoint until the user authorizes the device.
/// Returns true if the token was obtained and stored; false if still pending.
#[tauri::command]
pub async fn github_login_poll(
    app: tauri::AppHandle,
    device_code: String,
    interval: u64,
) -> LauncherResult<bool> {
    crate::auth::log_line(&format!(
        "github_login_poll command ENTERED device_code_len={} interval={}",
        device_code.len(),
        interval
    ));
    let token = crate::auth::poll_device_flow(device_code, interval).await?;
    if let Some(t) = token {
        crate::auth::store_token(&app, &t)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Sign out by deleting any stored GitHub token.
#[tauri::command]
pub async fn github_logout(app: tauri::AppHandle) -> Result<(), String> {
    crate::auth::clear_token(&app)
}

/// Whether a GitHub token is currently stored.
#[tauri::command]
pub async fn get_auth_status(app: tauri::AppHandle) -> bool {
    crate::auth::is_authenticated(&app)
}

/// Fetch the authenticated user's GitHub profile, if signed in.
#[tauri::command]
pub async fn get_github_profile(app: tauri::AppHandle) -> LauncherResult<Option<GithubProfile>> {
    match crate::auth::get_token(&app) {
        Some(token) => Ok(Some(crate::auth::get_github_user(token).await?)),
        None => Ok(None),
    }
}

/// Check whether a fresh crash report appeared after the instance's last launch.
#[tauri::command]
pub async fn check_instance_crash(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    instance_id: String,
) -> LauncherResult<Option<CrashReportInfo>> {
    tokio::task::spawn_blocking(move || crash_diagnostics::check_for_crash(&app, &instance_id))
        .await
        .map_err(|_| LauncherError::LocalStateFailed)?
}

/// Triage a crash log against curated signatures from the registry.
#[tauri::command]
pub async fn triage_crash_report(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    instance_id: String,
    filename: String,
) -> LauncherResult<CrashTriageResult> {
    tokio::task::spawn_blocking(move || {
        crash_diagnostics::triage_crash(&app, &instance_id, &filename)
    })
    .await
    .map_err(|_| LauncherError::LocalStateFailed)?
}

/// List all crash report files for an instance.
#[tauri::command]
pub async fn list_crash_reports_cmd(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    instance_id: String,
) -> LauncherResult<Vec<CrashReportInfo>> {
    tokio::task::spawn_blocking(move || crash_diagnostics::list_crash_reports(&app, &instance_id))
        .await
        .map_err(|_| LauncherError::LocalStateFailed)?
}

/// Read the content of a specific crash report file.
#[tauri::command]
pub async fn read_crash_log_cmd(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    instance_id: String,
    filename: String,
) -> LauncherResult<String> {
    tokio::task::spawn_blocking(move || {
        crash_diagnostics::read_crash_log(&app, &instance_id, &filename)
    })
    .await
    .map_err(|_| LauncherError::LocalStateFailed)?
}

/// List available mod versions for a registry item, resolving live data from
/// the upstream source (GitHub Releases or Modrinth).
#[tauri::command]
pub async fn list_mod_versions(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    instance_id: String,
    item_id: String,
) -> LauncherResult<Vec<ModVersionCandidate>> {
    mod_install::list_mod_versions(&app, &instance_id, &item_id).await
}

/// Install a specific mod version into an instance's `mods/` directory.
#[tauri::command]
pub async fn install_mod_version(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    instance_id: String,
    item_id: String,
    candidate: ModVersionCandidate,
) -> LauncherResult<InstalledMod> {
    mod_install::install_mod_version(&app, &instance_id, &item_id, &candidate).await
}

/// Remove a mod from an instance's `mods/` directory and update the manifest.
#[tauri::command]
pub async fn remove_mod_from_instance(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    instance_id: String,
    filename: String,
) -> LauncherResult<()> {
    mod_install::remove_mod_from_instance(&app, &instance_id, &filename).await
}

/// Whether the Modrinth integration is currently enabled (§6.3 toggle).
#[tauri::command]
pub async fn is_modrinth_enabled(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
) -> LauncherResult<bool> {
    Ok(modrinth_raw::is_modrinth_enabled(&app))
}

/// Live search of all of Modrinth (uncurated, §6.3). Gated by the
/// `modrinth_enabled` setting; returns `Err(ModrinthDisabled)` when off.
#[tauri::command]
pub async fn search_modrinth(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    params: modrinth_raw::ModrinthSearchParams,
) -> LauncherResult<modrinth_raw::ModrinthSearchPage> {
    modrinth_raw::search_modrinth(&app, &params).await
}

/// List Modrinth category tags for the filter UI.
#[tauri::command]
pub async fn list_modrinth_categories(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
) -> LauncherResult<Vec<modrinth_raw::ModrinthCategoryInfo>> {
    modrinth_raw::list_modrinth_categories(&app).await
}

/// List Modrinth loader tags for the filter UI.
#[tauri::command]
pub async fn list_modrinth_loaders(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
) -> LauncherResult<Vec<modrinth_raw::ModrinthLoaderInfo>> {
    modrinth_raw::list_modrinth_loaders(&app).await
}

/// List Modrinth game version tags for the filter UI.
#[tauri::command]
pub async fn list_modrinth_game_versions(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
) -> LauncherResult<Vec<modrinth_raw::ModrinthGameVersionInfo>> {
    modrinth_raw::list_modrinth_game_versions(&app).await
}

/// List raw Modrinth versions for a project, optionally scoped to an
/// instance's Minecraft version and loader.
#[tauri::command]
pub async fn list_raw_modrinth_versions(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    instance_id: Option<String>,
    project_id: String,
) -> LauncherResult<Vec<modrinth_raw::RawModrinthVersionCandidate>> {
    modrinth_raw::list_raw_modrinth_versions(&app, instance_id.as_deref(), &project_id).await
}

/// Install an uncurated Modrinth mod file, verified against the SHA-1 hash
/// published by Modrinth's API (§6.3).
#[tauri::command]
pub async fn install_raw_modrinth(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    instance_id: String,
    project_id: String,
    candidate: modrinth_raw::RawModrinthVersionCandidate,
) -> LauncherResult<InstalledMod> {
    modrinth_raw::install_raw_modrinth(&app, &instance_id, &project_id, &candidate).await
}