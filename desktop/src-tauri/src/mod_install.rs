use crate::auth;
use crate::db;
use crate::download;
use crate::error::{LauncherError, LauncherResult};
use crate::instances;
use crate::models::{InstalledMod, InstanceManifest, InstanceRow, ModVersionCandidate};
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

/// macOS/Linux: use the `sysinfo` crate to find the disk whose mount point
/// contains `path` and return its available space. Returns None on any failure
/// (callers proceed without the check -- do not block on unavailable info).
#[cfg(not(target_os = "windows"))]
pub(crate) fn available_disk_space_bytes(path: &std::path::Path) -> Option<u64> {
    use sysinfo::Disks;
    let target_canonical = path.canonicalize().ok()?;
    let target_components: Vec<_> = target_canonical.components().collect();
    let disks = Disks::new_with_refreshed_list();
    let mut best: Option<u64> = None;
    let mut best_prefix_len = 0usize;
    for d in disks.list() {
        let mount = d.mount_point();
        let mount_components: Vec<_> = mount.components().collect();
        if mount_components.len() > target_components.len() {
            continue;
        }
        if target_components.starts_with(&mount_components)
            && mount_components.len() > best_prefix_len
        {
            best_prefix_len = mount_components.len();
            best = Some(d.available_space());
        }
    }
    best
}

/// Hosts allowed for mod downloads (GitHub + Modrinth).
/// Separate from the loader-manifest allowlist to enforce the whitelist principle.
const MOD_DOWNLOAD_ALLOWLIST: &[&str] = &[
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
    "api.github.com",
    "cdn.modrinth.com",
    "api.modrinth.com",
];

/// Check whether a URL host is on the mod-download allowlist.
fn is_mod_download_host(host: &str) -> bool {
    MOD_DOWNLOAD_ALLOWLIST.contains(&host)
}

/// Parse and validate a mod-download URL before opening a connection.
///
/// Redirect validation alone is insufficient: without this gate, a caller can
/// make the first request to an arbitrary local or private endpoint and only
/// have *subsequent* redirect targets checked. Keep the scheme, host, and port
/// constrained to the same HTTPS sources used for normal mod downloads.
fn validate_mod_download_url(raw_url: &str) -> LauncherResult<reqwest::Url> {
    let url = reqwest::Url::parse(raw_url).map_err(|_| LauncherError::UntrustedSource)?;
    let host = url.host_str().ok_or(LauncherError::UntrustedSource)?;

    if url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !is_mod_download_host(host)
    {
        return Err(LauncherError::UntrustedSource);
    }

    Ok(url)
}

