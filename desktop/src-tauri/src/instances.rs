use crate::crash_investigator;
use crate::db;
use crate::download;
use crate::error::{LauncherError, LauncherResult};
use crate::launcher_profiles::{upsert_profile, LauncherProfileEntry};
use crate::loader_manifests;
use crate::models::{InstanceManifest, InstanceRow, JvmConfig};
use crate::mojang;
use crate::paths;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::Emitter;

/// Emit a staged progress event to the frontend during instance creation.
///
/// Failure to emit is non-fatal â€” the frontend watcher is best-effort.
fn emit_progress<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
    stage: &str,
    message: &str,
) {
    let _ = app.emit(
        "instance:create-progress",
        serde_json::json!({
            "instance_id": instance_id,
            "stage": stage,
            "message": message,
        }),
    );
}

/// Request payload for creating a custom instance (see Â§6.5b).
#[derive(Debug, Clone, Deserialize)]
pub struct CreateInstanceRequest {
    pub name: String,
    pub instance_id: String,
    pub minecraft_version: String,
    pub loader: String,
    pub loader_version: String,
    #[serde(default)]
    pub jvm_memory_mb: Option<i64>,
    #[serde(default)]
    pub jvm_gc: Option<String>,
    #[serde(default)]
    pub jvm_custom_args: Option<String>,
    #[serde(default)]
    pub jvm_always_pre_touch: Option<bool>,
}

/// Summary of an available pinned loader version (for the create-instance UI).
#[derive(Debug, Clone, Serialize)]
pub struct LoaderVersionSummary {
    pub loader: String,
    pub mc_version: String,
    pub loader_version: String,
    pub file_type: String,
}

/// Create an isolated instance directory, persist metadata, and inject the loader.
///
/// Ordering and rollback:
/// 1. Create dirs + manifest (blocking).
/// 2. Inject loader (async network). On failure, clean up the instance dir.
/// 3. Persist DB row + launcher profile (blocking). On failure, clean up the
///    instance dir and the loader version JSON written in step 2.
pub async fn create_instance<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    req: CreateInstanceRequest,
) -> LauncherResult<InstanceRow> {
    let instance_id = paths::sanitize_id(&req.instance_id);
    let app_for_blocking = app.clone();
    let req_for_prep = req.clone();
    let instance_id_for_prep = instance_id.clone();

    // Step 1: blocking directory/manifest setup.
    emit_progress(
        &app,
        &instance_id,
        "preparing",
        "Preparing instance directory...",
    );
    let prepared = tokio::task::spawn_blocking(move || {
        prepare_instance_dir(&app_for_blocking, &instance_id_for_prep, &req_for_prep)
    })
    .await
    .map_err(|_| LauncherError::InstanceCreateFailed)??;

    // Step 2: async loader injection (network + hash verification).
    if let Err(e) = inject_loader(
        &app,
        &instance_id,
        &req.loader,
        &req.minecraft_version,
        &req.loader_version,
    )
    .await
    {
        emit_progress(
            &app,
            &instance_id,
            "error",
            &format!("Failed during loader injection. See logs."),
        );
        cleanup_instance_dir(&app, &instance_id);
        return Err(e);
    }

    // Step 3: blocking DB + profile persistence.
    emit_progress(
        &app,
        &instance_id,
        "persisting",
        "Saving instance to local state...",
    );
    let app_for_persist = app.clone();
    let row =
        match tokio::task::spawn_blocking(move || persist_instance(&app_for_persist, &prepared))
            .await
        {
            Ok(Ok(row)) => row,
            Ok(Err(e)) => {
                emit_progress(
                    &app,
                    &instance_id,
                    "error",
                    &format!("Failed during persistence. See logs."),
                );
                cleanup_instance_dir(&app, &instance_id);
                cleanup_loader_version_json(
                    &req.loader,
                    &req.minecraft_version,
                    &req.loader_version,
                );
                return Err(e);
            }
            Err(_) => {
                emit_progress(
                    &app,
                    &instance_id,
                    "error",
                    "Failed during persistence (task error). See logs.",
                );
                cleanup_instance_dir(&app, &instance_id);
                cleanup_loader_version_json(
                    &req.loader,
                    &req.minecraft_version,
                    &req.loader_version,
                );
                return Err(LauncherError::InstanceCreateFailed);
            }
        };

    emit_progress(&app, &instance_id, "done", "Instance created successfully.");

    Ok(row)
}

