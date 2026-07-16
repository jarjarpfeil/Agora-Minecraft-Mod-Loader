use crate::error::{LauncherError, LauncherResult};
use rusqlite::OptionalExtension;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Embedded compile-time data (fallback when registry.db is unavailable).
// ---------------------------------------------------------------------------

/// Embedded copy of `loader-manifests/loader_manifests.json`.
const LOADER_MANIFESTS: &str = include_str!("../../../loader-manifests/loader_manifests.json");

/// Embedded copy of `loader-manifests/minecraft_versions.json`.
const MC_VERSIONS: &str = include_str!("../../../loader-manifests/minecraft_versions.json");

// ---------------------------------------------------------------------------
// Library pin enforcement gate
// ---------------------------------------------------------------------------

/// When `true`, [`materialize`](crate::launch_planner::materialize) refuses to
/// download any Fabric/Quilt library artifact whose Maven-relative path is not
/// present in `library_pins` with a matching SHA-256.
pub const LIBRARY_PIN_ENFORCEMENT_ENABLED: bool = true;

// ---------------------------------------------------------------------------
// Manifest types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Clone)]
pub struct LoaderEntry {
    pub mc_version: String,
    pub loader_version: String,
    pub source_url: String,
    pub sha256: String,
    pub file_name: String,
    pub file_type: String,
    /// SHA-256 of the version.json embedded inside an installer JAR.
    #[serde(default)]
    pub version_json_sha256: Option<String>,
    /// Install profile spec version (0 for legacy, 1+ for modern).
    #[serde(default)]
    pub installer_spec: Option<u64>,
}

/// Public catalog of modloader entries, library pins, and domain allowlist.
///
/// Loaded from the signed `registry.db` at runtime when available, falling
/// back to the compile-time embedded copy.  Mirrors the `loader_catalog`
/// singleton table schema in the registry.
#[derive(Debug, Deserialize, Clone)]
pub struct LoaderCatalog {
    #[serde(default)]
    pub domain_allowlist: Vec<String>,
    #[serde(default)]
    pub loaders: std::collections::BTreeMap<String, Vec<LoaderEntry>>,
    /// Global map of Maven-relative JAR path → lowercase 64-char SHA-256 hex.
    #[serde(default)]
    pub library_pins: HashMap<String, String>,
    /// Per-profile library path coverage index (Fabric/Quilt).
    #[serde(default)]
    #[allow(dead_code)]
    pub profile_library_paths: HashMap<String, Vec<String>>,
    /// Per-installer library path coverage index (Forge/NeoForge).
    #[serde(default)]
    pub installer_library_paths: HashMap<String, Vec<String>>,
}

// ---------------------------------------------------------------------------
// Static caches
// ---------------------------------------------------------------------------

/// Embedded fallback — parsed at first access and never changed.
static MANIFEST: OnceLock<LoaderCatalog> = OnceLock::new();

/// Runtime override populated from the signed `loader_catalog` table in
/// `registry.db` when available.  Takes priority over the embedded copy.
static CATALOG_OVERRIDE: OnceLock<LoaderCatalog> = OnceLock::new();

static MC_VERSIONS_LIST: OnceLock<Vec<String>> = OnceLock::new();

/// Return the effective catalog: runtime override when set, else embedded.
fn catalog() -> &'static LoaderCatalog {
    CATALOG_OVERRIDE.get().unwrap_or_else(|| {
        MANIFEST.get_or_init(|| {
            let m: LoaderCatalog =
                serde_json::from_str(LOADER_MANIFESTS).unwrap_or_else(|_| LoaderCatalog {
                    domain_allowlist: Vec::new(),
                    loaders: std::collections::BTreeMap::new(),
                    library_pins: HashMap::new(),
                    profile_library_paths: HashMap::new(),
                    installer_library_paths: HashMap::new(),
                });
            // Validate library_pins eagerly so bad data is caught at embed time.
            if let Err(err) = validate_library_pins(&m.library_pins) {
                #[cfg(debug_assertions)]
                eprintln!("[agora-core] loader_manifests.json library_pins validation: {err}");
            }
            m
        })
    })
}

/// Parse the embedded Mojang version list once and cache the result.
fn mc_versions_list() -> &'static [String] {
    MC_VERSIONS_LIST.get_or_init(|| serde_json::from_str(MC_VERSIONS).unwrap_or_default())
}

