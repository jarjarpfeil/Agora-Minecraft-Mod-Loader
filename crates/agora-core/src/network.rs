//! Network policy enforcement for the launch planner.
//!
//! Defines a fixed-size policy struct with five categories that control
//! which network endpoints the launch planner may contact. Every HTTP
//! request in the planner is gated by a `NetworkPolicy::check()` call.

use crate::db;
use crate::error::{LauncherError, LauncherResult};
use crate::loader_manifests;

/// Categories of network access controlled by the launch planner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NetworkCategory {
    /// Microsoft/Xbox Live authentication (MSA refresh, login).
    MicrosoftAuthentication,
    /// Mojang metadata: version manifest, version JSON, asset index.
    MojangMetadata,
    /// Mojang content: client JAR, libraries, natives, assets, logging config.
    MojangContent,
    /// Loader metadata and content: pinned profile JSONs, Maven libraries/sidecars.
    LoaderMetadataAndContent,
    /// Java runtime auto-provisioning (Adoptium Temurin JRE downloads).
    JavaRuntime,
}

/// Fixed-size bitmask network policy for the launch planner.
///
/// Each of the five categories can be independently enabled or disabled.
/// Construct with `all_enabled()` or `all_disabled()`, then customize
/// with builder-style `set_*` methods or with `set_category`.
#[derive(Debug, Clone)]
pub struct NetworkPolicy(u8);

// Bit positions for each category.
const BIT_MSA: u8 = 1 << 0;
const BIT_META: u8 = 1 << 1;
const BIT_CONTENT: u8 = 1 << 2;
const BIT_LOADER: u8 = 1 << 3;
const BIT_JAVA: u8 = 1 << 4;

impl NetworkPolicy {
    /// All categories enabled — unrestricted launch network access.
    pub const fn all_enabled() -> Self {
        Self(BIT_MSA | BIT_META | BIT_CONTENT | BIT_LOADER | BIT_JAVA)
    }

    /// All categories disabled — no launch network access.
    pub const fn all_disabled() -> Self {
        Self(0)
    }

    /// Enable or disable a specific category.
    pub fn set_category(&mut self, category: NetworkCategory, enabled: bool) {
        let bit = category_bit(category);
        if enabled {
            self.0 |= bit;
        } else {
            self.0 &= !bit;
        }
    }

    /// Builder-style: return a new policy with the given category enabled or disabled.
    pub fn with_category(mut self, category: NetworkCategory, enabled: bool) -> Self {
        self.set_category(category, enabled);
        self
    }

    /// Check whether a category is enabled.
    pub fn is_enabled(&self, category: NetworkCategory) -> bool {
        self.0 & category_bit(category) != 0
    }

    /// Check that the category is enabled. Returns a dedicated
    /// [`LauncherError`] variant if disabled.
    pub fn check(&self, category: NetworkCategory) -> LauncherResult<()> {
        if self.is_enabled(category) {
            Ok(())
        } else {
            Err(match category {
                NetworkCategory::MicrosoftAuthentication => LauncherError::NetworkMsaDisabled,
                NetworkCategory::MojangMetadata => LauncherError::NetworkMojangMetadataDisabled,
                NetworkCategory::MojangContent => LauncherError::NetworkMojangContentDisabled,
                NetworkCategory::LoaderMetadataAndContent => LauncherError::NetworkLoaderDisabled,
                NetworkCategory::JavaRuntime => LauncherError::NetworkJavaDisabled,
            })
        }
    }

    /// Construct a policy from `local_state.db` settings.
    ///
    /// Reads the `network_mojang_metadata_enabled`, `network_mojang_content_enabled`,
    /// `network_loader_enabled`, `network_msa_enabled`, and `network_adoptium_enabled`
    /// keys. All categories default to enabled when the key is missing.
    /// An explicit `"false"` or `false` in the DB disables the category.
    pub fn from_db(conn: &rusqlite::Connection) -> Self {
        let mut policy = Self::all_enabled();

        // All launch-network settings default to enabled.
        // Only explicit "false"/false in the DB disables the category.
        if !is_network_enabled_default_true(conn, "network_mojang_metadata_enabled") {
            policy.set_category(NetworkCategory::MojangMetadata, false);
        }
        if !is_network_enabled_default_true(conn, "network_mojang_content_enabled") {
            policy.set_category(NetworkCategory::MojangContent, false);
        }
        if !is_network_enabled_default_true(conn, "network_loader_enabled") {
            policy.set_category(NetworkCategory::LoaderMetadataAndContent, false);
        }
        if !db::is_network_enabled(conn, "network_msa_enabled") {
            policy.set_category(NetworkCategory::MicrosoftAuthentication, false);
        }
        if !db::is_network_enabled(conn, "network_adoptium_enabled") {
            policy.set_category(NetworkCategory::JavaRuntime, false);
        }
        policy
    }
}

