//! Registry database sync â€” download, verify, and atomically replace the
//! signed `registry.db` from GitHub Releases.
//!
//! This module is pure (no Tauri types) so it can be consumed by the Tauri
//! GUI, the standalone `agora` CLI, and the in-process MCP listener.

use crate::db;
use crate::error::{LauncherError, LauncherResult};
use crate::lock_manager::LockManager;
use crate::operation_manager::OperationManager;
use crate::paths;
use ed25519_dalek::{Signature, VerifyingKey, PUBLIC_KEY_LENGTH, SIGNATURE_LENGTH};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Resolve the registry repository with priority:
/// 1. CLI override (passed as parameter)
/// 2. `AGORA_REGISTRY_REPO` environment variable
/// 3. Built-in default: `"jarjarpfeil/Agora-Launcher"`
pub fn resolve_registry_repo(cli_override: Option<&str>) -> String {
    cli_override
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("AGORA_REGISTRY_REPO").ok())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "jarjarpfeil/Agora-Launcher".into())
}

/// Ed25519 public key (hex) for verifying registry.db signatures.
const REGISTRY_PUBKEY_HEX: &str = match option_env!("AGORA_REGISTRY_PUBKEY") {
    Some(v) => v,
    None => "47adee76cf587ee618f79eb2fa5bde003824d3bfc2dbb5080d33073c5a8f8c18",
};

/// App-side expected schema version for registry.db.
///
/// Bumped to 3 alongside the compiler adding a `modrinth_id` column to
/// `registry_items` (used for metadata hydration and as the version-resolution
/// fallback when a github_release mod's primary source fails). Compiler and
/// app ship in lockstep from a single commit, so clients always receive a
/// matching signed db via the update flow.
pub const APP_REGISTRY_SCHEMA_VERSION: i64 = 6;

