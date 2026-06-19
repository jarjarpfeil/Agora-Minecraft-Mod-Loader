use crate::db;
use crate::error::{LauncherError, LauncherResult};
use crate::paths;
use ed25519_dalek::{Signature, VerifyingKey, PUBLIC_KEY_LENGTH, SIGNATURE_LENGTH};
use serde::{Deserialize, Serialize};
/// The GitHub repository hosting registry release assets (`owner/repo`).
const REGISTRY_REPO: &str = match option_env!("AGORA_REGISTRY_REPO") { Some(v) => v, None => "jarjarpfeil/Agora-Minecraft-Mod-Loader" };

/// Ed25519 public key (hex) for verifying registry.db signatures.
const REGISTRY_PUBKEY_HEX: &str = match option_env!("AGORA_REGISTRY_PUBKEY") { Some(v) => v, None => "47adee76cf587ee618f79eb2fa5bde003824d3bfc2dbb5080d33073c5a8f8c18" };

/// App-side expected schema version for registry.db.
///
/// Bumped to 2 alongside the compiler adding supplementary metadata columns
/// (description, body_markdown, page_url, license_id, source_updated_at) to
/// `registry_items`. v1 and v2 dbs are produced/expected in lockstep from a
/// single commit, so clients always receive a matching signed db via the
/// update flow.
pub const APP_REGISTRY_SCHEMA_VERSION: i64 = 2;

