use crate::error::{LauncherError, LauncherResult};
use crate::paths;
use serde_json::{Map, Value};
use std::path::PathBuf;

/// A curated profile entry to inject into the official `launcher_profiles.json`.
#[derive(Debug, Clone)]
pub struct LauncherProfileEntry {
    pub profile_id: String,
    pub name: String,
    pub last_version_id: String,
    pub game_dir: PathBuf,
    pub java_args: String,
}

impl LauncherProfileEntry {
    fn to_json(&self) -> Value {
        let mut obj = Map::new();
        obj.insert("name".to_string(), Value::String(self.name.clone()));
        obj.insert("type".to_string(), Value::String("custom".to_string()));
        obj.insert(
            "created".to_string(),
            Value::String(chrono::Utc::now().to_rfc3339()),
        );
        obj.insert(
            "lastVersionId".to_string(),
            Value::String(self.last_version_id.clone()),
        );
        obj.insert("icon".to_string(), Value::String("Furnace".to_string()));
        obj.insert(
            "gameDir".to_string(),
            Value::String(self.game_dir.to_string_lossy().to_string()),
        );
        obj.insert("javaArgs".to_string(), Value::String(self.java_args.clone()));
        Value::Object(obj)
    }
}

/// Read, parse, inject, and atomically write `launcher_profiles.json`.
///
/// See §8.3.1. Writes to a `.tmp` file, backs up the current file to `.bak`,
/// then renames `.tmp` over the live file. On corruption, recovery is attempted
/// from `.bak` without overwriting the known-good backup.
pub fn upsert_profile(entry: &LauncherProfileEntry) -> LauncherResult<()> {
    let profiles_path = paths::launcher_profiles_path().ok_or(LauncherError::MojangNotFound)?;

    let mc_dir = profiles_path.parent().ok_or(LauncherError::MojangNotFound)?;
    std::fs::create_dir_all(mc_dir).map_err(|_| LauncherError::ProfileWriteFailed)?;

    let mut root: Value = read_or_recover(&profiles_path)?;

    // Ensure the standard `profiles` object exists.
    let profiles_obj = root
        .as_object_mut()
        .ok_or(LauncherError::ProfileWriteFailed)?
        .entry("profiles".to_string())
        .or_insert_with(|| Value::Object(Map::new()));

    let profiles_map = profiles_obj
        .as_object_mut()
        .ok_or(LauncherError::ProfileWriteFailed)?;
    profiles_map.insert(entry.profile_id.clone(), entry.to_json());

    atomic_write(&profiles_path, &root)
}

/// Remove a profile entry from `launcher_profiles.json` atomically.
pub fn remove_profile(profile_id: &str) -> LauncherResult<()> {
    let profiles_path = paths::launcher_profiles_path().ok_or(LauncherError::MojangNotFound)?;

    if !profiles_path.exists() {
        return Ok(());
    }

    let mc_dir = profiles_path.parent().ok_or(LauncherError::MojangNotFound)?;
    std::fs::create_dir_all(mc_dir).map_err(|_| LauncherError::ProfileWriteFailed)?;

    let mut root: Value = read_or_recover(&profiles_path)?;

    if let Some(profiles) = root
        .as_object_mut()
        .and_then(|o| o.get_mut("profiles"))
        .and_then(|p| p.as_object_mut())
    {
        profiles.remove(profile_id);
    }

    atomic_write(&profiles_path, &root)
}

/// Read `launcher_profiles.json`, recovering from `.bak` if it is invalid.
///
/// Recovery never overwrites the known-good `.bak` with the corrupt live file.
fn read_or_recover(profiles_path: &std::path::Path) -> LauncherResult<Value> {
    match read_json(profiles_path) {
        Ok(v) => Ok(v),
        Err(_) => {
            // Attempt recovery from the backup.
            let bak = bak_path(profiles_path);
            if bak.exists() {
                if let Ok(v) = read_json(&bak) {
                    // Reinstall the recovered backup as the live file without
                    // clobbering `.bak` (the live file is known-corrupt).
                    restore_live(profiles_path, &v)?;
                    return Ok(v);
                }
            }
            // Last resort: regenerate a minimal valid file.
            Ok(minimal_profiles())
        }
    }
}

fn read_json(path: &std::path::Path) -> LauncherResult<Value> {
    let text = std::fs::read_to_string(path).map_err(|_| LauncherError::ProfileWriteFailed)?;
    serde_json::from_str(&text).map_err(|_| LauncherError::ProfileWriteFailed)
}

fn minimal_profiles() -> Value {
    let mut root = Map::new();
    root.insert("profiles".to_string(), Value::Object(Map::new()));
    root.insert("settings".to_string(), Value::Object(Map::new()));
    Value::Object(root)
}

fn atomic_write(profiles_path: &std::path::Path, root: &Value) -> LauncherResult<()> {
    let serialized = serde_json::to_string_pretty(root).map_err(|_| LauncherError::ProfileWriteFailed)?;
    let tmp = profiles_path.with_extension("json.tmp");
    let bak = bak_path(profiles_path);

    std::fs::write(&tmp, serialized).map_err(|_| LauncherError::ProfileWriteFailed)?;

    // Back up the current on-disk file before replacing it.
    if profiles_path.exists() {
        let _ = std::fs::copy(profiles_path, &bak);
    }

    std::fs::rename(&tmp, profiles_path).map_err(|_| LauncherError::ProfileWriteFailed)?;
    Ok(())
}

/// Write a recovered value to the live file without touching `.bak`.
fn restore_live(profiles_path: &std::path::Path, root: &Value) -> LauncherResult<()> {
    let serialized = serde_json::to_string_pretty(root).map_err(|_| LauncherError::ProfileWriteFailed)?;
    let tmp = profiles_path.with_extension("json.tmp");
    std::fs::write(&tmp, serialized).map_err(|_| LauncherError::ProfileWriteFailed)?;
    std::fs::rename(&tmp, profiles_path).map_err(|_| LauncherError::ProfileWriteFailed)?;
    Ok(())
}

fn bak_path(profiles_path: &std::path::Path) -> PathBuf {
    let mut p = profiles_path.to_path_buf();
    p.set_extension("json.bak");
    p
}
