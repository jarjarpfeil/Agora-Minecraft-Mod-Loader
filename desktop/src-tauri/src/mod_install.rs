use crate::auth;
use crate::db;
use crate::download;
use crate::error::{LauncherError, LauncherResult};
use crate::models::{InstanceManifest, InstanceRow, InstalledMod, ModVersionCandidate};
use crate::paths;
use crate::registry;
use serde::Deserialize;

/// Minimum free disk space required before a mod download (500 MB).
pub(crate) const MIN_DISK_SPACE_BYTES: u64 = 500_000_000;

/// Return the available free disk space (in bytes) on the drive containing
/// the given path.  Returns `None` when the information cannot be determined.
///
/// Implementation: shells out to `fsutil volume diskfree` on Windows.
#[cfg(target_os = "windows")]
pub(crate) fn available_disk_space_bytes(path: &std::path::Path) -> Option<u64> {
    let root = path.ancestors().last()?;
    let output = std::process::Command::new("fsutil")
        .args(["volume", "diskfree"])
        .arg(root)
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("Available free bytes:") {
            return rest.trim().parse::<u64>().ok();
        }
    }
    None
}

/// Stub for non-Windows platforms (not currently targeted by this crate).
#[cfg(not(target_os = "windows"))]
pub(crate) fn available_disk_space_bytes(_path: &std::path::Path) -> Option<u64> {
    None
}

/// Hosts allowed for mod downloads (GitHub + Modrinth).
/// Separate from the loader-manifest allowlist to enforce the whitelist principle.
const MOD_DOWNLOAD_ALLOWLIST: &[&str] = &[
    "github.com",
    "objects.githubusercontent.com",
    "api.github.com",
    "cdn.modrinth.com",
    "api.modrinth.com",
];

/// Check whether a URL host is on the mod-download allowlist.
fn is_mod_download_host(host: &str) -> bool {
    MOD_DOWNLOAD_ALLOWLIST.contains(&host)
}

/// Download bytes from a mod-download URL with redirect-safe policy.
///
/// Redirects are only followed when the target host is on the mod-download
/// allowlist, preventing SSRF via compromised/malicious URLs.
pub(crate) async fn download_mod_bytes(url: &str) -> LauncherResult<Vec<u8>> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if let Some(host) = attempt.url().host_str() {
                if is_mod_download_host(host) {
                    return attempt.follow();
                }
            }
            attempt.stop()
        }))
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
        return Err(LauncherError::Generic {
            code: "ERR_NETWORK".to_string(),
            message: format!("HTTP {} for {}", resp.status(), url),
        });
    }

    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|_| LauncherError::NetworkOffline)
}

/// Internal: fetch a GitHub token (if stored) and return an optional Bearer header value.
fn github_auth_header(app: &tauri::AppHandle) -> Option<String> {
    auth::get_token(app).map(|t| format!("Bearer {t}"))
}

/// Resolve the instance's Minecraft version and loader from `local_state.db`.
fn load_instance_info(
    app: &tauri::AppHandle,
    instance_id: &str,
) -> LauncherResult<InstanceRow> {
    let conn = db::local_state_connection(app).map_err(|_| LauncherError::LocalStateFailed)?;
    db::get_instance(&conn, instance_id)
        .map_err(|_| LauncherError::LocalStateFailed)?
        .ok_or_else(|| LauncherError::Generic {
            code: "ERR_INSTANCE_NOT_FOUND".to_string(),
            message: format!("Instance '{instance_id}' not found."),
        })
}

/// Resolve a single registry item by ID.
fn load_registry_item(app: &tauri::AppHandle, item_id: &str) -> LauncherResult<registry::RegistryItem> {
    let conn = registry::open_registry(app)?;
    registry::get_item_by_id(&conn, item_id)
        .map_err(|_| LauncherError::RegistryMissing)?
        .ok_or_else(|| LauncherError::Generic {
            code: "ERR_ITEM_NOT_FOUND".to_string(),
            message: format!("Registry item '{item_id}' not found."),
        })
}

