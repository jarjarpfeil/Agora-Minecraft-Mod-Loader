//! Raw (uncurated) Modrinth API integration (§6.3).
//!
//! Provides live search against the Modrinth v2 search endpoint and downloads
//! mod files directly from Modrinth's CDN, verifying each file against the
//! SHA-1 hash published in Modrinth's API response before writing it to the
//! instance `mods/` directory.
//!
//! All entrypoints are gated behind the `modrinth_enabled` setting. When the
//! toggle is off, callers receive `LauncherError::ModrinthDisabled` rather than
//! touching the network.

use crate::db;
use crate::error::{LauncherError, LauncherResult};
use crate::mod_install::{available_disk_space_bytes, download_mod_bytes, MIN_DISK_SPACE_BYTES};
use crate::models::{InstalledMod, InstanceManifest, InstanceRow};
use crate::paths;

use serde::{Deserialize, Serialize};

// --- Modrinth project full-details types ---

#[derive(Debug, Deserialize)]
struct ModrinthProjectFullRaw {
    id: String,
    title: String,
    description: String,
    body: Option<String>,
    icon_url: Option<String>,
    slug: Option<String>,
    project_type: String,
    license: Option<ModrinthLicenseRaw>,
    updated: Option<String>,
    #[serde(default)]
    gallery: Option<Vec<ModrinthGalleryImageRaw>>,
}

#[derive(Debug, Deserialize)]
struct ModrinthLicenseRaw {
    id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModrinthGalleryImageRaw {
    url: Option<String>,
}

/// Full project details returned from the Modrinth v2 project endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct ModrinthProjectFull {
    pub id: String,
    pub title: String,
    pub description: String,
    pub body: Option<String>,
    pub icon_url: Option<String>,
    pub project_type: String,
    pub page_url: Option<String>,
    pub license_id: Option<String>,
    pub source_updated_at: Option<String>,
    pub gallery_urls: Vec<String>,
}

/// Read the `modrinth_enabled` boolean setting from `local_state.db`.
/// Returns `false` on any read failure (security default: off).
pub fn is_modrinth_enabled(app: &tauri::AppHandle) -> bool {
    let conn = match db::local_state_connection(app) {
        Ok(c) => c,
        Err(_) => return false,
    };
    match db::get_setting(&conn, "modrinth_enabled") {
        Ok(Some(v)) => v == true,
        _ => false,
    }
}

/// Enforce the Modrinth-enabled gate; returns `Err(ModrinthDisabled)` when off.
fn require_modrinth_enabled(app: &tauri::AppHandle) -> LauncherResult<()> {
    if is_modrinth_enabled(app) {
        Ok(())
    } else {
        Err(LauncherError::ModrinthDisabled)
    }
}

// --- Modrinth v2 search response types ---

#[derive(Debug, Deserialize)]
struct ModrinthSearchResponse {
    hits: Vec<ModrinthSearchHit>,
    #[serde(default)]
    total_hits: u64,
    #[serde(default)]
    offset: u64,
    #[serde(default)]
    limit: u64,
}

#[derive(Debug, Deserialize)]
struct ModrinthSearchHit {
    project_id: String,
    slug: Option<String>,
    title: String,
    description: Option<String>,
    icon_url: Option<String>,
    // Modrinth search returns author/categories as arrays.
    author: Option<String>,
    categories: Option<Vec<String>>,
    downloads: Option<i64>,
    project_type: Option<String>,
    follows: Option<i64>,
    date_created: Option<String>,
    date_modified: Option<String>,
    versions: Option<Vec<String>>,
    license: Option<String>,
}

/// A single Modrinth search result returned to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModrinthSearchResult {
    pub project_id: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub icon_url: Option<String>,
    pub author: String,
    pub categories: Vec<String>,
    pub downloads: i64,
    pub follows: i64,
    pub project_type: String,
    pub date_created: Option<String>,
    pub date_modified: Option<String>,
    pub versions: Vec<String>,
    pub license: Option<String>,
}

/// Per-file metadata resolved from Modrinth's API for a single project file.
#[derive(Debug, Clone, Serialize)]
pub struct ModrinthFileMetadata {
    pub url: String,
    pub sha1: String,
    pub sha512: String,
    pub size: u64,
}

