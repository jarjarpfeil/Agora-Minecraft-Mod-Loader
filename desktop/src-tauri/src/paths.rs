use std::path::PathBuf;
use tauri::Manager;

/// Resolve the official Minecraft data directory for the current OS.
///
/// | OS | Path |
/// |---|---|
/// | Windows | `%APPDATA%\.minecraft` |
/// | macOS | `~/Library/Application Support/minecraft` |
/// | Linux | `~/.minecraft` |
pub fn minecraft_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        dirs::data_dir().map(|d| d.join(".minecraft"))
    }
    #[cfg(target_os = "macos")]
    {
        dirs::data_dir().map(|d| d.join("minecraft"))
    }
    #[cfg(target_os = "linux")]
    {
        dirs::home_dir().map(|h| h.join(".minecraft"))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

/// Path to `launcher_profiles.json` inside the official Minecraft directory.
pub fn launcher_profiles_path() -> Option<PathBuf> {
    minecraft_dir().map(|d| d.join("launcher_profiles.json"))
}

/// The app data directory (`%APPDATA%/agora-mc` on Windows, etc.).
pub fn app_data_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> anyhow::Result<PathBuf> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| anyhow::anyhow!("Failed to resolve app data dir: {}", e))?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// The root directory holding all user instances.
pub fn instances_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> anyhow::Result<PathBuf> {
    let dir = app_data_dir(app)?.join("instances");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Directory for a single instance (e.g. `instances/<instance_id>`).
pub fn instance_dir<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
) -> anyhow::Result<PathBuf> {
    Ok(instances_dir(app)?.join(sanitize_id(instance_id)))
}

/// Path to an instance's `instance_manifest.json`.
pub fn instance_manifest_path<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
) -> anyhow::Result<PathBuf> {
    Ok(instance_dir(app, instance_id)?.join("instance_manifest.json"))
}

/// Path to the cached read-only registry database.
pub fn registry_db_path<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> anyhow::Result<PathBuf> {
    Ok(app_data_dir(app)?.join("registry.db"))
}

/// Path to the mutable local state database.
pub fn local_state_db_path<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> anyhow::Result<PathBuf> {
    Ok(app_data_dir(app)?.join("local_state.db"))
}

/// Normalize an instance id so it is safe to use as a directory name.
///
/// Allows alphanumerics, `-`, and `_`. Everything else is replaced with `-`.
pub fn sanitize_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}
