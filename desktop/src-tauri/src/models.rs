use serde::{Deserialize, Serialize};

/// A row in `local_state.db`'s `user_instances` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceRow {
    pub instance_id: String,
    pub name: String,
    pub minecraft_version: String,
    pub loader: String,
    pub loader_version: String,
    pub is_modpack: bool,
    pub is_locked: bool,
    pub last_launched_at: Option<String>,
    pub jvm_memory_mb: i64,
    pub jvm_gc: String,
    pub jvm_custom_args: String,
    pub jvm_always_pre_touch: bool,
    pub created_at: String,
}

/// JVM configuration assembled from instance settings (see §8.5).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JvmConfig {
    pub memory_mb: i64,
    pub gc: String,
    pub custom_args: String,
    pub always_pre_touch: bool,
}

impl JvmConfig {
    /// Build the `javaArgs` string consumed by the Mojang launcher profile.
    pub fn to_args(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mem = format!("-Xmx{}M -Xms{}M", self.memory_mb, self.memory_mb);
        parts.push(mem);

        match self.gc.as_str() {
            "zgc" => parts.push("-XX:+UseZGC".to_string()),
            "shenandoah" => parts.push("-XX:+UseShenandoahGC".to_string()),
            "g1gc" => parts.push("-XX:+UseG1GC".to_string()),
            _ => {}
        }

        if !self.custom_args.trim().is_empty() {
            parts.push(self.custom_args.trim().to_string());
        }
        if self.always_pre_touch {
            parts.push("-XX:+UnlockExperimentalVMOptions".to_string());
            parts.push("-XX:+AlwaysPreTouch".to_string());
        }
        parts.join(" ")
    }
}

/// An installed mod tracked by `instance_manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledMod {
    pub filename: String,
    pub registry_id: Option<String>,
    pub modrinth_id: Option<String>,
    pub source: String,
    #[serde(default)]
    pub source_url: Option<String>,
    pub version: Option<String>,
    pub sha256: String,
    pub installed_at: String,
    #[serde(default)]
    pub java_packages: Vec<String>,
    #[serde(default)]
    pub mod_jar_id: Option<String>,
    /// Additional loader-visible IDs supplied by this physical JAR (Fabric/
    /// Quilt `provides` aliases and explicitly declared nested modules).
    #[serde(default)]
    pub provided_mod_ids: Vec<String>,
    /// REQUIRED dependencies only (Fabric `depends`, Forge type=required);
    /// see `optional_deps` and `incompatible_deps` for non-required dep types.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Optional dependencies (Fabric `recommends`/`suggests`; Forge type=optional).
    #[serde(default)]
    pub optional_deps: Vec<String>,
    /// Incompatible dependencies (Forge type=incompatible). Stored but not
    /// used in install/remove flow for v1.
    #[serde(default)]
    pub incompatible_deps: Vec<String>,
}

/// A candidate version returned by the mod version resolution API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModVersionCandidate {
    pub version: String,
    pub filename: String,
    pub download_url: String,
    pub mc_version: Option<String>,
    pub loader: Option<String>,
    pub release_date: Option<String>,
    pub is_compatible: bool,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub sha512: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    /// Compatibility tier: `"compatible"` (exact MC version + loader match),
    /// `"major_match"` (same major version, different minor), or `""` (incompatible).
    #[serde(default)]
    pub version_compat: String,
}

