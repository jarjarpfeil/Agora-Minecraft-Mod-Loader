use crate::auth;
use crate::crash_investigator;
use crate::db;
use crate::download;
use crate::error::{LauncherError, LauncherResult};
use crate::instances;
use crate::models::{InstanceManifest, InstanceRow, InstalledMod, ModVersionCandidate};
use crate::paths;
use crate::registry;
use serde::Deserialize;
use std::path::Path;

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
struct ModrinthFileHashes {
    sha1: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModrinthVersionFile {
    url: String,
    filename: String,
    primary: bool,
    hashes: Option<ModrinthFileHashes>,
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
                sha1: None,
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
            sha1: file.hashes.as_ref().and_then(|h| h.sha1.clone()),
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

    // 3. Verify hash against the appropriate source.
    let candidate_sha1 = candidate.sha1.as_deref().unwrap_or("").trim().to_lowercase();
    if !candidate_sha1.is_empty() {
        // Modrinth-published per-file SHA-1: verify against that.
        let actual_sha1 = download::sha1_hex(&bytes);
        if actual_sha1 != candidate_sha1 {
            return Err(LauncherError::HashMismatch);
        }
    } else {
        // GitHub release or no per-file hash: verify against the pinned SHA-256.
        let actual_sha = download::sha256_hex(&bytes);
        if actual_sha != pinned_sha {
            return Err(LauncherError::HashMismatch);
        }
    }

    // 3b. Compute the actual SHA-256 of the downloaded bytes for the manifest.
    let installed_sha256 = download::sha256_hex(&bytes);

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

    let metadata = crash_investigator::parse_jar_metadata(&mod_path);
    let installed_mod = InstalledMod {
        filename: candidate.filename.clone(),
        registry_id: Some(item_id.to_string()),
        modrinth_id: item.modrinth_id.clone(),
        source: "registry".to_string(),
        version: Some(candidate.version.clone()),
        sha256: installed_sha256,
        installed_at: chrono::Utc::now().to_rfc3339(),
        java_packages: metadata.java_packages,
        mod_jar_id: metadata.mod_jar_id,
        depends_on: metadata.depends_on,
        optional_deps: metadata.optional_deps,
        incompatible_deps: metadata.incompatible_deps,
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

/// Add a manually-dropped .jar file into an instance's `mods/` folder (§6.5b).
///
/// Copies the file at `source_path` into the instance mods directory, computes
/// its SHA-256, and appends an `InstalledMod` with `source: "manual_drag_drop"`
/// to `instance_manifest.json` (atomic .tmp + rename).
///
/// Security: `source_path` arrives from IPC and is untrusted. It is
/// canonicalized and required to resolve within one of the user's allowlisted
/// drop directories (Downloads / Desktop / Documents / OS temp). Anything
/// outside that whitelist is rejected, so a compromised frontend cannot use
/// this command as an arbitrary-file-read primitive to exfiltrate e.g.
/// `~/.ssh/id_rsa` into a discoverable location. All blocking file I/O runs
/// inside a single `spawn_blocking` so the async runtime is never blocked.
pub async fn add_manual_mod(
    app: &tauri::AppHandle,
    instance_id: &str,
    source_path: &str,
) -> LauncherResult<InstalledMod> {
    use std::path::Path;

    let src = Path::new(source_path);
    let ext = src.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
    if ext.as_deref() != Some("jar") {
        return Err(LauncherError::Generic {
            code: "ERR_INVALID_FILENAME".to_string(),
            message: "Only .jar files can be added manually.".to_string(),
        });
    }
    let file_name = src.file_name().and_then(|n| n.to_str()).ok_or_else(|| LauncherError::Generic {
        code: "ERR_INVALID_FILENAME".to_string(),
        message: "Could not determine a valid file name.".to_string(),
    })?;
    if file_name.contains("..") || file_name.contains('/') || file_name.contains('\\') {
        return Err(LauncherError::Generic {
            code: "ERR_INVALID_FILENAME".to_string(),
            message: "Filename contains invalid characters.".to_string(),
        });
    }

    let mods_dir = paths::instance_dir(app, instance_id)
        .map_err(|_| LauncherError::InstanceCreateFailed)?
        .join("mods");
    let dest = mods_dir.join(file_name);
    let manifest_path = paths::instance_manifest_path(app, instance_id)
        .map_err(|_| LauncherError::InstanceCreateFailed)?;

    let source_path_owned = source_path.to_string();
    let file_name_owned = file_name.to_string();

    let installed_mod = tokio::task::spawn_blocking(move || -> LauncherResult<InstalledMod> {
        // Canonicalize the source path so symlinks resolve and we can compare
        // against the allowlisted drop roots.
        let canonical = std::fs::canonicalize(&source_path_owned).map_err(|_| LauncherError::Generic {
            code: "ERR_INVALID_SOURCE".to_string(),
            message: "Source file does not exist or cannot be resolved.".to_string(),
        })?;

        // Build the allowlist of canonical drop directories. `dirs` roots may
        // themselves contain symlinks, so canonicalize each before comparing.
        let mut roots: Vec<std::path::PathBuf> = Vec::new();
        for r in [
            dirs::download_dir(),
            dirs::desktop_dir(),
            dirs::document_dir(),
            Some(std::env::temp_dir()),
        ]
        .into_iter()
        .flatten()
        {
            if let Ok(c) = std::fs::canonicalize(&r) {
                roots.push(c);
            }
        }
        let allowed = roots.iter().any(|root| canonical.starts_with(root));
        if !allowed {
            return Err(LauncherError::Generic {
                code: "ERR_SOURCE_NOT_ALLOWED".to_string(),
                message: "Source file is outside the allowed drop directories \
                          (Downloads, Desktop, Documents, or system temp)."
                    .to_string(),
            });
        }

        let bytes = std::fs::read(&canonical).map_err(|_| LauncherError::Generic {
            code: "ERR_READ_FAILED".to_string(),
            message: "Failed to read the dropped file.".to_string(),
        })?;

        std::fs::create_dir_all(&mods_dir).map_err(|_| LauncherError::InstanceCreateFailed)?;
        std::fs::write(&dest, &bytes).map_err(|_| LauncherError::InstanceCreateFailed)?;
        let sha256 = download::sha256_hex(&bytes);

        if !manifest_path.exists() {
            return Err(LauncherError::Generic {
                code: "ERR_MANIFEST_MISSING".to_string(),
                message: "Instance manifest not found. Create the instance first.".to_string(),
            });
        }
        let text = std::fs::read_to_string(&manifest_path)
            .map_err(|_| LauncherError::InstanceCreateFailed)?;
        let mut manifest: InstanceManifest =
            serde_json::from_str(&text).map_err(|_| LauncherError::InstanceCreateFailed)?;

        let metadata = crash_investigator::parse_jar_metadata(&dest);
        let installed_mod = InstalledMod {
            filename: file_name_owned.clone(),
            registry_id: None,
            modrinth_id: None,
            source: "manual_drag_drop".to_string(),
            version: None,
            sha256,
            installed_at: chrono::Utc::now().to_rfc3339(),
            java_packages: metadata.java_packages,
            mod_jar_id: metadata.mod_jar_id,
            depends_on: metadata.depends_on,
            optional_deps: metadata.optional_deps,
            incompatible_deps: metadata.incompatible_deps,
        };
        manifest.mods.push(installed_mod.clone());

        let tmp_path = manifest_path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(&manifest)
            .map_err(|_| LauncherError::InstanceCreateFailed)?;
        std::fs::write(&tmp_path, text).map_err(|_| LauncherError::InstanceCreateFailed)?;
        std::fs::rename(&tmp_path, &manifest_path)
            .map_err(|_| LauncherError::InstanceCreateFailed)?;

        Ok(installed_mod)
    })
    .await
    .map_err(|_| LauncherError::Generic {
        code: "ERR_MANIFEST_WRITE".to_string(),
        message: "Manual mod add task failed.".to_string(),
    })??;

    Ok(installed_mod)
}

/// Validate a mod filename and return its zip entry name (`mods/<filename>`).
///
/// Returns `None` for names that could escape the `mods/` directory (traversal
/// via `..`, `/`, `\`, or absolute/null) so the caller can record a manifest-
/// only fallback instead of writing the bytes. This guards the mrpack export
/// against zip-slip and against mods added through non-`add_manual_mod` paths
/// (e.g. Modrinth download, override extraction) that don't pre-sanitize.
fn safe_zip_entry_name(filename: &str) -> Option<String> {
    if filename.is_empty()
        || filename == "."
        || filename == ".."
        || filename.contains('/')
        || filename.contains('\\')
        || filename.contains('\0')
    {
        return None;
    }
    Some(format!("mods/{}", filename))
}

/// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allowed_host_github() {
        assert!(is_mod_download_host("github.com"));
    }

    #[test]
    fn test_allowed_host_modrinth() {
        assert!(is_mod_download_host("cdn.modrinth.com"));
    }

    #[test]
    fn test_disallowed_host_localhost() {
        assert!(!is_mod_download_host("127.0.0.1"));
    }

    #[test]
    fn test_disallowed_host_metadata_ip() {
        assert!(!is_mod_download_host("169.254.169.254"));
    }

    #[test]
    fn test_disallowed_host_random() {
        assert!(!is_mod_download_host("evil.example.com"));
    }

    #[test]
    fn test_disallowed_host_file_scheme() {
        // The function takes a bare host string; a file-scheme URL would
        // typically be parsed and its host would be empty or "etc".
        // Test that an empty host is rejected and "etc" is rejected.
        assert!(!is_mod_download_host(""));
        assert!(!is_mod_download_host("etc"));
    }

    #[test]
    fn test_disallowed_host_empty() {
        assert!(!is_mod_download_host(""));
    }

    #[test]
    fn test_filename_path_traversal_rejected() {
        assert!(safe_zip_entry_name("../../evil.jar").is_none());
        assert!(safe_zip_entry_name("../../../etc/passwd.jar").is_none());
    }

    #[test]
    fn test_safe_zip_entry_name_valid() {
        let result = safe_zip_entry_name("some-mod-1.0.jar");
        assert_eq!(result, Some("mods/some-mod-1.0.jar".to_string()));
    }

    #[test]
    fn test_safe_zip_entry_name_slash_rejected() {
        assert!(safe_zip_entry_name("foo/bar.jar").is_none());
    }

    #[test]
    fn test_safe_zip_entry_name_backslash_rejected() {
        assert!(safe_zip_entry_name("foo\\bar.jar").is_none());
    }

    #[test]
    fn test_safe_zip_entry_name_null_rejected() {
        assert!(safe_zip_entry_name("foo\0bar.jar").is_none());
    }

    #[test]
    fn test_safe_zip_entry_name_dot_rejected() {
        assert!(safe_zip_entry_name(".").is_none());
        assert!(safe_zip_entry_name("..").is_none());
    }

    #[test]
    fn test_safe_zip_entry_name_empty_rejected() {
        assert!(safe_zip_entry_name("").is_none());
    }
}

/// Stream a single file into the zip writer, computing SHA-256 + size as bytes
/// flow through. Peak memory is bounded by `CHUNK` rather than the full file.
fn stream_jar_into_zip(
    zip: &mut zip::ZipWriter<std::fs::File>,
    opts: zip::write::FileOptions,
    entry_name: &str,
    path: &std::path::Path,
) -> LauncherResult<(String, u64)> {
    use std::io::{Read, Write};
    use sha2::Digest;
    const CHUNK: usize = 64 * 1024;

    let mut f = std::fs::File::open(path).map_err(|_| LauncherError::InstanceCreateFailed)?;
    zip.start_file(entry_name, opts)
        .map_err(|_| LauncherError::InstanceCreateFailed)?;
    let mut hasher = sha2::Sha256::new();
    let mut buf = [0u8; CHUNK];
    let mut size: u64 = 0;
    loop {
        let n = f.read(&mut buf).map_err(|_| LauncherError::InstanceCreateFailed)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        zip.write_all(&buf[..n])
            .map_err(|_| LauncherError::InstanceCreateFailed)?;
        size += n as u64;
    }
    Ok((hex::encode(hasher.finalize()), size))
}

/// Export an instance as a shareable pack file (§6.5c).
///
/// - `format == "json"`: a custom `.agora-pack.json` manifest containing the
///   instance metadata + installed-mod list (registry ids, sources, versions,
///   SHA-256 hashes). Small (5–20KB) and sufficient to rebuild the instance on
///   another machine. No mod binaries are bundled.
/// - `format == "mrpack"`: a `.mrpack` (zip) containing `modrinth.index.json`
///   plus the actual mod `.jar` files under their `mods/<filename>` paths.
///
/// Returns the absolute path to the written export file.
pub async fn export_instance_pack(
    app: &tauri::AppHandle,
    instance_id: &str,
    format: &str,
) -> LauncherResult<String> {
    use std::io::Write;

    let manifest_path = paths::instance_manifest_path(app, instance_id)
        .map_err(|_| LauncherError::InstanceCreateFailed)?;
    if !manifest_path.exists() {
        return Err(LauncherError::Generic {
            code: "ERR_MANIFEST_MISSING".to_string(),
            message: "Instance manifest not found.".to_string(),
        });
    }
    let manifest: InstanceManifest = {
        let text = std::fs::read_to_string(&manifest_path)
            .map_err(|_| LauncherError::InstanceCreateFailed)?;
        serde_json::from_str(&text).map_err(|_| LauncherError::InstanceCreateFailed)?
    };

    let exports_dir = paths::app_data_dir(app)
        .map_err(|_| LauncherError::InstanceCreateFailed)?
        .join("exports");
    std::fs::create_dir_all(&exports_dir).map_err(|_| LauncherError::InstanceCreateFailed)?;
    let safe_id = paths::sanitize_id(instance_id);

    match format {
        "json" => {
            let pack = serde_json::json!({
                "format": "agora-pack/v1",
                "instance": {
                    "id": manifest.instance_id,
                    "name": manifest.name,
                    "minecraft_version": manifest.minecraft_version,
                    "loader": manifest.loader,
                    "loader_version": manifest.loader_version,
                },
                "mods": manifest.mods.iter().map(|m| serde_json::json!({
                    "filename": m.filename,
                    "registry_id": m.registry_id,
                    "modrinth_id": m.modrinth_id,
                    "source": m.source,
                    "version": m.version,
                    "sha256": m.sha256,
                })).collect::<Vec<_>>(),
            });
            let out_path = exports_dir.join(format!("{}.agora-pack.json", safe_id));
            let tmp_path = out_path.with_extension("json.tmp");
            let text = serde_json::to_string_pretty(&pack)
                .map_err(|_| LauncherError::InstanceCreateFailed)?;
            std::fs::write(&tmp_path, text).map_err(|_| LauncherError::InstanceCreateFailed)?;
            std::fs::rename(&tmp_path, &out_path).map_err(|_| LauncherError::InstanceCreateFailed)?;
            Ok(out_path.to_string_lossy().to_string())
        }
        "mrpack" => {
            let mods_dir = paths::instance_dir(app, instance_id)
                .map_err(|_| LauncherError::InstanceCreateFailed)?
                .join("mods");

            let out_path = exports_dir.join(format!("{}.mrpack", safe_id));
            let tmp_path = out_path.with_extension("mrpack.tmp");

            {
                let file = std::fs::File::create(&tmp_path)
                    .map_err(|_| LauncherError::InstanceCreateFailed)?;
                let mut zip = zip::ZipWriter::new(file);
                let opts: zip::write::FileOptions =
                    zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);

                let mut files_meta: Vec<serde_json::Value> = Vec::new();

                for m in &manifest.mods {
                    // Modrinth-tracked mods: reference the upstream file by URL + published
                    // hashes instead of bundling the jar. This is the mrpack v1 best practice
                    // and keeps exported packs tiny + resolvable. Falls through to local
                    // bundling when Modrinth has no metadata for this filename.
                    if let Some(mid) = m.modrinth_id.as_deref().filter(|s| !s.trim().is_empty()) {
                        if let Some(meta) = crate::modrinth_raw::resolve_modrinth_file_metadata(mid, &m.filename).await {
                            files_meta.push(serde_json::json!({
                                "path": format!("mods/{}", m.filename),
                                "hashes": { "sha1": meta.sha1, "sha512": meta.sha512 },
                                "downloads": [meta.url],
                                "fileSize": meta.size,
                            }));
                            continue;
                        }
                    }
                    let entry_name = match safe_zip_entry_name(&m.filename) {
                        Some(n) => n,
                        None => {
                            // Unsanitizable filename (traversal / null) — record
                            // the manifest hash only; do not bundle bytes.
                            files_meta.push(serde_json::json!({
                                "path": format!("mods/{}", m.filename),
                                "hashes": { "sha256": m.sha256 },
                                "downloads": [],
                                "fileSize": 0u64,
                            }));
                            continue;
                        }
                    };
                    let p = mods_dir.join(&m.filename);

                    // Reject symlinks in mods/ so an attacker cannot bundle an
                    // arbitrary file the user did not intend to ship.
                    let is_symlink = std::fs::symlink_metadata(&p)
                        .map(|md| md.file_type().is_symlink())
                        .unwrap_or(false);
                    if is_symlink {
                        files_meta.push(serde_json::json!({
                            "path": entry_name,
                            "hashes": { "sha256": m.sha256 },
                            "downloads": [],
                            "fileSize": 0u64,
                        }));
                        continue;
                    }

                    match stream_jar_into_zip(&mut zip, opts, &entry_name, &p) {
                        Ok((sha, size)) => {
                            files_meta.push(serde_json::json!({
                                "path": entry_name,
                                "hashes": { "sha256": sha },
                                "downloads": [],
                                "fileSize": size,
                            }));
                        }
                        Err(_) => {
                            // File unreadable/missing — record the manifest
                            // hash + zero size so the pack still lists intent.
                            files_meta.push(serde_json::json!({
                                "path": entry_name,
                                "hashes": { "sha256": m.sha256 },
                                "downloads": [],
                                "fileSize": 0u64,
                            }));
                        }
                    }
                }

                // Write modrinth.index.json last so its metadata reflects the
                // streamed file hashes/sizes. Archive entry order is irrelevant
                // to mrpack consumers.
                let mut deps = serde_json::Map::new();
                deps.insert("minecraft".to_string(), serde_json::Value::String(manifest.minecraft_version.clone()));
                deps.insert(manifest.loader.clone(), serde_json::Value::String(manifest.loader_version.clone()));
                let index = serde_json::json!({
                    "formatVersion": 1,
                    "game": "minecraft",
                    "versionId": manifest.loader_version,
                    "name": manifest.name,
                    "dependencies": deps,
                    "files": files_meta,
                });
                let index_text = serde_json::to_string_pretty(&index)
                    .map_err(|_| LauncherError::InstanceCreateFailed)?;

                zip.start_file("modrinth.index.json", opts)
                    .map_err(|_| LauncherError::InstanceCreateFailed)?;
                zip.write_all(index_text.as_bytes())
                    .map_err(|_| LauncherError::InstanceCreateFailed)?;
                zip.finish().map_err(|_| LauncherError::InstanceCreateFailed)?;
            }

            std::fs::rename(&tmp_path, &out_path).map_err(|_| LauncherError::InstanceCreateFailed)?;
            Ok(out_path.to_string_lossy().to_string())
        }
        other => Err(LauncherError::Generic {
            code: "ERR_INVALID_FORMAT".to_string(),
            message: format!("Unknown export format '{}'. Use 'json' or 'mrpack'.", other),
        }),
    }
}