/// Minimum interval between automatic update checks (1 hour).
const UPDATE_CHECK_INTERVAL_SECS: i64 = 3600;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryStatus {
    pub has_cached_db: bool,
    pub cached_tag: Option<String>,
    pub cached_schema_version: Option<i64>,
    pub latest_tag: Option<String>,
    pub update_available: bool,
    pub checked: bool,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Check GitHub Releases for a newer registry.db and download + verify it.
///
/// Steps (per section 4.1a):
/// 1. Query GitHub Releases API for the latest tagged release.
/// 2. Compare the tag with the locally cached tag.
/// 3. If newer: download registry.db and registry.db.sig from release assets.
/// 4. Verify the Ed25519 signature.
/// 5. Check schema_version for forward/backward compatibility.
/// 6. Atomically replace the cached DB (write .tmp, rename).
/// 7. Update the cached tag in local_state.db.
///
/// When `operation_manager` is provided, a "Update registry" operation is
/// registered for the duration of the download and verification.
pub async fn check_and_download_update(
    app_data_dir: &Path,
    local_state_path: &Path,
    force: bool,
    github_token: Option<String>,
    operation_manager: Option<&OperationManager>,
    registry_repo: &str,
    lock_manager: &LockManager,
) -> LauncherResult<RegistryStatus> {
    let conn =
        db::local_state_connection(local_state_path).map_err(|e| LauncherError::Generic {
            code: "ERR_DB".into(),
            message: e.to_string(),
        })?;
    if !db::is_network_enabled(&conn, "network_registry_sync_enabled") {
        return Err(LauncherError::Generic {
            code: "ERR_NETWORK_DISABLED".into(),
            message: "Registry sync is disabled in Privacy settings.".into(),
        });
    }
    drop(conn);
    let cached_tag = get_cached_tag(local_state_path)?;
    let has_cached_db = paths::registry_db_path(app_data_dir)
        .map(|p| p.exists())
        .unwrap_or(false);
    let cached_schema_version = if has_cached_db {
        let reg_path = paths::registry_db_path(app_data_dir).ok();
        read_cached_schema_version(reg_path.as_deref()).ok()
    } else {
        None
    };

    if !force {
        if let Some(last_check) = get_last_check_time(local_state_path)? {
            let now = chrono::Utc::now().timestamp();
            if now - last_check < UPDATE_CHECK_INTERVAL_SECS && cached_tag.is_some() {
                return Ok(RegistryStatus {
                    has_cached_db,
                    cached_tag,
                    cached_schema_version,
                    latest_tag: None,
                    update_available: false,
                    checked: false,
                    message: "Registry is up to date.".to_string(),
                });
            }
        }
    }

    // Acquire cross-process RegistryUpdate lock for the entire
    // check / download / atomic-replace / cached-tag update transaction.
    let _registry_lock = lock_manager.acquire(
        crate::lock_manager::LockResource::RegistryUpdate,
        "registry-sync",
    )?;

    let latest = match query_latest_release(registry_repo, github_token.as_deref()).await {
        Ok(release) => release,
        Err(e) => {
            return Ok(RegistryStatus {
                has_cached_db,
                cached_tag,
                cached_schema_version,
                latest_tag: None,
                update_available: false,
                checked: false,
                message: format!("Could not check for updates: {}. Using cached registry.", e),
            });
        }
    };

    set_last_check_time(local_state_path, chrono::Utc::now().timestamp())?;

    let update_available = cached_tag.as_deref() != Some(&latest.tag_name);

    if !update_available && has_cached_db {
        return Ok(RegistryStatus {
            has_cached_db,
            cached_tag,
            cached_schema_version,
            latest_tag: Some(latest.tag_name.clone()),
            update_available: false,
            checked: true,
            message: "Registry is up to date.".to_string(),
        });
    }

    // Register operation only when we are about to start network I/O.
    let _op = operation_manager.map(|m| m.register("Update registry"));

    let db_id = latest
        .find_asset("registry.db")
        .ok_or(LauncherError::RegistryDownloadFailed)?;
    let sig_id = latest
        .find_asset("registry.db.sig")
        .ok_or(LauncherError::RegistrySignatureInvalid)?;

    // Download via the GitHub Assets API endpoint, which 302-redirects to a
    // signed URL. The `browser_download_url` returns 404 for this repo.
    let db_url = format!(
        "https://api.github.com/repos/{}/releases/assets/{}",
        registry_repo, db_id
    );
    let sig_url = format!(
        "https://api.github.com/repos/{}/releases/assets/{}",
        registry_repo, sig_id
    );

    let db_bytes = download_file(&db_url, github_token.as_deref()).await?;
    let sig_bytes = download_file(&sig_url, github_token.as_deref()).await?;

    verify_signature(&db_bytes, &sig_bytes)?;

    let schema_version = read_schema_version_from_bytes(app_data_dir, &db_bytes)?;
    if schema_version > APP_REGISTRY_SCHEMA_VERSION {
        return Err(LauncherError::Generic {
            code: "ERR_REGISTRY_NEWER_SCHEMA".to_string(),
            message: format!(
                "Registry schema version {} is newer than supported {}. Please update the app.",
                schema_version, APP_REGISTRY_SCHEMA_VERSION
            ),
        });
    }

    let db_path =
        paths::registry_db_path(app_data_dir).map_err(|_| LauncherError::RegistryMissing)?;
    let sig_path =
        paths::registry_sig_path(app_data_dir).map_err(|_| LauncherError::RegistryMissing)?;
    atomic_replace_db(&db_path, &sig_path, &db_bytes, &sig_bytes)?;
    set_cached_tag(local_state_path, &latest.tag_name)?;

    if let Some(op) = _op {
        op.complete();
    }

    Ok(RegistryStatus {
        has_cached_db: true,
        cached_tag: Some(latest.tag_name.clone()),
        cached_schema_version: Some(schema_version),
        latest_tag: Some(latest.tag_name.clone()),
        update_available: false,
        checked: true,
        message: format!("Registry updated to {}.", latest.tag_name),
    })
}

/// Return the current registry status without performing a network check.
pub fn get_status(app_data_dir: &Path, local_state_path: &Path) -> RegistryStatus {
    let has_cached_db = paths::registry_db_path(app_data_dir)
        .map(|p| p.exists())
        .unwrap_or(false);
    let cached_tag = get_cached_tag(local_state_path).ok().flatten();
    let cached_schema_version = if has_cached_db {
        let reg_path = paths::registry_db_path(app_data_dir).ok();
        read_cached_schema_version(reg_path.as_deref()).ok()
    } else {
        None
    };
    RegistryStatus {
        has_cached_db,
        cached_tag,
        cached_schema_version,
        latest_tag: None,
        update_available: false,
        checked: false,
        message: if has_cached_db {
            "Using cached registry.".to_string()
        } else {
            "No registry database found. Click Check for Updates.".to_string()
        },
    }
}

/// On first run with no cached DB, copy the local registry.db from the repo
/// if it exists. Development convenience for local testing.
#[cfg(debug_assertions)]
pub fn seed_from_local_build(app_data_dir: &Path) -> LauncherResult<bool> {
    let dest = paths::registry_db_path(app_data_dir).map_err(|_| LauncherError::RegistryMissing)?;

    if dest.exists() {
        let cached_version = read_cached_schema_version(Some(&dest)).unwrap_or(0);
        let cached_mtime = dest.metadata().and_then(|m| m.modified()).ok();
        let mut search_dir = std::env::current_dir().ok();
        for _ in 0..5 {
            if let Some(dir) = search_dir {
                let candidate = dir.join("registry.db");
                if candidate.exists() {
                    if let Some(local_version) = read_schema_version_at(&candidate) {
                        // Re-seed when the local build's schema version is
                        // newer, OR when the local registry.db file was
                        // modified more recently than the cached copy. The
                        // version check alone misses content-only recompiles
                        // (added/edited mods at an unchanged schema version),
                        // leaving the dev cache stale.
                        let local_mtime = candidate.metadata().and_then(|m| m.modified()).ok();
                        let newer_mtime = match (cached_mtime, local_mtime) {
                            (Some(c), Some(l)) => l > c,
                            _ => false,
                        };
                        if local_version > cached_version || newer_mtime {
                            let dest_sig = paths::registry_sig_path(app_data_dir)
                                .map_err(|_| LauncherError::RegistryMissing)?;
                            let local_sig = candidate.with_extension("db.sig");
                            std::fs::copy(&candidate, &dest)
                                .map_err(|_| LauncherError::RegistryDownloadFailed)?;
                            if local_sig.exists() {
                                std::fs::copy(&local_sig, &dest_sig)
                                    .map_err(|_| LauncherError::RegistryDownloadFailed)?;
                            }
                            eprintln!(
                                "Re-seeded registry.db (schema {} -> {}) from local build at {}",
                                cached_version,
                                local_version,
                                candidate.display()
                            );
                            return Ok(true);
                        }
                    }
                    break;
                }
                search_dir = dir.parent().map(std::path::Path::to_path_buf);
            } else {
                break;
            }
        }
        return Ok(false);
    }

    let mut search_dir = std::env::current_dir().ok();
    for _ in 0..5 {
        if let Some(dir) = search_dir {
            let candidate = dir.join("registry.db");
            if candidate.exists() {
                std::fs::copy(&candidate, &dest)
                    .map_err(|_| LauncherError::RegistryDownloadFailed)?;
                let local_sig = candidate.with_extension("db.sig");
                if local_sig.exists() {
                    let dest_sig = paths::registry_sig_path(app_data_dir)
                        .map_err(|_| LauncherError::RegistryMissing)?;
                    std::fs::copy(&local_sig, &dest_sig)
                        .map_err(|_| LauncherError::RegistryDownloadFailed)?;
                }
                eprintln!(
                    "Seeded registry.db from local build at {}",
                    candidate.display()
                );
                return Ok(true);
            }
            search_dir = dir.parent().map(std::path::Path::to_path_buf);
        } else {
            break;
        }
    }

    Ok(false)
}

// ---------------------------------------------------------------------------
// Internal helpers â€” GitHub API
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    id: u64,
    name: String,
}