/// Valid Modrinth sort indexes for the search endpoint.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModrinthSort {
    Relevance,
    Downloads,
    Follows,
    Newest,
    Updated,
}

impl Default for ModrinthSort {
    fn default() -> Self {
        Self::Relevance
    }
}

impl ModrinthSort {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Relevance => "relevance",
            Self::Downloads => "downloads",
            Self::Follows => "follows",
            Self::Newest => "newest",
            Self::Updated => "updated",
        }
    }
}

/// Filter + paging parameters for a single search request page.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ModrinthSearchParams {
    pub query: Option<String>,
    /// Content categories (e.g. "adventure", "magic"). Multiple selected
    /// categories are AND-combined (mod must be in every category), matching
    /// Modrinth's native search behaviour.
    pub categories: Option<Vec<String>>,
    /// Modloaders (e.g. "fabric", "neoforge"). Multiple loaders are
    /// OR-combined within a single facet group.
    pub loaders: Option<Vec<String>>,
    /// Minecraft versions (e.g. "1.21.1"). Multiple versions are OR-combined.
    pub game_versions: Option<Vec<String>>,
    pub sort: Option<ModrinthSort>,
    pub offset: Option<u32>,
    pub limit: Option<u32>,
    /// Modrinth project type filter: "mod", "shader", "resourcepack", "modpack", "datapack".
    /// Defaults to "mod" when None (preserving existing behavior).
    pub project_type: Option<String>,
}

/// A page of search results with paging metadata for infinite scroll.
#[derive(Debug, Clone, Serialize)]
pub struct ModrinthSearchPage {
    pub results: Vec<ModrinthSearchResult>,
    pub total_hits: u64,
    pub offset: u64,
    pub limit: u64,
}

// --- Modrinth v2 tag (facet value) types ---

#[derive(Debug, Deserialize)]
struct ModrinthCategoryTag {
    name: String,
    project_type: String,
    #[serde(default)]
    header: String,
}