/// Blocking helper: create the instance directory tree and write the manifest.
fn prepare_instance_dir<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
    req: &CreateInstanceRequest,
) -> LauncherResult<InstanceRow> {
    let dir =
        paths::instance_dir(app, instance_id).map_err(|_| LauncherError::InstanceCreateFailed)?;

    if dir.exists() {
        return Err(LauncherError::Generic {
            code: "ERR_INSTANCE_EXISTS".to_string(),
            message: format!("An instance named '{instance_id}' already exists."),
        });
    }

    for sub in [
        "mods",
        "resourcepacks",
        "shaderpacks",
        "datapacks",
        "config",
        "crash-reports",
        "logs",
        "saves",
        "screenshots",
    ] {
        std::fs::create_dir_all(dir.join(sub)).map_err(|_| LauncherError::InstanceCreateFailed)?;
    }

    let manifest = InstanceManifest {
        instance_id: instance_id.to_string(),
        name: req.name.clone(),
        created_from_pack: None,
        minecraft_version: req.minecraft_version.clone(),
        loader: req.loader.clone(),
        loader_version: req.loader_version.clone(),
        is_locked: false,
        mods: Vec::new(),
        resourcepacks: Vec::new(),
        shaders: Vec::new(),
        datapacks: Vec::new(),
        worlds: Vec::new(),
        user_preferences: serde_json::json!({}),
    };
    write_manifest(app, instance_id, &manifest)?;

    Ok(InstanceRow {
        instance_id: instance_id.to_string(),
        name: req.name.clone(),
        minecraft_version: req.minecraft_version.clone(),
        loader: req.loader.clone(),
        loader_version: req.loader_version.clone(),
        is_modpack: false,
        is_locked: false,
        last_launched_at: None,
        jvm_memory_mb: req.jvm_memory_mb.unwrap_or(4096),
        jvm_gc: req.jvm_gc.clone().unwrap_or_else(|| "g1gc".to_string()),
        jvm_custom_args: req.jvm_custom_args.clone().unwrap_or_default(),
        jvm_always_pre_touch: req.jvm_always_pre_touch.unwrap_or(true),
        created_at: chrono::Utc::now().to_rfc3339(),
    })
}

/// Blocking helper: upsert the instance row and register the launcher profile.
fn persist_instance<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    row: &InstanceRow,
) -> LauncherResult<InstanceRow> {
    let conn = db::local_state_connection(app).map_err(|_| LauncherError::LocalStateFailed)?;
    db::upsert_instance(&conn, row).map_err(|_| LauncherError::LocalStateFailed)?;

    let entry = build_profile_entry(app, row)?;
    upsert_profile(&entry)?;

    Ok(row.clone())
}

/// Remove the instance directory if creation fails before persistence.
fn cleanup_instance_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>, instance_id: &str) {
    if let Ok(dir) = paths::instance_dir(app, instance_id) {
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

/// Remove the loader version JSON written to `.minecraft/versions/` if no other
/// instance references it.
fn cleanup_loader_version_json(loader: &str, mc_version: &str, loader_version: &str) {
    let version_id = loader_version_id(loader, loader_version, mc_version);
    let Some(mc_dir) = paths::minecraft_dir() else {
        return;
    };
    let version_dir = mc_dir.join("versions").join(&version_id);
    if version_dir.exists() {
        let _ = std::fs::remove_dir_all(&version_dir);
    }
}

/// List all user instances from `local_state.db`.
pub fn list_instances<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> LauncherResult<Vec<InstanceRow>> {
    let conn = db::local_state_connection(app).map_err(|_| LauncherError::LocalStateFailed)?;
    db::list_instances(&conn).map_err(|_| LauncherError::LocalStateFailed)
}

/// Fetch a single instance and its on-disk manifest.
pub fn get_instance_detail<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
) -> LauncherResult<Option<InstanceDetail>> {
    let conn = db::local_state_connection(app).map_err(|_| LauncherError::LocalStateFailed)?;
    let row = db::get_instance(&conn, instance_id).map_err(|_| LauncherError::LocalStateFailed)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let manifest = read_manifest(app, instance_id).unwrap_or_else(|_| None);
    Ok(Some(InstanceDetail { row, manifest }))
}

/// Delete an instance: remove from DB, remove its launcher profile, trash the
/// directory, and clean up the loader version JSON if no other instance uses it.
pub fn delete_instance<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
) -> LauncherResult<()> {
    let sanitized = paths::sanitize_id(instance_id);
    let conn = db::local_state_connection(app).map_err(|_| LauncherError::LocalStateFailed)?;

    let row = db::get_instance(&conn, &sanitized).map_err(|_| LauncherError::LocalStateFailed)?;

    db::delete_instance(&conn, &sanitized).map_err(|_| LauncherError::LocalStateFailed)?;

    // Remove the curated profile from the official Mojang launcher.
    let profile_id = profile_id_for(&sanitized);
    if let Err(e) = crate::launcher_profiles::remove_profile(&profile_id) {
        eprintln!("Failed to remove launcher profile {profile_id}: {e}");
    }

    if let Some(row) = row {
        let remaining = db::count_instances_by_loader_version(
            &conn,
            &row.loader,
            &row.minecraft_version,
            &row.loader_version,
        )
        .unwrap_or(1);
        if remaining == 0 {
            cleanup_loader_version_json(&row.loader, &row.minecraft_version, &row.loader_version);
        }
    }

    let dir = paths::instance_dir(app, &sanitized).map_err(|_| LauncherError::LocalStateFailed)?;
    if dir.exists() {
        trash::delete(&dir).map_err(|_| LauncherError::InstanceCreateFailed)?;
    }
    Ok(())
}