// ---------------------------------------------------------------------------
// LoaderCatalog — instance methods
// ---------------------------------------------------------------------------

impl LoaderCatalog {
    /// Parse the embeded compile-time copy.
    ///
    /// Panics if the embedded JSON is structurally invalid (this is a
    /// compile-time invariant checked by tests).
    pub fn embedded() -> Self {
        let m: Self = serde_json::from_str(LOADER_MANIFESTS)
            .expect("embedded loader_manifests.json should be valid");
        if let Err(err) = validate_library_pins(&m.library_pins) {
            #[cfg(debug_assertions)]
            eprintln!("[agora-core] embedded LoaderCatalog library_pins validation: {err}");
        }
        m
    }

    /// Load the loader catalog from the `loader_catalog` table in a signed
    /// `registry.db`.
    ///
    /// Returns `Ok(None)` when no loader_catalog row exists (pre-catalog
    /// registry releases).
    pub fn from_registry(
        conn: &rusqlite::Connection,
    ) -> LauncherResult<Option<Self>> {
        let json: Option<String> = conn
            .query_row(
                "SELECT catalog_json FROM loader_catalog WHERE singleton_id = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| LauncherError::Generic {
                code: "ERR_LOADER_CATALOG_QUERY".into(),
                message: format!("Failed to query loader_catalog: {e}"),
            })?;

        let Some(json) = json else { return Ok(None) };

        let catalog: Self = serde_json::from_str(&json).map_err(|e| {
            LauncherError::Generic {
                code: "ERR_LOADER_CATALOG_PARSE".into(),
                message: format!("Failed to parse loader_catalog JSON: {e}"),
            }
        })?;

