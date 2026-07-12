//! Thin compat shim: preserves the original `&tauri::AppHandle` signatures
//! so no caller across the desktop crate needs to change.
//!
//! Internally this module resolves the app data directory from the handle
//! once, then delegates to `agora_core::paths` for actual path construction.

use std::path::PathBuf;
use tauri::Manager;

// Re-export pure (non-AppHandle) helpers directly from core.
pub use agora_core::paths::{launcher_profiles_path, minecraft_dir, sanitize_id};

/// Resolve the official app data directory from the Tauri `AppHandle`.
///
/// This is the only Tauri-specific path resolution — everything else
/// delegates to `agora_core::paths` once the base is known.
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
    let base = app_data_dir(app)?;
    agora_core::paths::instances_dir(&base)
}

/// Directory for a single instance (e.g. `instances/<instance_id>`).
pub fn instance_dir<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
) -> anyhow::Result<PathBuf> {
    let base = app_data_dir(app)?;
    agora_core::paths::instance_dir(&base, instance_id)
}

/// Path to an instance's `instance_manifest.json`.
pub fn instance_manifest_path<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
) -> anyhow::Result<PathBuf> {
    let base = app_data_dir(app)?;
    agora_core::paths::instance_manifest_path(&base, instance_id)
}

/// Path to the cached read-only registry database.
pub fn registry_db_path<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> anyhow::Result<PathBuf> {
    let base = app_data_dir(app)?;
    agora_core::paths::registry_db_path(&base)
}

/// Path to the cached registry.db Ed25519 signature file.
pub fn registry_sig_path<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> anyhow::Result<PathBuf> {
    let base = app_data_dir(app)?;
    agora_core::paths::registry_sig_path(&base)
}

/// Path to the mutable local state database.
pub fn local_state_db_path<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> anyhow::Result<PathBuf> {
    let base = app_data_dir(app)?;
    agora_core::paths::local_state_db_path(&base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_id_preserves_alphanumeric() {
        let result = sanitize_id("my-instance-1");
        assert!(result.contains("my-instance-1"));
    }

    #[test]
    fn test_sanitize_id_removes_path_separators() {
        assert!(!sanitize_id("foo/bar").contains('/'));
        assert!(!sanitize_id("foo\\bar").contains('\\'));
    }

    #[test]
    fn test_sanitize_id_removes_dot_dot() {
        let result = sanitize_id("..");
        assert!(!result.contains(".."));
    }

    #[test]
    fn test_sanitize_id_removes_dot_dot_slash() {
        let result = sanitize_id("../etc/passwd");
        assert!(!result.contains(".."));
        assert!(!result.contains('/'));
    }

    #[test]
    fn test_sanitize_id_removes_special_chars() {
        let result = sanitize_id("foo!@#bar");
        assert!(result.contains("foo"));
        assert!(result.contains("bar"));
        assert!(!result.contains(|c: char| matches!(c, '!' | '@' | '#')));
    }

    #[test]
    fn test_sanitize_id_empty_string() {
        let result = sanitize_id("");
        assert_eq!(result, "");
    }

    #[test]
    fn test_sanitize_id_unicode_preserved() {
        let result = sanitize_id("café");
        assert!(!result.is_empty());
    }

    #[test]
    fn test_sanitize_id_null_bytes_removed() {
        let result = sanitize_id("foo\0bar");
        assert!(!result.contains('\0'));
    }
}