/// Unlock a locked pack instance for manual mod management (§6.5).
/// Sets is_locked=false in the DB to allow manual mod changes.
pub async fn unlock_instance<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
) -> LauncherResult<()> {
    let sanitized = paths::sanitize_id(instance_id);
    let app = app.clone();
    let id = sanitized.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db::local_state_connection(&app).map_err(|_| LauncherError::LocalStateFailed)?;
        agora_core::db::set_locked(&conn, &id, false).map_err(|_| LauncherError::LocalStateFailed)
    })
    .await
    .map_err(|_| LauncherError::LocalStateFailed)?
}

/// Lock an unlocked pack instance, discarding the lock snapshot.
pub async fn lock_instance<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
) -> LauncherResult<()> {
    let sanitized = paths::sanitize_id(instance_id);
    let app = app.clone();
    let id = sanitized.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db::local_state_connection(&app).map_err(|_| LauncherError::LocalStateFailed)?;
        agora_core::db::set_locked(&conn, &id, true).map_err(|_| LauncherError::LocalStateFailed)
    })
    .await
    .map_err(|_| LauncherError::LocalStateFailed)?
}

/// Rename an instance in the local state DB.
pub async fn rename_instance<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
    new_name: &str,
) -> LauncherResult<()> {
    let sanitized = paths::sanitize_id(instance_id);
    let app = app.clone();
    let id = sanitized.clone();
    let name = new_name.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = db::local_state_connection(&app).map_err(|_| LauncherError::LocalStateFailed)?;
        agora_core::db::rename_instance(&conn, &id, &name)
            .map_err(|_| LauncherError::LocalStateFailed)
    })
    .await
    .map_err(|_| LauncherError::LocalStateFailed)?
}

/// Revert an unlocked instance to its lock snapshot (§6.5).
/// Re-locks the instance (further revert logic is a future enhancement).
pub async fn revert_instance<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
) -> LauncherResult<()> {
    lock_instance(app, instance_id).await
}

/// Launch an instance by delegating to the official Mojang launcher.
pub fn launch_instance<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
) -> LauncherResult<()> {
    let sanitized = paths::sanitize_id(instance_id);
    let conn = db::local_state_connection(app).map_err(|_| LauncherError::LocalStateFailed)?;
    let row = db::get_instance(&conn, &sanitized)
        .map_err(|_| LauncherError::LocalStateFailed)?
        .ok_or(LauncherError::LaunchFailed)?;

    // Re-sync the launcher profile in case the user edited JVM settings.
    let entry = build_profile_entry(app, &row)?;
    upsert_profile(&entry)?;

    let user_override = db::get_setting(&conn, "mojang_launcher_path")
        .map_err(|_| LauncherError::LocalStateFailed)?
        .and_then(|v| v.as_str().map(|s| s.to_string()));

    let launcher_path = mojang::resolve_launcher_path(user_override.as_deref())?;

    // Update last_launched_at BEFORE spawning the game (Â§9.1).
    // This prevents crash-prompt loops where the interceptor sees a stale
    // last_launched_at and re-offers "Launch Anyway" indefinitely.
    let now = chrono::Utc::now().to_rfc3339();
    let _ = db::touch_last_launched(&conn, &sanitized, &now);

    let profile_id = profile_id_for(&sanitized);
    std::process::Command::new(&launcher_path)
        .arg("--profile")
        .arg(&profile_id)
        .spawn()
        .map_err(|_| LauncherError::LaunchFailed)?;

    // Signal D: record which mods survived this launch for survival baseline learning.
    if let Ok(manifest_text) =
        std::fs::read_to_string(paths::instance_manifest_path(app, &sanitized).unwrap_or_default())
    {
        if let Ok(manifest) = serde_json::from_str::<InstanceManifest>(&manifest_text) {
            let mod_ids: Vec<String> = manifest
                .mods
                .iter()
                .map(|m| m.registry_id.clone().unwrap_or_else(|| m.filename.clone()))
                .collect();
            let _ = crash_investigator::record_survival(app, &sanitized, &mod_ids);
        }
    }

    Ok(())
}