        validate_library_pins(&catalog.library_pins)?;
        Ok(Some(catalog))
    }

    /// Load the catalog with priority: signed registry.db > embedded.
    ///
    /// Pass `Some(&conn)` when a registry database is available; pass `None`
    /// to always use the embedded copy.
    pub fn effective(registry: Option<&rusqlite::Connection>) -> LauncherResult<Self> {
        if let Some(conn) = registry {
            match Self::from_registry(conn) {
                Ok(Some(catalog)) => return Ok(catalog),
                Ok(None) => {}
                Err(e) => {
                    #[cfg(debug_assertions)]
                    eprintln!("[agora-core] Registry loader_catalog invalid: {e}");
                }
            }
        }
        Ok(Self::embedded())
    }

    /// One-shot override: replace the global static catalog with a
    /// registry-sourced version.  This affects all *free-function* callers
    /// (`find_entry`, `get_library_pin`, etc.) for the remainder of the
    /// process lifetime.
    ///
    /// Returns `Ok(true)` when the override was applied, `Ok(false)` when
    /// the registry has no loader_catalog row, or `Err` on parse/validation
    /// failure.
    ///
    /// # Panics
    /// Panics if called more than once (the override slot can only be
    /// populated once per process).
    pub fn init_from_registry(conn: &rusqlite::Connection) -> LauncherResult<bool> {
        match Self::from_registry(conn)? {
            Some(catalog) => {
                CATALOG_OVERRIDE
                    .set(catalog)
                    .unwrap_or_else(|_| panic!("LoaderCatalog::init_from_registry called more than once"));
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Find a pinned loader entry for a `(loader, mc_version, loader_version)` triple.
    pub fn find_entry(
        &self,
        loader: &str,
        mc_version: &str,
        loader_version: &str,
    ) -> Option<&LoaderEntry> {
        self.loaders.get(loader).and_then(|entries| {
            entries
                .iter()
                .find(|e| e.mc_version == mc_version && e.loader_version == loader_version)
        })
    }

    /// Look up a pinned SHA-256 by normalized Maven-relative JAR path.
    pub fn get_library_pin(&self, path: &str) -> Option<&str> {
        self.library_pins.get(path).map(|s| s.as_str())
    }

    /// True if `path` appears in the pinned library map.
    pub fn has_library_pin(&self, path: &str) -> bool {
        self.library_pins.contains_key(path)
    }

    /// Access the raw `library_pins` map (read-only).
    pub fn library_pins(&self) -> &HashMap<String, String> {
        &self.library_pins
    }

    /// Get the sorted unique Maven-relative library paths for an installer JAR.
    pub fn get_installer_library_paths(&self, file_name: &str) -> Vec<&str> {
        self.installer_library_paths
            .get(file_name)
            .map(|paths| paths.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default()
    }

    /// True if `file_name` has a non-empty installer_library_paths entry.
    pub fn has_installer_library_paths(&self, file_name: &str) -> bool {
        self.installer_library_paths
            .get(file_name)
            .is_some_and(|paths| !paths.is_empty())
    }

    /// Verify that a URL's host is on the modloader domain allowlist.
    pub fn ensure_allowed_domain(&self, raw_url: &str) -> LauncherResult<()> {
        let host = reqwest::Url::parse(raw_url)
            .map_err(|e| LauncherError::Generic {
                code: "ERR_UNTRUSTED_SOURCE".to_string(),
                message: format!("Invalid loader URL: {e}"),
            })?
            .host_str()
            .ok_or(LauncherError::UntrustedSource)?
            .to_string();

        if self.is_allowed_host(&host) {
            Ok(())
        } else {
            Err(LauncherError::UntrustedSource)
        }
    }

    /// Whether a host is on the loader domain allowlist.
    pub fn is_allowed_host(&self, host: &str) -> bool {
        self.domain_allowlist.iter().any(|d| d == host)
    }

    /// List pinned loader entries for a loader + Minecraft version.
    pub fn list_versions(&self, loader: &str, mc_version: &str) -> Vec<&LoaderEntry> {
        self.loaders
            .get(loader)
            .map(|entries| {
                entries
                    .iter()
                    .filter(|e| e.mc_version == mc_version)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Distinct loader names present in the catalog (sorted A→Z).
    pub fn list_loaders(&self) -> Vec<&str> {
        self.loaders.keys().map(|k| k.as_str()).collect()
    }

    /// All stable Minecraft versions (from Mojang's manifest), or only those
    /// supported by a specific loader. Sorted newest-first.
    pub fn list_mc_versions(&self, loader: Option<&str>) -> Vec<String> {
        let all_versions = mc_versions_list();
        match loader {
            None => all_versions.to_vec(),
            Some(l) => {
                let supported: std::collections::HashSet<&str> = self
                    .loaders
                    .get(l)
                    .map(|entries| entries.iter().map(|e| e.mc_version.as_str()).collect())
                    .unwrap_or_default();
                all_versions
                    .iter()
                    .filter(|v| supported.contains(v.as_str()))
                    .cloned()
                    .collect()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Library pin validation
// ---------------------------------------------------------------------------

/// Validate every entry in a `library_pins` map.
pub fn validate_library_pins(pins: &HashMap<String, String>) -> LauncherResult<()> {
    for (path, hash) in pins {
        let is_known_ext = path.ends_with(".jar")
            || path.ends_with(".zip")
            || path.ends_with(".txt")
            || path.ends_with(".tsrg");
        if !is_known_ext {
            return Err(LauncherError::Generic {
                code: "ERR_LIBRARY_PIN_PATH".into(),
                message: format!(
                    "Library pin path must end with .jar, .zip, .txt, or .tsrg: {path}"
                ),
            });
        }
        if path.starts_with('/')
            || path.starts_with("//")
            || path.starts_with("..")
            || path.contains(":")
        {
            return Err(LauncherError::Generic {
                code: "ERR_LIBRARY_PIN_PATH".into(),
                message: format!("Library pin path is not a safe relative path: {path}"),
            });
        }
        if hash.len() != 64 {
            return Err(LauncherError::Generic {
                code: "ERR_LIBRARY_PIN_HASH".into(),
                message: format!(
                    "Library pin hash for {path} is {} chars, expected 64",
                    hash.len()
                ),
            });
        }
        if !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(LauncherError::Generic {
                code: "ERR_LIBRARY_PIN_HASH".into(),
                message: format!("Library pin hash for {path} contains non-hex characters"),
            });
        }
        if *hash != hash.to_ascii_lowercase() {
            return Err(LauncherError::Generic {
                code: "ERR_LIBRARY_PIN_HASH".into(),
                message: format!("Library pin hash for {path} must be lowercase"),
            });
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Backward-compatible free functions (delegate to global catalog).
//
// These read from the effective catalog (override > embedded) and are the
// default entry-point for all existing callers.  New code should prefer the
// instance methods on `LoaderCatalog` when a pre-resolved catalog is available.
// ---------------------------------------------------------------------------

/// Find a pinned loader entry (delegates to global effective catalog).
pub fn find_entry(
    loader: &str,
    mc_version: &str,
    loader_version: &str,
) -> Option<&'static LoaderEntry> {
    catalog().find_entry(loader, mc_version, loader_version)
}

/// Look up a pinned SHA-256 (delegates to global effective catalog).
pub fn get_library_pin(path: &str) -> Option<&'static str> {
    catalog().library_pins.get(path).map(|s| s.as_str())
}

/// True if `path` appears in the pinned library map (delegates).
pub fn has_library_pin(path: &str) -> bool {
    catalog().library_pins.contains_key(path)
}

/// Access the raw `library_pins` map (delegates).
pub fn library_pins() -> &'static HashMap<String, String> {
    &catalog().library_pins
}

/// Return the current enforcement mode for the release-gate test.
pub fn is_enforcement_enabled() -> bool {
    LIBRARY_PIN_ENFORCEMENT_ENABLED
}

/// Get installed library paths (delegates).
pub fn get_installer_library_paths(file_name: &str) -> Vec<&'static str> {
    catalog()
        .installer_library_paths
        .get(file_name)
        .map(|paths| paths.iter().map(|s| s.as_str()).collect())
        .unwrap_or_default()
}

/// True if `file_name` has a non-empty installer_library_paths entry (delegates).
pub fn has_installer_library_paths(file_name: &str) -> bool {
    catalog()
        .installer_library_paths
        .get(file_name)
        .is_some_and(|paths| !paths.is_empty())
}

/// Verify URL host is allowed (delegates).
pub fn ensure_allowed_domain(raw_url: &str) -> LauncherResult<()> {
    catalog().ensure_allowed_domain(raw_url)
}

/// Whether a host is on the loader domain allowlist (delegates).
pub fn is_allowed_host(host: &str) -> bool {
    catalog().is_allowed_host(host)
}

/// List pinned loader versions (delegates).
pub fn list_versions(loader: &str, mc_version: &str) -> Vec<&'static LoaderEntry> {
    catalog().list_versions(loader, mc_version)
}

/// Distinct loader names (delegates).
pub fn list_loaders() -> Vec<&'static str> {
    catalog().list_loaders()
}

/// All stable Minecraft versions (delegates).
pub fn list_mc_versions(loader: Option<&str>) -> Vec<String> {
    catalog().list_mc_versions(loader)
}

/// Convert a `sha256:hex` string to raw lowercase hex.
pub fn strip_sha_prefix(s: &str) -> &str {
    s.strip_prefix("sha256:").unwrap_or(s)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allowed_loader_domain() {
        assert!(is_allowed_host("files.minecraftforge.net"));
        assert!(is_allowed_host("maven.fabricmc.net"));
        assert!(is_allowed_host("maven.neoforged.net"));
        assert!(is_allowed_host("maven.quiltmc.org"));
        assert!(is_allowed_host("neoforged.net"));
    }

    #[test]
    fn test_disallowed_localhost() {
        assert!(!is_allowed_host("127.0.0.1"));
    }

    #[test]
    fn test_disallowed_metadata_ip() {
        assert!(!is_allowed_host("169.254.169.254"));
    }

    #[test]
    fn test_disallowed_random_host() {
        assert!(!is_allowed_host("evil.com"));
    }

    #[test]
    fn test_disallowed_file_scheme() {
        let result = ensure_allowed_domain("file:///etc/passwd");
        assert!(result.is_err());
    }

    #[test]
    fn test_manifest_allowlist_nonempty() {
        let m = catalog();
        assert!(!m.domain_allowlist.is_empty());
    }

    #[test]
    fn test_ensure_allowed_domain_valid() {
        let result = ensure_allowed_domain(
            "https://maven.fabricmc.net/v2/versions/loader/1.21/0.19.0/profile/json",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_ensure_allowed_domain_invalid() {
        let result = ensure_allowed_domain("https://evil.example.com/loader.jar");
        assert!(result.is_err());
    }

    #[test]
    fn test_ensure_allowed_domain_invalid_url() {
        let result = ensure_allowed_domain("not-a-valid-url");
        assert!(result.is_err());
    }

    #[test]
    fn test_strip_sha_prefix() {
        assert_eq!(strip_sha_prefix("sha256:abc123"), "abc123");
        assert_eq!(strip_sha_prefix("abc123"), "abc123");
    }

    #[test]
    fn test_list_mc_versions_includes_legacy() {
        let versions = list_mc_versions(None);
        assert!(
            versions.len() > 50,
            "Expected 50+ versions, got {}",
            versions.len()
        );
        assert!(
            versions.contains(&"1.12.2".to_string()),
            "1.12.2 should be in the list"
        );
        assert!(
            versions.contains(&"1.7.10".to_string()),
            "1.7.10 should be in the list"
        );
    }

    #[test]
    fn test_list_mc_versions_filtered_by_loader() {
        let all = list_mc_versions(None);
        let fabric = list_mc_versions(Some("fabric"));
        assert!(
            fabric.len() < all.len(),
            "Fabric should have fewer versions than the full list"
        );
        assert!(
            !fabric.contains(&"1.7.10".to_string()),
            "Fabric should not support 1.7.10"
        );
    }

    // -----------------------------------------------------------------------
    // library_pins: schema default and lookup
    // -----------------------------------------------------------------------

    #[test]
    fn library_pins_defaults_to_empty_map() {
        let pins = library_pins();
        let valid_ext = |k: &str| {
            k.ends_with(".jar")
                || k.ends_with(".zip")
                || k.ends_with(".txt")
                || k.ends_with(".tsrg")
        };
        assert!(
            pins.iter().all(|(k, v)| {
                valid_ext(k)
                    && v.len() == 64
                    && v.bytes().all(|b| b.is_ascii_hexdigit())
                    && v == &v.to_ascii_lowercase()
            }),
            "Every library_pin entry must have a .jar/.zip/.txt/.tsrg key and 64-char lowercase hex value"
        );
    }

    #[test]
    fn get_library_pin_returns_none_for_unknown() {
        let pin = get_library_pin("nonexistent/path/to/lib.jar");
        assert!(pin.is_none(), "Unknown path should return None");
    }

    #[test]
    fn has_library_pin_returns_false_for_unknown() {
        assert!(!has_library_pin("nonexistent/path/to/lib.jar"));
    }

    // -----------------------------------------------------------------------
    // library_pins: validation
    // -----------------------------------------------------------------------

    #[test]
    fn validate_library_pins_accepts_valid_entry() {
        let mut pins = HashMap::new();
        pins.insert(
            "net/fabricmc/fabric-loader/0.19.0/fabric-loader-0.19.0.jar".into(),
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890".into(),
        );
        assert!(validate_library_pins(&pins).is_ok());
    }

    #[test]
    fn validate_library_pins_accepts_empty() {
        let pins = HashMap::new();
        assert!(validate_library_pins(&pins).is_ok());
    }

    #[test]
    fn validate_library_pins_accepts_tsrg_key() {
        let mut pins = HashMap::new();
        pins.insert(
            "net/minecraft/client/1.21/client-1.21-mappings.tsrg".into(),
            "a".repeat(64),
        );
        assert!(validate_library_pins(&pins).is_ok());
    }

    #[test]
    fn validate_library_pins_accepts_zip_key() {
        let mut pins = HashMap::new();
        pins.insert(
            "de/oceanlabs/mcp/mcp_config/1.20.1-20230612.114412/mcp_config-1.20.1-20230612.114412.zip".into(),
            "a".repeat(64),
        );
        assert!(validate_library_pins(&pins).is_ok());
    }

    #[test]
    fn validate_library_pins_accepts_txt_key() {
        let mut pins = HashMap::new();
        pins.insert(
            "de/oceanlabs/mcp/mcp_config/1.20.1-20230612.114412/mcp_config-1.20.1-20230612.114412-mappings.txt".into(),
            "a".repeat(64),
        );
        assert!(validate_library_pins(&pins).is_ok());
    }

    #[test]
    fn validate_library_pins_accepts_plus_in_version() {
        // Paths with `+` in the version (e.g. sponge-mixin) must be accepted.
        let mut pins = HashMap::new();
        pins.insert(
            "net/fabricmc/sponge-mixin/0.14.0+mixin.0.8.6/sponge-mixin-0.14.0+mixin.0.8.6.jar"
                .into(),
            "3f22c86d1a89e0c2b1cdd4388d495b50c744add9a7f2e96a8937f0dfd8d0f0b1".into(),
        );
        assert!(validate_library_pins(&pins).is_ok());
    }

    #[test]
    fn validate_library_pins_rejects_unknown_ext_key() {
        let mut pins = HashMap::new();
        pins.insert("path/to/lib.dll".into(), "a".repeat(64));
        assert!(validate_library_pins(&pins).is_err());
    }

    #[test]
    fn validate_library_pins_rejects_absolute_path() {
        let mut pins = HashMap::new();
        pins.insert("/absolute/path/lib.jar".into(), "a".repeat(64));
        assert!(validate_library_pins(&pins).is_err());
    }

    #[test]
    fn validate_library_pins_rejects_traversal_path() {
        let mut pins = HashMap::new();
        pins.insert("../../lib.jar".into(), "a".repeat(64));
        assert!(validate_library_pins(&pins).is_err());
    }

    #[test]
    fn validate_library_pins_rejects_windows_drive_path() {
        let mut pins = HashMap::new();
        pins.insert("C:/Windows/lib.jar".into(), "a".repeat(64));
        assert!(validate_library_pins(&pins).is_err());
    }

    #[test]
    fn validate_library_pins_rejects_short_hash() {
        let mut pins = HashMap::new();
        pins.insert("path/lib.jar".into(), "too_short".into());
        assert!(validate_library_pins(&pins).is_err());
    }

    #[test]
    fn validate_library_pins_rejects_uppercase_hash() {
        let mut pins = HashMap::new();
        pins.insert("path/lib.jar".into(), "A".repeat(64));
        assert!(validate_library_pins(&pins).is_err());
    }

    #[test]
    fn validate_library_pins_rejects_non_hex_hash() {
        let mut pins = HashMap::new();
        pins.insert("path/lib.jar".into(), "z".repeat(64));
        assert!(validate_library_pins(&pins).is_err());
    }

    // -----------------------------------------------------------------------
    // Enforcement gate
    // -----------------------------------------------------------------------

    #[test]
    fn library_pin_enforcement_is_enabled() {
        assert!(
            is_enforcement_enabled(),
            "LIBRARY_PIN_ENFORCEMENT_ENABLED must be true after the data refresh."
        );
    }

    // -----------------------------------------------------------------------
    // New field: LoaderEntry serde defaults
    // -----------------------------------------------------------------------

    #[test]
    fn loader_entry_version_json_sha256_defaults_none() {
        let json = r#"{
            "mc_version": "1.21",
            "loader_version": "0.19.0",
            "source_url": "https://example.com/profile.json",
            "sha256": "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
            "file_name": "test.json",
            "file_type": "profile_json"
        }"#;
        let entry: LoaderEntry = serde_json::from_str(json).unwrap();
        assert!(entry.version_json_sha256.is_none());
        assert!(entry.installer_spec.is_none());
    }

    #[test]
    fn loader_entry_parses_new_fields() {
        let json = r#"{
            "mc_version": "1.20.1",
            "loader_version": "47.4.21",
            "source_url": "https://example.com/forge-installer.jar",
            "sha256": "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
            "file_name": "forge-1.20.1-47.4.21-installer.jar",
            "file_type": "installer_jar",
            "version_json_sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "installer_spec": 1
        }"#;
        let entry: LoaderEntry = serde_json::from_str(json).unwrap();
        assert_eq!(
            entry.version_json_sha256.as_deref(),
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
        );
        assert_eq!(entry.installer_spec, Some(1));
    }

    // -----------------------------------------------------------------------
    // installer_library_paths
    // -----------------------------------------------------------------------

    #[test]
    fn get_installer_library_paths_returns_empty_for_unknown() {
        let paths = get_installer_library_paths("nonexistent-installer.jar");
        assert!(paths.is_empty());
    }

    #[test]
    fn has_installer_library_paths_returns_false_for_unknown() {
        assert!(!has_installer_library_paths("nonexistent-installer.jar"));
    }

    // -----------------------------------------------------------------------
    // Release-gate helper: complete pin coverage assertion
    // -----------------------------------------------------------------------

    /// Assert that every Fabric/Quilt/Forge/NeoForge profile entry has complete
    /// library pin coverage by verifying that every library path referenced
    /// in the coverage index has a corresponding valid SHA-256 entry in
    /// `library_pins`.
    ///
    /// For Forge/NeoForge this checks entries using `installer_library_paths`
    /// which covers only installed version.json runtime libraries with
    /// downloadable upstream artifacts. Processor-generated outputs are
    /// receipt-bound and do not require manifest pins.
    #[test]
    fn release_gate_pin_coverage() {
        if !is_enforcement_enabled() {
            return;
        }
        let m = catalog();
        let pins = &m.library_pins;
        let plp = &m.profile_library_paths;
        let ilp = &m.installer_library_paths;

        let mut missing_paths: Vec<String> = Vec::new();
        let mut uncovered_entries: Vec<String> = Vec::new();
        let mut coverage_path_count: usize = 0;
        let mut installer_coverage_count: usize = 0;
        let mut entry_count: usize = 0;

        // Collect every path referenced by any Fabric/Quilt profile.
        let mut all_referenced_paths: std::collections::HashSet<&str> =
            std::collections::HashSet::new();

        for loader in &["fabric", "quilt"] {
            if let Some(entries) = m.loaders.get(*loader) {
                for entry in entries {
                    entry_count += 1;
                    let paths = match plp.get(&entry.file_name) {
                        Some(p) => p,
                        None => {
                            uncovered_entries.push(format!(
                                "{}/{} {} (no profile_library_paths entry)",
                                loader, entry.mc_version, entry.loader_version
                            ));
                            continue;
                        }
                    };

                    if paths.is_empty() {
                        continue;
                    }

                    for path in paths {
                        coverage_path_count += 1;
                        all_referenced_paths.insert(path.as_str());
                        if !pins.contains_key(path) {
                            missing_paths.push(format!(
                                "  {}/{} {}: {}",
                                loader, entry.mc_version, entry.loader_version, path
                            ));
                        }
                    }
                }
            }
        }

        // Check Forge/NeoForge installer entries that have a non-empty
        // installer_library_paths entry. Every such entry MUST have every
        // path in library_pins. Entries without installer_library_paths are
        // pre-analysis legacy entries and are skipped.
        for loader in &["forge", "neoforge"] {
            if let Some(entries) = m.loaders.get(*loader) {
                for entry in entries {
                    // Skip entries that have no installer_library_paths yet
                    let paths = match ilp.get(&entry.file_name) {
                        Some(p) if !p.is_empty() => p,
                        _ => {
                            continue;
                        }
                    };

                    entry_count += 1;

                    for path in paths {
                        installer_coverage_count += 1;
                        all_referenced_paths.insert(path.as_str());
                        if !pins.contains_key(path) {
                            missing_paths.push(format!(
                                "  {}/{} {}: {}",
                                loader, entry.mc_version, entry.loader_version, path
                            ));
                        }
                    }
                }
            }
        }

        // Warn about orphan paths in library_pins that no curated profile
        // references. These are stale entries from previous profile versions
        // — harmless but worth noting for cleanup.
        let orphan_paths: Vec<String> = pins
            .keys()
            .filter(|path| !all_referenced_paths.contains(path.as_str()))
            .map(|path| format!("  {path}"))
            .collect();
        if !orphan_paths.is_empty() {
            eprintln!(
                "[coverage] WARNING: {} library_pin entries not referenced by any curated profile (stale):\n{}",
                orphan_paths.len(),
                orphan_paths.join("\n"),
            );
        }

        // All entries with enforcement enabled must have complete coverage.
        // This includes Fabric/Quilt profiles AND Forge/NeoForge direct-launch entries.
        assert!(
            uncovered_entries.is_empty(),
            "Enforcement enabled but {} entries are missing from coverage indices:\n  {}",
            uncovered_entries.len(),
            uncovered_entries.join("\n  "),
        );

        assert!(
            missing_paths.is_empty(),
            "Enforcement enabled but {} library paths referenced by profiles are missing from library_pins:\n{}",
            missing_paths.len(),
            missing_paths.join("\n  "),
        );

        eprintln!(
            "[coverage] {} curated entries, {} Fabric/Quilt paths + {} Forge/NeoForge installer paths = {} total, {} pinned distinct JARs (coverage verified)",
            entry_count,
            coverage_path_count,
            installer_coverage_count,
            coverage_path_count + installer_coverage_count,
            pins.len(),
        );
    }
}