/// Minimum interval between automatic update checks (1 hour).
const UPDATE_CHECK_INTERVAL_SECS: i64 = 3600;

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
pub async fn check_and_download_update<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    force: bool,
) -> LauncherResult<RegistryStatus> {
    let cached_tag = get_cached_tag(app)?;
    let has_cached_db = paths::registry_db_path(app)
        .map(|p| p.exists())
        .unwrap_or(false);
    let cached_schema_version = if has_cached_db {
        read_cached_schema_version(app).ok()
    } else {
        None
    };

    if !force {
        if let Some(last_check) = get_last_check_time(app)? {
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

    let latest = match query_latest_release().await {
        Ok(release) => release,
        Err(e) => {
            return Ok(RegistryStatus {
                has_cached_db,
                cached_tag,
                cached_schema_version,
                latest_tag: None,
                update_available: false,
                checked: false,
                message: format!(
                    "Could not check for updates: {}. Using cached registry.",
                    e
                ),
            });
        }
    };

    set_last_check_time(app, chrono::Utc::now().timestamp())?;

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

    let db_url = latest
        .find_asset("registry.db")
        .ok_or(LauncherError::RegistryDownloadFailed)?;
    let sig_url = latest
        .find_asset("registry.db.sig")
        .ok_or(LauncherError::RegistrySignatureInvalid)?;

    let db_bytes = download_file(&db_url).await?;
    let sig_bytes = download_file(&sig_url).await?;

    verify_signature(&db_bytes, &sig_bytes)?;

    let schema_version = read_schema_version_from_bytes(app, &db_bytes)?;
    if schema_version > APP_REGISTRY_SCHEMA_VERSION {
        return Err(LauncherError::Generic {
            code: "ERR_REGISTRY_NEWER_SCHEMA".to_string(),
            message: format!(
                "Registry schema version {} is newer than supported {}. Please update the app.",
                schema_version, APP_REGISTRY_SCHEMA_VERSION
            ),
        });
    }

    atomic_replace_db(app, &db_bytes, &sig_bytes)?;
    set_cached_tag(app, &latest.tag_name)?;

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
pub fn get_status<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> RegistryStatus {
    let has_cached_db = paths::registry_db_path(app)
        .map(|p| p.exists())
        .unwrap_or(false);
    let cached_tag = get_cached_tag(app).ok().flatten();
    let cached_schema_version = if has_cached_db {
        read_cached_schema_version(app).ok()
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

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

impl GitHubRelease {
    fn find_asset(&self, name: &str) -> Option<String> {
        self.assets
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.browser_download_url.clone())
    }
}

async fn query_latest_release() -> Result<GitHubRelease, String> {
    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        REGISTRY_REPO
    );
    let client = reqwest::Client::builder()
        .user_agent("AgoraLauncher/1.0")
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    resp.json::<GitHubRelease>()
        .await
        .map_err(|e| e.to_string())
}

async fn download_file(url: &str) -> LauncherResult<Vec<u8>> {
    let client = reqwest::Client::builder()
        .user_agent("AgoraLauncher/1.0")
        .build()
        .map_err(|e| LauncherError::Generic {
            code: "ERR_NETWORK".to_string(),
            message: format!("Failed to build HTTP client: {e}"),
        })?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|_| LauncherError::NetworkOffline)?;
    if !resp.status().is_success() {
        return Err(LauncherError::RegistryDownloadFailed);
    }
    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|_| LauncherError::RegistryDownloadFailed)
}

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

    let pubkey_bytes =
        hex::decode(REGISTRY_PUBKEY_HEX).map_err(|_| LauncherError::Generic {
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

fn read_schema_version_from_bytes<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    db_bytes: &[u8],
) -> LauncherResult<i64> {
    // Write to the app's private data dir (not shared OS temp) to prevent
    // symlink-based TOCTOU attacks.
    let data_dir = paths::app_data_dir(app).map_err(|_| LauncherError::RegistryMissing)?;
    let temp_db = data_dir.join("registry_verify.tmp");

    // Clean up any stale file from a prior failed run.
    let _ = std::fs::remove_file(&temp_db);

    std::fs::write(&temp_db, db_bytes)
        .map_err(|_| LauncherError::RegistryDownloadFailed)?;

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

fn read_cached_schema_version<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> LauncherResult<i64> {
    let path = paths::registry_db_path(app).map_err(|_| LauncherError::RegistryMissing)?;
    let conn = rusqlite::Connection::open_with_flags(
        &path,
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

fn atomic_replace_db<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    db_bytes: &[u8],
    sig_bytes: &[u8],
) -> LauncherResult<()> {
    let db_path = paths::registry_db_path(app).map_err(|_| LauncherError::RegistryMissing)?;
    let sig_path = paths::registry_sig_path(app).map_err(|_| LauncherError::RegistryMissing)?;

    let db_tmp = db_path.with_extension("db.tmp");
    let sig_tmp = sig_path.with_extension("sig.tmp");

    std::fs::write(&db_tmp, db_bytes).map_err(|_| LauncherError::RegistryDownloadFailed)?;
    std::fs::write(&sig_tmp, sig_bytes).map_err(|_| LauncherError::RegistryDownloadFailed)?;

    // Rename sig first, then db. If the sig rename fails, the old db is still
    // intact. If the sig rename succeeds but db rename fails, old-db + new-sig
    // still verifies against the old db and allows a clean retry on next check.
    std::fs::rename(&sig_tmp, &sig_path).map_err(|_| LauncherError::RegistryDownloadFailed)?;
    std::fs::rename(&db_tmp, &db_path).map_err(|_| LauncherError::RegistryDownloadFailed)?;

    Ok(())
}

fn get_cached_tag<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> LauncherResult<Option<String>> {
    let conn = db::local_state_connection(app).map_err(|_| LauncherError::LocalStateFailed)?;
    db::get_setting(&conn, "cached_registry_tag")
        .map_err(|_| LauncherError::LocalStateFailed)
        .map(|v| v.and_then(|v| v.as_str().map(|s| s.to_string())))
}

fn set_cached_tag<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    tag: &str,
) -> LauncherResult<()> {
    let conn = db::local_state_connection(app).map_err(|_| LauncherError::LocalStateFailed)?;
    db::set_setting(
        &conn,
        "cached_registry_tag",
        &serde_json::Value::String(tag.to_string()),
    )
    .map_err(|_| LauncherError::LocalStateFailed)
}

fn get_last_check_time<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> LauncherResult<Option<i64>> {
    let conn = db::local_state_connection(app).map_err(|_| LauncherError::LocalStateFailed)?;
    db::get_setting(&conn, "last_registry_check")
        .map_err(|_| LauncherError::LocalStateFailed)
        .map(|v| v.and_then(|v| v.as_i64()))
}

fn set_last_check_time<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    timestamp: i64,
) -> LauncherResult<()> {
    let conn = db::local_state_connection(app).map_err(|_| LauncherError::LocalStateFailed)?;
    db::set_setting(
        &conn,
        "last_registry_check",
        &serde_json::Value::Number(timestamp.into()),
    )
    .map_err(|_| LauncherError::LocalStateFailed)
}

/// On first run with no cached DB, copy the local registry.db from the repo
/// if it exists. Development convenience for local testing.
pub fn seed_from_local_build<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> LauncherResult<bool> {
    let dest = paths::registry_db_path(app).map_err(|_| LauncherError::RegistryMissing)?;
    if dest.exists() {
        return Ok(false);
    }

    // When `npm run tauri:dev` launches the exe, current_dir() is typically
    // `desktop/src-tauri/`. The compiler writes `registry.db` at the repo
    // root, so we walk up to four parent directories looking for it. This
    // covers dev workflows where the user runs `python compiler/compile.py`
    // from the repo root before launching the app.
    let mut search_dir = std::env::current_dir().ok();
    for _ in 0..5 {
        if let Some(dir) = search_dir {
            let candidate = dir.join("registry.db");
            if candidate.exists() {
                std::fs::copy(&candidate, &dest)
                    .map_err(|_| LauncherError::RegistryDownloadFailed)?;
                let local_sig = candidate.with_extension("db.sig");
                if local_sig.exists() {
                    let dest_sig = paths::registry_sig_path(app)
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