impl GitHubRelease {
    /// Return the asset id for the named asset.
    ///
    /// We resolve downloads through the GitHub Assets API endpoint
    /// (`/releases/assets/{id}`) rather than the `browser_download_url`,
    /// because the latter returns 404 for assets that require authentication.
    /// The Assets endpoint 302-redirects to a signed `objects.githubusercontent.com`
    /// URL that `reqwest` follows automatically and works for public repos.
    fn find_asset(&self, name: &str) -> Option<u64> {
        self.assets.iter().find(|a| a.name == name).map(|a| a.id)
    }
}

async fn query_latest_release(repo: &str, token: Option<&str>) -> Result<GitHubRelease, String> {
    let url = format!("https://api.github.com/repos/{}/releases/latest", repo);
    let _permit = crate::github_ratelimit::acquire_github_permit().await;
    let clients = crate::http_client::HttpClients::new().map_err(|e| e.to_string())?;
    let headers = token
        .map(|t| vec![("Authorization".into(), format!("Bearer {t}"))])
        .unwrap_or_default();
    let resp = crate::http_client::checked_request_with_headers(
        &clients,
        crate::http_client::ClientCategory::GitHub,
        &url,
        headers,
    )
    .await
    .map_err(|e| e.to_string())?;
    if crate::github_ratelimit::is_rate_limit_response(&resp) {
        let retry = crate::github_ratelimit::parse_retry_after(&resp);
        crate::github_ratelimit::report_rate_limit(retry).await;
        return Err(format!("GitHub rate limited (HTTP {})", resp.status()));
    }
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let body = crate::http_client::checked_response_bytes(
        resp,
        crate::http_client::ClientCategory::GitHub,
    )
    .await
    .map_err(|e| e.to_string())?;
    serde_json::from_slice::<GitHubRelease>(&body).map_err(|e| e.to_string())
}