/// Heuristic: check whether a filename contains both the Minecraft version
/// and loader strings. Returns (mc_version, loader) if both are found.
fn parse_version_from_filename(filename: &str, mc_version: &str, loader: &str) -> Option<(String, String)> {
    let lower = filename.to_lowercase();
    if lower.contains(&mc_version.to_lowercase()) && lower.contains(&loader.to_lowercase()) {
        Some((mc_version.to_string(), loader.to_string()))
    } else {
        None
    }
}

/// --- GitHub Releases API types ---

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    published_at: Option<String>,
    assets: Vec<GitHubReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubReleaseAsset {
    name: String,
    browser_download_url: String,
}

/// --- Modrinth API types ---

#[derive(Debug, Deserialize)]
struct ModrinthVersion {
    version_number: String,
    files: Vec<ModrinthVersionFile>,
}

#[derive(Debug, Deserialize)]
struct ModrinthVersionFile {
    url: String,
    filename: String,
    primary: bool,
}

/// List available mod versions for a registry item, resolving live data from
/// the upstream source (GitHub Releases or Modrinth).
///
/// Auto-fallback (§6.3 resilience): for `github_release` mods that also carry
/// a `modrinth_id`, if the primary GitHub resolver fails (network error,
/// rate-limit, or returns no candidates), the resolver transparently retries
/// against Modrinth. The installed file is still SHA-256-verified against the
/// pinned registry hash in `install_mod_version`, so a different build from
/// the alternate source is rejected rather than silently installed.
///
/// Modrinth toggle (§6.3): the `modrinth_id` strategy path and the
/// `github_release`→Modrinth fallback are both gated by the `modrinth_enabled`
/// setting. When the user has disabled Modrinth integration, NO Modrinth API
/// calls are made — GitHub remains the sole source for `github_release` mods,
/// and `modrinth_id`-strategy items surface `ModrinthDisabled`.
pub async fn list_mod_versions(
    app: &tauri::AppHandle,
    instance_id: &str,
    item_id: &str,
) -> LauncherResult<Vec<ModVersionCandidate>> {
    let instance = load_instance_info(app, instance_id)?;
    let item = load_registry_item(app, item_id)?;

    let mc_version = &instance.minecraft_version;
    let loader = &instance.loader;
    let strategy = item.download_strategy.as_str();
    let modrinth_on = crate::modrinth_raw::is_modrinth_enabled(app);

    match strategy {
        "github_release" => {
            let primary = resolve_github_releases(app, &item, mc_version, loader).await;
            // Fallback to Modrinth only when (a) GitHub yielded nothing useful,
            // (b) a modrinth_id is declared, AND (c) the Modrinth toggle is on.
            // When the toggle is off, no Modrinth API call is attempted and the
            // (possibly empty) GitHub result is returned as-is.
            let candidates = match primary {
                Ok(c) if !c.is_empty() => return Ok(c),
                Ok(_) => Vec::new(),
                Err(e) => {
                    crate::auth::log_line(&format!(
                        "list_mod_versions: github_release primary failed for '{}' ({}); {}",
                        item_id,
                        e,
                        if modrinth_on { "trying Modrinth fallback" } else { "Modrinth disabled, no fallback" }
                    ));
                    Vec::new()
                }
            };
            if modrinth_on {
                if let Some(mid) = item.modrinth_id.as_deref().filter(|s| !s.is_empty()) {
                    let alt = resolve_modrinth_versions_by_id(mid, mc_version, loader).await?;
                    if !alt.is_empty() {
                        return Ok(alt);
                    }
                }
            }
            Ok(candidates)
        }
        "modrinth_id" => {
            if !modrinth_on {
                return Err(LauncherError::ModrinthDisabled);
            }
            resolve_modrinth_versions(&item, mc_version, loader).await
        }
        _ => Err(LauncherError::Generic {
            code: "ERR_UNSUPPORTED_STRATEGY".to_string(),
            message: format!(
                "Download strategy '{}' is not supported for version resolution.",
                item.download_strategy
            ),
        }),
    }
}

