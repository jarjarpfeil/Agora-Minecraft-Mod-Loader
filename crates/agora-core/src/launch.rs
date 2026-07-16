//! Mojang metadata types, Maven descriptor parsing, and version merging consumed
//! by [`crate::launch_planner`].
//!
//! # Supported usage
//! - Mojang version manifest and version info types
//! - Library and asset types
//! - Maven descriptor parsing and conversion
//! - Version merging (used by Fabric/Quilt profiles and installed-profile adoption)
//! - LoaderInfo type
//!
//! # Removed (managed installer path)
//! `InstallProfile`, `Processor`, `ProcessorData`, `parse_pinned_installer`,
//! `read_bounded_zip_entry`, `canonical_version_json`, `ResolvedInstallerPlan`,
//! The previous managed-installer orchestration and its staging types have
//! been removed; the official pinned installer is the sole install authority.
//! Forge/NeoForge launch uses the installed-profile adoption path only.

use crate::error::{LauncherError, LauncherResult};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MojangVersionManifest {
    pub latest: MojangLatest,
    pub versions: Vec<MojangVersionRef>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MojangLatest {
    pub release: String,
    pub snapshot: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MojangVersionRef {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(rename = "type")]
    pub type_: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct VersionInfo {
    pub id: String,
    #[serde(rename = "mainClass")]
    pub main_class: String,
    pub arguments: Option<VersionArguments>,
    #[serde(rename = "minecraftArguments")]
    pub minecraft_arguments: Option<String>,
    pub libraries: Vec<Library>,
    #[serde(rename = "assetIndex")]
    pub asset_index: Option<AssetIndex>,
    pub assets: Option<String>,
    #[serde(rename = "type")]
    pub type_: String,
    /// Version-level downloads (the Minecraft client.jar at minimum).
    #[serde(default)]
    pub downloads: Option<VersionDownloads>,
    /// Required Java runtime major version.
    #[serde(default, rename = "javaVersion")]
    pub java_version: Option<JavaVersion>,
    /// Log4j configuration referenced by JVM arguments.
    #[serde(default)]
    pub logging: Option<LoggingConfig>,
    /// Parent version this profile inherits from (Fabric/Quilt partials use this).
    #[serde(default, rename = "inheritsFrom")]
    pub inherits_from: Option<String>,
    #[serde(default, rename = "minimumLauncherVersion")]
    pub minimum_launcher_version: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VersionArguments {
    pub jvm: Vec<serde_json::Value>,
    pub game: Vec<serde_json::Value>,
}

/// Top-level downloads object on a version JSON. `client` is the Minecraft
/// client.jar that must be placed on the classpath.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct VersionDownloads {
    #[serde(default)]
    pub client: Option<DownloadArtifact>,
    #[serde(default, rename = "client_mappings")]
    pub client_mappings: Option<DownloadArtifact>,
    #[serde(default)]
    pub server: Option<DownloadArtifact>,
    #[serde(default, rename = "server_mappings")]
    pub server_mappings: Option<DownloadArtifact>,
}

/// A downloadable artifact described by URL + hash + size. Used both for the
/// version-level client.jar and the logging configuration file.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DownloadArtifact {
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<i64>,
    pub url: String,
}

/// Required Java runtime metadata.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct JavaVersion {
    #[serde(default)]
    pub component: String,
    #[serde(rename = "majorVersion", default)]
    pub major_version: i64,
}