async fn download_file(url: &str, token: Option<&str>) -> LauncherResult<Vec<u8>> {
    let _permit = crate::github_ratelimit::acquire_github_permit().await;
    let clients = crate::http_client::HttpClients::new()?;
    let mut headers = vec![("Accept".into(), "application/octet-stream".into())];
    if let Some(t) = token {
        headers.push(("Authorization".into(), format!("Bearer {t}")));
    }
    let resp = crate::http_client::checked_request_with_headers(
        &clients,
        crate::http_client::ClientCategory::GitHub,
        url,
        headers,
    )
    .await?;
    if crate::github_ratelimit::is_rate_limit_response(&resp) {
        let retry = crate::github_ratelimit::parse_retry_after(&resp);
        crate::github_ratelimit::report_rate_limit(retry).await;
        return Err(LauncherError::Generic {
            code: "ERR_RATE_LIMITED".to_string(),
            message: "GitHub rate limit hit during registry download.".to_string(),
        });
    }
    if !resp.status().is_success() {
        return Err(LauncherError::RegistryDownloadFailed);
    }
    crate::http_client::checked_response_bytes(resp, crate::http_client::ClientCategory::GitHub)
        .await
        .map_err(|_| LauncherError::RegistryDownloadFailed)
}

// ---------------------------------------------------------------------------
// Internal helpers â€” Ed25519 signature verification
// ---------------------------------------------------------------------------