/// List pinned loader versions available for a loader + Minecraft version.
pub fn list_loader_versions(loader: &str, mc_version: &str) -> Vec<LoaderVersionSummary> {
    loader_manifests::list_versions(loader, mc_version)
        .into_iter()
        .map(|e| LoaderVersionSummary {
            loader: loader.to_string(),
            mc_version: mc_version.to_string(),
            loader_version: e.loader_version.clone(),
            file_type: e.file_type.clone(),
        })
        .collect()
}

/// Inject the modloader version JSON into the official Minecraft directory.
///
/// For Fabric/Quilt profile JSONs, this writes the verified JSON to
/// `~/.minecraft/versions/<version_id>/<version_id>.json`. NeoForge/Forge ship an
/// installer jar, so the verified jar is staged in the app data dir and the
/// installer is run with `java -jar <installer> --installClient`. The installer
/// itself writes into `~/.minecraft/versions/` and `~/.minecraft/libraries/`.
async fn inject_loader<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
    loader: &str,
    mc_version: &str,
    loader_version: &str,
) -> LauncherResult<()> {
    let entry = loader_manifests::find_entry(loader, mc_version, loader_version)
        .ok_or(LauncherError::UnsupportedLoader)?;

    emit_progress(
        app,
        instance_id,
        "downloading_loader",
        &format!("Downloading {loader} {loader_version}..."),
    );

    let data = download::download_verified(
        loader,
        &entry.file_name,
        &entry.file_type,
        &entry.source_url,
        &entry.sha256,
    )
    .await?;

    emit_progress(app, instance_id, "verifying_loader", "Verifying SHA-256...");

    if entry.file_type == "profile_json" {
        emit_progress(
            app,
            instance_id,
            "injecting_loader",
            &format!("Writing profile JSON for {loader} {loader_version}..."),
        );
        let version_id = entry.file_name.trim_end_matches(".json");
        let mc_dir = paths::minecraft_dir().ok_or(LauncherError::MojangNotFound)?;
        let version_dir = mc_dir.join("versions").join(version_id);
        std::fs::create_dir_all(&version_dir).map_err(|_| LauncherError::InstanceCreateFailed)?;
        std::fs::write(version_dir.join(format!("{version_id}.json")), data)
            .map_err(|_| LauncherError::InstanceCreateFailed)?;
        Ok(())
    } else if entry.file_type == "installer_jar" {
        emit_progress(
            app,
            instance_id,
            "injecting_loader",
            &format!("Staging installer jar for {loader} {loader_version}..."),
        );
        let app_data = paths::app_data_dir(app).map_err(|_| LauncherError::InstanceCreateFailed)?;
        let installer_path = app_data.join(format!("{loader}-installer.jar"));

        // Stage the verified installer jar in the app data dir.
        std::fs::write(&installer_path, &data).map_err(|_| LauncherError::InstanceCreateFailed)?;

        let java_path = get_java_path(app);
        let installer_path_for_task = installer_path.clone();
        let loader_label = loader.to_string();

        // Run the installer on a blocking thread; `java -jar` invocations can take a while.
        let result = tokio::task::spawn_blocking(move || {
            std::process::Command::new(&java_path)
                .arg("-jar")
                .arg(&installer_path_for_task)
                .arg("--installClient")
                .output()
        })
        .await;

        // Always clean up the staged installer jar, regardless of outcome.
        let _ = std::fs::remove_file(&installer_path);

        match result {
            Ok(Ok(output)) if output.status.success() => Ok(()),
            Ok(Ok(output)) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(LauncherError::Generic {
                    code: "ERR_INSTALLER_FAILED".to_string(),
                    message: format!(
                        "Installer for {loader_label} exited with {}: {stderr}",
                        output.status
                    ),
                })
            }
            Ok(Err(e)) => Err(LauncherError::Generic {
                code: "ERR_INSTALLER_FAILED".to_string(),
                message: format!("Failed to run installer for {loader_label}: {e}"),
            }),
            Err(e) => Err(LauncherError::Generic {
                code: "ERR_INSTALLER_FAILED".to_string(),
                message: format!("Installer task panicked for {loader_label}: {e}"),
            }),
        }
    } else {
        Err(LauncherError::UnsupportedLoader)
    }
}