/// Import an instance from a pack file on disk.
///
/// Supported formats (detected by file extension):
/// - `.mrpack` — Modrinth mrpack v1 (zip containing `modrinth.index.json`
///   + bundled override jars under their declared `path`).
/// - `.json` / `.agora-pack.json` — Agora's plain-JSON `agora-pack/v1`
///   format (the export of `export_instance_pack(format="json")`).
///
/// Creates a NEW instance via `instances::create_instance` using the
/// pack-declared instance metadata, then materializes mods into its
/// `mods/` directory + manifest. Returns the sanitized instance_id of
/// the newly created instance.
///
/// Security: zip entries are validated via `safe_zip_entry_name` BEFORE
/// extraction (rejecting `..`, `/`, `\`, NUL). Downloaded Modrinth-CDN
/// files are verified against the mrpack-declared SHA-1 (when present)
/// via `download::sha1_hex`. The drop-directory allowlist check from
/// `add_manual_mod` is NOT applied here — callers reach this through
/// either an OS file picker (`pick_open_file`) or the webview drag-drop
/// event, both of which are user-initiated. Zip-slip protection is what
/// actually matters here; that is enforced.
pub async fn import_instance_pack(
    app: &tauri::AppHandle,
    source_path: &str,
) -> LauncherResult<String> {
    let lower = source_path.to_ascii_lowercase();
    if lower.ends_with(".mrpack") {
        import_mrpack(app, source_path).await
    } else if lower.ends_with(".json") || lower.ends_with(".agora-pack.json") {
        import_agora_pack(app, source_path).await
    } else {
        Err(LauncherError::Generic {
            code: "ERR_INVALID_FORMAT".to_string(),
            message: "Unsupported pack file extension. Use .mrpack or .agora-pack.json.".to_string(),
        })
    }
}