#[derive(Debug, Deserialize)]
struct ModrinthLoaderTag {
    name: String,
    #[serde(default)]
    supported_project_types: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ModrinthGameVersionTag {
    version: String,
    version_type: String,
    date: String,
    major: bool,
}

/// A category tag suitable for filter UI.
#[derive(Debug, Clone, Serialize)]
pub struct ModrinthCategoryInfo {
    pub name: String,
    pub project_type: String,
    pub header: String,
}

/// A loader tag suitable for filter UI.
#[derive(Debug, Clone, Serialize)]
pub struct ModrinthLoaderInfo {
    pub name: String,
    pub supported_project_types: Vec<String>,
}

/// A game version tag suitable for filter UI.
#[derive(Debug, Clone, Serialize)]
pub struct ModrinthGameVersionInfo {
    pub version: String,
    pub version_type: String,
    pub date: String,
    pub major: bool,
}

// --- Modrinth v2 project version response types (for version-listing API) ---

#[derive(Debug, Deserialize)]
struct ModrinthVersion {
    id: String,
    name: Option<String>,
    version_number: String,
    date_published: Option<String>,
    game_versions: Option<Vec<String>>,
    loaders: Option<Vec<String>>,
    files: Vec<ModrinthVersionFile>,
    #[serde(default)]
    dependencies: Vec<ModrinthDependency>,
    #[serde(default)]
    changelog: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModrinthVersionFile {
    url: String,
    filename: String,
    primary: bool,
    /// Modrinth publishes both sha1 and sha512 hashes for each file.
    /// Per §6.3 the launcher verifies against the SHA-1 hash published by
    /// Modrinth's API.
    hashes: Option<ModrinthFileHashes>,
    #[serde(default)]
    size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawModrinthDependency {
    pub project_id: Option<String>,
    pub version_id: Option<String>,
    pub dependency_type: String,
}

#[derive(Debug, Deserialize)]
struct ModrinthDependency {
    project_id: Option<String>,
    version_id: Option<String>,
    dependency_type: String,
}

#[derive(Debug, Deserialize)]
struct ModrinthFileHashes {
    sha1: Option<String>,
    /// Modrinth also publishes sha512; retained for documentation / future use.
    #[allow(dead_code)]
    sha512: Option<String>,
}

// --- Private API types for resolve_modrinth_file_metadata (self-contained) ---

#[derive(Debug, Deserialize)]
struct ModrinthVersionFileRaw {
    url: String,
    filename: String,
    primary: Option<bool>,
    size: Option<u64>,
    hashes: Option<ModrinthFileHashesRaw>,
}

#[derive(Debug, Deserialize)]
struct ModrinthFileHashesRaw {
    sha1: Option<String>,
    sha512: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModrinthVersionRaw {
    #[serde(default)]
    files: Vec<ModrinthVersionFileRaw>,
}

/// A candidate version returned to the frontend for the raw Modrinth picker.
///
/// Carries the SHA-1 hash from the Modrinth API so the frontend can pass it
/// back to `install_raw_modrinth` without a second API round-trip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawModrinthVersionCandidate {
    pub version: String,
    pub version_id: String,
    pub name: String,
    pub filename: String,
    pub download_url: String,
    pub sha1: Option<String>,
    pub sha512: Option<String>,
    pub size: Option<u64>,
    pub dependencies: Vec<RawModrinthDependency>,
    pub mc_versions: Vec<String>,
    pub loaders: Vec<String>,
    pub release_date: Option<String>,
    pub primary: bool,
    pub changelog: Option<String>,
}

/// Live Modrinth search with facets, sorting and offset pagination.
///
/// Facets are built to mirror Modrinth's native search semantics:
///   - multiple categories → AND (the mod must have every category)
///   - multiple loaders    → OR  (mod supports any selected loader)
///   - multiple game versions → OR
///   - `project_type:mod` is always applied (this tab only surfaces mods).
///
/// Returns a [`ModrinthSearchPage`] including `total_hits` so the frontend
/// can drive infinite scroll / "load more" paging.
pub async fn search_modrinth(
    app: &tauri::AppHandle,
    params: &ModrinthSearchParams,
) -> LauncherResult<ModrinthSearchPage> {
    require_modrinth_enabled(app)?;

    let limit = params.limit.unwrap_or(20).clamp(1, 100);
    let offset = params.offset.unwrap_or(0);
    let sort = params.sort.unwrap_or_default();
    let q = match params.query.as_deref() {
        Some(s) if !s.trim().is_empty() => s.trim(),
        _ => "*",
    };

    // Build facets as a JSON array of arrays (OR within a group, AND across
    // groups), per the Modrinth v2 search facet spec. Loaders map onto the
    // `categories` facet because Modrinth treats loaders as categories in
    // search.
    let mut facet_groups: Vec<Vec<String>> = Vec::new();

    if let Some(loaders) = &params.loaders {
        let group: Vec<String> = loaders
            .iter()
            .filter(|l| !l.is_empty())
            .map(|l| format!("categories:{}", l))
            .collect();
        if !group.is_empty() {
            facet_groups.push(group);
        }
    }

    // Each selected category becomes its own single-element group so that
    // multiple categories combine with AND (mod must have all of them).
    if let Some(cats) = &params.categories {
        for c in cats.iter().filter(|c| !c.is_empty()) {
            facet_groups.push(vec![format!("categories:{}", c)]);
        }
    }

    if let Some(versions) = &params.game_versions {
        let group: Vec<String> = versions
            .iter()
            .filter(|v| !v.is_empty())
            .map(|v| format!("versions:{}", v))
            .collect();
        if !group.is_empty() {
            facet_groups.push(group);
        }
    }

    // Restrict by project type (mod, shader, resourcepack, modpack, datapack).
    let ptype = params.project_type.as_deref().unwrap_or("mod");
    facet_groups.push(vec![format!("project_type:{}", ptype)]);

    let facets_json = serde_json::to_string(&facet_groups).unwrap_or_else(|_| "[]".to_string());

    let url = format!(
        "https://api.modrinth.com/v2/search?query={query}&limit={limit}&offset={offset}&index={index}&facets={facets}",
        query = urlencoding::encode(q),
        limit = limit,
        offset = offset,
        index = sort.as_str(),
        facets = urlencoding::encode(&facets_json),
    );

    let client = reqwest::Client::builder()
        .user_agent("AgoraLauncher/1.0")
        .build()
        .map_err(|e| LauncherError::Generic {
            code: "ERR_NETWORK".to_string(),
            message: format!("Failed to build HTTP client: {e}"),
        })?;

    let resp: ModrinthSearchResponse = client
        .get(&url)
        .send()
        .await
        .map_err(|_| LauncherError::NetworkOffline)?
        .error_for_status()
        .map_err(|e| LauncherError::Generic {
            code: "ERR_NETWORK".to_string(),
            message: format!("Modrinth search request failed: {e}"),
        })?
        .json()
        .await
        .map_err(|_| LauncherError::Generic {
            code: "ERR_NETWORK".to_string(),
            message: "Failed to parse Modrinth search response.".to_string(),
        })?;

    let total_hits = resp.total_hits;
    let returned_offset = resp.offset;
    let returned_limit = resp.limit;

    let results = resp
        .hits
        .into_iter()
        .map(|h| ModrinthSearchResult {
            project_id: h.project_id,
            slug: h.slug.unwrap_or_default(),
            title: h.title,
            description: h.description.unwrap_or_default(),
            icon_url: h.icon_url,
            author: h.author.unwrap_or_default(),
            categories: h.categories.unwrap_or_default(),
            downloads: h.downloads.unwrap_or(0),
            follows: h.follows.unwrap_or(0),
            project_type: h.project_type.unwrap_or_else(|| "mod".to_string()),
            date_created: h.date_created,
            date_modified: h.date_modified,
            versions: h.versions.unwrap_or_default(),
            license: h.license,
        })
        .collect();

    Ok(ModrinthSearchPage {
        results,
        total_hits,
        offset: returned_offset,
        limit: returned_limit,
    })
}

/// Fetch the full list of Modrinth category tags (for filter UI).
pub async fn list_modrinth_categories(
    app: &tauri::AppHandle,
) -> LauncherResult<Vec<ModrinthCategoryInfo>> {
    require_modrinth_enabled(app)?;
    modrinth_get_json::<Vec<ModrinthCategoryTag>>("https://api.modrinth.com/v2/tag/category")
        .await
        .map(|tags| {
            tags.into_iter()
                .filter(|t| t.project_type == "mod")
                .map(|t| ModrinthCategoryInfo {
                    name: t.name,
                    project_type: t.project_type,
                    header: t.header,
                })
                .collect()
        })
}

/// Fetch the full list of Modrinth loader tags (for filter UI).
pub async fn list_modrinth_loaders(
    app: &tauri::AppHandle,
) -> LauncherResult<Vec<ModrinthLoaderInfo>> {
    require_modrinth_enabled(app)?;
    modrinth_get_json::<Vec<ModrinthLoaderTag>>("https://api.modrinth.com/v2/tag/loader")
        .await
        .map(|tags| {
            tags.into_iter()
                .filter(|t| t.supported_project_types.iter().any(|p| p == "mod"))
                .map(|t| ModrinthLoaderInfo {
                    name: t.name,
                    supported_project_types: t.supported_project_types,
                })
                .collect()
        })
}

/// Fetch the full list of Modrinth game version tags (for filter UI).
pub async fn list_modrinth_game_versions(
    app: &tauri::AppHandle,
) -> LauncherResult<Vec<ModrinthGameVersionInfo>> {
    require_modrinth_enabled(app)?;
    modrinth_get_json::<Vec<ModrinthGameVersionTag>>("https://api.modrinth.com/v2/tag/game_version")
        .await
        .map(|tags| {
            tags.into_iter()
                .map(|t| ModrinthGameVersionInfo {
                    version: t.version,
                    version_type: t.version_type,
                    date: t.date,
                    major: t.major,
                })
                .collect()
        })
}

/// Internal: GET a JSON endpoint from the Modrinth v2 API with the standard
/// user agent and the project's error mapping.
async fn modrinth_get_json<T: serde::de::DeserializeOwned>(url: &str) -> LauncherResult<T> {
    let client = reqwest::Client::builder()
        .user_agent("AgoraLauncher/1.0")
        .build()
        .map_err(|e| LauncherError::Generic {
            code: "ERR_NETWORK".to_string(),
            message: format!("Failed to build HTTP client: {e}"),
        })?;

    client
        .get(url)
        .send()
        .await
        .map_err(|_| LauncherError::NetworkOffline)?
        .error_for_status()
        .map_err(|e| LauncherError::Generic {
            code: "ERR_NETWORK".to_string(),
            message: format!("Modrinth request failed: {e}"),
        })?
        .json::<T>()
        .await
        .map_err(|_| LauncherError::Generic {
            code: "ERR_NETWORK".to_string(),
            message: "Failed to parse Modrinth response.".to_string(),
        })
}

/// Fetch a single Modrinth project's full details including the body (markdown
/// description) via `GET /v2/project/{id}`.
pub async fn fetch_project_full(
    app: &tauri::AppHandle,
    project_id: &str,
) -> LauncherResult<ModrinthProjectFull> {
    require_modrinth_enabled(app)?;

    let client = reqwest::Client::builder()
        .user_agent("AgoraLauncher/1.0")
        .build()
        .map_err(|e| LauncherError::Generic {
            code: "ERR_NETWORK".to_string(),
            message: format!("Failed to build HTTP client: {e}"),
        })?;

    let url = format!(
        "https://api.modrinth.com/v2/project/{pid}",
        pid = urlencoding::encode(project_id),
    );

    let resp: ModrinthProjectFullRaw = client
        .get(&url)
        .send()
        .await
        .map_err(|_| LauncherError::NetworkOffline)?
        .error_for_status()
        .map_err(|_| LauncherError::Generic {
            code: "ERR_MODRINTH_FETCH".to_string(),
            message: format!("Failed to fetch project '{project_id}' from Modrinth.").to_string(),
        })?
        .json()
        .await
        .map_err(|_| LauncherError::Generic {
            code: "ERR_MODRINTH_FETCH".to_string(),
            message: "Failed to parse Modrinth project response.".to_string(),
        })?;

    let page_url = resp
        .slug
        .as_ref()
        .map(|slug| format!("https://modrinth.com/{}/{}", resp.project_type, slug));

    Ok(ModrinthProjectFull {
        id: resp.id,
        title: resp.title,
        description: resp.description,
        body: resp.body,
        icon_url: resp.icon_url,
        project_type: resp.project_type,
        page_url,
        license_id: resp.license.and_then(|l| l.id),
        source_updated_at: resp.updated,
        gallery_urls: resp
            .gallery
            .unwrap_or_default()
            .into_iter()
            .filter_map(|g| g.url.filter(|u| u.starts_with("https://")))
            .collect(),
    })
}

/// Resolve Modrinth-published per-file metadata (URL + sha1 + sha512 + size)
/// for a single project by matching the given filename.
///
/// Iterates all versions returned by `GET /v2/project/{project_id}/version`
/// and returns the first file whose `filename` equals `filename`. Prefers
/// `primary` files. Returns `None` if no match is found or the API call fails
/// (callers fall back to bundling the jar locally, mirroring mrpack 1.x spec
/// behaviour for unresolvable files).
pub async fn resolve_modrinth_file_metadata(
    project_id: &str,
    filename: &str,
) -> Option<ModrinthFileMetadata> {
    let client = reqwest::Client::builder()
        .user_agent("AgoraLauncher/1.0")
        .build()
        .ok()?;

    let url = format!(
        "https://api.modrinth.com/v2/project/{pid}/version",
        pid = urlencoding::encode(project_id),
    );

    let versions: Vec<ModrinthVersionRaw> =
        client.get(&url).send().await.ok()?.json().await.ok()?;

    for version in &versions {
        // Prefer the primary file matching filename.
        if let Some(file) = version
            .files
            .iter()
            .find(|f| f.filename == filename && f.primary == Some(true))
        {
            if let Some(ref hashes) = file.hashes {
                let sha1 = hashes.sha1.as_deref().unwrap_or("").trim().to_lowercase();
                let sha512 = hashes.sha512.as_deref().unwrap_or("").trim().to_lowercase();
                if !sha1.is_empty() && !sha512.is_empty() {
                    return Some(ModrinthFileMetadata {
                        url: file.url.clone(),
                        sha1,
                        sha512,
                        size: file.size.unwrap_or(0),
                    });
                }
            }
        }
    }

    // Fallback: first non-primary match.
    for version in &versions {
        if let Some(file) = version.files.iter().find(|f| f.filename == filename) {
            if let Some(ref hashes) = file.hashes {
                let sha1 = hashes.sha1.as_deref().unwrap_or("").trim().to_lowercase();
                let sha512 = hashes.sha512.as_deref().unwrap_or("").trim().to_lowercase();
                if !sha1.is_empty() && !sha512.is_empty() {
                    return Some(ModrinthFileMetadata {
                        url: file.url.clone(),
                        sha1,
                        sha512,
                        size: file.size.unwrap_or(0),
                    });
                }
            }
        }
    }

    None
}

/// List versions for a raw Modrinth project (by project ID or slug).
///
/// Optionally filters by the target instance's Minecraft version + loader
/// when an instance is supplied. Without an instance, all versions are
/// returned sorted newest-first.
pub async fn list_raw_modrinth_versions(
    app: &tauri::AppHandle,
    instance_id: Option<&str>,
    project_id: &str,
    project_type: Option<&str>,
) -> LauncherResult<Vec<RawModrinthVersionCandidate>> {
    require_modrinth_enabled(app)?;

    // If an instance is provided, scope the request to its MC version + loader
    // so the API does not return incompatible noise. Only include the loader
    // filter for mod-type content — shaders, resource packs, and data packs are
    // loader-agnostic and would return zero results if filtered by loader.
    let mut url = format!(
        "https://api.modrinth.com/v2/project/{pid}/version",
        pid = urlencoding::encode(project_id),
    );

    if let Some(iid) = instance_id {
        let inst = load_instance_info(app, iid)?;
        let gv = serde_json::to_string(&[inst.minecraft_version.as_str()])
            .unwrap_or_else(|_| "[]".to_string());
        url.push_str("?game_versions=");
        url.push_str(&urlencoding::encode(&gv));
        let pt = project_type.unwrap_or("mod");
        if pt == "mod" || pt == "modpack" {
            let lv =
                serde_json::to_string(&[inst.loader.as_str()]).unwrap_or_else(|_| "[]".to_string());
            url.push_str("&loaders=");
            url.push_str(&urlencoding::encode(&lv));
        }
    }

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
            message: format!("Modrinth version request failed: {e}"),
        })?
        .json()
        .await
        .map_err(|_| LauncherError::Generic {
            code: "ERR_NETWORK".to_string(),
            message: "Failed to parse Modrinth version response.".to_string(),
        })?;