/// Resolve the Java binary path used to run installer jars.
///
/// Reads the `java_path` user setting from `local_state.db`. If it is unset or
/// unreadable, falls back to `"java"` so the system PATH is searched.
fn get_java_path<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> String {
    let conn = match db::local_state_connection(app) {
        Ok(c) => c,
        Err(_) => return "java".to_string(),
    };
    db::get_setting(&conn, "java_path")
        .ok()
        .flatten()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "java".to_string())
}

/// Compute the effective AlwaysPreTouch value based on GC type, instance setting,
/// and optional user-level override.
///
/// GC-conditional default (Â§8.5):
/// - G1GC (or empty/unknown): true â€” safe and beneficial
/// - ZGC / Shenandoah: false â€” may cause issues
/// User override always wins when present.
fn compute_always_pre_touch(gc: &str, instance_setting: bool, user_override: Option<bool>) -> bool {
    user_override.unwrap_or_else(|| {
        if !instance_setting {
            return false;
        }
        let gc_lower = gc.to_lowercase();
        if gc_lower.contains("zgc") || gc_lower.contains("shenandoah") {
            false
        } else {
            true
        }
    })
}

fn build_profile_entry<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    row: &InstanceRow,
) -> LauncherResult<LauncherProfileEntry> {
    let game_dir = paths::instance_dir(app, &row.instance_id)
        .map_err(|_| LauncherError::InstanceCreateFailed)?;

    // Allow user-level override via `jvm_always_pre_touch` setting in user_settings.
    let user_override = db::get_setting(
        &db::local_state_connection(app).map_err(|_| LauncherError::LocalStateFailed)?,
        "jvm_always_pre_touch",
    )
    .ok()
    .flatten()
    .and_then(|v| v.as_bool());

    let always_pre_touch =
        compute_always_pre_touch(&row.jvm_gc, row.jvm_always_pre_touch, user_override);

    let jvm = JvmConfig {
        memory_mb: row.jvm_memory_mb,
        gc: row.jvm_gc.clone(),
        custom_args: row.jvm_custom_args.clone(),
        always_pre_touch,
    };
    let version_id = loader_version_id(&row.loader, &row.loader_version, &row.minecraft_version);

    Ok(LauncherProfileEntry {
        profile_id: profile_id_for(&row.instance_id),
        name: format!("{} (Agora)", row.name),
        last_version_id: version_id,
        game_dir,
        java_args: jvm.to_args(),
    })
}

/// Derive the Mojang launcher `lastVersionId` for a loader profile JSON.
fn loader_version_id(loader: &str, loader_version: &str, mc_version: &str) -> String {
    match loader {
        "fabric" => format!("fabric-loader-{loader_version}-{mc_version}"),
        "quilt" => format!("quilt-loader-{loader_version}-{mc_version}"),
        "neoforge" => format!("neoforge-{loader_version}"),
        "forge" => format!("forge-{mc_version}-{loader_version}"),
        _ => format!("{loader}-{loader_version}-{mc_version}"),
    }
}

fn profile_id_for(instance_id: &str) -> String {
    format!("agora-{instance_id}")
}

/// A combined view of an instance row plus its on-disk manifest.
#[derive(Debug, Clone, Serialize)]
pub struct InstanceDetail {
    pub row: InstanceRow,
    pub manifest: Option<InstanceManifest>,
}

fn manifest_path<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
) -> LauncherResult<PathBuf> {
    paths::instance_manifest_path(app, instance_id).map_err(|_| LauncherError::InstanceCreateFailed)
}

fn read_manifest<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
) -> LauncherResult<Option<InstanceManifest>> {
    let path = manifest_path(app, instance_id)?;
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).map_err(|_| LauncherError::InstanceCreateFailed)?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|_| LauncherError::InstanceCreateFailed)
}