/// Download bytes from a mod-download URL with redirect-safe policy.
///
/// Redirects are only followed when the target host is on the mod-download
/// allowlist, preventing SSRF via compromised/malicious URLs.
pub(crate) async fn download_mod_bytes(url: &str) -> LauncherResult<Vec<u8>> {
    let url = validate_mod_download_url(url)?;
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
        .get(url.clone())
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
pub fn load_instance_info(
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
pub fn load_registry_item(
    app: &tauri::AppHandle,
    item_id: &str,
) -> LauncherResult<registry::RegistryItem> {
    let conn = registry::open_registry(app)?;
    registry::get_item_by_id(&conn, item_id)
        .map_err(|_| LauncherError::RegistryMissing)?
        .ok_or_else(|| LauncherError::Generic {
            code: "ERR_ITEM_NOT_FOUND".to_string(),
            message: format!("Registry item '{item_id}' not found."),
        })
}

/// Heuristic: check whether a filename contains both the Minecraft version
/// Known Minecraft mod loaders (lowercase) used for heuristic matching.
const KNOWN_LOADERS: &[&str] = &["fabric", "forge", "neoforge", "quilt"];

/// Extract a Minecraft version hint from a string.
///
/// Matches patterns like `1.21.11`, `mc1.21.11`, `mc26.2`, `26.2` etc.
/// `mc`-prefixed versions are always preferred.  Returns the normalized
/// version (without `mc` prefix) if found.
fn extract_mc_version(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let bytes_lower = lower.as_bytes();
    let bytes_orig = text.as_bytes();

    // Pass 1 â€” scan for `mc` prefix with word-boundary check.
    // `mc` must be preceded by a non-alphanumeric char (or start of string)
    // and followed by a digit.
    let mut pos = 0;
    while pos < bytes_lower.len() {
        if pos + 1 < bytes_lower.len() && bytes_lower[pos] == b'm' && bytes_lower[pos + 1] == b'c' {
            let before_ok = pos == 0 || !bytes_lower[pos - 1].is_ascii_alphanumeric();
            let after_pos = pos + 2;
            if before_ok && after_pos < bytes_lower.len() && bytes_lower[after_pos].is_ascii_digit()
            {
                // Take the version portion (until a non-version char)
                let rest = &text[after_pos..];
                let end = rest
                    .find(|c: char| !c.is_ascii_digit() && c != '.')
                    .unwrap_or(rest.len());
                let ver = &rest[..end];
                let ver = ver.strip_suffix('.').unwrap_or(ver);
                if !ver.is_empty() {
                    return Some(ver.to_string());
                }
            }
            pos += 2;
        } else {
            pos += 1;
        }
    }

    // Pass 2 â€” bare version: look for `X.Y.Z` or `X.Y` pattern preceded by a
    // non-alphanumeric char (or start of string).
    let mut i = 0;
    while i < bytes_lower.len() {
        if bytes_lower[i].is_ascii_digit() {
            // Check that the char before is not alphanumeric (word boundary)
            if i > 0 && bytes_lower[i - 1].is_ascii_alphanumeric() {
                i += 1;
                continue;
            }
            // Find the end of this version segment
            let mut end = i + 1;
            while end < bytes_lower.len()
                && (bytes_lower[end].is_ascii_digit() || bytes_lower[end] == b'.')
            {
                end += 1;
            }
            // Strip trailing dot (e.g. "1.21.11." â†’ "1.21.11")
            let mut ver_end = end;
            while ver_end > i + 1 && bytes_lower[ver_end - 1] == b'.' {
                ver_end -= 1;
            }
            // Must contain at least one dot (e.g. 1.21 or 26.2)
            let candidate = &lower[i..ver_end];
            if candidate.contains('.') {
                // Avoid matching things that look like semver library versions
                // (e.g. 0.154.0 from fabric-api-0.154.0+26.2) by requiring
                // the first segment to be <=25 or the whole thing to start
                // with `1.` (typical MC versions).
                if let Some(major_str) = candidate.split('.').next() {
                    if let Ok(major) = major_str.parse::<u32>() {
                        if major == 1 || major > 25 {
                            return Some(candidate.to_string());
                        }
                    }
                }
            }
            // Skip past this version segment to avoid re-matching parts of it
            i = end;
        } else {
            i += 1;
        }
    }

    None
}

/// Extract a loader hint from a string.
///
/// Returns the lowercase loader name if found.
fn extract_loader(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    for loader in KNOWN_LOADERS {
        // Word-boundary check: the loader must not be part of a larger word.
        // Simple heuristic: char before and after must be non-alphanumeric.
        let mut idx = 0;
        while let Some(pos) = lower[idx..].find(loader) {
            let abs = idx + pos;
            let before_ok = abs == 0 || !lower.as_bytes()[abs - 1].is_ascii_alphanumeric();
            let after_pos = abs + loader.len();
            let after_ok =
                after_pos >= lower.len() || !lower.as_bytes()[after_pos].is_ascii_alphanumeric();
            if before_ok && after_ok {
                return Some(loader.to_string());
            }
            idx = abs + 1;
        }
    }
    None
}

/// Determine MC version and loader compatibility for a GitHub release asset.
///
/// Checks both the asset filename and the release tag name for version and
/// loader hints. Returns `(mc_version, loader, compat)` where each is `Some`
/// if a match was found, and `compat` indicates the compatibility tier.
fn parse_version_from_github_asset(
    filename: &str,
    tag_name: &str,
    mc_version: &str,
    loader: &str,
) -> (Option<String>, Option<String>, &'static str) {
    // Check filename first, then tag name as fallback
    let mc = extract_mc_version(filename).or_else(|| extract_mc_version(tag_name));
    let lo = extract_loader(filename).or_else(|| extract_loader(tag_name));

    // Validate MC version against the requested instance
    let mc_match = mc.as_deref().map(|v| {
        let target = mc_version.to_lowercase();
        // Exact match (after stripping leading "1.")
        let stripped_target = target.strip_prefix("1.").unwrap_or(&target);
        let stripped_found = v.strip_prefix("1.").unwrap_or(v);
        stripped_found == stripped_target
    });

    let lo_match = lo.as_deref().map(|l| l.eq_ignore_ascii_case(loader));

    let loader_ok = lo_match == Some(true);

    // Loader explicitly detected but doesn't match the instance
    let loader_mismatch = lo.is_some() && !loader_ok;

    // Helper: does the found MC version share the same major as the target?
    let major_matches = mc.as_deref().map_or(false, |found| {
        let target = mc_version.to_lowercase();
        let stripped_target = target.strip_prefix("1.").unwrap_or(&target);
        let stripped_found = found.strip_prefix("1.").unwrap_or(found);
        let target_major = stripped_target.split('.').next().unwrap_or("");
        let found_major = stripped_found.split('.').next().unwrap_or("");
        if target_major.is_empty() || found_major.is_empty() {
            return false;
        }
        target_major == found_major
            && (stripped_found.starts_with(stripped_target)
                || stripped_target.starts_with(stripped_found))
    });

    // Determine compatibility tier
    let compat = if mc_match == Some(true) && !loader_mismatch {
        "compatible"
    } else if major_matches && !loader_mismatch {
        "major_match"
    } else {
        ""
    };

    // Always return detected values so the caller can display them.
    let matched_mc = if mc_match == Some(true) {
        Some(mc_version.to_string())
    } else {
        mc
    };
    let matched_lo = lo;

    (matched_mc, matched_lo, compat)
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
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    digest: Option<String>,
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
    sha256: Option<String>,
    sha512: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModrinthVersionFile {
    url: String,
    filename: String,
    primary: bool,
    hashes: Option<ModrinthFileHashes>,
    #[serde(default)]
    size: Option<u64>,
}

/// List available mod versions for a registry item, resolving live data from
/// the upstream source (GitHub Releases or Modrinth).
///
/// Auto-fallback (Â§6.3 resilience): for `github_release` mods that also carry
/// a `modrinth_id`, if the primary GitHub resolver fails (network error,
/// rate-limit, or returns no candidates), the resolver transparently retries
/// against Modrinth. The installed file is still SHA-256-verified against the
/// pinned registry hash in `install_mod_version`, so a different build from
/// the alternate source is rejected rather than silently installed.
///
/// Modrinth toggle (Â§6.3): the `modrinth_id` strategy path and the
/// `github_release`â†’Modrinth fallback are both gated by the `modrinth_enabled`
/// setting. When the user has disabled Modrinth integration, NO Modrinth API
/// calls are made â€” GitHub remains the sole source for `github_release` mods,
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
            let primary = resolve_github_releases_all(app, &item, mc_version, loader).await;
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
                        if modrinth_on {
                            "trying Modrinth fallback"
                        } else {
                            "Modrinth disabled, no fallback"
                        }
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

/// Quick compatibility probe â€” fetches only the first page of releases,
/// runs the same per-asset parser as `list_mod_versions`, and returns the
/// best compat tier (`"compatible"`, `"major_match"`, or `""` for no match).
/// Intended for the browse page so each mod card can show a compatibility
/// badge without pulling the full version list.  Only the first page is
/// checked because GitHub returns newest releases first, so the most
/// relevant MC-version matches are on page 1.
///
/// A page-fetch failure (rate limit, timeout) is tolerated: we log it and
/// return `""` so the browse page badge stays neutral instead of erroring
/// out the entire card.
pub async fn check_mod_compat(
    app: &tauri::AppHandle,
    instance_id: &str,
    item_id: &str,
) -> LauncherResult<String> {
    let instance = load_instance_info(app, instance_id)?;
    let item = load_registry_item(app, item_id)?;
    let mc_version = &instance.minecraft_version;
    let loader = &instance.loader;

    let candidates = match item.download_strategy.as_str() {
        "github_release" => {
            match resolve_github_releases_page(app, &item, mc_version, loader, 1).await {
                Ok((versions, _)) => versions,
                Err(e) => {
                    crate::auth::log_line(&format!(
                        "check_mod_compat: github page 1 failed for '{}' ({}); returning no badge",
                        item_id, e,
                    ));
                    Vec::new()
                }
            }
        }
        "modrinth_id" => {
            if !crate::modrinth_raw::is_modrinth_enabled(app) {
                return Ok(String::new());
            }
            resolve_modrinth_versions(&item, mc_version, loader).await?
        }
        _ => Vec::new(),
    };

    Ok(candidates
        .iter()
        .map(|c| c.version_compat.as_str())
        .find(|c| !c.is_empty())
        .unwrap_or("")
        .to_string())
}

/// Fetch ALL GitHub release pages for a registry item, parse each asset, and
/// return the complete sorted version list.  Pages are fetched sequentially
/// until the API returns an empty list.  A 30-second per-request timeout and
/// a 50-page safety limit prevent runaway requests.
///
/// Individual page failures (rate limit, timeout, network) are tolerated:
/// the error is logged and we continue with whatever data was already
/// collected, rather than failing the entire operation.
async fn resolve_github_releases_all(
    app: &tauri::AppHandle,
    item: &registry::RegistryItem,
    mc_version: &str,
    loader: &str,
) -> LauncherResult<Vec<ModVersionCandidate>> {
    let mut all_candidates: Vec<ModVersionCandidate> = Vec::new();
    let mut page: u32 = 1;
    let max_pages = 50;
    let mut errored = false;

    loop {
        if page > max_pages || errored {
            if page > max_pages {
                crate::auth::log_line(&format!(
                    "resolve_github_releases_all: hit {max_pages}-page safety limit for '{}'",
                    item.source_identifier,
                ));
            }
            break;
        }

        match resolve_github_releases_page(app, item, mc_version, loader, page).await {
            Ok((candidates, _total_pages)) => {
                let has_more = !candidates.is_empty() || page < _total_pages;
                all_candidates.extend(candidates);
                if !has_more {
                    break;
                }
            }
            Err(e) => {
                crate::auth::log_line(&format!(
                    "resolve_github_releases_all: page {page} failed for '{}' ({}); stopping pagination but returning {} candidates already collected",
                    item.source_identifier,
                    e,
                    all_candidates.len(),
                ));
                errored = true;
            }
        }

        page += 1;
    }

    sort_versions_by_compatibility(&mut all_candidates);
    Ok(all_candidates)
}

/// Bi-directional initial fetch: grab page 1 (newest) and, when no compatible
/// versions are found, also grab the last few pages (oldest) so that users on
/// older Minecraft versions see their matching versions at the top without
/// having to scroll through hundreds of newer releases.
///
/// Returns `(all_candidates_sorted, total_pages, pages_fetched)`.
pub async fn resolve_github_releases_initial(
    app: &tauri::AppHandle,
    item: &registry::RegistryItem,
    mc_version: &str,
    loader: &str,
) -> LauncherResult<(Vec<ModVersionCandidate>, u32, Vec<u32>)> {
    let (page1, total_pages) =
        resolve_github_releases_page(app, item, mc_version, loader, 1).await?;
    let mut all = page1;
    let mut pages_fetched = vec![1u32];

    if total_pages <= 1 || has_compatible(&all) {
        sort_versions_by_compatibility(&mut all);
        return Ok((all, total_pages, pages_fetched));
    }

    // No compatibles on page 1 â€” fetch the last few pages concurrently
    // (they contain the oldest releases which are most likely to match
    // the user's older MC version).
    let mut tail_pages: Vec<u32> = (2..=total_pages).rev().collect();
    // Only grab up to 3 tail pages so we don't overwhelm the rate limit.
    tail_pages.truncate(3);

    if !tail_pages.is_empty() {
        let mc_owned = mc_version.to_owned();
        let ld_owned = loader.to_owned();
        let source = item.source_identifier.clone();
        let app_clone = app.clone();
        let mut handles = Vec::new();

        for &p in &tail_pages {
            let app = app_clone.clone();
            let src = source.clone();
            let mc = mc_owned.clone();
            let ld = ld_owned.clone();
            handles.push(tokio::spawn(async move {
                fetch_github_releases_page(&app, &src, &mc, &ld, p).await
            }));
        }

        for (i, handle) in handles.into_iter().enumerate() {
            match handle.await {
                Ok(Ok((cands, _))) => {
                    if let Some(&p) = tail_pages.get(i) {
                        pages_fetched.push(p);
                    }
                    all.extend(cands);
                }
                Ok(Err(e)) => {
                    crate::auth::log_line(&format!(
                        "resolve_github_releases_initial: tail page {} failed: {e}",
                        tail_pages[i],
                    ));
                }
                Err(e) => {
                    crate::auth::log_line(&format!(
                        "resolve_github_releases_initial: task join failed: {e}",
                    ));
                }
            }
        }
    }

    sort_versions_by_compatibility(&mut all);
    Ok((all, total_pages, pages_fetched))
}

/// Fetch a batch of specific GitHub release pages (for lazy load).  Returns
/// a map from page number to the candidates found on that page.
pub async fn fetch_github_versions_batch(
    app: &tauri::AppHandle,
    source: &str,
    mc_version: &str,
    loader: &str,
    pages: &[u32],
) -> LauncherResult<Vec<(u32, Vec<ModVersionCandidate>)>> {
    let mc_owned = mc_version.to_owned();
    let ld_owned = loader.to_owned();
    let src_owned = source.to_owned();
    let app_clone = app.clone();
    let mut handles = Vec::new();

    for &p in pages {
        let app = app_clone.clone();
        let src = src_owned.clone();
        let mc = mc_owned.clone();
        let ld = ld_owned.clone();
        handles.push(tokio::spawn(async move {
            let result = fetch_github_releases_page(&app, &src, &mc, &ld, p).await;
            (p, result)
        }));
    }

    let mut results = Vec::new();
    for handle in handles {
        match handle.await {
            Ok((page_num, Ok((cands, _)))) => {
                results.push((page_num, cands));
            }
            Ok((page_num, Err(e))) => {
                crate::auth::log_line(&format!(
                    "fetch_github_versions_batch: page {page_num} failed: {e}",
                ));
            }
            Err(e) => {
                crate::auth::log_line(&format!(
                    "fetch_github_versions_batch: task join failed: {e}",
                ));
            }
        }
    }

    Ok(results)
}

/// Parse the GitHub API `Link` response header to discover the total number
fn parse_link_total_pages(header_value: Option<&str>) -> u32 {
    let value = match header_value {
        Some(v) => v,
        None => return 1,
    };
    for part in value.split(',') {
        let trimmed = part.trim();
        if trimmed.contains("rel=\"last\"") {
            if let Some(close) = trimmed.rfind('>') {
                let substr = &trimmed[..close];
                if let Some(open) = substr.rfind('<') {
                    let url = &substr[open + 1..];
                    for segment in url.split(&['?', '&'][..]) {
                        if let Some(num) = segment.strip_prefix("page=") {
                            return num.parse::<u32>().unwrap_or(1);
                        }
                    }
                }
            }
        }
    }
    1
}

/// Low-level page fetcher â€” takes owned copies so it can be spawned.
/// Uses the shared GitHub client, global concurrency semaphore, and
/// cooldown-aware rate limiting.
async fn fetch_github_releases_page(
    app: &tauri::AppHandle,
    source: &str,
    mc_version: &str,
    loader: &str,
    page: u32,
) -> LauncherResult<(Vec<ModVersionCandidate>, u32)> {
    let url = format!("https://api.github.com/repos/{source}/releases?per_page=100&page={page}");

    let _permit = agora_core::github_ratelimit::acquire_github_permit().await;

    let mut req = agora_core::github_ratelimit::github_client().get(&url);

    if let Some(token) = github_auth_header(app) {
        req = req.header("Authorization", token);
    }

    let response = req
        .send()
        .await
        .map_err(|_| LauncherError::NetworkOffline)?;

    // Check for rate limit BEFORE consuming the body.
    if agora_core::github_ratelimit::is_rate_limit_response(&response) {
        let retry = agora_core::github_ratelimit::parse_retry_after(&response);
        agora_core::github_ratelimit::report_rate_limit(retry).await;
        return Err(LauncherError::Generic {
            code: "ERR_RATE_LIMITED".to_string(),
            message: format!("GitHub rate limit hit while fetching releases for {source}."),
        });
    }

    let link_value = response
        .headers()
        .get("link")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let releases: Vec<GitHubRelease> = response
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

    let total_pages = parse_link_total_pages(link_value.as_deref());
    let mut candidates: Vec<ModVersionCandidate> = Vec::new();

    for release in &releases {
        for asset in &release.assets {
            if !asset.name.ends_with(".jar") {
                continue;
            }
            let (mc, loader_str, compat) =
                parse_version_from_github_asset(&asset.name, &release.tag_name, mc_version, loader);

            let download_url = format!(
                "https://github.com/{}/releases/download/{}/{}",
                source,
                urlencoding::encode(&release.tag_name),
                asset.name,
            );

            candidates.push(ModVersionCandidate {
                version: release.tag_name.clone(),
                filename: asset.name.clone(),
                download_url,
                mc_version: mc,
                loader: loader_str,
                release_date: release.published_at.clone(),
                is_compatible: compat == "compatible",
                version_compat: compat.to_string(),
                sha1: None,
                sha256: asset
                    .digest
                    .as_deref()
                    .and_then(|digest| digest.strip_prefix("sha256:"))
                    .map(str::to_string),
                sha512: None,
                size: asset.size,
            });
        }
    }

    Ok((candidates, total_pages))
}

/// Convenience wrapper that calls `fetch_github_releases_page` using
/// `RegistryItem::source_identifier` as the repository source.
async fn resolve_github_releases_page(
    app: &tauri::AppHandle,
    item: &registry::RegistryItem,
    mc_version: &str,
    loader: &str,
    page: u32,
) -> LauncherResult<(Vec<ModVersionCandidate>, u32)> {
    fetch_github_releases_page(app, &item.source_identifier, mc_version, loader, page).await
}

/// Sort version candidates by compatibility tier (compatible â†’ major_match â†’
/// other), then by release date descending within each tier.
pub fn sort_versions_by_compatibility(versions: &mut Vec<ModVersionCandidate>) {
    versions.sort_by(|a, b| {
        let tier = |c: &ModVersionCandidate| -> u8 {
            match c.version_compat.as_str() {
                "compatible" => 0,
                "major_match" => 1,
                _ => 2,
            }
        };
        let tier_a = tier(a);
        let tier_b = tier(b);
        tier_a.cmp(&tier_b).then_with(|| {
            b.release_date
                .as_deref()
                .unwrap_or("")
                .cmp(a.release_date.as_deref().unwrap_or(""))
        })
    });
}

fn has_compatible(candidates: &[ModVersionCandidate]) -> bool {
    candidates.iter().any(|c| c.version_compat == "compatible")
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

        let (mc, loader_str, compat) = parse_version_from_github_asset(
            &file.filename,
            &version.version_number,
            mc_version,
            loader,
        );

        candidates.push(ModVersionCandidate {
            version: version.version_number.clone(),
            filename: file.filename.clone(),
            download_url: file.url.clone(),
            mc_version: mc,
            loader: loader_str,
            release_date: None,
            is_compatible: compat == "compatible",
            version_compat: compat.to_string(),
            sha1: file.hashes.as_ref().and_then(|h| h.sha1.clone()),
            sha256: file.hashes.as_ref().and_then(|h| h.sha256.clone()),
            sha512: file.hashes.as_ref().and_then(|h| h.sha512.clone()),
            size: file.size,
        });
    }

    Ok(candidates)
}

/// Read the instance manifest and return `Err(InstanceLocked)` if `is_locked` is true.
pub(crate) fn check_not_locked(app: &tauri::AppHandle, instance_id: &str) -> LauncherResult<()> {
    let manifest_path = paths::instance_manifest_path(app, instance_id)
        .map_err(|_| LauncherError::InstanceCreateFailed)?;
    if !manifest_path.exists() {
        return Ok(());
    }
    let text =
        std::fs::read_to_string(&manifest_path).map_err(|_| LauncherError::InstanceCreateFailed)?;
    let manifest: InstanceManifest =
        serde_json::from_str(&text).map_err(|_| LauncherError::InstanceCreateFailed)?;
    if manifest.is_locked {
        return Err(LauncherError::InstanceLocked);
    }
    Ok(())
}

/// Map a content_type string to the instance subdirectory name.
pub(crate) fn content_subdir(content_type: &str) -> &str {
    match content_type {
        "resourcepack" => "resourcepacks",
        "shader" => "shaderpacks",
        "datapack" => "datapacks",
        "world" => "saves",
        _ => "mods", // includes "mod" and unknown types
    }
}

/// Push an installed item to the correct array in the manifest.
pub(crate) fn push_to_content_array(manifest: &mut InstanceManifest, item: &InstalledMod) {
    match item.content_type.as_str() {
        "resourcepack" => manifest.resourcepacks.push(item.clone()),
        "shader" => manifest.shaders.push(item.clone()),
        "datapack" => manifest.datapacks.push(item.clone()),
        "world" => manifest.worlds.push(item.clone()),
        _ => manifest.mods.push(item.clone()),
    }
}

/// Remove an entry with the given filename from whichever manifest array it
/// resides in.  Returns `true` if found and removed.
fn remove_from_content_array(manifest: &mut InstanceManifest, filename: &str) -> bool {
    for arr in [
        &mut manifest.mods,
        &mut manifest.resourcepacks,
        &mut manifest.shaders,
        &mut manifest.datapacks,
        &mut manifest.worlds,
    ] {
        let before = arr.len();
        arr.retain(|m| m.filename != filename);
        if arr.len() < before {
            return true;
        }
    }
    false
}

/// Install a specific mod version into an instance's `mods/` directory.
pub async fn install_mod_version(
    app: &tauri::AppHandle,
    instance_id: &str,
    item_id: &str,
    candidate: &ModVersionCandidate,
) -> LauncherResult<InstalledMod> {
    check_not_locked(app, instance_id)?;

    // 1. Load the registry item to get the pinned SHA-256.
    let item = load_registry_item(app, item_id)?;
    let pinned_sha = item.sha256.trim().to_string();

    // 2. Pre-check: verify sufficient disk space before any network request (Â§7.1.2).
    let instance_dir =
        paths::instance_dir(app, instance_id).map_err(|_| LauncherError::InstanceCreateFailed)?;
    if let Some(free) = available_disk_space_bytes(&instance_dir) {
        if free < MIN_DISK_SPACE_BYTES {
            return Err(LauncherError::DiskFull);
        }
    }
    // If we cannot determine free space (None), proceed â€” do not block on unavailable info.

    // 3. Download bytes from the candidate URL.
    let bytes = download_mod_bytes(&candidate.download_url).await?;

    // 3. Verify hash against the appropriate source.
    let candidate_sha1 = candidate
        .sha1
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if !candidate_sha1.is_empty() {
        // Modrinth-published per-file SHA-1: verify against that.
        let actual_sha1 = download::sha1_hex(&bytes);
        if actual_sha1 != candidate_sha1 {
            return Err(LauncherError::HashMismatch);
        }
    } else if item.download_strategy != "github_release" && !pinned_sha.is_empty() {
        // Direct-hash or other strategies: verify against the pinned SHA-256.
        let actual_sha = download::sha256_hex(&bytes);
        if actual_sha != pinned_sha {
            return Err(LauncherError::HashMismatch);
        }
    }
    // For `github_release` strategy the pinned SHA-256 is only valid for the
    // single version the compiler hashed at build time.  Users can pick any
    // release version, so enforcing the pinned hash would reject every version
    // except the one the compiler saw.  Transport integrity is provided by
    // HTTPS; the downloaded hash is still recorded in the instance manifest.

    // 3b. Compute the actual SHA-256 of the downloaded bytes for the manifest.
    let installed_sha256 = download::sha256_hex(&bytes);

    // 4. Ensure target subdirectory exists and write the file.
    let instance_dir =
        paths::instance_dir(app, instance_id).map_err(|_| LauncherError::InstanceCreateFailed)?;
    let target_dir = instance_dir.join(content_subdir(&item.content_type));
    std::fs::create_dir_all(&target_dir).map_err(|_| LauncherError::InstanceCreateFailed)?;

    let item_path = target_dir.join(&candidate.filename);
    std::fs::write(&item_path, &bytes).map_err(|_| LauncherError::InstanceCreateFailed)?;

    // 5. Update the instance manifest atomically.
    let manifest_path = paths::instance_manifest_path(app, instance_id)
        .map_err(|_| LauncherError::InstanceCreateFailed)?;

    let mut manifest: InstanceManifest = if manifest_path.exists() {
        let text = std::fs::read_to_string(&manifest_path)
            .map_err(|_| LauncherError::InstanceCreateFailed)?;
        serde_json::from_str(&text).map_err(|_| LauncherError::InstanceCreateFailed)?
    } else {
        return Err(LauncherError::Generic {
            code: "ERR_MANIFEST_MISSING".to_string(),
            message: format!(
                "Instance manifest not found at '{}'. Create the instance first.",
                manifest_path.display()
            ),
        });
    };

    let metadata = agora_core::jar_metadata::parse_jar_metadata(&item_path);
    let installed_mod = InstalledMod {
        filename: candidate.filename.clone(),
        registry_id: Some(item_id.to_string()),
        modrinth_id: item.modrinth_id.clone(),
        source: "registry".to_string(),
        source_url: Some(candidate.download_url.clone()),
        version: Some(candidate.version.clone()),
        sha256: installed_sha256,
        installed_at: chrono::Utc::now().to_rfc3339(),
        java_packages: metadata.java_packages,
        mod_jar_id: metadata.mod_jar_id,
        depends_on: metadata.depends_on,
        optional_deps: metadata.optional_deps,
        incompatible_deps: metadata.incompatible_deps,
        provided_mod_ids: metadata
            .provided_mods
            .into_iter()
            .map(|provided| provided.mod_id)
            .collect(),
        enabled: true,
        content_type: if item.content_type.is_empty() {
            "mod".to_string()
        } else {
            item.content_type.clone()
        },
    };

    // Add to the correct array
    push_to_content_array(&mut manifest, &installed_mod);

    // Atomic write: .tmp then rename.
    let tmp_path = manifest_path.with_extension("json.tmp");
    let text =
        serde_json::to_string_pretty(&manifest).map_err(|_| LauncherError::InstanceCreateFailed)?;
    std::fs::write(&tmp_path, text).map_err(|_| LauncherError::InstanceCreateFailed)?;
    std::fs::rename(&tmp_path, &manifest_path).map_err(|_| LauncherError::InstanceCreateFailed)?;

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
    check_not_locked(app, instance_id)?;

    // Zip Slip protection: reject filenames containing path traversal or separators.
    if filename.contains("..") || filename.contains('/') || filename.contains('\\') {
        return Err(LauncherError::Generic {
            code: "ERR_INVALID_FILENAME".to_string(),
            message: "Filename contains invalid characters.".to_string(),
        });
    }

    // 1. Delete the file from whichever content subdirectory it lives in.
    let instance_dir =
        paths::instance_dir(app, instance_id).map_err(|_| LauncherError::InstanceCreateFailed)?;
    let subdirs = ["mods", "resourcepacks", "shaderpacks", "datapacks", "saves"];
    let filename_owned = filename.to_string();
    tokio::task::spawn_blocking(move || {
        for sub in &subdirs {
            let candidate = instance_dir.join(sub).join(&filename_owned);
            if candidate.exists() {
                std::fs::remove_file(&candidate)
                    .map_err(|_| LauncherError::InstanceCreateFailed)?;
                break;
            }
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

        let mut manifest: InstanceManifest =
            serde_json::from_str(&text).map_err(|_| LauncherError::InstanceCreateFailed)?;

        if remove_from_content_array(&mut manifest, filename) {
            // Atomic write: .tmp then rename.
            let tmp_path = manifest_path.with_extension("json.tmp");
            let write_text = serde_json::to_string_pretty(&manifest)
                .map_err(|_| LauncherError::InstanceCreateFailed)?;
            tokio::task::spawn_blocking(move || {
                std::fs::write(&tmp_path, write_text)
                    .map_err(|_| LauncherError::InstanceCreateFailed)?;
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

/// Find the content subdirectory containing `filename` (or `filename.disabled`
/// when `enable` is true), rename it to the opposite state, and return
/// `Some(subdir_name)` on success or `None` if no matching file was found.
fn rename_in_content_dir(base: &Path, filename: &str, enable: bool) -> Option<String> {
    const SUBDIRS: &[&str] = &["mods", "resourcepacks", "shaderpacks", "datapacks", "saves"];
    for sub in SUBDIRS {
        let dir = base.join(sub);
        if enable {
            let source = dir.join(format!("{}.disabled", filename));
            let dest = dir.join(filename);
            if source.exists() && !dest.exists() {
                std::fs::rename(&source, &dest).ok()?;
                return Some(sub.to_string());
            }
        } else {
            let source = dir.join(filename);
            let dest = dir.join(format!("{}.disabled", filename));
            if source.exists() && !dest.exists() {
                std::fs::rename(&source, &dest).ok()?;
                return Some(sub.to_string());
            }
        }
    }
    None
}

/// Set `enabled` on the manifest entry matching `filename` across all arrays.
/// Returns whether the entry was found.
fn set_enabled_in_all_arrays(
    manifest: &mut InstanceManifest,
    filename: &str,
    enabled: bool,
) -> bool {
    for arr in [
        &mut manifest.mods,
        &mut manifest.resourcepacks,
        &mut manifest.shaders,
        &mut manifest.datapacks,
        &mut manifest.worlds,
    ] {
        if let Some(entry) = arr.iter_mut().find(|m| m.filename == filename) {
            entry.enabled = enabled;
            return true;
        }
    }
    false
}

/// Disable a mod by renaming `mods/<filename>` to `mods/<filename>.disabled` and
/// setting `enabled: false` in the manifest. Minecraft ignores `.disabled` files,
/// so the mod is not loaded by either direct spawn or the Mojang launcher.
pub fn disable_instance_mod(
    app: &tauri::AppHandle,
    instance_id: &str,
    filename: &str,
) -> LauncherResult<()> {
    let sanitized = paths::sanitize_id(instance_id);
    let instance_dir =
        paths::instance_dir(app, &sanitized).map_err(|_| LauncherError::InstanceCreateFailed)?;

    if rename_in_content_dir(&instance_dir, filename, false).is_none() {
        return Err(LauncherError::Generic {
            code: "ERR_MOD_FILE_NOT_FOUND".to_string(),
            message: format!("File '{filename}' not found in any content directory."),
        });
    }

    // Update manifest
    let manifest_path = paths::instance_manifest_path(app, &sanitized)
        .map_err(|_| LauncherError::InstanceCreateFailed)?;
    if manifest_path.exists() {
        let text = std::fs::read_to_string(&manifest_path)
            .map_err(|_| LauncherError::InstanceCreateFailed)?;
        let mut manifest: InstanceManifest =
            serde_json::from_str(&text).map_err(|_| LauncherError::InstanceCreateFailed)?;
        set_enabled_in_all_arrays(&mut manifest, filename, false);

        let tmp_path = manifest_path.with_extension("json.tmp");
        let write_text = serde_json::to_string_pretty(&manifest)
            .map_err(|_| LauncherError::InstanceCreateFailed)?;
        std::fs::write(&tmp_path, write_text).map_err(|_| LauncherError::InstanceCreateFailed)?;
        std::fs::rename(&tmp_path, &manifest_path)
            .map_err(|_| LauncherError::InstanceCreateFailed)?;
    }

    Ok(())
}

/// Re-enable a disabled mod by renaming `mods/<filename>.disabled` back to
/// `mods/<filename>` and setting `enabled: true` in the manifest.
pub fn enable_instance_mod(
    app: &tauri::AppHandle,
    instance_id: &str,
    filename: &str,
) -> LauncherResult<()> {
    let sanitized = paths::sanitize_id(instance_id);
    let instance_dir =
        paths::instance_dir(app, &sanitized).map_err(|_| LauncherError::InstanceCreateFailed)?;

    if rename_in_content_dir(&instance_dir, filename, true).is_none() {
        return Ok(()); // already enabled or file not found
    }

    // Update manifest
    let manifest_path = paths::instance_manifest_path(app, &sanitized)
        .map_err(|_| LauncherError::InstanceCreateFailed)?;
    if manifest_path.exists() {
        let text = std::fs::read_to_string(&manifest_path)
            .map_err(|_| LauncherError::InstanceCreateFailed)?;
        let mut manifest: InstanceManifest =
            serde_json::from_str(&text).map_err(|_| LauncherError::InstanceCreateFailed)?;
        set_enabled_in_all_arrays(&mut manifest, filename, true);

        let tmp_path = manifest_path.with_extension("json.tmp");
        let write_text = serde_json::to_string_pretty(&manifest)
            .map_err(|_| LauncherError::InstanceCreateFailed)?;
        std::fs::write(&tmp_path, write_text).map_err(|_| LauncherError::InstanceCreateFailed)?;
        std::fs::rename(&tmp_path, &manifest_path)
            .map_err(|_| LauncherError::InstanceCreateFailed)?;
    }

    Ok(())
}

/// Add a manually-dropped .jar file into an instance's `mods/` folder (Â§6.5b).
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
    check_not_locked(app, instance_id)?;

    use std::path::Path;

    let src = Path::new(source_path);
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    if ext.as_deref() != Some("jar") {
        return Err(LauncherError::Generic {
            code: "ERR_INVALID_FILENAME".to_string(),
            message: "Only .jar files can be added manually.".to_string(),
        });
    }
    let file_name =
        src.file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| LauncherError::Generic {
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
        let canonical =
            std::fs::canonicalize(&source_path_owned).map_err(|_| LauncherError::Generic {
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

        let metadata = agora_core::jar_metadata::parse_jar_metadata(&dest);
        let installed_mod = InstalledMod {
            filename: file_name_owned.clone(),
            registry_id: None,
            modrinth_id: None,
            source: "manual_drag_drop".to_string(),
            source_url: None,
            version: None,
            sha256,
            installed_at: chrono::Utc::now().to_rfc3339(),
            java_packages: metadata.java_packages,
            mod_jar_id: metadata.mod_jar_id,
            depends_on: metadata.depends_on,
            optional_deps: metadata.optional_deps,
            incompatible_deps: metadata.incompatible_deps,
            provided_mod_ids: metadata
                .provided_mods
                .into_iter()
                .map(|provided| provided.mod_id)
                .collect(),
            enabled: true,
            content_type: "mod".to_string(),
        };
        push_to_content_array(&mut manifest, &installed_mod);

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
    fn test_download_url_requires_trusted_https_origin() {
        assert!(validate_mod_download_url("https://cdn.modrinth.com/data/example.mrpack").is_ok());
        assert!(validate_mod_download_url(
            "https://github.com/example/mod/releases/download/v1/mod.jar"
        )
        .is_ok());
        assert!(matches!(
            validate_mod_download_url("http://cdn.modrinth.com/data/example.mrpack"),
            Err(LauncherError::UntrustedSource)
        ));
        assert!(matches!(
            validate_mod_download_url("https://127.0.0.1:39741/private"),
            Err(LauncherError::UntrustedSource)
        ));
        assert!(matches!(
            validate_mod_download_url("https://cdn.modrinth.com:8443/data/example.mrpack"),
            Err(LauncherError::UntrustedSource)
        ));
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

    // --- Version-from-filename parser ---

    #[test]
    fn test_parse_version_matches() {
        let (mc, lo, compat) = parse_version_from_github_asset(
            "fabric-api-0.92.0+1.20.1.jar",
            "v0.92.0+1.20.1",
            "1.20.1",
            "fabric",
        );
        assert_eq!(mc, Some("1.20.1".to_string()));
        assert_eq!(lo, Some("fabric".to_string()));
        assert_eq!(compat, "compatible");
    }

    #[test]
    fn test_parse_version_no_match() {
        let (mc, lo, compat) =
            parse_version_from_github_asset("some-random-mod.jar", "v1.0.0", "1.20.1", "fabric");
        assert!(mc.is_none());
        assert!(lo.is_none());
        assert_eq!(compat, "");
    }

    #[test]
    fn test_parse_version_from_tag() {
        let (mc, lo, compat) = parse_version_from_github_asset(
            "lithium-0.25.1.jar",
            "mc1.21.1-0.25.1",
            "1.21.1",
            "fabric",
        );
        assert_eq!(mc, Some("1.21.1".to_string()));
        assert_eq!(lo, None);
        // MC version matches exactly â†’ compatible even without loader in filename
        assert_eq!(compat, "compatible");
    }

    #[test]
    fn test_parse_version_major_match() {
        // fabric-api-0.92.0+1.21.1.jar on a 1.21.11 fabric instance â†’ major_match
        let (mc, lo, compat) = parse_version_from_github_asset(
            "fabric-api-0.92.0+1.21.1.jar",
            "v0.92.0+1.21.1",
            "1.21.11",
            "fabric",
        );
        assert_eq!(lo, Some("fabric".to_string()));
        assert_eq!(compat, "major_match");
    }

    #[test]
    fn test_parse_version_loader_mismatch() {
        // fabric-api-0.154.0+26.2.jar on a forge instance â†’ incompatible
        // The '+' in the filename is 0x2B, not a URL-encoded space
        let filename = "fabric-api-0.154.0+26.2.jar";
        assert_eq!(
            extract_loader(filename),
            Some("fabric".to_string()),
            "extract_loader should find fabric"
        );

        let (mc, lo, compat) =
            parse_version_from_github_asset(filename, "0.154.0+26.2", "26.2", "forge");
        assert_eq!(mc, Some("26.2".to_string()));
        assert_eq!(lo, Some("fabric".to_string()));
        assert_eq!(compat, "");
    }

    #[test]
    fn test_parse_version_major_match_no_loader() {
        // iris-1.7.3+mc1.21.jar on a 1.21.11 instance â†’ major_match
        let (mc, lo, compat) = parse_version_from_github_asset(
            "iris-1.7.3+mc1.21.jar",
            "v1.7.3+mc1.21",
            "1.21.11",
            "fabric",
        );
        assert_eq!(mc, Some("1.21".to_string()));
        assert!(lo.is_none());
        assert_eq!(compat, "major_match");
    }

    #[test]
    fn test_parse_version_iris_mc_prefix() {
        // iris-1.7.3+mc1.21.jar â€” mc prefix wins over bare 1.7.3
        // No loader in filename but MC version matches â†’ compatible
        let (mc, lo, compat) = parse_version_from_github_asset(
            "iris-1.7.3+mc1.21.jar",
            "v1.7.3+mc1.21",
            "1.21",
            "fabric",
        );
        assert_eq!(mc, Some("1.21".to_string()));
        assert!(lo.is_none());
        assert_eq!(compat, "compatible");
    }

    #[test]
    fn test_parse_version_iris_with_loader() {
        // iris-fabric-1.7.3+mc1.21.jar â€” mc prefix + loader match
        let (mc, lo, compat) = parse_version_from_github_asset(
            "iris-fabric-1.7.3+mc1.21.jar",
            "v1.7.3+mc1.21",
            "1.21",
            "fabric",
        );
        assert_eq!(mc, Some("1.21".to_string()));
        assert_eq!(lo, Some("fabric".to_string()));
        assert_eq!(compat, "compatible");
    }

    #[test]
    fn test_extract_mc_version_bare() {
        assert_eq!(
            extract_mc_version("my-mod-1.21.11.jar"),
            Some("1.21.11".to_string())
        );
    }

    #[test]
    fn test_extract_mc_version_mc_prefix() {
        assert_eq!(
            extract_mc_version("mc1.21.1.jar"),
            Some("1.21.1".to_string())
        );
    }

    #[test]
    fn test_extract_mc_version_no_1_prefix() {
        assert_eq!(
            extract_mc_version("mc26.2-0.25.1.jar"),
            Some("26.2".to_string())
        );
    }

    #[test]
    fn test_extract_mc_version_prefers_mc_prefix() {
        // iris-1.7.3+mc1.21.jar â€” should pick mc1.21, not 1.7.3
        assert_eq!(
            extract_mc_version("iris-1.7.3+mc1.21.jar"),
            Some("1.21".to_string())
        );
    }

    #[test]
    fn test_extract_mc_version_fabricmc_no_false_match() {
        // "fabricmc" should NOT match as mc prefix
        assert_eq!(
            extract_mc_version("fabricmc-1.21.jar"),
            Some("1.21".to_string())
        );
    }

    #[test]
    fn test_extract_mc_version_fabricmc_with_mc() {
        // "fabricmc-mc1.21.jar" â€” second mc is the real prefix
        assert_eq!(
            extract_mc_version("fabricmc-mc1.21.jar"),
            Some("1.21".to_string())
        );
    }

    #[test]
    fn test_extract_loader() {
        assert_eq!(
            extract_loader("fabric-api-0.92.0.jar"),
            Some("fabric".to_string())
        );
        assert_eq!(
            extract_loader("fabric-api-0.154.0+26.2.jar"),
            Some("fabric".to_string())
        );
        assert_eq!(
            extract_loader("my-forge-mod.jar"),
            Some("forge".to_string())
        );
        assert_eq!(
            extract_loader("neoforge-thing.jar"),
            Some("neoforge".to_string())
        );
        assert_eq!(extract_loader("random.jar"), None);
    }

    #[test]
    fn test_extract_loader_word_boundary() {
        assert_eq!(extract_loader("fabricate-1.0.jar"), None);
        assert_eq!(extract_loader("fabricated.jar"), None);
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
    use sha2::Digest;
    use std::io::{Read, Write};
    const CHUNK: usize = 64 * 1024;

    let mut f = std::fs::File::open(path).map_err(|_| LauncherError::InstanceCreateFailed)?;
    zip.start_file(entry_name, opts)
        .map_err(|_| LauncherError::InstanceCreateFailed)?;
    let mut hasher = sha2::Sha256::new();
    let mut buf = [0u8; CHUNK];
    let mut size: u64 = 0;
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|_| LauncherError::InstanceCreateFailed)?;
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

/// Export an instance as a shareable pack file (Â§6.5c).
///
/// - `format == "json"`: a custom `.agora-pack.json` manifest containing the
///   instance metadata + installed-mod list (registry ids, sources, versions,
///   SHA-256 hashes). Small (5â€“20KB) and sufficient to rebuild the instance on
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
            std::fs::rename(&tmp_path, &out_path)
                .map_err(|_| LauncherError::InstanceCreateFailed)?;
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
                let opts: zip::write::FileOptions = zip::write::FileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored);

                let mut files_meta: Vec<serde_json::Value> = Vec::new();

                for m in &manifest.mods {
                    // Modrinth-tracked mods: reference the upstream file by URL + published
                    // hashes instead of bundling the jar. This is the mrpack v1 best practice
                    // and keeps exported packs tiny + resolvable. Falls through to local
                    // bundling when Modrinth has no metadata for this filename.
                    if let Some(mid) = m.modrinth_id.as_deref().filter(|s| !s.trim().is_empty()) {
                        if let Some(meta) =
                            crate::modrinth_raw::resolve_modrinth_file_metadata(mid, &m.filename)
                                .await
                        {
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
                            // Unsanitizable filename (traversal / null) â€” record
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
                            // File unreadable/missing â€” record the manifest
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
                deps.insert(
                    "minecraft".to_string(),
                    serde_json::Value::String(manifest.minecraft_version.clone()),
                );
                deps.insert(
                    manifest.loader.clone(),
                    serde_json::Value::String(manifest.loader_version.clone()),
                );
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
                zip.finish()
                    .map_err(|_| LauncherError::InstanceCreateFailed)?;
            }

            std::fs::rename(&tmp_path, &out_path)
                .map_err(|_| LauncherError::InstanceCreateFailed)?;
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
/// - `.mrpack` â€” Modrinth mrpack v1 (zip containing `modrinth.index.json`
///   + bundled override jars under their declared `path`).
/// - `.json` / `.agora-pack.json` â€” Agora's plain-JSON `agora-pack/v1`
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
/// `add_manual_mod` is NOT applied here â€” callers reach this through
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
            message: "Unsupported pack file extension. Use .mrpack or .agora-pack.json."
                .to_string(),
        })
    }
}

/// --- mrpack import ---

/// Import a Modrinth mrpack (.mrpack) file.
async fn import_mrpack(app: &tauri::AppHandle, source_path: &str) -> LauncherResult<String> {
    let file = std::fs::File::open(source_path).map_err(|_| LauncherError::Generic {
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
        let mut entry =
            archive
                .by_name("modrinth.index.json")
                .map_err(|_| LauncherError::Generic {
                    code: "ERR_PACK_PARSE".to_string(),
                    message: "modrinth.index.json not found in mrpack.".to_string(),
                })?;
        entry
            .read_to_string(&mut index_text)
            .map_err(|_| LauncherError::Generic {
                code: "ERR_PACK_READ".to_string(),
                message: "Failed to read modrinth.index.json.".to_string(),
            })?;
    }
    let index: serde_json::Value =
        serde_json::from_str(&index_text).map_err(|_| LauncherError::Generic {
            code: "ERR_PACK_PARSE".to_string(),
            message: "Failed to parse modrinth.index.json.".to_string(),
        })?;

    // Extract dependencies
    let deps = index
        .get("dependencies")
        .and_then(|d| d.as_object())
        .ok_or_else(|| LauncherError::Generic {
            code: "ERR_PACK_PARSE".to_string(),
            message: "mrpack has no dependencies map.".to_string(),
        })?;

    let minecraft_version = deps
        .get("minecraft")
        .and_then(|v| v.as_str())
        .ok_or_else(|| LauncherError::Generic {
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
    let instance_id = if instance_id.is_empty() {
        "imported-pack".to_string()
    } else {
        instance_id
    };

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
            let path = file_entry
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !path.starts_with("mods/") {
                continue;
            }
            let basename = path.strip_prefix("mods/").unwrap();
            if basename.is_empty() {
                continue;
            }
            // Skip nested paths (only flat mods/<filename> is supported)
            if basename.contains('/') || basename.contains('\\') {
                auth::log_line(&format!(
                    "import_mrpack: path contains nested slashes, skipping '{basename}'"
                ));
                continue;
            }

            if safe_zip_entry_name(basename).is_none() {
                auth::log_line(&format!(
                    "import_mrpack: unsafe basename '{basename}', skipping"
                ));
                continue;
            }

            let downloads = file_entry.get("downloads").and_then(|d| d.as_array());
            if let Some(downloads) = downloads {
                if let Some(url) = downloads.first().and_then(|u| u.as_str()) {
                    // Download from Modrinth CDN (host-allowlisted by download_mod_bytes)
                    let bytes = download_mod_bytes(url).await?;
                    // SHA-1 verification if declared
                    if let Some(expected_sha1) = file_entry
                        .get("hashes")
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
                    std::fs::write(&mod_path, &bytes)
                        .map_err(|_| LauncherError::InstanceCreateFailed)?;
                    let metadata = agora_core::jar_metadata::parse_jar_metadata(&mod_path);
                    installed_mods.push(InstalledMod {
                        filename: basename.to_string(),
                        registry_id: None,
                        modrinth_id: None,
                        source: "modrinth_pack".to_string(),
                        source_url: downloads
                            .first()
                            .and_then(|value| value.as_str())
                            .map(str::to_string),
                        version: None,
                        sha256,
                        installed_at: now.clone(),
                        java_packages: metadata.java_packages,
                        mod_jar_id: metadata.mod_jar_id,
                        depends_on: metadata.depends_on,
                        optional_deps: metadata.optional_deps,
                        incompatible_deps: metadata.incompatible_deps,
                        provided_mod_ids: metadata
                            .provided_mods
                            .into_iter()
                            .map(|provided| provided.mod_id)
                            .collect(),
                        enabled: true,
                        content_type: "mod".to_string(),
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
                    std::fs::write(&mod_path, &bytes)
                        .map_err(|_| LauncherError::InstanceCreateFailed)?;
                    let metadata = agora_core::jar_metadata::parse_jar_metadata(&mod_path);
                    installed_mods.push(InstalledMod {
                        filename: basename.to_string(),
                        registry_id: None,
                        modrinth_id: None,
                        source: "modrinth_pack_bundle".to_string(),
                        source_url: None,
                        version: None,
                        sha256,
                        installed_at: now.clone(),
                        java_packages: metadata.java_packages,
                        mod_jar_id: metadata.mod_jar_id,
                        depends_on: metadata.depends_on,
                        optional_deps: metadata.optional_deps,
                        incompatible_deps: metadata.incompatible_deps,
                        provided_mod_ids: metadata
                            .provided_mods
                            .into_iter()
                            .map(|provided| provided.mod_id)
                            .collect(),
                        enabled: true,
                        content_type: "mod".to_string(),
                    });
                } else {
                    auth::log_line(&format!(
                        "import_mrpack: bundled file not found in zip: '{path}'"
                    ));
                }
            }
        }
    }

    // Extract override files from the zip (overrides/ and client_overrides/).
    let instance_root =
        paths::instance_dir(app, &instance_id).map_err(|_| LauncherError::InstanceCreateFailed)?;

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
        let allowed = [
            "config/",
            "defaultconfigs/",
            "resourcepacks/",
            "shaderpacks/",
            "datapacks/",
            "kubejs/",
        ];
        if !allowed.iter().any(|p| normalized.starts_with(p)) {
            override_skipped.push(normalized.clone());
            continue;
        }

        // Check banned extensions.
        let lower = normalized.to_lowercase();
        let banned = [
            ".jar", ".class", ".exe", ".bat", ".cmd", ".sh", ".ps1", ".dll", ".so", ".dylib",
            ".msi", ".dmg",
        ];
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
        let mut manifest: InstanceManifest =
            serde_json::from_str(&text).map_err(|_| LauncherError::InstanceCreateFailed)?;
        manifest.mods.extend(installed_mods);

        let tmp_path = manifest_path.with_extension("json.tmp");
        let write_text = serde_json::to_string_pretty(&manifest)
            .map_err(|_| LauncherError::InstanceCreateFailed)?;
        tokio::task::spawn_blocking(move || {
            std::fs::write(&tmp_path, write_text)
                .map_err(|_| LauncherError::InstanceCreateFailed)?;
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
    let pack: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| LauncherError::Generic {
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
    let instance_id = if instance_id.is_empty() {
        "imported-pack".to_string()
    } else {
        instance_id
    };

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
                        let candidate =
                            candidates
                                .iter()
                                .find(|c| c.filename == filename)
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
                        "import_agora_pack: skipping modrinth mod '{mid}' â€” Modrinth integration disabled"
                    ));
                    continue;
                }
                let filename = mod_entry
                    .get("filename")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                match crate::modrinth_raw::list_raw_modrinth_versions(
                    app,
                    Some(&instance_id),
                    mid,
                    Some("mod"),
                )
                .await
                {
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
                            if let Err(e) = crate::modrinth_raw::install_raw_modrinth(
                                app,
                                &instance_id,
                                mid,
                                c,
                                "mod",
                            )
                            .await
                            {
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