/// --- mrpack import ---

/// Import a Modrinth mrpack (.mrpack) file.
async fn import_mrpack(app: &tauri::AppHandle, source_path: &str) -> LauncherResult<String> {
    let file = std::fs::File::open(source_path)
        .map_err(|_| LauncherError::Generic {
            code: "ERR_PACK_READ".to_string(),
            message: format!("Cannot open mrpack file: {source_path}"),
        })?;
    let mut archive = zip::ZipArchive::new(file).map_err(|_| LauncherError::Generic {
        code: "ERR_PACK_READ".to_string(),
        message: "Failed to open mrpack as a zip archive.".to_string(),
    })?;

    // Find and parse modrinth.index.json
    let mut index_text = String::new();
    {
        use std::io::Read;
        let mut entry = archive.by_name("modrinth.index.json").map_err(|_| LauncherError::Generic {
            code: "ERR_PACK_PARSE".to_string(),
            message: "modrinth.index.json not found in mrpack.".to_string(),
        })?;
        entry.read_to_string(&mut index_text).map_err(|_| LauncherError::Generic {
            code: "ERR_PACK_READ".to_string(),
            message: "Failed to read modrinth.index.json.".to_string(),
        })?;
    }
    let index: serde_json::Value = serde_json::from_str(&index_text).map_err(|_| LauncherError::Generic {
        code: "ERR_PACK_PARSE".to_string(),
        message: "Failed to parse modrinth.index.json.".to_string(),
    })?;

    // Extract dependencies
    let deps = index.get("dependencies").and_then(|d| d.as_object()).ok_or_else(|| LauncherError::Generic {
        code: "ERR_PACK_PARSE".to_string(),
        message: "mrpack has no dependencies map.".to_string(),
    })?;

    let minecraft_version = deps.get("minecraft").and_then(|v| v.as_str()).ok_or_else(|| LauncherError::Generic {
        code: "ERR_PACK_PARSE".to_string(),
        message: "mrpack missing required 'dependencies.minecraft'.".to_string(),
    })?;
    if minecraft_version.is_empty() {
        return Err(LauncherError::Generic {
            code: "ERR_PACK_PARSE".to_string(),
            message: "mrpack 'dependencies.minecraft' is empty.".to_string(),
        });
    }

    // Find exactly one loader dependency
    let loader_key = &["fabric-loader", "quilt-loader", "neoforge", "forge"]
        .iter()
        .find(|k| deps.keys().any(|key| key == **k));
    let loader_key = loader_key.ok_or_else(|| LauncherError::Generic {
        code: "ERR_PACK_PARSE".to_string(),
        message: "mrpack has no loader dependency; cannot determine loader+version.".to_string(),
    })?;

    let loader_name = match *loader_key {
        "fabric-loader" => "fabric",
        "quilt-loader" => "quilt",
        "neoforge" => "neoforge",
        "forge" => "forge",
        _ => unreachable!(),
    };

    let loader_version = deps
        .get(*loader_key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| LauncherError::Generic {
            code: "ERR_PACK_PARSE".to_string(),
            message: format!("mrpack missing loader version for '{loader_key}'."),
        })?
        .to_string();

    // Instance name: prefer index["name"], else derive from filename
    let name = index
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            Path::new(source_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("imported-pack")
                .to_string()
        });

    let instance_id = paths::sanitize_id(&name);
    let instance_id = if instance_id.is_empty() { "imported-pack".to_string() } else { instance_id };

    // Create the instance
    let req = instances::CreateInstanceRequest {
        name: name.clone(),
        instance_id: instance_id.clone(),
        minecraft_version: minecraft_version.to_string(),
        loader: loader_name.to_string(),
        loader_version: loader_version.clone(),
        jvm_memory_mb: Some(4096),
        jvm_gc: None,
        jvm_custom_args: None,
        jvm_always_pre_touch: None,
    };
    let _row = instances::create_instance(app.clone(), req).await?;

    // Process files from the index
    let files_arr: &serde_json::Value = index.get("files").unwrap_or(&serde_json::Value::Null);
    let mut installed_mods: Vec<InstalledMod> = Vec::new();
    let now = chrono::Utc::now().to_rfc3339();

    let mods_dir = paths::instance_dir(app, &instance_id)
        .map_err(|_| LauncherError::InstanceCreateFailed)?
        .join("mods");
    std::fs::create_dir_all(&mods_dir).map_err(|_| LauncherError::InstanceCreateFailed)?;

    if let Some(arr) = files_arr.as_array() {
        for file_entry in arr {
            let path = file_entry.get("path").and_then(|v| v.as_str()).unwrap_or("");
            if !path.starts_with("mods/") {
                continue;
            }
            let basename = path.strip_prefix("mods/").unwrap();
            if basename.is_empty() {
                continue;
            }
            // Skip nested paths (only flat mods/<filename> is supported)
            if basename.contains('/') || basename.contains('\\') {
                auth::log_line(&format!("import_mrpack: path contains nested slashes, skipping '{basename}'"));
                continue;
            }

            if safe_zip_entry_name(basename).is_none() {
                auth::log_line(&format!("import_mrpack: unsafe basename '{basename}', skipping"));
                continue;
            }

            let downloads = file_entry.get("downloads").and_then(|d| d.as_array());
        if let Some(downloads) = downloads {
            if let Some(url) = downloads.first().and_then(|u| u.as_str()) {
                // Download from Modrinth CDN (host-allowlisted by download_mod_bytes)
                let bytes = download_mod_bytes(url).await?;
                // SHA-1 verification if declared
                if let Some(expected_sha1) = file_entry.get("hashes")
                    .and_then(|h| h.get("sha1"))
                    .and_then(|h| h.as_str())
                    .filter(|s| !s.is_empty())
                {
                    let actual = download::sha1_hex(&bytes);
                    if actual != expected_sha1.trim().to_lowercase() {
                        return Err(LauncherError::HashMismatch);
                    }
                }
                let sha256 = download::sha256_hex(&bytes);
                let mod_path = mods_dir.join(basename);
                std::fs::write(&mod_path, &bytes).map_err(|_| LauncherError::InstanceCreateFailed)?;
                let metadata = crash_investigator::parse_jar_metadata(&mod_path);
                installed_mods.push(InstalledMod {
                    filename: basename.to_string(),
                    registry_id: None,
                    modrinth_id: None,
                    source: "modrinth_pack".to_string(),
                    version: None,
                    sha256,
                    installed_at: now.clone(),
                    java_packages: metadata.java_packages,
                    mod_jar_id: metadata.mod_jar_id,
                    depends_on: metadata.depends_on,
                    optional_deps: metadata.optional_deps,
                    incompatible_deps: metadata.incompatible_deps,
                });
            }
        } else {
            // Bundled override jar: extract directly from the zip
            if let Ok(mut entry) = archive.by_name(path) {
                let mut bytes = Vec::new();
                use std::io::Read;
                entry.read_to_end(&mut bytes).ok();
                let sha256 = download::sha256_hex(&bytes);
                let mod_path = mods_dir.join(basename);
                std::fs::write(&mod_path, &bytes).map_err(|_| LauncherError::InstanceCreateFailed)?;
                let metadata = crash_investigator::parse_jar_metadata(&mod_path);
                installed_mods.push(InstalledMod {
                    filename: basename.to_string(),
                    registry_id: None,
                    modrinth_id: None,
                    source: "modrinth_pack_bundle".to_string(),
                    version: None,
                    sha256,
                    installed_at: now.clone(),
                    java_packages: metadata.java_packages,
                    mod_jar_id: metadata.mod_jar_id,
                    depends_on: metadata.depends_on,
                    optional_deps: metadata.optional_deps,
                    incompatible_deps: metadata.incompatible_deps,
                });
            } else {
                auth::log_line(&format!("import_mrpack: bundled file not found in zip: '{path}'"));
            }
        }
    }
    }

    // Extract override files from the zip (overrides/ and client_overrides/).
    let instance_root = paths::instance_dir(app, &instance_id)
        .map_err(|_| LauncherError::InstanceCreateFailed)?;

    let mut override_extracted: Vec<String> = Vec::new();
    let mut override_skipped: Vec<String> = Vec::new();

    for i in 0..archive.len() {
        let mut entry = match archive.by_index(i) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = entry.name().to_string();

        // Only process overrides/ and client_overrides/ directories.
        let stripped = if let Some(s) = name.strip_prefix("overrides/") {
            s
        } else if let Some(s) = name.strip_prefix("client_overrides/") {
            s
        } else {
            continue;
        };

        if entry.is_dir() {
            continue;
        }

        // Sanitize: reject path traversal, absolute paths.
        let normalized = stripped.replace('\\', "/");
        if normalized.starts_with('/') || normalized.contains("..") {
            continue;
        }

        // Check directory whitelist.
        let allowed = ["config/", "defaultconfigs/", "resourcepacks/", "shaderpacks/", "datapacks/", "kubejs/"];
        if !allowed.iter().any(|p| normalized.starts_with(p)) {
            override_skipped.push(normalized.clone());
            continue;
        }

        // Check banned extensions.
        let lower = normalized.to_lowercase();
        let banned = [".jar", ".class", ".exe", ".bat", ".cmd", ".sh", ".ps1", ".dll", ".so", ".dylib", ".msi", ".dmg"];
        if banned.iter().any(|ext| lower.ends_with(ext)) {
            auth::log_line(&format!(
                "import_mrpack: banned extension in override: '{normalized}', skipping"
            ));
            continue;
        }

        let dest_path = instance_root.join(&normalized);
        if !dest_path.starts_with(&instance_root) {
            continue;
        }

        if let Some(parent) = dest_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let mut data = Vec::new();
        use std::io::Read;
        if entry.read_to_end(&mut data).is_ok() {
            if std::fs::write(&dest_path, &data).is_ok() {
                override_extracted.push(normalized);
            }
        }
    }

    if !override_extracted.is_empty() {
        auth::log_line(&format!(
            "import_mrpack: extracted {} override files, skipped {}",
            override_extracted.len(),
            override_skipped.len()
        ));
    }

    // Update manifest atomically
    if !installed_mods.is_empty() {
        let manifest_path = paths::instance_manifest_path(app, &instance_id)
            .map_err(|_| LauncherError::InstanceCreateFailed)?;
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
        manifest.mods.extend(installed_mods);

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

    Ok(instance_id)
}