fn write_manifest<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
    manifest: &InstanceManifest,
) -> LauncherResult<()> {
    let path = manifest_path(app, instance_id)?;
    let text =
        serde_json::to_string_pretty(manifest).map_err(|_| LauncherError::InstanceCreateFailed)?;
    // Atomic write: write to .tmp then rename. Abort-safe against mid-write crashes.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|_| LauncherError::InstanceCreateFailed)?;
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(LauncherError::Generic {
            code: "ERR_INSTANCE_WRITE".to_string(),
            message: format!("Failed to write instance_manifest atomically: {}", e),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- compute_always_pre_touch tests ---

    #[test]
    fn test_pre_touch_g1gc_default_on() {
        assert!(compute_always_pre_touch("G1GC", true, None));
    }

    #[test]
    fn test_pre_touch_empty_default_on() {
        assert!(compute_always_pre_touch("", true, None));
    }

    #[test]
    fn test_pre_touch_zgc_default_off() {
        assert!(!compute_always_pre_touch("ZGC", true, None));
    }

    #[test]
    fn test_pre_touch_shenandoah_default_off() {
        assert!(!compute_always_pre_touch("Shenandoah", true, None));
    }

    #[test]
    fn test_pre_touch_case_insensitive() {
        assert!(!compute_always_pre_touch("zgc", true, None));
        assert!(compute_always_pre_touch("g1gc", true, None));
    }

    #[test]
    fn test_pre_touch_user_override_true() {
        // Override wins: ZGC would default to false, but override forces true.
        assert!(compute_always_pre_touch("ZGC", false, Some(true)));
    }

    #[test]
    fn test_pre_touch_user_override_false() {
        // Override wins: G1GC would default to true, but override forces false.
        assert!(!compute_always_pre_touch("G1GC", true, Some(false)));
    }

    #[test]
    fn test_pre_touch_unknown_gc_default_on() {
        assert!(compute_always_pre_touch("ParallelGC", true, None));
    }

    #[test]
    fn test_pre_touch_zgc_in_mixed_string() {
        assert!(!compute_always_pre_touch("-XX:+UseZGC", true, None));
    }

    #[test]
    fn test_pre_touch_g1_in_mixed_string() {
        assert!(compute_always_pre_touch("-XX:+UseG1GC", true, None));
    }

    // Additional edge-case tests

    #[test]
    fn test_pre_touch_instance_false_no_override() {
        // Instance explicitly disabled, no user override â†’ false regardless of GC.
        assert!(!compute_always_pre_touch("G1GC", false, None));
        assert!(!compute_always_pre_touch("ZGC", false, None));
    }

    #[test]
    fn test_pre_touch_never_override_true() {
        // User override true always wins, even with instance disabled.
        assert!(compute_always_pre_touch("ZGC", false, Some(true)));
        assert!(compute_always_pre_touch("Shenandoah", false, Some(true)));
    }

    #[test]
    fn test_pre_touch_never_override_false() {
        // User override false always wins, even with instance enabled + G1GC.
        assert!(!compute_always_pre_touch("G1GC", true, Some(false)));
        assert!(!compute_always_pre_touch("", true, Some(false)));
    }

    // --- loader_version_id tests (pure helper) ---

    #[test]
    fn test_loader_version_id_fabric() {
        assert_eq!(
            loader_version_id("fabric", "0.15.0", "1.21"),
            "fabric-loader-0.15.0-1.21"
        );
    }

    #[test]
    fn test_loader_version_id_quilt() {
        assert_eq!(
            loader_version_id("quilt", "0.20.0", "1.21"),
            "quilt-loader-0.20.0-1.21"
        );
    }

    #[test]
    fn test_loader_version_id_neoforge() {
        assert_eq!(
            loader_version_id("neoforge", "21.1.0", "1.21"),
            "neoforge-21.1.0"
        );
    }

    #[test]
    fn test_loader_version_id_forge() {
        assert_eq!(
            loader_version_id("forge", "52.0.0", "1.21"),
            "forge-1.21-52.0.0"
        );
    }

    #[test]
    fn test_loader_version_id_unknown() {
        assert_eq!(
            loader_version_id("custom", "1.0", "1.20"),
            "custom-1.0-1.20"
        );
    }

    // --- profile_id_for tests (pure helper) ---

    #[test]
    fn test_profile_id_for() {
        assert_eq!(profile_id_for("my_instance"), "agora-my_instance");
    }

    #[test]
    fn test_profile_id_for_special_chars() {
        assert_eq!(profile_id_for("test-123"), "agora-test-123");
    }
}