/// Log4j configuration shipped with a version.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LoggingConfig {
    #[serde(default)]
    pub client: Option<LoggingClient>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LoggingClient {
    pub argument: String,
    #[serde(default)]
    pub file: Option<LoggingFile>,
    #[serde(rename = "type")]
    pub type_: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LoggingFile {
    pub id: String,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<i64>,
    pub url: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Library {
    pub name: String,
    /// Fabric/Quilt profiles may put integrity metadata directly on the
    /// library rather than under `downloads.artifact`.
    #[serde(default)]
    pub sha1: Option<String>,
    /// SHA-256 carried directly in pinned Fabric/Quilt profile metadata.
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub size: Option<i64>,
    #[serde(default)]
    pub downloads: Option<LibraryDownloads>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub rules: Option<Vec<LibraryRule>>,
    /// OS name → classifier. Present only for native libraries.
    #[serde(default)]
    pub natives: Option<HashMap<String, String>>,
    /// Extraction rules (typically `{"exclude": ["META-INF/"]}`).
    #[serde(default)]
    pub extract: Option<ExtractRules>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct LibraryDownloads {
    #[serde(default)]
    pub artifact: Option<LibraryArtifact>,
    /// Per-classifier artifacts. Native libraries declare one classifier per OS.
    #[serde(default)]
    pub classifiers: Option<HashMap<String, LibraryArtifact>>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct LibraryArtifact {
    pub path: String,
    pub url: String,
    #[serde(default)]
    pub sha1: Option<String>,
    /// SHA-256 carried directly in pinned Fabric/Quilt profile metadata.
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub size: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct ExtractRules {
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct LibraryRule {
    pub action: String,
    #[serde(default)]
    pub os: Option<LibraryOs>,
    /// Feature gates (is_demo_user, has_custom_resolution, quick-play flags, …).
    #[serde(default)]
    pub features: Option<HashMap<String, bool>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct LibraryOs {
    #[serde(default)]
    pub name: String,
    /// Regex matched against `os.version` (e.g. `"^10\\."`).
    #[serde(default)]
    pub version: Option<String>,
    /// `"x86"` on 32-bit Windows in modern metadata.
    #[serde(default)]
    pub arch: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AssetIndex {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<i64>,
    #[serde(default, rename = "totalSize")]
    pub total_size: Option<i64>,
}

/// Identifies the mod loader for launch.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LoaderInfo {
    pub loader_type: String,
    pub version: String,
    /// URL to the loader's meta API (fabric/quilt) or Maven installer (forge/neoforge).
    pub version_url: String,
}

// ---------------------------------------------------------------------------
// Maven path conversion
// ---------------------------------------------------------------------------

/// Convert Maven `group:artifact:version` to a relative jar path.
///
/// `net.minecraft:launchwrapper:1.12` →
/// `net/minecraft/launchwrapper/launchwrapper-1.12.jar`
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn name_to_path(name: &str) -> String {
    let parts: Vec<&str> = name.split(':').collect();
    if parts.len() < 3 {
        return name.replace(':', "/") + ".jar";
    }
    let group = parts[0].replace('.', "/");
    let artifact = parts[1];
    let version = parts[2];
    format!("{group}/{artifact}/{version}/{artifact}-{version}.jar")
}

// ---------------------------------------------------------------------------
// Maven descriptor parser (audited)
// ---------------------------------------------------------------------------

/// A parsed Maven descriptor: `group:name:version[:classifier][@extension]`.
///
/// Produces a normalized relative path and is safe against traversal,
/// absolute paths, empty components, and unsupported extensions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MavenDescriptor {
    pub group: String,
    pub name: String,
    pub version: String,
    pub classifier: Option<String>,
    pub extension: String,
}

/// Supported Maven artifact extensions for installer processing.
pub const SUPPORTED_MAVEN_EXTENSIONS: &[&str] = &["jar", "zip", "txt"];

/// Parse a Maven coordinate string with full audit.
///
/// Grammar: `group:name:version[:classifier][@extension]`
///
/// # Rejection rules
/// - Empty group, name, or version component
/// - Traversal (`..`) or absolute path components anywhere
/// - Colons inside components (would make parsing ambiguous)
/// - Unsupported extension (only `jar`, `zip`, `txt`)
/// - Extension with unsupported characters
///
/// # Normalization
/// - Default extension is `jar`
/// - Classifier is optional
/// - Produces path: `group/name/version/name-version[-classifier].ext`
pub fn parse_maven_descriptor(descriptor: &str) -> LauncherResult<MavenDescriptor> {
    if descriptor.is_empty() {
        return Err(LauncherError::MavenDescriptor);
    }
    if descriptor.contains('\0') {
        return Err(LauncherError::MavenDescriptor);
    }
    // Reject traversal in any position
    if descriptor.contains("..") {
        return Err(LauncherError::MavenDescriptor);
    }

    // Split off optional @extension
    let (coord, extension) = if let Some(at_pos) = descriptor.rfind('@') {
        let ext = &descriptor[at_pos + 1..];
        if ext.is_empty() || !ext.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(LauncherError::MavenDescriptor);
        }
        if !SUPPORTED_MAVEN_EXTENSIONS.contains(&ext) {
            return Err(LauncherError::MavenDescriptor);
        }
        (&descriptor[..at_pos], ext.to_string())
    } else {
        (descriptor, "jar".to_string())
    };

    // Split on ':' — exactly 3 or 4 parts
    let parts: Vec<&str> = coord.split(':').collect();
    if parts.len() < 3 || parts.len() > 4 {
        return Err(LauncherError::MavenDescriptor);
    }
    for (_i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            return Err(LauncherError::MavenDescriptor);
        }
        // No colon allowed inside a component (should be caught by split but check anyway)
        if part.contains(':') {
            return Err(LauncherError::MavenDescriptor);
        }
        // Reject components that look like absolute paths or are traversal
        if *part == ".." || part.starts_with('/') || part.starts_with("\\\\") {
            return Err(LauncherError::MavenDescriptor);
        }
        // Reject Windows drive letters in any component
        if part.len() == 2 && part.ends_with(':') {
            return Err(LauncherError::MavenDescriptor);
        }
        // Reject UNC prefixes
        if *part == "//" || part.starts_with("//") {
            return Err(LauncherError::MavenDescriptor);
        }
    }

    let group = parts[0].to_string();
    let name = parts[1].to_string();
    let version = parts[2].to_string();
    let classifier = if parts.len() > 3 {
        Some(parts[3].to_string())
    } else {
        None
    };

    Ok(MavenDescriptor {
        group,
        name,
        version,
        classifier,
        extension,
    })
}

impl MavenDescriptor {
    /// Produce the normalized relative path for this descriptor.
    ///
    /// `net/minecraft/launchwrapper/1.12/launchwrapper-1.12.jar`
    pub fn to_relative_path(&self) -> String {
        let group_path = self.group.replace('.', "/");
        let file_name = match &self.classifier {
            Some(c) => format!("{}-{}-{}.{}", self.name, self.version, c, self.extension),
            None => format!("{}-{}.{}", self.name, self.version, self.extension),
        };
        format!(
            "{}/{}/{}/{}",
            group_path, self.name, self.version, file_name
        )
    }

    /// The Maven repository path component (relative path from repo root).
    pub fn to_repo_path(&self) -> String {
        self.to_relative_path()
    }
}

// ---------------------------------------------------------------------------
// Maven name conversion (extended, deprecated in favor of parse_maven_descriptor)
// ---------------------------------------------------------------------------

/// Convert Maven `group:artifact:version[:classifier]` to a relative jar path.
///
/// `net.minecraftforge:forge:1.20.1-47.1.0:installer` →
/// `net/minecraftforge/forge/1.20.1-47.1.0/forge-1.20.1-47.1.0-installer.jar`
///
/// **Deprecated**: New code should use [`parse_maven_descriptor`] which
/// performs audited rejection of malformed/traversal/absolute coordinates.
pub fn maven_name_to_path(name: &str) -> String {
    let parts: Vec<&str> = name.split(':').collect();
    let group = parts[0].replace('.', "/");
    let artifact = parts[1];
    let version = parts[2];
    let classifier = if parts.len() > 3 {
        Some(parts[3])
    } else {
        None
    };
    let jar_name = match classifier {
        Some(c) => format!("{artifact}-{version}-{c}.jar"),
        None => format!("{artifact}-{version}.jar"),
    };
    format!("{group}/{artifact}/{version}/{jar_name}")
}

// ---------------------------------------------------------------------------
// Version merging
// ---------------------------------------------------------------------------

/// Merge a partial version info (from a Forge/NeoForge install_profile) with
/// the base Mojang version. The base version takes priority for most fields.
pub fn merge_forge_version(partial: &VersionInfo, base: &VersionInfo) -> VersionInfo {
    // Loader profiles inherit the full base version. Start from a clone so
    // client.jar metadata, Java requirements, logging configuration and other
    // fields cannot be silently discarded as new metadata fields are added.
    let mut merged = base.clone();

    // Preserve library order while allowing a partial profile to replace an
    // exact Maven coordinate. Different versions remain distinct entries;
    // collapsing by group:artifact would suppress legitimate loader libraries.
    for partial_library in &partial.libraries {
        if let Some(index) = merged
            .libraries
            .iter()
            .position(|base_library| base_library.name == partial_library.name)
        {
            merged.libraries[index] = partial_library.clone();
        } else {
            merged.libraries.push(partial_library.clone());
        }
    }

    if !partial.main_class.is_empty() {
        merged.main_class = partial.main_class.clone();
    }

    let arguments = match (&partial.arguments, &base.arguments) {
        (Some(p), Some(b)) => {
            let mut jvm = b.jvm.clone();
            jvm.extend(p.jvm.clone());
            let mut game = b.game.clone();
            game.extend(p.game.clone());
            Some(VersionArguments { jvm, game })
        }
        (Some(p), None) => Some(p.clone()),
        (None, b) => b.clone(),
    };

    merged.arguments = arguments;
    if partial.minecraft_arguments.is_some() {
        merged.minecraft_arguments = partial.minecraft_arguments.clone();
    }
    if !partial.type_.is_empty() {
        merged.type_ = partial.type_.clone();
    }
    if !partial.id.is_empty() {
        merged.id = partial.id.clone();
    }

    // These fields normally come from the base profile, but retain explicit
    // partial overrides for third-party profiles that fully specify them.
    if let Some(partial_downloads) = &partial.downloads {
        if partial_downloads.client.is_some() {
            merged
                .downloads
                .get_or_insert_with(VersionDownloads::default)
                .client = partial_downloads.client.clone();
        }
    }
    if partial
        .java_version
        .as_ref()
        .is_some_and(|java| java.major_version > 0)
    {
        merged.java_version = partial.java_version.clone();
    }
    if partial
        .logging
        .as_ref()
        .and_then(|logging| logging.client.as_ref())
        .is_some()
    {
        merged.logging = partial.logging.clone();
    }
    if partial
        .asset_index
        .as_ref()
        .is_some_and(|index| !index.id.is_empty() && !index.url.is_empty())
    {
        merged.asset_index = partial.asset_index.clone();
    }
    if partial
        .assets
        .as_ref()
        .is_some_and(|assets| !assets.is_empty())
    {
        merged.assets = partial.assets.clone();
    }
    if partial
        .minimum_launcher_version
        .is_some_and(|version| version > 0)
    {
        merged.minimum_launcher_version = partial.minimum_launcher_version;
    }

    // Inheritance has been consumed by this merge; the result is standalone.
    merged.inherits_from = None;
    merged
}

// ---------------------------------------------------------------------------
// Tests (version merging, Maven descriptors, general types)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_to_path_standard() {
        assert_eq!(
            name_to_path("net.minecraft:launchwrapper:1.12"),
            "net/minecraft/launchwrapper/1.12/launchwrapper-1.12.jar"
        );
    }

    #[test]
    fn name_to_path_three_parts() {
        assert_eq!(
            name_to_path("org.ow2.asm:asm-tree:9.7"),
            "org/ow2/asm/asm-tree/9.7/asm-tree-9.7.jar"
        );
    }

    #[test]
    fn name_to_path_short() {
        assert_eq!(name_to_path("a:b:1"), "a/b/1/b-1.jar");
    }

    #[test]
    fn name_to_path_no_version() {
        let result = name_to_path("a:b");
        assert!(result.ends_with(".jar"));
    }

    #[test]
    fn maven_name_to_path_no_classifier() {
        assert_eq!(
            maven_name_to_path("net.minecraft:launchwrapper:1.12"),
            "net/minecraft/launchwrapper/1.12/launchwrapper-1.12.jar"
        );
    }

    #[test]
    fn maven_name_to_path_with_classifier() {
        assert_eq!(
            maven_name_to_path("net.minecraftforge:forge:1.20.1-47.1.0:installer"),
            "net/minecraftforge/forge/1.20.1-47.1.0/forge-1.20.1-47.1.0-installer.jar"
        );
    }

    #[test]
    fn maven_name_to_path_short() {
        assert_eq!(maven_name_to_path("a:b:1:classy"), "a/b/1/b-1-classy.jar");
    }

    #[test]
    fn merge_forge_version_dedup_libraries() {
        let base = VersionInfo {
            id: "1.21".into(),
            main_class: "net.minecraft.client.main.Main".into(),
            arguments: None,
            minecraft_arguments: None,
            libraries: vec![Library {
                name: "net.minecraft:minecraft:1.21".into(),
                downloads: None,
                url: None,
                rules: None,
                ..Default::default()
            }],
            asset_index: Some(AssetIndex {
                id: "1.21".into(),
                url: "https://example.com/1.21.json".into(),
                ..Default::default()
            }),
            assets: Some("1.21".into()),
            type_: "release".into(),
            ..Default::default()
        };

        let partial = VersionInfo {
            id: String::new(),
            main_class: "net.minecraftforge.Main".into(),
            arguments: None,
            minecraft_arguments: None,
            libraries: vec![
                Library {
                    name: "net.minecraft:minecraft:1.21".into(),
                    downloads: None,
                    url: None,
                    rules: None,
                    ..Default::default()
                },
                Library {
                    name: "net.minecraftforge:forge:47.1.0".into(),
                    downloads: None,
                    url: None,
                    rules: None,
                    ..Default::default()
                },
            ],
            asset_index: None,
            assets: None,
            type_: String::new(),
            ..Default::default()
        };

        let merged = merge_forge_version(&partial, &base);
        // base id wins
        assert_eq!(merged.id, "1.21");
        // partial main_class wins
        assert_eq!(merged.main_class, "net.minecraftforge.Main");
        // dedup: minecraft library should appear only once (from base)
        assert_eq!(merged.libraries.len(), 2);
        assert!(merged
            .libraries
            .iter()
            .any(|l| l.name == "net.minecraft:minecraft:1.21"));
        assert!(merged
            .libraries
            .iter()
            .any(|l| l.name == "net.minecraftforge:forge:47.1.0"));
        // base assets win
        assert_eq!(merged.assets.as_deref(), Some("1.21"));
    }

    #[test]
    fn merge_forge_version_main_class_fallback() {
        let base = VersionInfo {
            id: "1.21".into(),
            main_class: "net.minecraft.client.main.Main".into(),
            arguments: None,
            minecraft_arguments: None,
            libraries: vec![],
            asset_index: None,
            assets: None,
            type_: "release".into(),
            ..Default::default()
        };
        let partial = VersionInfo {
            id: String::new(),
            main_class: String::new(),
            arguments: None,
            minecraft_arguments: None,
            libraries: vec![],
            asset_index: None,
            assets: None,
            type_: String::new(),
            ..Default::default()
        };
        let merged = merge_forge_version(&partial, &base);
        // When partial has empty main_class, the base one is used
        assert_eq!(merged.main_class, "net.minecraft.client.main.Main");
    }

    #[test]
    fn merge_forge_version_arguments_concat() {
        let base = VersionInfo {
            id: "1.21".into(),
            main_class: "x".into(),
            arguments: Some(VersionArguments {
                jvm: vec![serde_json::json!("-Xmx2G")],
                game: vec![serde_json::json!("--username")],
            }),
            minecraft_arguments: None,
            libraries: vec![],
            asset_index: None,
            assets: None,
            type_: "release".into(),
            ..Default::default()
        };
        let partial = VersionInfo {
            id: String::new(),
            main_class: "y".into(),
            arguments: Some(VersionArguments {
                jvm: vec![serde_json::json!("-Dforge=true")],
                game: vec![serde_json::json!("--accessToken")],
            }),
            minecraft_arguments: None,
            libraries: vec![],
            asset_index: None,
            assets: None,
            type_: String::new(),
            ..Default::default()
        };
        let merged = merge_forge_version(&partial, &base);
        let args = merged.arguments.unwrap();
        assert_eq!(args.jvm.len(), 2);
        assert_eq!(args.game.len(), 2);
    }

    #[test]
    fn merge_loader_preserves_base_runtime_metadata() {
        let base = VersionInfo {
            id: "1.21".into(),
            main_class: "net.minecraft.client.main.Main".into(),
            downloads: Some(VersionDownloads {
                client: Some(DownloadArtifact {
                    sha1: Some("abc".into()),
                    size: Some(42),
                    url: "https://piston-data.mojang.com/client.jar".into(),
                }),
                ..Default::default()
            }),
            java_version: Some(JavaVersion {
                component: "java-runtime-gamma".into(),
                major_version: 21,
            }),
            logging: Some(LoggingConfig { client: None }),
            minimum_launcher_version: Some(21),
            ..Default::default()
        };
        let partial = VersionInfo {
            id: "fabric-loader-0.16.0-1.21".into(),
            main_class: "net.fabricmc.loader.impl.launch.knot.KnotClient".into(),
            inherits_from: Some("1.21".into()),
            ..Default::default()
        };

        let merged = merge_forge_version(&partial, &base);
        assert_eq!(merged.id, "fabric-loader-0.16.0-1.21");
        assert!(merged.downloads.unwrap().client.is_some());
        assert_eq!(merged.java_version.unwrap().major_version, 21);
        assert!(merged.logging.is_some());
        assert_eq!(merged.minimum_launcher_version, Some(21));
        assert!(merged.inherits_from.is_none());
    }

    #[test]
    fn merge_loader_keeps_distinct_maven_versions() {
        let base = VersionInfo {
            libraries: vec![Library {
                name: "org.example:library:1.0".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let partial = VersionInfo {
            libraries: vec![Library {
                name: "org.example:library:2.0".into(),
                ..Default::default()
            }],
            ..Default::default()
        };

        let merged = merge_forge_version(&partial, &base);
        assert_eq!(merged.libraries.len(), 2);
    }

    // -----------------------------------------------------------------------
    // Maven descriptor parser tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_maven_descriptor_standard() {
        let desc = parse_maven_descriptor("net.minecraft:launchwrapper:1.12").unwrap();
        assert_eq!(desc.group, "net.minecraft");
        assert_eq!(desc.name, "launchwrapper");
        assert_eq!(desc.version, "1.12");
        assert_eq!(desc.classifier, None);
        assert_eq!(desc.extension, "jar");
        assert_eq!(
            desc.to_relative_path(),
            "net/minecraft/launchwrapper/1.12/launchwrapper-1.12.jar"
        );
    }

    #[test]
    fn parse_maven_descriptor_with_classifier() {
        let desc =
            parse_maven_descriptor("net.minecraftforge:forge:1.20.1-47.1.0:installer").unwrap();
        assert_eq!(desc.group, "net.minecraftforge");
        assert_eq!(desc.name, "forge");
        assert_eq!(desc.version, "1.20.1-47.1.0");
        assert_eq!(desc.classifier, Some("installer".into()));
        assert_eq!(
            desc.to_relative_path(),
            "net/minecraftforge/forge/1.20.1-47.1.0/forge-1.20.1-47.1.0-installer.jar"
        );
    }

    #[test]
    fn parse_maven_descriptor_with_extension() {
        let desc = parse_maven_descriptor("net.minecraft:srg:1.0@zip").unwrap();
        assert_eq!(desc.extension, "zip");
        assert_eq!(desc.to_relative_path(), "net/minecraft/srg/1.0/srg-1.0.zip");
    }

    #[test]
    fn parse_maven_descriptor_classifier_with_extension() {
        let desc = parse_maven_descriptor("org.ow2.asm:asm-tree:9.7:sources@jar").unwrap();
        assert_eq!(desc.classifier, Some("sources".into()));
        assert_eq!(desc.extension, "jar");
        assert_eq!(
            desc.to_relative_path(),
            "org/ow2/asm/asm-tree/9.7/asm-tree-9.7-sources.jar"
        );
    }

    #[test]
    fn parse_maven_descriptor_txt_extension() {
        let desc = parse_maven_descriptor("net.minecraft:mappings:1.21@txt").unwrap();
        assert_eq!(desc.extension, "txt");
    }

    #[test]
    fn parse_maven_descriptor_rejects_empty() {
        assert!(parse_maven_descriptor("").is_err());
    }

    #[test]
    fn parse_maven_descriptor_rejects_traversal() {
        assert!(parse_maven_descriptor("a:b:..").is_err());
        assert!(parse_maven_descriptor("a:..:1").is_err());
    }

    #[test]
    fn parse_maven_descriptor_rejects_empty_components() {
        assert!(parse_maven_descriptor("a::c:1").is_err());
        assert!(parse_maven_descriptor(":b:c:1").is_err());
    }

    #[test]
    fn parse_maven_descriptor_rejects_too_many_parts() {
        assert!(parse_maven_descriptor("a:b:c:d:e").is_err());
    }

    #[test]
    fn parse_maven_descriptor_rejects_too_few_parts() {
        assert!(parse_maven_descriptor("a:b").is_err());
    }

    #[test]
    fn parse_maven_descriptor_rejects_unsupported_extension() {
        assert!(parse_maven_descriptor("a:b:c@exe").is_err());
        assert!(parse_maven_descriptor("a:b:c@dll").is_err());
    }

    #[test]
    fn parse_maven_descriptor_rejects_empty_extension() {
        assert!(parse_maven_descriptor("a:b:c@").is_err());
    }

    #[test]
    fn parse_maven_descriptor_rejects_windows_drive() {
        assert!(parse_maven_descriptor("C::1.0").is_err());
    }

    #[test]
    fn parse_maven_descriptor_rejects_null_bytes() {
        assert!(parse_maven_descriptor("a:b:c\0d").is_err());
    }

    #[test]
    fn parse_maven_descriptor_accepted_known_extensions() {
        // jar, zip, txt are all supported
        assert!(parse_maven_descriptor("a:b:c@jar").is_ok());
        assert!(parse_maven_descriptor("a:b:c@zip").is_ok());
        assert!(parse_maven_descriptor("a:b:c@txt").is_ok());
    }
}