fn verify_signature(db_bytes: &[u8], sig_bytes: &[u8]) -> LauncherResult<()> {
    // In debug builds only, allow skipping verification when no pubkey is set.
    // Release builds must have AGORA_REGISTRY_PUBKEY compiled in.
    #[cfg(not(debug_assertions))]
    {
        if REGISTRY_PUBKEY_HEX.is_empty() {
            return Err(LauncherError::Generic {
                code: "ERR_REGISTRY_PUBKEY_NOT_CONFIGURED".to_string(),
                message: "Registry public key not compiled in; refusing to verify. \
                          Set AGORA_REGISTRY_PUBKEY (Ed25519 public key, hex) as an \
                          environment variable before building the desktop app: \
                          `$env:AGORA_REGISTRY_PUBKEY='...'; npm run tauri:dev`. \
                          See README.md > \"Environment variables for the Tauri build\"."
                    .to_string(),
            });
        }
    }

    #[cfg(debug_assertions)]
    {
        if REGISTRY_PUBKEY_HEX.is_empty() {
            eprintln!(
                "WARNING: AGORA_REGISTRY_PUBKEY not set; skipping signature verification (debug build only).\n\
                 Set `$env:AGORA_REGISTRY_PUBKEY` (Ed25519 public key, hex) before `npm run tauri:dev` to enable verification.\n\
                 See README.md > \"Environment variables for the Tauri build\"."
            );
            if sig_bytes.is_empty() {
                return Err(LauncherError::RegistrySignatureInvalid);
            }
            return Ok(());
        }
    }

    let pubkey_bytes = hex::decode(REGISTRY_PUBKEY_HEX).map_err(|_| LauncherError::Generic {
        code: "ERR_REGISTRY_PUBKEY".to_string(),
        message: "Invalid compiled-in registry public key.".to_string(),
    })?;

    if pubkey_bytes.len() != PUBLIC_KEY_LENGTH {
        return Err(LauncherError::Generic {
            code: "ERR_REGISTRY_PUBKEY".to_string(),
            message: "Compiled-in registry public key is wrong length.".to_string(),
        });
    }

    if sig_bytes.len() != SIGNATURE_LENGTH {
        return Err(LauncherError::RegistrySignatureInvalid);
    }

    let pubkey_array: [u8; PUBLIC_KEY_LENGTH] = pubkey_bytes
        .try_into()
        .map_err(|_| LauncherError::RegistrySignatureInvalid)?;

    let sig_array: [u8; SIGNATURE_LENGTH] = sig_bytes
        .try_into()
        .map_err(|_| LauncherError::RegistrySignatureInvalid)?;

    let verifying_key = VerifyingKey::from_bytes(&pubkey_array)
        .map_err(|_| LauncherError::RegistrySignatureInvalid)?;
    let signature = Signature::from_bytes(&sig_array);

    use ed25519_dalek::Verifier;
    verifying_key
        .verify(db_bytes, &signature)
        .map_err(|_| LauncherError::RegistrySignatureInvalid)
}

// ---------------------------------------------------------------------------
// Internal helpers â€” schema version reading
// ---------------------------------------------------------------------------

/// Read the schema version from raw database bytes.
///
/// Writes to the app's private data dir (not shared OS temp) to prevent
/// symlink-based TOCTOU attacks.
fn read_schema_version_from_bytes(
    app_data_dir: &std::path::Path,
    db_bytes: &[u8],
) -> LauncherResult<i64> {
    let temp_db = app_data_dir.join("registry_verify.tmp");

    // Clean up any stale file from a prior failed run.
    let _ = std::fs::remove_file(&temp_db);

    std::fs::write(&temp_db, db_bytes).map_err(|_| LauncherError::RegistryDownloadFailed)?;

    let result = (|| -> LauncherResult<i64> {
        let conn = rusqlite::Connection::open_with_flags(
            &temp_db,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|_| LauncherError::RegistryDownloadFailed)?;

        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |row| row.get(0),
            )
            .map_err(|_| LauncherError::RegistryDownloadFailed)?;

        drop(conn);
        Ok(version)
    })();

    // Always clean up the temp file, even on error.
    let _ = std::fs::remove_file(&temp_db);

    result
}

/// Read the schema version from the cached registry database at `path`.
fn read_cached_schema_version(path: Option<&std::path::Path>) -> LauncherResult<i64> {
    let path = path.ok_or(LauncherError::RegistryMissing)?;
    let conn = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| LauncherError::RegistryMissing)?;

    conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |row| row.get(0),
    )
    .map_err(|_| LauncherError::RegistryMissing)
}

/// Open a read-only connection at an arbitrary path and read the schema version.
#[cfg(debug_assertions)]
fn read_schema_version_at(path: &std::path::Path) -> Option<i64> {
    let conn = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let _ = conn.pragma_update(None, "query_only", "ON");
    conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |row| row.get(0),
    )
    .ok()
}

// ---------------------------------------------------------------------------
// Internal helpers â€” atomic replace
// ---------------------------------------------------------------------------