/// Like `db::is_network_enabled` but returns `true` (default-enabled) when the
/// setting key is absent from the database. Matches the first-party-network-defaults
/// behaviour where missing keys are treated as enabled.
fn is_network_enabled_default_true(conn: &rusqlite::Connection, key: &str) -> bool {
    db::get_setting(conn, key)
        .ok()
        .flatten()
        .map(|v| match v {
            serde_json::Value::Bool(b) => b,
            serde_json::Value::String(s) => s == "true",
            _ => false,
        })
        .unwrap_or(true)
}

fn category_bit(category: NetworkCategory) -> u8 {
    match category {
        NetworkCategory::MicrosoftAuthentication => BIT_MSA,
        NetworkCategory::MojangMetadata => BIT_META,
        NetworkCategory::MojangContent => BIT_CONTENT,
        NetworkCategory::LoaderMetadataAndContent => BIT_LOADER,
        NetworkCategory::JavaRuntime => BIT_JAVA,
    }
}

/// Classify a URL host into a network category.
///
/// This enables callers to map an already-validated URL to the right
/// policy category before opening a socket. Returns `None` for hosts
/// that are not recognised as a known launch-network endpoint.
pub fn classify_host(host: &str) -> Option<NetworkCategory> {
    if matches!(host, "piston-meta.mojang.com" | "launcher.mojang.com") {
        return Some(NetworkCategory::MojangMetadata);
    }
    if matches!(
        host,
        "piston-data.mojang.com" | "libraries.minecraft.net" | "resources.download.minecraft.net"
    ) {
        return Some(NetworkCategory::MojangContent);
    }
    if loader_manifests::is_allowed_host(host) {
        return Some(NetworkCategory::LoaderMetadataAndContent);
    }
    None
}

