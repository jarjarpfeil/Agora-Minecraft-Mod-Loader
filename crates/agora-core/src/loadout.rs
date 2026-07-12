use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

use crate::models::{InstalledMod, InstanceManifest};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadoutProfile {
    pub name: String,
    pub enabled_mods: Vec<String>,
    pub created_at: String,
}

fn loadouts_dir(instance_dir: &Path) -> std::path::PathBuf {
    instance_dir.join(".agora_loadouts")
}

fn sanitize_profile_name(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            ':' | '/' | '\\' | '<' | '>' | '"' | '|' | '?' | '*' => '_',
            _ => c,
        })
        .collect()
}

fn content_subdir(content_type: &str) -> &str {
    match content_type {
        "resourcepack" => "resourcepacks",
        "shader" => "shaderpacks",
        "datapack" => "datapacks",
        "world" => "saves",
        _ => "mods",
    }
}

fn read_manifest(instance_dir: &Path) -> Result<InstanceManifest, String> {
    let path = instance_dir.join("instance_manifest.json");
    let text = fs::read_to_string(&path).map_err(|e| format!("Cannot read manifest: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("Cannot parse manifest: {e}"))
}

fn write_manifest(instance_dir: &Path, manifest: &InstanceManifest) -> Result<(), String> {
    let path = instance_dir.join("instance_manifest.json");
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(manifest)
        .map_err(|e| format!("Cannot serialize manifest: {e}"))?;
    fs::write(&tmp, &json).map_err(|e| format!("Cannot write manifest: {e}"))?;
    fs::rename(&tmp, &path).map_err(|e| format!("Cannot finalize manifest: {e}"))?;
    Ok(())
}

fn all_content_entries(manifest: &InstanceManifest) -> impl Iterator<Item = &InstalledMod> {
    manifest
        .mods
        .iter()
        .chain(manifest.resourcepacks.iter())
        .chain(manifest.shaders.iter())
        .chain(manifest.datapacks.iter())
        .chain(manifest.worlds.iter())
}

fn all_content_entries_mut(
    manifest: &mut InstanceManifest,
) -> impl Iterator<Item = &mut InstalledMod> {
    manifest
        .mods
        .iter_mut()
        .chain(manifest.resourcepacks.iter_mut())
        .chain(manifest.shaders.iter_mut())
        .chain(manifest.datapacks.iter_mut())
        .chain(manifest.worlds.iter_mut())
}

/// Create a new loadout profile from the current enabled state in the manifest.
pub fn create_profile(instance_dir: &Path, name: &str) -> Result<LoadoutProfile, String> {
    let manifest = read_manifest(instance_dir)?;

    let enabled_mods: Vec<String> = all_content_entries(&manifest)
        .filter(|m| m.enabled)
        .map(|m| m.filename.clone())
        .collect();

    let profile = LoadoutProfile {
        name: name.to_string(),
        enabled_mods,
        created_at: Utc::now().to_rfc3339(),
    };

    let profiles_dir = loadouts_dir(instance_dir);
    fs::create_dir_all(&profiles_dir).map_err(|e| format!("Cannot create loadouts dir: {e}"))?;

    let safe_name = sanitize_profile_name(name);
    let profile_path = profiles_dir.join(format!("{safe_name}.json"));
    let json = serde_json::to_string_pretty(&profile)
        .map_err(|e| format!("Cannot serialize profile: {e}"))?;
    fs::write(&profile_path, json).map_err(|e| format!("Cannot write profile: {e}"))?;

    Ok(profile)
}

/// List all loadout profiles for an instance.
pub fn list_profiles(instance_dir: &Path) -> Result<Vec<LoadoutProfile>, String> {
    let profiles_dir = loadouts_dir(instance_dir);
    if !profiles_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut profiles = Vec::new();
    let entries =
        fs::read_dir(&profiles_dir).map_err(|e| format!("Cannot read loadouts dir: {e}"))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Cannot read entry: {e}"))?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let content =
            fs::read_to_string(&path).map_err(|e| format!("Cannot read {:?}: {e}", path))?;
        let profile: LoadoutProfile =
            serde_json::from_str(&content).map_err(|e| format!("Invalid profile JSON: {e}"))?;
        profiles.push(profile);
    }

    profiles.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(profiles)
}

/// Apply a loadout profile: enable or disable each content entry to match the
/// profile, updating both the manifest's `enabled` field and renaming files on
/// disk (`.ext` ↔ `.ext.disabled`) so the game does not load disabled content.
pub fn apply_profile(instance_dir: &Path, profile_name: &str) -> Result<(), String> {
    let profiles_dir = loadouts_dir(instance_dir);
    let safe_name = sanitize_profile_name(profile_name);
    let profile_path = profiles_dir.join(format!("{safe_name}.json"));

    let content =
        fs::read_to_string(&profile_path).map_err(|e| format!("Cannot read profile: {e}"))?;
    let profile: LoadoutProfile =
        serde_json::from_str(&content).map_err(|e| format!("Invalid profile JSON: {e}"))?;

    let mut manifest = read_manifest(instance_dir)?;
    let enabled_set: std::collections::HashSet<String> =
        profile.enabled_mods.iter().cloned().collect();

    for entry in all_content_entries_mut(&mut manifest) {
        let should_enable = enabled_set.contains(&entry.filename);
        if entry.enabled == should_enable {
            continue;
        }

        let subdir = content_subdir(&entry.content_type);
        let dir = instance_dir.join(subdir);
        let original = dir.join(&entry.filename);
        let disabled = dir.join(format!("{}.disabled", &entry.filename));

        if should_enable {
            // Currently disabled → enable
            if disabled.exists() {
                fs::rename(&disabled, &original)
                    .map_err(|e| format!("Cannot enable {}: {e}", entry.filename))?;
            }
            entry.enabled = true;
        } else {
            // Currently enabled → disable
            if original.exists() {
                fs::rename(&original, &disabled)
                    .map_err(|e| format!("Cannot disable {}: {e}", entry.filename))?;
            }
            entry.enabled = false;
        }
    }

    write_manifest(instance_dir, &manifest)?;
    Ok(())
}

/// Delete a loadout profile.
pub fn delete_profile(instance_dir: &Path, profile_name: &str) -> Result<(), String> {
    let profiles_dir = loadouts_dir(instance_dir);
    let safe_name = sanitize_profile_name(profile_name);
    let profile_path = profiles_dir.join(format!("{safe_name}.json"));

    if !profile_path.exists() {
        return Err(format!("Profile '{profile_name}' not found"));
    }

    fs::remove_file(&profile_path).map_err(|e| format!("Cannot delete profile: {e}"))?;

    if profiles_dir.exists() {
        let remaining: Vec<_> = fs::read_dir(&profiles_dir)
            .map_err(|e| format!("Cannot read loadouts dir: {e}"))?
            .filter_map(|e| e.ok())
            .collect();
        if remaining.is_empty() {
            let _ = fs::remove_dir(&profiles_dir);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::InstalledMod;

    fn make_manifest(_instance_dir: &Path, mod_files: &[&str]) -> InstanceManifest {
        let mods: Vec<InstalledMod> = mod_files
            .iter()
            .map(|f| InstalledMod {
                filename: f.to_string(),
                registry_id: None,
                modrinth_id: None,
                source: "test".to_string(),
                source_url: None,
                version: None,
                sha256: String::new(),
                installed_at: String::new(),
                java_packages: Vec::new(),
                mod_jar_id: None,
                depends_on: Vec::new(),
                optional_deps: Vec::new(),
                incompatible_deps: Vec::new(),
                provided_mod_ids: Vec::new(),
                enabled: true,
                content_type: "mod".to_string(),
            })
            .collect();
        InstanceManifest {
            instance_id: "test".to_string(),
            name: "Test".to_string(),
            created_from_pack: None,
            minecraft_version: "1.21".to_string(),
            loader: "fabric".to_string(),
            loader_version: "0.16".to_string(),
            is_locked: false,
            mods,
            resourcepacks: Vec::new(),
            shaders: Vec::new(),
            datapacks: Vec::new(),
            worlds: Vec::new(),
            user_preferences: serde_json::json!({}),
        }
    }

    fn setup_instance(tmp: &tempfile::TempDir, mod_files: &[&str]) -> std::path::PathBuf {
        let dir = tmp.path().join("test-instance");
        let mods = dir.join("mods");
        fs::create_dir_all(&mods).unwrap();
        for f in mod_files {
            fs::write(mods.join(f), f).unwrap();
        }
        let manifest = make_manifest(&dir, mod_files);
        write_manifest(&dir, &manifest).unwrap();
        dir
    }

    #[test]
    fn test_create_profile_records_enabled_from_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = setup_instance(&tmp, &["sodium.jar", "lithium.jar", "phosphor.jar"]);

        let profile = create_profile(&dir, "test-profile").unwrap();
        assert_eq!(profile.name, "test-profile");
        assert_eq!(profile.enabled_mods.len(), 3);
        assert!(profile.enabled_mods.contains(&"sodium.jar".to_string()));
    }

    #[test]
    fn test_create_profile_skips_disabled() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = setup_instance(&tmp, &["sodium.jar", "lithium.jar", "phosphor.jar"]);

        // Manually disable one in the manifest
        let mut manifest = read_manifest(&dir).unwrap();
        manifest.mods[1].enabled = false;
        write_manifest(&dir, &manifest).unwrap();
        // Also rename file on disk to match disabled state
        fs::rename(
            dir.join("mods").join("lithium.jar"),
            dir.join("mods").join("lithium.jar.disabled"),
        )
        .unwrap();

        let profile = create_profile(&dir, "partial").unwrap();
        assert!(profile.enabled_mods.contains(&"sodium.jar".to_string()));
        assert!(!profile.enabled_mods.contains(&"lithium.jar".to_string()));
        assert!(profile.enabled_mods.contains(&"phosphor.jar".to_string()));
    }

    #[test]
    fn test_apply_profile_disables_and_updates_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = setup_instance(&tmp, &["sodium.jar", "lithium.jar", "phosphor.jar"]);

        create_profile(&dir, "full").unwrap();

        let minimal = LoadoutProfile {
            name: "minimal".to_string(),
            enabled_mods: vec!["sodium.jar".to_string()],
            created_at: Utc::now().to_rfc3339(),
        };
        let profiles_dir = loadouts_dir(&dir);
        fs::create_dir_all(&profiles_dir).unwrap();
        fs::write(
            profiles_dir.join("minimal.json"),
            serde_json::to_string_pretty(&minimal).unwrap(),
        )
        .unwrap();

        apply_profile(&dir, "minimal").unwrap();

        // Files on disk
        let mods = dir.join("mods");
        let entries: Vec<String> = fs::read_dir(&mods)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(entries.contains(&"sodium.jar".to_string()));
        assert!(entries.contains(&"lithium.jar.disabled".to_string()));
        assert!(entries.contains(&"phosphor.jar.disabled".to_string()));

        // Manifest updated
        let manifest = read_manifest(&dir).unwrap();
        assert!(manifest.mods[0].enabled); // sodium
        assert!(!manifest.mods[1].enabled); // lithium
        assert!(!manifest.mods[2].enabled); // phosphor
    }

    #[test]
    fn test_apply_profile_enables_disabled() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = setup_instance(&tmp, &["sodium.jar", "lithium.jar", "phosphor.jar"]);

        // Start with all disabled
        let mut manifest = read_manifest(&dir).unwrap();
        for m in &mut manifest.mods {
            m.enabled = false;
        }
        write_manifest(&dir, &manifest).unwrap();
        let mods_dir = dir.join("mods");
        fs::rename(
            mods_dir.join("sodium.jar"),
            mods_dir.join("sodium.jar.disabled"),
        )
        .unwrap();
        fs::rename(
            mods_dir.join("lithium.jar"),
            mods_dir.join("lithium.jar.disabled"),
        )
        .unwrap();
        fs::rename(
            mods_dir.join("phosphor.jar"),
            mods_dir.join("phosphor.jar.disabled"),
        )
        .unwrap();

        let profile = LoadoutProfile {
            name: "all-on".to_string(),
            enabled_mods: vec![
                "sodium.jar".to_string(),
                "lithium.jar".to_string(),
                "phosphor.jar".to_string(),
            ],
            created_at: Utc::now().to_rfc3339(),
        };
        let profiles_dir = loadouts_dir(&dir);
        fs::create_dir_all(&profiles_dir).unwrap();
        fs::write(
            profiles_dir.join("all-on.json"),
            serde_json::to_string_pretty(&profile).unwrap(),
        )
        .unwrap();

        apply_profile(&dir, "all-on").unwrap();

        let mods = dir.join("mods");
        let entries: Vec<String> = fs::read_dir(&mods)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(entries.contains(&"sodium.jar".to_string()));
        assert!(entries.contains(&"lithium.jar".to_string()));
        assert!(entries.contains(&"phosphor.jar".to_string()));
        assert!(!entries.iter().any(|n| n.ends_with(".disabled")));

        let manifest = read_manifest(&dir).unwrap();
        assert!(manifest.mods.iter().all(|m| m.enabled));
    }

    #[test]
    fn test_profile_applies_content_type_aware() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("test-instance");
        fs::create_dir_all(dir.join("mods")).unwrap();
        fs::create_dir_all(dir.join("shaderpacks")).unwrap();
        fs::write(dir.join("mods").join("sodium.jar"), b"sodium").unwrap();
        fs::write(dir.join("shaderpacks").join("bsl.zip"), b"bsl").unwrap();

        let mut manifest = make_manifest(&dir, &["sodium.jar"]);
        manifest.shaders.push(InstalledMod {
            filename: "bsl.zip".to_string(),
            registry_id: None,
            modrinth_id: None,
            source: "test".to_string(),
            source_url: None,
            version: None,
            sha256: String::new(),
            installed_at: String::new(),
            java_packages: Vec::new(),
            mod_jar_id: None,
            depends_on: Vec::new(),
            optional_deps: Vec::new(),
            incompatible_deps: Vec::new(),
            provided_mod_ids: Vec::new(),
            enabled: true,
            content_type: "shader".to_string(),
        });
        write_manifest(&dir, &manifest).unwrap();

        let profile = LoadoutProfile {
            name: "no-shaders".to_string(),
            enabled_mods: vec!["sodium.jar".to_string()],
            created_at: Utc::now().to_rfc3339(),
        };
        let profiles_dir = loadouts_dir(&dir);
        fs::create_dir_all(&profiles_dir).unwrap();
        fs::write(
            profiles_dir.join("no-shaders.json"),
            serde_json::to_string_pretty(&profile).unwrap(),
        )
        .unwrap();

        apply_profile(&dir, "no-shaders").unwrap();

        // Shader should be disabled on disk
        let shader_dir = dir.join("shaderpacks");
        let entries: Vec<String> = fs::read_dir(&shader_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(entries.contains(&"bsl.zip.disabled".to_string()));

        // Manifest shader entry updated
        let manifest = read_manifest(&dir).unwrap();
        assert!(manifest.mods[0].enabled); // sodium still on
        assert!(!manifest.shaders[0].enabled); // bsl now off
    }

    #[test]
    fn test_list_profiles_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("empty");
        let profiles = list_profiles(&dir).unwrap();
        assert!(profiles.is_empty());
    }

    #[test]
    fn test_create_list_delete_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = setup_instance(&tmp, &["sodium.jar"]);

        create_profile(&dir, "alpha").unwrap();
        create_profile(&dir, "beta").unwrap();

        let profiles = list_profiles(&dir).unwrap();
        assert_eq!(profiles.len(), 2);

        delete_profile(&dir, "alpha").unwrap();
        let profiles = list_profiles(&dir).unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].name, "beta");

        delete_profile(&dir, "beta").unwrap();
        let profiles = list_profiles(&dir).unwrap();
        assert!(profiles.is_empty());
    }

    #[test]
    fn test_delete_profile_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("no-profiles");
        let result = delete_profile(&dir, "nonexistent");
        assert!(result.is_err());
    }
}