/// The lightweight JSON manifest that lives in each instance directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceManifest {
    pub instance_id: String,
    pub name: String,
    #[serde(default)]
    pub created_from_pack: Option<String>,
    pub minecraft_version: String,
    pub loader: String,
    pub loader_version: String,
    #[serde(default)]
    pub is_locked: bool,
    pub mods: Vec<InstalledMod>,
    #[serde(default)]
    pub user_preferences: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- InstalledMod back-compat (serde defaults) ----

    #[test]
    fn test_installed_mod_full_deserialize() {
        let json = r#"{
            "filename": "cloth-config-13.0.0.jar",
            "registry_id": "cloth-config",
            "modrinth_id": "bR6B5nA7",
            "source": "modrinth",
            "version": "13.0.0",
            "sha256": "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
            "installed_at": "2025-01-15T10:30:00Z",
            "java_packages": ["me.shedaniel.clothconfig2"],
            "mod_jar_id": "cloth-config-jar",
            "depends_on": ["fabric-api"],
            "optional_deps": ["architectury-api"],
            "incompatible_deps": ["old-conflicting-mod"]
        }"#;
        let mod_: InstalledMod =
            serde_json::from_str(json).expect("should deserialize full InstalledMod");
        assert_eq!(mod_.filename, "cloth-config-13.0.0.jar");
        assert_eq!(mod_.registry_id, Some("cloth-config".to_string()));
        assert_eq!(mod_.modrinth_id, Some("bR6B5nA7".to_string()));
        assert_eq!(mod_.source, "modrinth");
        assert_eq!(mod_.version, Some("13.0.0".to_string()));
        assert_eq!(
            mod_.sha256,
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
        );
        assert_eq!(mod_.installed_at, "2025-01-15T10:30:00Z");
        assert_eq!(
            mod_.java_packages,
            vec!["me.shedaniel.clothconfig2".to_string()]
        );
        assert_eq!(mod_.mod_jar_id, Some("cloth-config-jar".to_string()));
        assert_eq!(mod_.depends_on, vec!["fabric-api".to_string()]);
        assert_eq!(mod_.optional_deps, vec!["architectury-api".to_string()]);
        assert_eq!(
            mod_.incompatible_deps,
            vec!["old-conflicting-mod".to_string()]
        );
    }

    #[test]
    fn test_installed_mod_missing_java_packages() {
        let json = r#"{
            "filename": "sodium-0.6.0.jar",
            "source": "modrinth",
            "sha256": "1111111111111111111111111111111111111111111111111111111111111111",
            "installed_at": "2025-02-01T00:00:00Z"
        }"#;
        let mod_: InstalledMod =
            serde_json::from_str(json).expect("should deserialize without java_packages");
        assert_eq!(mod_.java_packages, Vec::<String>::new());
    }

    #[test]
    fn test_installed_mod_missing_mod_jar_id() {
        let json = r#"{
            "filename": "sodium-0.6.0.jar",
            "source": "modrinth",
            "sha256": "1111111111111111111111111111111111111111111111111111111111111111",
            "installed_at": "2025-02-01T00:00:00Z"
        }"#;
        let mod_: InstalledMod =
            serde_json::from_str(json).expect("should deserialize without mod_jar_id");
        assert_eq!(mod_.mod_jar_id, None);
    }

    #[test]
    fn test_installed_mod_missing_depends_on() {
        let json = r#"{
            "filename": "sodium-0.6.0.jar",
            "source": "modrinth",
            "sha256": "1111111111111111111111111111111111111111111111111111111111111111",
            "installed_at": "2025-02-01T00:00:00Z"
        }"#;
        let mod_: InstalledMod =
            serde_json::from_str(json).expect("should deserialize without depends_on");
        assert_eq!(mod_.depends_on, Vec::<String>::new());
    }

    #[test]
    fn test_installed_mod_missing_optional_deps() {
        let json = r#"{
            "filename": "sodium-0.6.0.jar",
            "source": "modrinth",
            "sha256": "1111111111111111111111111111111111111111111111111111111111111111",
            "installed_at": "2025-02-01T00:00:00Z"
        }"#;
        let mod_: InstalledMod =
            serde_json::from_str(json).expect("should deserialize without optional_deps");
        assert_eq!(mod_.optional_deps, Vec::<String>::new());
    }

    #[test]
    fn test_installed_mod_missing_incompatible_deps() {
        let json = r#"{
            "filename": "sodium-0.6.0.jar",
            "source": "modrinth",
            "sha256": "1111111111111111111111111111111111111111111111111111111111111111",
            "installed_at": "2025-02-01T00:00:00Z"
        }"#;
        let mod_: InstalledMod =
            serde_json::from_str(json).expect("should deserialize without incompatible_deps");
        assert_eq!(mod_.incompatible_deps, Vec::<String>::new());
    }

    #[test]
    fn test_installed_mod_minimal() {
        let json = r#"{
            "filename": "sodium-0.6.0.jar",
            "source": "modrinth",
            "sha256": "1111111111111111111111111111111111111111111111111111111111111111",
            "installed_at": "2025-02-01T00:00:00Z"
        }"#;
        let mod_: InstalledMod =
            serde_json::from_str(json).expect("should deserialize minimal InstalledMod");
        assert_eq!(mod_.filename, "sodium-0.6.0.jar");
        assert_eq!(mod_.source, "modrinth");
        assert_eq!(
            mod_.sha256,
            "1111111111111111111111111111111111111111111111111111111111111111"
        );
        assert_eq!(mod_.installed_at, "2025-02-01T00:00:00Z");
        assert_eq!(mod_.registry_id, None);
        assert_eq!(mod_.modrinth_id, None);
        assert_eq!(mod_.version, None);
        assert_eq!(mod_.java_packages, Vec::<String>::new());
        assert_eq!(mod_.mod_jar_id, None);
        assert_eq!(mod_.depends_on, Vec::<String>::new());
        assert_eq!(mod_.optional_deps, Vec::<String>::new());
        assert_eq!(mod_.incompatible_deps, Vec::<String>::new());
    }

    // ---- InstanceManifest ----

    #[test]
    fn test_instance_manifest_with_mods() {
        let json = r#"{
            "instance_id": "my-instance",
            "name": "My Modded Instance",
            "created_from_pack": "fabrik-3.2.0",
            "minecraft_version": "1.21.1",
            "loader": "fabric",
            "loader_version": "0.16.0",
            "is_locked": false,
            "mods": [
                {
                    "filename": "sodium-0.6.0.jar",
                    "source": "modrinth",
                    "sha256": "1111111111111111111111111111111111111111111111111111111111111111",
                    "installed_at": "2025-03-01T00:00:00Z",
                    "java_packages": ["me.jellysquid.mods.sodium"],
                    "mod_jar_id": "sodium-jar",
                    "depends_on": [],
                    "optional_deps": [],
                    "incompatible_deps": []
                },
                {
                    "filename": "fabric-api-0.100.0.jar",
                    "source": "modrinth",
                    "sha256": "2222222222222222222222222222222222222222222222222222222222222222",
                    "installed_at": "2025-03-01T00:00:00Z",
                    "java_packages": [],
                    "mod_jar_id": null,
                    "depends_on": [],
                    "optional_deps": [],
                    "incompatible_deps": []
                }
            ],
            "user_preferences": {"fullscreen": true}
        }"#;
        let manifest: InstanceManifest =
            serde_json::from_str(json).expect("should deserialize InstanceManifest with mods");
        assert_eq!(manifest.instance_id, "my-instance");
        assert_eq!(manifest.name, "My Modded Instance");
        assert_eq!(manifest.created_from_pack, Some("fabrik-3.2.0".to_string()));
        assert_eq!(manifest.minecraft_version, "1.21.1");
        assert_eq!(manifest.loader, "fabric");
        assert_eq!(manifest.loader_version, "0.16.0");
        assert_eq!(manifest.is_locked, false);
        assert_eq!(manifest.mods.len(), 2);
        assert_eq!(manifest.mods[0].filename, "sodium-0.6.0.jar");
        assert_eq!(
            manifest.mods[0].java_packages,
            vec!["me.jellysquid.mods.sodium".to_string()]
        );
        assert_eq!(manifest.mods[0].mod_jar_id, Some("sodium-jar".to_string()));
        assert_eq!(manifest.mods[1].filename, "fabric-api-0.100.0.jar");
        assert_eq!(manifest.mods[1].mod_jar_id, None);
    }

    #[test]
    fn test_instance_manifest_empty_mods() {
        let json = r#"{
            "instance_id": "empty-instance",
            "name": "Empty Instance",
            "minecraft_version": "1.21.1",
            "loader": "fabric",
            "loader_version": "0.16.0",
            "mods": [],
            "user_preferences": {}
        }"#;
        let manifest: InstanceManifest = serde_json::from_str(json)
            .expect("should deserialize InstanceManifest with empty mods");
        assert_eq!(manifest.instance_id, "empty-instance");
        assert_eq!(manifest.mods.len(), 0);
    }

    #[test]
    fn test_instance_manifest_roundtrip() {
        let manifest = InstanceManifest {
            instance_id: "roundtrip-instance".to_string(),
            name: "Roundtrip Test".to_string(),
            created_from_pack: Some("test-pack-1.0".to_string()),
            minecraft_version: "1.21.1".to_string(),
            loader: "fabric".to_string(),
            loader_version: "0.16.0".to_string(),
            is_locked: true,
            mods: vec![
                InstalledMod {
                    filename: "sodium-0.6.0.jar".to_string(),
                    registry_id: Some("sodium".to_string()),
                    modrinth_id: Some("AANobbMI".to_string()),
                    source: "modrinth".to_string(),
                    source_url: None,
                    version: Some("0.6.0".to_string()),
                    sha256: "1111111111111111111111111111111111111111111111111111111111111111"
                        .to_string(),
                    installed_at: "2025-04-01T12:00:00Z".to_string(),
                    java_packages: vec!["me.jellysquid.mods.sodium".to_string()],
                    mod_jar_id: Some("sodium-jar".to_string()),
                    depends_on: vec!["fabric-api".to_string()],
                    optional_deps: vec!["lazy-df".to_string()],
                    incompatible_deps: vec!["optifine".to_string()],
                },
                InstalledMod {
                    filename: "fabric-api-0.100.0.jar".to_string(),
                    registry_id: Some("fabric-api".to_string()),
                    modrinth_id: Some("UWpQ9kB0".to_string()),
                    source: "modrinth".to_string(),
                    source_url: None,
                    version: Some("0.100.0".to_string()),
                    sha256: "2222222222222222222222222222222222222222222222222222222222222222"
                        .to_string(),
                    installed_at: "2025-04-01T12:00:01Z".to_string(),
                    java_packages: vec![],
                    mod_jar_id: None,
                    depends_on: vec![],
                    optional_deps: vec![],
                    incompatible_deps: vec![],
                },
            ],
            user_preferences: serde_json::json!({"fullscreen": true, "window_width": 1280}),
        };

        let json = serde_json::to_string(&manifest).expect("should serialize InstanceManifest");
        let deserialized: InstanceManifest =
            serde_json::from_str(&json).expect("should roundtrip InstanceManifest");

        assert_eq!(deserialized.instance_id, manifest.instance_id);
        assert_eq!(deserialized.name, manifest.name);
        assert_eq!(deserialized.created_from_pack, manifest.created_from_pack);
        assert_eq!(deserialized.minecraft_version, manifest.minecraft_version);
        assert_eq!(deserialized.loader, manifest.loader);
        assert_eq!(deserialized.loader_version, manifest.loader_version);
        assert_eq!(deserialized.is_locked, manifest.is_locked);
        assert_eq!(deserialized.mods.len(), manifest.mods.len());
        assert_eq!(deserialized.mods[0].filename, manifest.mods[0].filename);
        assert_eq!(
            deserialized.mods[0].registry_id,
            manifest.mods[0].registry_id
        );
        assert_eq!(
            deserialized.mods[0].modrinth_id,
            manifest.mods[0].modrinth_id
        );
        assert_eq!(deserialized.mods[0].source, manifest.mods[0].source);
        assert_eq!(deserialized.mods[0].version, manifest.mods[0].version);
        assert_eq!(deserialized.mods[0].sha256, manifest.mods[0].sha256);
        assert_eq!(
            deserialized.mods[0].installed_at,
            manifest.mods[0].installed_at
        );
        assert_eq!(
            deserialized.mods[0].java_packages,
            manifest.mods[0].java_packages
        );
        assert_eq!(deserialized.mods[0].mod_jar_id, manifest.mods[0].mod_jar_id);
        assert_eq!(deserialized.mods[0].depends_on, manifest.mods[0].depends_on);
        assert_eq!(
            deserialized.mods[0].optional_deps,
            manifest.mods[0].optional_deps
        );
        assert_eq!(
            deserialized.mods[0].incompatible_deps,
            manifest.mods[0].incompatible_deps
        );
        assert_eq!(deserialized.mods[1].filename, manifest.mods[1].filename);
        assert_eq!(deserialized.mods[1].mod_jar_id, manifest.mods[1].mod_jar_id);
        assert_eq!(deserialized.user_preferences, manifest.user_preferences);
    }
}