async fn resolve_github_releases(
    app: &tauri::AppHandle,
    item: &registry::RegistryItem,
    mc_version: &str,
    loader: &str,
) -> LauncherResult<Vec<ModVersionCandidate>> {
    let source = &item.source_identifier;
    let url = format!("https://api.github.com/repos/{source}/releases");

    // Build the request with optional Bearer auth.
    let mut request = reqwest::Client::builder()
        .user_agent("AgoraLauncher/1.0")
        .build()
        .map_err(|e| LauncherError::Generic {
            code: "ERR_NETWORK".to_string(),
            message: format!("Failed to build HTTP client: {e}"),
        })?
        .get(&url);

    // Attach Bearer token if available to avoid 60 req/hr rate limit.
    if let Some(token) = github_auth_header(app) {
        request = request.header("Authorization", token);
    }

    let releases: Vec<GitHubRelease> = request
        .send()
        .await
        .map_err(|_| LauncherError::NetworkOffline)?
        .error_for_status()
        .map_err(|e| LauncherError::Generic {
            code: "ERR_NETWORK".to_string(),
            message: format!("GitHub API request failed: {e}"),
        })?
        .json()
        .await
        .map_err(|_| LauncherError::Generic {
            code: "ERR_NETWORK".to_string(),
            message: "Failed to parse GitHub releases response.".to_string(),
        })?;

    let mut candidates: Vec<ModVersionCandidate> = Vec::new();

    for release in &releases {
        for asset in &release.assets {
            if !asset.name.ends_with(".jar") {
                continue;
            }
            let (mc, loader_str) = parse_version_from_filename(&asset.name, mc_version, loader)
                .unwrap_or_else(|| (String::new(), String::new()));

            let mc_empty = mc.is_empty();
            let loader_empty = loader_str.is_empty();

            candidates.push(ModVersionCandidate {
                version: release.tag_name.clone(),
                filename: asset.name.clone(),
                download_url: asset.browser_download_url.clone(),
                mc_version: if mc_empty { None } else { Some(mc) },
                loader: if loader_empty { None } else { Some(loader_str) },
                release_date: release.published_at.clone(),
                is_compatible: !mc_empty && !loader_empty,
            });
        }
    }

    Ok(candidates)
}

async fn resolve_modrinth_versions(
    item: &registry::RegistryItem,
    mc_version: &str,
    loader: &str,
) -> LauncherResult<Vec<ModVersionCandidate>> {
    // For the `modrinth_id` strategy, source_identifier IS the Modrinth
    // project id (it doubles, allowing the minimal 5-line manifest).
    resolve_modrinth_versions_by_id(&item.source_identifier, mc_version, loader).await
}

