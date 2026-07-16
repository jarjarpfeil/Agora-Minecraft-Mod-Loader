use crate::error::{LauncherError, LauncherResult};
use rusqlite::OptionalExtension;
use serde::Deserialize;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Embedded compile-time data (fallback when registry.db is unavailable).
// ---------------------------------------------------------------------------

/// Embedded copy of `loader-manifests/loader_manifests.json`.
const LOADER_MANIFESTS: &str = include_str!("../../../loader-manifests/loader_manifests.json");

/// Embedded copy of `loader-manifests/minecraft_versions.json`.
const MC_VERSIONS: &str = include_str!("../../../loader-manifests/minecraft_versions.json");

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

/// Public catalog of modloader entries and domain allowlist.
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
            serde_json::from_str(LOADER_MANIFESTS).unwrap_or_else(|_| LoaderCatalog {
                domain_allowlist: Vec::new(),
                loaders: std::collections::BTreeMap::new(),
            })
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
        serde_json::from_str(LOADER_MANIFESTS)
            .expect("embedded loader_manifests.json should be valid")
    }

    /// Load the loader catalog from the `loader_catalog` table in a signed
    /// `registry.db`.
    ///
    /// Returns `Ok(None)` when no loader_catalog row exists (pre-catalog
    /// registry releases).
    pub fn from_registry(conn: &rusqlite::Connection) -> LauncherResult<Option<Self>> {
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

        let catalog: Self = serde_json::from_str(&json).map_err(|e| LauncherError::Generic {
            code: "ERR_LOADER_CATALOG_PARSE".into(),
            message: format!("Failed to parse loader_catalog JSON: {e}"),
        })?;

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
    /// for the remainder of the process lifetime.
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
                CATALOG_OVERRIDE.set(catalog).unwrap_or_else(|_| {
                    panic!("LoaderCatalog::init_from_registry called more than once")
                });
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
}
