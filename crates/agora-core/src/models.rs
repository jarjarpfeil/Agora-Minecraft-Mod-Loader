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

fn default_true() -> bool {
    true
}
fn default_mod_content_type() -> String {
    "mod".to_string()
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
    /// This is a cache only; health checks re-parse JAR metadata directly.
    #[serde(default)]
    pub provided_mod_ids: Vec<String>,
    /// Whether this mod is enabled. Disabled mods have their `.jar` renamed to
    /// `.jar.disabled` so the game does not load them, but the manifest entry is
    /// preserved for easy re-enabling.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Content type discriminator: `"mod"`, `"resourcepack"`, `"shader"`,
    /// `"datapack"`, or `"world"`.  Legacy manifests without this field
    /// deserialize as `"mod"`.
    #[serde(default = "default_mod_content_type")]
    pub content_type: String,
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
    pub resourcepacks: Vec<InstalledMod>,
    #[serde(default)]
    pub shaders: Vec<InstalledMod>,
    #[serde(default)]
    pub datapacks: Vec<InstalledMod>,
    #[serde(default)]
    pub worlds: Vec<InstalledMod>,
    #[serde(default)]
    pub user_preferences: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_installed_mod_missing_java_packages() {
        let json = r#"{
            "filename": "test.jar",
            "source": "local",
            "sha256": "abc123",
            "installed_at": "2024-01-01T00:00:00Z"
        }"#;
        let mod_: InstalledMod = serde_json::from_str(json).unwrap();
        assert_eq!(mod_.java_packages, Vec::<String>::new());
    }

    #[test]
    fn test_installed_mod_missing_mod_jar_id() {
        let json = r#"{
            "filename": "test.jar",
            "source": "local",
            "sha256": "abc123",
            "installed_at": "2024-01-01T00:00:00Z"
        }"#;
        let mod_: InstalledMod = serde_json::from_str(json).unwrap();
        assert!(mod_.mod_jar_id.is_none());
    }

    #[test]
    fn test_installed_mod_missing_depends_on() {
        let json = r#"{
            "filename": "test.jar",
            "source": "local",
            "sha256": "abc123",
            "installed_at": "2024-01-01T00:00:00Z"
        }"#;
        let mod_: InstalledMod = serde_json::from_str(json).unwrap();
        assert_eq!(mod_.depends_on, Vec::<String>::new());
    }

    #[test]
    fn test_installed_mod_missing_optional_deps() {
        let json = r#"{
            "filename": "test.jar",
            "source": "local",
            "sha256": "abc123",
            "installed_at": "2024-01-01T00:00:00Z"
        }"#;
        let mod_: InstalledMod = serde_json::from_str(json).unwrap();
        assert_eq!(mod_.optional_deps, Vec::<String>::new());
    }

    #[test]
    fn test_installed_mod_missing_incompatible_deps() {
        let json = r#"{
            "filename": "test.jar",
            "source": "local",
            "sha256": "abc123",
            "installed_at": "2024-01-01T00:00:00Z"
        }"#;
        let mod_: InstalledMod = serde_json::from_str(json).unwrap();
        assert_eq!(mod_.incompatible_deps, Vec::<String>::new());
    }

    #[test]
    fn test_installed_mod_minimal_fields() {
        let json = r#"{
            "filename": "test.jar",
            "source": "local",
            "sha256": "abc123",
            "installed_at": "2024-01-01T00:00:00Z"
        }"#;
        let mod_: InstalledMod = serde_json::from_str(json).unwrap();
        assert_eq!(mod_.filename, "test.jar");
        assert_eq!(mod_.source, "local");
        assert_eq!(mod_.sha256, "abc123");
        assert_eq!(mod_.installed_at, "2024-01-01T00:00:00Z");
        assert_eq!(mod_.registry_id, None);
        assert_eq!(mod_.modrinth_id, None);
        assert_eq!(mod_.version, None);
        assert_eq!(mod_.java_packages, Vec::<String>::new());
        assert_eq!(mod_.mod_jar_id, None);
        assert_eq!(mod_.provided_mod_ids, Vec::<String>::new());
        assert_eq!(mod_.depends_on, Vec::<String>::new());
        assert_eq!(mod_.optional_deps, Vec::<String>::new());
        assert_eq!(mod_.incompatible_deps, Vec::<String>::new());
    }

    #[test]
    fn test_instance_manifest_with_mods() {
        let json = r#"{
            "instance_id": "my-instance",
            "name": "My Instance",
            "minecraft_version": "1.20.1",
            "loader": "fabric",
            "loader_version": "0.15.0",
            "mods": [
                {
                    "filename": "cloth-config.jar",
                    "source": "modrinth",
                    "sha256": "def456",
                    "installed_at": "2024-01-01T00:00:00Z",
                    "depends_on": ["fabric-api"]
                }
            ],
            "user_preferences": {}
        }"#;
        let manifest: InstanceManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.instance_id, "my-instance");
        assert_eq!(manifest.name, "My Instance");
        assert_eq!(manifest.mods.len(), 1);
        assert_eq!(manifest.mods[0].filename, "cloth-config.jar");
        assert_eq!(manifest.mods[0].depends_on, vec!["fabric-api"]);
    }

    #[test]
    fn test_instance_manifest_roundtrip() {
        let manifest = InstanceManifest {
            instance_id: "rt-instance".to_string(),
            name: "RoundTrip".to_string(),
            created_from_pack: Some("some-pack".to_string()),
            minecraft_version: "1.21.0".to_string(),
            loader: "forge".to_string(),
            loader_version: "52.0.0".to_string(),
            is_locked: true,
            mods: vec![InstalledMod {
                filename: "rt-mod.jar".to_string(),
                registry_id: Some("reg-1".to_string()),
                modrinth_id: None,
                source: "github".to_string(),
                source_url: Some("https://example.com/rt-mod.jar".to_string()),
                version: Some("1.0.0".to_string()),
                sha256: "sha123".to_string(),
                installed_at: "2024-06-01T12:00:00Z".to_string(),
                java_packages: vec!["com.example.mod".to_string()],
                mod_jar_id: Some("jar-1".to_string()),
                depends_on: vec!["core-lib".to_string()],
                optional_deps: vec!["opt-mod".to_string()],
                incompatible_deps: vec!["bad-mod".to_string()],
                provided_mod_ids: vec!["nested_api".to_string(), "legacy_alias".to_string()],
                enabled: true,
                content_type: "mod".to_string(),
            }],
            resourcepacks: vec![],
            shaders: vec![],
            datapacks: vec![],
            worlds: vec![],
            user_preferences: serde_json::json!({"key": "value"}),
        };

        let serialized = serde_json::to_string(&manifest).unwrap();
        let deserialized: InstanceManifest = serde_json::from_str(&serialized).unwrap();

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
        assert_eq!(
            deserialized.mods[0].provided_mod_ids,
            manifest.mods[0].provided_mod_ids
        );
        assert_eq!(deserialized.mods[0].depends_on, manifest.mods[0].depends_on);
        assert_eq!(
            deserialized.mods[0].optional_deps,
            manifest.mods[0].optional_deps
        );
        assert_eq!(
            deserialized.mods[0].incompatible_deps,
            manifest.mods[0].incompatible_deps
        );
        assert_eq!(deserialized.user_preferences, manifest.user_preferences);
    }
}