fn atomic_replace_db(
    db_path: &std::path::Path,
    sig_path: &std::path::Path,
    db_bytes: &[u8],
    sig_bytes: &[u8],
) -> LauncherResult<()> {
    let db_tmp = db_path.with_extension("db.tmp");
    let sig_tmp = sig_path.with_extension("sig.tmp");

    std::fs::write(&db_tmp, db_bytes).map_err(|_| LauncherError::RegistryDownloadFailed)?;
    std::fs::write(&sig_tmp, sig_bytes).map_err(|_| LauncherError::RegistryDownloadFailed)?;

    // Rename sig first, then db. If the sig rename fails, the old db is still
    // intact. If the sig rename succeeds but db rename fails, old-db + new-sig
    // still verifies against the old db and allows a clean retry on next check.
    std::fs::rename(sig_tmp, sig_path).map_err(|_| LauncherError::RegistryDownloadFailed)?;
    std::fs::rename(db_tmp, db_path).map_err(|_| LauncherError::RegistryDownloadFailed)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers â€” local state (cached tag, check time)
// ---------------------------------------------------------------------------

fn get_cached_tag(local_state_path: &std::path::Path) -> LauncherResult<Option<String>> {
    let conn = db::local_state_connection(local_state_path)
        .map_err(|_| LauncherError::LocalStateFailed)?;
    db::get_setting(&conn, "cached_registry_tag")
        .map_err(|_| LauncherError::LocalStateFailed)
        .map(|v| v.and_then(|v| v.as_str().map(|s| s.to_string())))
}

fn set_cached_tag(local_state_path: &std::path::Path, tag: &str) -> LauncherResult<()> {
    let conn = db::local_state_connection(local_state_path)
        .map_err(|_| LauncherError::LocalStateFailed)?;
    db::set_setting(
        &conn,
        "cached_registry_tag",
        &serde_json::Value::String(tag.to_string()),
    )
    .map_err(|_| LauncherError::LocalStateFailed)
}

fn get_last_check_time(local_state_path: &std::path::Path) -> LauncherResult<Option<i64>> {
    let conn = db::local_state_connection(local_state_path)
        .map_err(|_| LauncherError::LocalStateFailed)?;
    db::get_setting(&conn, "last_registry_check")
        .map_err(|_| LauncherError::LocalStateFailed)
        .map(|v| v.and_then(|v| v.as_i64()))
}

fn set_last_check_time(local_state_path: &std::path::Path, timestamp: i64) -> LauncherResult<()> {
    let conn = db::local_state_connection(local_state_path)
        .map_err(|_| LauncherError::LocalStateFailed)?;
    db::set_setting(
        &conn,
        "last_registry_check",
        &serde_json::Value::Number(timestamp.into()),
    )
    .map_err(|_| LauncherError::LocalStateFailed)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_db() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "agora-registry-sync-test-{}.db",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = fs::remove_file(&path);
        db::init_local_state_db(&path).expect("failed to init test db");
        path
    }

    #[test]
    fn test_get_cached_tag_absent() {
        let path = temp_db();
        let result = get_cached_tag(&path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_set_and_get_cached_tag() {
        let path = temp_db();
        set_cached_tag(&path, "v1.2.3").unwrap();
        let result = get_cached_tag(&path).unwrap();
        assert_eq!(result, Some("v1.2.3".to_string()));
    }

    #[test]
    fn test_set_and_get_last_check_time() {
        let path = temp_db();
        set_last_check_time(&path, 1_000_000_000).unwrap();
        let result = get_last_check_time(&path).unwrap();
        assert_eq!(result, Some(1_000_000_000));
    }

    #[test]
    fn test_schema_version_from_bytes() {
        let tmp_dir = std::env::temp_dir().join("agora-test-schema-version");
        let _ = fs::remove_dir_all(&tmp_dir);
        fs::create_dir_all(&tmp_dir).unwrap();

        // Build a minimal registry.db with schema_version table.
        let db_path = tmp_dir.join("registry.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version (version) VALUES (5);
             CREATE TABLE registry_items (id TEXT PRIMARY KEY);",
        )
        .unwrap();
        drop(conn);

        let bytes = fs::read(&db_path).unwrap();
        let version = read_schema_version_from_bytes(&tmp_dir, &bytes).unwrap();
        assert_eq!(version, 5);

        let _ = fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_schema_version_missing_table() {
        let tmp_dir = std::env::temp_dir().join("agora-test-schema-missing");
        let _ = fs::remove_dir_all(&tmp_dir);
        fs::create_dir_all(&tmp_dir).unwrap();

        let db_path = tmp_dir.join("registry.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("CREATE TABLE foo (id TEXT)", []).unwrap();
        drop(conn);

        let bytes = fs::read(&db_path).unwrap();
        // The query uses COALESCE(MAX(version), 0) so a missing table
        // will error (table doesn't exist), not return 0.
        let result = read_schema_version_from_bytes(&tmp_dir, &bytes);
        assert!(result.is_err());

        let _ = fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_schema_version_from_cached() {
        let tmp_dir = std::env::temp_dir().join("agora-test-schema-cached");
        let _ = fs::remove_dir_all(&tmp_dir);
        fs::create_dir_all(&tmp_dir).unwrap();

        let db_path = tmp_dir.join("registry.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version (version) VALUES (3);",
        )
        .unwrap();
        drop(conn);

        let version = read_cached_schema_version(Some(&db_path)).unwrap();
        assert_eq!(version, 3);

        let _ = fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_schema_version_zero_when_missing() {
        let err = read_cached_schema_version(None).unwrap_err();
        assert!(matches!(err, LauncherError::RegistryMissing));
    }

    #[test]
    fn test_atomic_replace_creates_files() {
        let tmp_dir = std::env::temp_dir().join("agora-test-atomic");
        let _ = fs::remove_dir_all(&tmp_dir);
        fs::create_dir_all(&tmp_dir).unwrap();

        let db_path = tmp_dir.join("registry.db");
        let sig_path = tmp_dir.join("registry.db.sig");
        let db_bytes = b"fake db content";
        let sig_bytes = b"fake sig";

        atomic_replace_db(&db_path, &sig_path, db_bytes, sig_bytes).unwrap();

        assert!(db_path.exists());
        assert!(sig_path.exists());
        assert_eq!(fs::read(&db_path).unwrap(), db_bytes);
        assert_eq!(fs::read(&sig_path).unwrap(), sig_bytes);

        let _ = fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_registry_status_default() {
        let tmp_dir = std::env::temp_dir().join("agora-test-status");
        let _ = fs::remove_dir_all(&tmp_dir);
        fs::create_dir_all(&tmp_dir).unwrap();

        let ls_path = tmp_dir.join("local_state.db");
        db::init_local_state_db(&ls_path).unwrap();

        let status = get_status(&tmp_dir, &ls_path);
        assert!(!status.has_cached_db);
        assert!(status.cached_tag.is_none());
        assert!(status.cached_schema_version.is_none());
        assert!(!status.update_available);
        assert!(!status.checked);

        let _ = fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_verify_signature_debug_skips_when_no_key() {
        // With a non-empty default key, this should actually verify.
        // The default key is set in REGISTRY_PUBKEY_HEX, so we can't
        // easily test the "no key" path. Instead, test that the
        // verification function is callable without panicking.
        let db_bytes = b"test data";
        let sig_bytes = b"00000000000000000000000000000000\
                         00000000000000000000000000000000"; // 64 zero bytes
                                                            // This will fail verification (wrong key), but should not panic.
        let result = verify_signature(db_bytes, sig_bytes);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_signature_wrong_length() {
        let db_bytes = b"test data";
        let short_sig = b"deadbeef"; // 8 bytes, not 64
        let result = verify_signature(db_bytes, short_sig);
        assert!(result.is_err());
    }

    #[test]
    fn test_github_release_find_asset() {
        let release = GitHubRelease {
            tag_name: "v1.0.0".to_string(),
            assets: vec![
                GitHubAsset {
                    id: 1,
                    name: "registry.db".to_string(),
                },
                GitHubAsset {
                    id: 2,
                    name: "registry.db.sig".to_string(),
                },
            ],
        };
        assert_eq!(release.find_asset("registry.db"), Some(1));
        assert_eq!(release.find_asset("registry.db.sig"), Some(2));
        assert_eq!(release.find_asset("nonexistent"), None);
    }

    #[test]
    fn test_app_schema_version_constant() {
        // Sanity check: the expected schema version should be >= 1.
        const { assert!(APP_REGISTRY_SCHEMA_VERSION >= 1) };
    }
}