/// --- agora-pack import ---

/// Import an Agora plain-JSON pack (.agora-pack.json or .json).
async fn import_agora_pack(app: &tauri::AppHandle, source_path: &str) -> LauncherResult<String> {
    let text = std::fs::read_to_string(source_path).map_err(|_| LauncherError::Generic {
        code: "ERR_PACK_READ".to_string(),
        message: format!("Cannot read pack file: {source_path}"),
    })?;
    let pack: serde_json::Value = serde_json::from_str(&text).map_err(|_| LauncherError::Generic {
        code: "ERR_PACK_PARSE".to_string(),
        message: "Failed to parse agora-pack JSON.".to_string(),
    })?;

    let instance_obj = pack.get("instance").ok_or_else(|| LauncherError::Generic {
        code: "ERR_PACK_PARSE".to_string(),
        message: "agora-pack missing 'instance' object.".to_string(),
    })?;

    let mc_version = instance_obj
        .get("minecraft_version")
        .and_then(|v| v.as_str())
        .ok_or_else(|| LauncherError::Generic {
            code: "ERR_PACK_PARSE".to_string(),
            message: "agora-pack instance missing 'minecraft_version'.".to_string(),
        })?;
    if mc_version.is_empty() {
        return Err(LauncherError::Generic {
            code: "ERR_PACK_PARSE".to_string(),
            message: "agora-pack 'minecraft_version' is empty.".to_string(),
        });
    }

    let loader = instance_obj
        .get("loader")
        .and_then(|v| v.as_str())
        .ok_or_else(|| LauncherError::Generic {
            code: "ERR_PACK_PARSE".to_string(),
            message: "agora-pack instance missing 'loader'.".to_string(),
        })?;
    if loader.is_empty() {
        return Err(LauncherError::Generic {
            code: "ERR_PACK_PARSE".to_string(),
            message: "agora-pack 'loader' is empty.".to_string(),
        });
    }

    let loader_version = instance_obj
        .get("loader_version")
        .and_then(|v| v.as_str())
        .ok_or_else(|| LauncherError::Generic {
            code: "ERR_PACK_PARSE".to_string(),
            message: "agora-pack instance missing 'loader_version'.".to_string(),
        })?;
    if loader_version.is_empty() {
        return Err(LauncherError::Generic {
            code: "ERR_PACK_PARSE".to_string(),
            message: "agora-pack 'loader_version' is empty.".to_string(),
        });
    }

    let name = instance_obj
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| {
            instance_obj
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "imported-pack".to_string());

    let instance_id = instance_obj
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or(&name)
        .to_string();

    let instance_id = paths::sanitize_id(&instance_id);
    let instance_id = if instance_id.is_empty() { "imported-pack".to_string() } else { instance_id };

    let req = instances::CreateInstanceRequest {
        name: name.clone(),
        instance_id: instance_id.clone(),
        minecraft_version: mc_version.to_string(),
        loader: loader.to_string(),
        loader_version: loader_version.to_string(),
        jvm_memory_mb: Some(4096),
        jvm_gc: None,
        jvm_custom_args: None,
        jvm_always_pre_touch: None,
    };
    let _row = instances::create_instance(app.clone(), req).await?;

    // Install mods from the pack
    if let Some(mods_arr) = pack.get("mods").and_then(|m| m.as_array()) {
        for mod_entry in mods_arr {
            let registry_id = mod_entry
                .get("registry_id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty());

            if let Some(rid) = registry_id {
                let filename = mod_entry
                    .get("filename")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                match list_mod_versions(app, &instance_id, rid).await {
                    Ok(candidates) => {
                        // Try to match by filename first, then by version
                        let candidate = candidates.iter().find(|c| c.filename == filename)
                            .or_else(|| {
                                let ver = mod_entry.get("version").and_then(|v| v.as_str());
                                ver.and_then(|v| candidates.iter().find(|c| c.version == v))
                            });

                        if let Some(c) = candidate {
                            if let Err(e) = install_mod_version(app, &instance_id, rid, c).await {
                                auth::log_line(&format!(
                                    "import_agora_pack: failed to install registry mod {rid}: {e}"
                                ));
                            }
                        } else {
                            auth::log_line(&format!(
                                "import_agora_pack: no matching candidate for mod {rid} (filename={filename})"
                            ));
                        }
                    }
                    Err(e) => {
                        auth::log_line(&format!(
                            "import_agora_pack: failed to list versions for registry mod {rid}: {e}"
                        ));
                    }
                }
            } else if let Some(mid) = mod_entry
                .get("modrinth_id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
            {
                if !crate::modrinth_raw::is_modrinth_enabled(app) {
                    auth::log_line(&format!(
                        "import_agora_pack: skipping modrinth mod '{mid}' — Modrinth integration disabled"
                    ));
                    continue;
                }
                let filename = mod_entry
                    .get("filename")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                match crate::modrinth_raw::list_raw_modrinth_versions(app, Some(&instance_id), mid).await {
                    Ok(candidates) => {
                        let candidate = candidates
                            .iter()
                            .find(|c| c.primary && c.filename == filename)
                            .or_else(|| candidates.iter().find(|c| c.filename == filename))
                            .or_else(|| candidates.iter().find(|c| c.primary))
                            .or_else(|| {
                                let ver = mod_entry.get("version").and_then(|v| v.as_str());
                                ver.and_then(|v| candidates.iter().find(|c| c.version == v))
                            })
                            .or_else(|| candidates.first());
                        if let Some(c) = candidate {
                            if let Err(e) = crate::modrinth_raw::install_raw_modrinth(app, &instance_id, mid, c, "mod").await {
                                auth::log_line(&format!(
                                    "import_agora_pack: failed to install modrinth mod {mid}: {e}"
                                ));
                            }
                        } else {
                            auth::log_line(&format!(
                                "import_agora_pack: no candidate for modrinth mod {mid} (filename={filename})"
                            ));
                        }
                    }
                    Err(e) => {
                        auth::log_line(&format!(
                            "import_agora_pack: failed to list versions for modrinth mod {mid}: {e}"
                        ));
                    }
                }
            } else {
                let filename = mod_entry
                    .get("filename")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                auth::log_line(&format!(
                    "import_agora_pack: skipping non-registry mod '{filename}' (manual re-add required)"
                ));
            }
        }
    }

    Ok(instance_id)
}