/// Resolve Modrinth versions for an explicit project id/slug.
///
/// Shared by the `modrinth_id` strategy path and the `github_release`
/// auto-fallback path (which passes the optional `modrinth_id` field so a
/// GitHub-hosted mod can fall back to Modrinth when GitHub fails).
async fn resolve_modrinth_versions_by_id(
    project_id: &str,
    mc_version: &str,
    loader: &str,
) -> LauncherResult<Vec<ModVersionCandidate>> {
    let url = format!(
        "https://api.modrinth.com/v2/project/{project_id}/version?game_versions=[\"{mc_version}\"]&loaders=[\"{loader}\"]"
    );

    let client = reqwest::Client::builder()
        .user_agent("AgoraLauncher/1.0")
        .build()
        .map_err(|e| LauncherError::Generic {
            code: "ERR_NETWORK".to_string(),
            message: format!("Failed to build HTTP client: {e}"),
        })?;

    let versions: Vec<ModrinthVersion> = client
        .get(&url)
        .send()
        .await
        .map_err(|_| LauncherError::NetworkOffline)?
        .error_for_status()
        .map_err(|e| LauncherError::Generic {
            code: "ERR_NETWORK".to_string(),
            message: format!("Modrinth API request failed: {e}"),
        })?
        .json()
        .await
        .map_err(|_| LauncherError::Generic {
            code: "ERR_NETWORK".to_string(),
            message: "Failed to parse Modrinth versions response.".to_string(),
        })?;

    let mut candidates: Vec<ModVersionCandidate> = Vec::new();

    for version in &versions {
        // Use the primary file if present; otherwise fall back to the first file.
        let primary_file = version
            .files
            .iter()
            .find(|f| f.primary)
            .or_else(|| version.files.first());

        let file = match primary_file {
            Some(f) => f,
            None => continue,
        };

        let (mc, loader_str) = parse_version_from_filename(&file.filename, mc_version, loader)
            .unwrap_or_else(|| (String::new(), String::new()));

        let mc_empty = mc.is_empty();
        let loader_empty = loader_str.is_empty();

        candidates.push(ModVersionCandidate {
            version: version.version_number.clone(),
            filename: file.filename.clone(),
            download_url: file.url.clone(),
            mc_version: if mc_empty { None } else { Some(mc) },
            loader: if loader_empty { None } else { Some(loader_str) },
            release_date: None,
            is_compatible: !mc_empty && !loader_empty,
        });
    }

    Ok(candidates)
}

/// Install a specific mod version into an instance's `mods/` directory.
pub async fn install_mod_version(
    app: &tauri::AppHandle,
    instance_id: &str,
    item_id: &str,
    candidate: &ModVersionCandidate,
) -> LauncherResult<InstalledMod> {
    // 1. Load the registry item to get the pinned SHA-256.
    let item = load_registry_item(app, item_id)?;
    let pinned_sha = item.sha256.trim().to_string();

    // 2. Pre-check: verify sufficient disk space before any network request (§7.1.2).
    let instance_dir = paths::instance_dir(app, instance_id)
        .map_err(|_| LauncherError::InstanceCreateFailed)?;
    if let Some(free) = available_disk_space_bytes(&instance_dir) {
        if free < MIN_DISK_SPACE_BYTES {
            return Err(LauncherError::DiskFull);
        }
    }
    // If we cannot determine free space (None), proceed — do not block on unavailable info.

    // 3. Download bytes from the candidate URL.
    let bytes = download_mod_bytes(&candidate.download_url).await?;

    // 3. Verify SHA-256 against the pinned hash.
    let actual_sha = download::sha256_hex(&bytes);
    if actual_sha != pinned_sha {
        return Err(LauncherError::HashMismatch);
    }

    // 4. Ensure mods/ directory exists and write the file.
    let instance_dir = paths::instance_dir(app, instance_id)
        .map_err(|_| LauncherError::InstanceCreateFailed)?;
    let mods_dir = instance_dir.join("mods");
    std::fs::create_dir_all(&mods_dir).map_err(|_| LauncherError::InstanceCreateFailed)?;

    let mod_path = mods_dir.join(&candidate.filename);
    std::fs::write(&mod_path, &bytes).map_err(|_| LauncherError::InstanceCreateFailed)?;

    // 5. Update the instance manifest atomically.
    let manifest_path = paths::instance_manifest_path(app, instance_id)
        .map_err(|_| LauncherError::InstanceCreateFailed)?;

    let mut manifest: InstanceManifest = if manifest_path.exists() {
        let text = std::fs::read_to_string(&manifest_path)
            .map_err(|_| LauncherError::InstanceCreateFailed)?;
        serde_json::from_str(&text)
            .map_err(|_| LauncherError::InstanceCreateFailed)?
    } else {
        return Err(LauncherError::Generic {
            code: "ERR_MANIFEST_MISSING".to_string(),
            message: format!(
                "Instance manifest not found at '{}'. Create the instance first.",
                manifest_path.display()
            ),
        });
    };

    let installed_mod = InstalledMod {
        filename: candidate.filename.clone(),
        registry_id: Some(item_id.to_string()),
        modrinth_id: None,
        source: "registry".to_string(),
        version: Some(candidate.version.clone()),
        sha256: pinned_sha,
        installed_at: chrono::Utc::now().to_rfc3339(),
    };

    manifest.mods.push(installed_mod.clone());

    // Atomic write: .tmp then rename.
    let tmp_path = manifest_path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(&manifest)
        .map_err(|_| LauncherError::InstanceCreateFailed)?;
    std::fs::write(&tmp_path, text).map_err(|_| LauncherError::InstanceCreateFailed)?;
    std::fs::rename(&tmp_path, &manifest_path)
        .map_err(|_| LauncherError::InstanceCreateFailed)?;

    Ok(installed_mod)
}

