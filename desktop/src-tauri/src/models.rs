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
    pub version: Option<String>,
    pub sha256: String,
    pub installed_at: String,
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