/// Classify a full URL string into a network category by parsing its host.
pub fn classify_url(raw: &str) -> Option<NetworkCategory> {
    let url = reqwest::Url::parse(raw).ok()?;
    let host = url.host_str()?;
    classify_host(host)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_db() -> rusqlite::Connection {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("agora-network-test-{}.db", n));
        let _ = std::fs::remove_file(&path);
        let _ = crate::db::init_local_state_db(&path);
        rusqlite::Connection::open(&path).expect("test db")
    }

    #[test]
    fn all_enabled_allows_all_categories() {
        let policy = NetworkPolicy::all_enabled();
        assert!(policy.is_enabled(NetworkCategory::MojangMetadata));
        assert!(policy.is_enabled(NetworkCategory::MojangContent));
        assert!(policy.is_enabled(NetworkCategory::LoaderMetadataAndContent));
        assert!(policy.is_enabled(NetworkCategory::MicrosoftAuthentication));
        assert!(policy.is_enabled(NetworkCategory::JavaRuntime));
    }

    #[test]
    fn all_disabled_denies_all_categories() {
        let policy = NetworkPolicy::all_disabled();
        assert!(!policy.is_enabled(NetworkCategory::MojangMetadata));
        assert!(!policy.is_enabled(NetworkCategory::MojangContent));
        assert!(!policy.is_enabled(NetworkCategory::LoaderMetadataAndContent));
        assert!(!policy.is_enabled(NetworkCategory::MicrosoftAuthentication));
        assert!(!policy.is_enabled(NetworkCategory::JavaRuntime));
    }

    #[test]
    fn check_returns_dedicated_errors() {
        let policy = NetworkPolicy::all_disabled();
        assert!(matches!(
            policy.check(NetworkCategory::MojangMetadata),
            Err(LauncherError::NetworkMojangMetadataDisabled)
        ));
        assert!(matches!(
            policy.check(NetworkCategory::MojangContent),
            Err(LauncherError::NetworkMojangContentDisabled)
        ));
        assert!(matches!(
            policy.check(NetworkCategory::LoaderMetadataAndContent),
            Err(LauncherError::NetworkLoaderDisabled)
        ));
        assert!(matches!(
            policy.check(NetworkCategory::MicrosoftAuthentication),
            Err(LauncherError::NetworkMsaDisabled)
        ));
        assert!(matches!(
            policy.check(NetworkCategory::JavaRuntime),
            Err(LauncherError::NetworkJavaDisabled)
        ));
    }

    #[test]
    fn set_category_mutates_policy() {
        let mut policy = NetworkPolicy::all_disabled();
        policy.set_category(NetworkCategory::MojangMetadata, true);
        assert!(policy.is_enabled(NetworkCategory::MojangMetadata));
        assert!(!policy.is_enabled(NetworkCategory::MojangContent));
    }

    #[test]
    fn with_category_is_idempotent() {
        let policy =
            NetworkPolicy::all_enabled().with_category(NetworkCategory::MojangMetadata, true);
        assert!(policy.is_enabled(NetworkCategory::MojangMetadata));
    }

    #[test]
    fn classify_mojang_metadata_hosts() {
        assert_eq!(
            classify_host("piston-meta.mojang.com"),
            Some(NetworkCategory::MojangMetadata)
        );
        assert_eq!(
            classify_host("launcher.mojang.com"),
            Some(NetworkCategory::MojangMetadata)
        );
    }

    #[test]
    fn classify_mojang_content_hosts() {
        assert_eq!(
            classify_host("piston-data.mojang.com"),
            Some(NetworkCategory::MojangContent)
        );
        assert_eq!(
            classify_host("libraries.minecraft.net"),
            Some(NetworkCategory::MojangContent)
        );
        assert_eq!(
            classify_host("resources.download.minecraft.net"),
            Some(NetworkCategory::MojangContent)
        );
    }

    #[test]
    fn classify_loader_hosts() {
        // Fabric Maven — should NOT classify as Mojang content
        if let Some(cat) = classify_host("maven.fabricmc.net") {
            assert_eq!(cat, NetworkCategory::LoaderMetadataAndContent);
        }
        if let Some(cat) = classify_host("maven.quiltmc.org") {
            assert_eq!(cat, NetworkCategory::LoaderMetadataAndContent);
        }
    }

    #[test]
    fn classify_unknown_host_returns_none() {
        assert_eq!(classify_host("example.com"), None);
        assert_eq!(classify_host("127.0.0.1"), None);
    }

    #[test]
    fn from_db_all_keys_default_enabled_when_missing() {
        // Connection with NO launch-network keys at all — all categories
        // default to enabled (first-party network defaults).
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("agora-network-raw-{}.db", n));
        let _ = std::fs::remove_file(&path);
        let conn = rusqlite::Connection::open(&path).expect("raw test db");
        // Apply only minimal schema (no init, no default settings).
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS user_settings (
                  key TEXT PRIMARY KEY,
                  value_json TEXT NOT NULL
              );",
        )
        .expect("create settings table");

        let policy = NetworkPolicy::from_db(&conn);
        // Missing keys now default to enabled (first-party defaults)
        assert!(policy.is_enabled(NetworkCategory::MojangMetadata));
        assert!(policy.is_enabled(NetworkCategory::MojangContent));
        assert!(policy.is_enabled(NetworkCategory::LoaderMetadataAndContent));
        assert!(policy.is_enabled(NetworkCategory::MicrosoftAuthentication));
        assert!(policy.is_enabled(NetworkCategory::JavaRuntime));
    }

    #[test]
    fn from_db_defaults_enabled_when_keys_absent() {
        // Freshly-initialized DB: all keys default to true.
        let conn = test_db();
        let policy = NetworkPolicy::from_db(&conn);
        // All keys are now enabled by default
        assert!(policy.is_enabled(NetworkCategory::MojangMetadata));
        assert!(policy.is_enabled(NetworkCategory::MojangContent));
        assert!(policy.is_enabled(NetworkCategory::LoaderMetadataAndContent));
        assert!(policy.is_enabled(NetworkCategory::MicrosoftAuthentication));
        assert!(policy.is_enabled(NetworkCategory::JavaRuntime));
    }

    #[test]
    fn from_db_reads_enabled_for_keys() {
        let conn = test_db();
        // Override the three fail-closed keys to "true"
        crate::db::set_setting(
            &conn,
            "network_mojang_metadata_enabled",
            &serde_json::Value::String("true".into()),
        )
        .unwrap();
        crate::db::set_setting(
            &conn,
            "network_mojang_content_enabled",
            &serde_json::Value::String("true".into()),
        )
        .unwrap();
        crate::db::set_setting(
            &conn,
            "network_loader_enabled",
            &serde_json::Value::String("true".into()),
        )
        .unwrap();

        let policy = NetworkPolicy::from_db(&conn);
        assert!(policy.is_enabled(NetworkCategory::MojangMetadata));
        assert!(policy.is_enabled(NetworkCategory::MojangContent));
        assert!(policy.is_enabled(NetworkCategory::LoaderMetadataAndContent));
    }

    #[test]
    fn from_db_accepts_boolean_false_for_fail_closed_keys() {
        let conn = test_db();
        // Set fail-closed keys as JSON booleans (the new write format).
        crate::db::set_setting(
            &conn,
            "network_mojang_metadata_enabled",
            &serde_json::Value::Bool(false),
        )
        .unwrap();
        crate::db::set_setting(
            &conn,
            "network_mojang_content_enabled",
            &serde_json::Value::Bool(false),
        )
        .unwrap();
        crate::db::set_setting(
            &conn,
            "network_loader_enabled",
            &serde_json::Value::Bool(false),
        )
        .unwrap();

        let policy = NetworkPolicy::from_db(&conn);
        assert!(!policy.is_enabled(NetworkCategory::MojangMetadata));
        assert!(!policy.is_enabled(NetworkCategory::MojangContent));
        assert!(!policy.is_enabled(NetworkCategory::LoaderMetadataAndContent));
    }

    #[test]
    fn from_db_accepts_boolean_true_for_fail_closed_keys() {
        let conn = test_db();
        crate::db::set_setting(
            &conn,
            "network_mojang_metadata_enabled",
            &serde_json::Value::Bool(true),
        )
        .unwrap();
        crate::db::set_setting(
            &conn,
            "network_mojang_content_enabled",
            &serde_json::Value::Bool(true),
        )
        .unwrap();
        crate::db::set_setting(
            &conn,
            "network_loader_enabled",
            &serde_json::Value::Bool(true),
        )
        .unwrap();

        let policy = NetworkPolicy::from_db(&conn);
        assert!(policy.is_enabled(NetworkCategory::MojangMetadata));
        assert!(policy.is_enabled(NetworkCategory::MojangContent));
        assert!(policy.is_enabled(NetworkCategory::LoaderMetadataAndContent));
    }

    #[test]
    fn from_db_accepts_mixed_representations() {
        let conn = test_db();
        // One key as boolean, one as string, one left at its default (true).
        crate::db::set_setting(
            &conn,
            "network_mojang_metadata_enabled",
            &serde_json::Value::Bool(true),
        )
        .unwrap();
        crate::db::set_setting(
            &conn,
            "network_mojang_content_enabled",
            &serde_json::Value::String("false".into()),
        )
        .unwrap();
        // network_loader_enabled left at its init default (string "true").

        let policy = NetworkPolicy::from_db(&conn);
        assert!(policy.is_enabled(NetworkCategory::MojangMetadata));
        assert!(!policy.is_enabled(NetworkCategory::MojangContent));
        assert!(policy.is_enabled(NetworkCategory::LoaderMetadataAndContent));
    }

    #[test]
    fn from_db_reads_disabled_settings() {
        let conn = test_db();
        crate::db::set_setting(
            &conn,
            "network_mojang_metadata_enabled",
            &serde_json::Value::String("false".into()),
        )
        .unwrap();
        crate::db::set_setting(
            &conn,
            "network_mojang_content_enabled",
            &serde_json::Value::String("false".into()),
        )
        .unwrap();
        crate::db::set_setting(
            &conn,
            "network_loader_enabled",
            &serde_json::Value::String("false".into()),
        )
        .unwrap();
        crate::db::set_setting(
            &conn,
            "network_msa_enabled",
            &serde_json::Value::String("false".into()),
        )
        .unwrap();
        crate::db::set_setting(
            &conn,
            "network_adoptium_enabled",
            &serde_json::Value::String("false".into()),
        )
        .unwrap();

        let policy = NetworkPolicy::from_db(&conn);
        assert!(!policy.is_enabled(NetworkCategory::MojangMetadata));
        assert!(!policy.is_enabled(NetworkCategory::MojangContent));
        assert!(!policy.is_enabled(NetworkCategory::LoaderMetadataAndContent));
        assert!(!policy.is_enabled(NetworkCategory::MicrosoftAuthentication));
        assert!(!policy.is_enabled(NetworkCategory::JavaRuntime));
    }

    #[test]
    fn classify_url_rejects_invalid_urls() {
        assert_eq!(classify_url("not-a-url"), None);
    }

    #[test]
    fn classify_url_parses_host() {
        assert_eq!(
            classify_url("https://piston-meta.mojang.com/mc/game/version_manifest_v2.json"),
            Some(NetworkCategory::MojangMetadata)
        );
        assert_eq!(
            classify_url("https://piston-data.mojang.com/v2/1.21/client.jar"),
            Some(NetworkCategory::MojangContent)
        );
        assert_eq!(
            classify_url("https://maven.fabricmc.net/v2/0.19.0/profile.json"),
            Some(NetworkCategory::LoaderMetadataAndContent)
        );
    }
}