/// Remove a mod from an instance's `mods/` directory and update the manifest.
///
/// Best-effort: if the mod isn't in the manifest but the file exists on disk,
/// it is still deleted. If the mod is in the manifest but the file is missing,
/// the manifest is still updated.
pub async fn remove_mod_from_instance(
    app: &tauri::AppHandle,
    instance_id: &str,
    filename: &str,
) -> LauncherResult<()> {
    // Zip Slip protection: reject filenames containing path traversal or separators.
    if filename.contains("..") || filename.contains('/') || filename.contains('\\') {
        return Err(LauncherError::Generic {
            code: "ERR_INVALID_FILENAME".to_string(),
            message: "Filename contains invalid characters.".to_string(),
        });
    }

    // 1. Resolve and optionally delete the jar file.
    let instance_dir = paths::instance_dir(app, instance_id)
        .map_err(|_| LauncherError::InstanceCreateFailed)?;
    let mods_dir = instance_dir.join("mods");
    let mod_path = mods_dir.join(filename);

    tokio::task::spawn_blocking(move || {
        if mod_path.exists() {
            std::fs::remove_file(&mod_path).map_err(|_| LauncherError::InstanceCreateFailed)?;
        }
        Ok::<_, LauncherError>(())
    })
    .await
    .map_err(|_| LauncherError::Generic {
        code: "ERR_REMOVE_FAILED".to_string(),
        message: "Remove file task failed.".to_string(),
    })??;

    // 2. Load, filter, and rewrite the manifest.
    let manifest_path = paths::instance_manifest_path(app, instance_id)
        .map_err(|_| LauncherError::InstanceCreateFailed)?;

    if manifest_path.exists() {
        let text = tokio::task::spawn_blocking({
            let manifest_path = manifest_path.clone();
            move || {
                let text = std::fs::read_to_string(&manifest_path)
                    .map_err(|_| LauncherError::InstanceCreateFailed)?;
                Ok::<_, LauncherError>(text)
            }
        })
        .await
        .map_err(|_| LauncherError::Generic {
            code: "ERR_MANIFEST_READ".to_string(),
            message: "Manifest read task failed.".to_string(),
        })??;

        let mut manifest: InstanceManifest = serde_json::from_str(&text)
            .map_err(|_| LauncherError::InstanceCreateFailed)?;

        let before = manifest.mods.len();
        manifest.mods.retain(|m| m.filename != filename);
        if manifest.mods.len() < before {
            // Atomic write: .tmp then rename.
            let tmp_path = manifest_path.with_extension("json.tmp");
            let write_text = serde_json::to_string_pretty(&manifest)
                .map_err(|_| LauncherError::InstanceCreateFailed)?;
            tokio::task::spawn_blocking(move || {
                std::fs::write(&tmp_path, write_text).map_err(|_| LauncherError::InstanceCreateFailed)?;
                std::fs::rename(&tmp_path, &manifest_path)
                    .map_err(|_| LauncherError::InstanceCreateFailed)?;
                Ok::<_, LauncherError>(())
            })
            .await
            .map_err(|_| LauncherError::Generic {
                code: "ERR_MANIFEST_WRITE".to_string(),
                message: "Manifest write task failed.".to_string(),
            })??;
        }
    }

    Ok(())
}