    Ok(versions
        .into_iter()
        .map(|v| {
            let primary_file = v
                .files
                .iter()
                .find(|f| f.primary)
                .or_else(|| v.files.first());
            let (filename, download_url, sha1, sha512, size) = match primary_file {
                Some(f) => (
                    f.filename.clone(),
                    f.url.clone(),
                    f.hashes.as_ref().and_then(|h| h.sha1.clone()),
                    f.hashes.as_ref().and_then(|h| h.sha512.clone()),
                    f.size,
                ),
                None => (String::new(), String::new(), None, None, None),
            };
            RawModrinthVersionCandidate {
                version: v.version_number,
                version_id: v.id,
                name: v.name.unwrap_or_default(),
                filename,
                download_url,
                sha1,
                sha512,
                size,
                dependencies: v
                    .dependencies
                    .into_iter()
                    .map(|dependency| RawModrinthDependency {
                        project_id: dependency.project_id,
                        version_id: dependency.version_id,
                        dependency_type: dependency.dependency_type,
                    })
                    .collect(),
                mc_versions: v.game_versions.unwrap_or_default(),
                loaders: v
                    .loaders
                    .unwrap_or_default()
                    .into_iter()
                    .map(|l| l.to_lowercase())
                    .collect(),
                release_date: v.date_published,
                primary: primary_file.map(|f| f.primary).unwrap_or(false),
                changelog: v.changelog,
            }
        })
        .filter(|c| !c.download_url.is_empty())
        .collect())
}

/// Install a raw (uncurated) Modrinth mod file into an instance.
///
/// Downloads the file from Modrinth's CDN (host-allowlist enforced by
/// `download_mod_bytes`) and verifies the SHA-1 hash published by the
/// Modrinth API before writing it to the appropriate instance directory
/// based on `project_type` (mods → `mods/`, shaders → `shaderpacks/`,
/// resourcepacks → `resourcepacks/`, datapacks → `datapacks/`).
/// The manifest entry uses `source: "modrinth_raw"` per the spec (§6.3 / §6.5).
pub async fn install_raw_modrinth(
    app: &tauri::AppHandle,
    instance_id: &str,
    project_id: &str,
    candidate: &RawModrinthVersionCandidate,
    project_type: &str,
) -> LauncherResult<InstalledMod> {
    require_modrinth_enabled(app)?;

    // Modpacks must use the pack import flow, not single-file install.
    if project_type == "modpack" {
        return Err(LauncherError::Generic {
            code: "ERR_USE_PACK_IMPORT".to_string(),
            message: "Modpacks must be imported via the pack import flow, not installed as a single file."
                .to_string(),
        });
    }

    // Modrinth's API must have published a SHA-1 hash for this file. Without
    // it we cannot integrity-check the download, so we refuse to install.
    let expected_sha1 = match candidate.sha1.as_deref() {
        Some(h) if !h.trim().is_empty() => h.trim().to_lowercase(),
        _ => {
            return Err(LauncherError::Generic {
                code: "ERR_HASH_UNAVAILABLE".to_string(),
                message: "Modrinth did not publish a SHA-1 hash for this file. Install refused for integrity safety."
                    .to_string(),
            });
        }
    };

    // Pre-check free disk space (§7.1.2).
    let instance_dir =
        paths::instance_dir(app, instance_id).map_err(|_| LauncherError::InstanceCreateFailed)?;
    if let Some(free) = available_disk_space_bytes(&instance_dir) {
        if free < MIN_DISK_SPACE_BYTES {
            return Err(LauncherError::DiskFull);
        }
    }

    // Download from Modrinth CDN (allowlist + redirect-safe policy enforced).
    let bytes = download_mod_bytes(&candidate.download_url).await?;

    // Verify SHA-1 against the Modrinth-published hash.
    let actual_sha1 = sha1_hex(&bytes);
    if actual_sha1 != expected_sha1 {
        return Err(LauncherError::HashMismatch);
    }

    // Route to the correct instance subdirectory based on project type.
    let target_subdir = match project_type {
        "shader" => "shaderpacks",
        "resourcepack" => "resourcepacks",
        "datapack" => "datapacks",
        _ => "mods",
    };
    let target_dir = instance_dir.join(target_subdir);
    std::fs::create_dir_all(&target_dir).map_err(|_| LauncherError::InstanceCreateFailed)?;
    let mod_path = target_dir.join(&candidate.filename);
    std::fs::write(&mod_path, &bytes).map_err(|_| LauncherError::InstanceCreateFailed)?;

    // Update the instance manifest atomically.
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

    // We store SHA-256 of the bytes in the manifest (consistent with the rest
    // of the manifest schema which uses sha256 for all entries), while the
    // install was verified against Modrinth's SHA-1.
    let sha256 = crate::download::sha256_hex(&bytes);
    let metadata = agora_core::jar_metadata::parse_jar_metadata(&mod_path);
    let installed_mod = InstalledMod {
        filename: candidate.filename.clone(),
        registry_id: None,
        modrinth_id: Some(project_id.to_string()),
        source: "modrinth_raw".to_string(),
        source_url: Some(candidate.download_url.clone()),
        version: Some(candidate.version.clone()),
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
        content_type: if project_type.is_empty() || project_type == "modpack" {
            "mod".to_string()
        } else {
            project_type.to_string()
        },
    };

    crate::mod_install::push_to_content_array(&mut manifest, &installed_mod);

    let tmp_path = manifest_path.with_extension("json.tmp");
    let text =
        serde_json::to_string_pretty(&manifest).map_err(|_| LauncherError::InstanceCreateFailed)?;
    std::fs::write(&tmp_path, text).map_err(|_| LauncherError::InstanceCreateFailed)?;
    std::fs::rename(&tmp_path, &manifest_path).map_err(|_| LauncherError::InstanceCreateFailed)?;

    Ok(installed_mod)
}

/// Resolve instance metadata for raw-modrinth version scoping.
fn load_instance_info(app: &tauri::AppHandle, instance_id: &str) -> LauncherResult<InstanceRow> {
    let conn = db::local_state_connection(app).map_err(|_| LauncherError::LocalStateFailed)?;
    db::get_instance(&conn, instance_id)
        .map_err(|_| LauncherError::LocalStateFailed)?
        .ok_or_else(|| LauncherError::Generic {
            code: "ERR_INSTANCE_NOT_FOUND".to_string(),
            message: format!("Instance '{instance_id}' not found."),
        })
}

/// Lowercase SHA-1 hex digest of a byte slice.
fn sha1_hex(data: &[u8]) -> String {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}
