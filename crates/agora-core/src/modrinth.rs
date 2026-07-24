//! Raw (uncurated) Modrinth API integration (Â§6.3).
//!
//! Pure HTTP functions that talk to the Modrinth v2 API.
//! All functions take a `&rusqlite::Connection` for settings checks
//! (e.g. `modrinth_enabled`) so the core crate stays free of `tauri` types.
//!
//! Install functions that write to an instance directory live in the
//! desktop crate's `modrinth_raw` shim, which resolves `AppHandle` â†’ paths
//! and delegates to these core functions.

use crate::ctx::Ctx;
use crate::db;
use crate::error::{LauncherError, LauncherResult};
use crate::http_client::{self, ClientCategory, HttpClients};
use crate::install_service::InstallService;
use crate::instance_service::InstanceService;
use crate::models::{InstalledMod, InstanceManifest, InstanceRow};

use serde::{Deserialize, Serialize};
use std::path::Path;

// --- Modrinth project full-details types ---

#[derive(Debug, Deserialize)]
pub(crate) struct ModrinthProjectFullRaw {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) body: Option<String>,
    pub(crate) icon_url: Option<String>,
    pub(crate) slug: Option<String>,
    pub(crate) project_type: String,
    pub(crate) license: Option<ModrinthLicenseRaw>,
    pub(crate) updated: Option<String>,
    #[serde(default)]
    pub(crate) gallery: Option<Vec<ModrinthGalleryImageRaw>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ModrinthLicenseRaw {
    pub(crate) id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ModrinthGalleryImageRaw {
    pub(crate) url: Option<String>,
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

/// Enforce the Modrinth-enabled gate; returns `Err(ModrinthDisabled)` when off.
fn require_modrinth_enabled(conn: &rusqlite::Connection) -> LauncherResult<()> {
    match db::get_setting(conn, "modrinth_enabled") {
        Ok(Some(v)) if v == true => Ok(()),
        _ => Err(LauncherError::ModrinthDisabled),
    }
}

// --- Modrinth v2 search response types ---

#[derive(Debug, Deserialize)]
pub(crate) struct ModrinthSearchResponse {
    pub(crate) hits: Vec<ModrinthSearchHit>,
    #[serde(default)]
    total_hits: u64,
    #[serde(default)]
    offset: u64,
    #[serde(default)]
    limit: u64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ModrinthSearchHit {
    pub(crate) project_id: String,
    pub(crate) slug: Option<String>,
    pub(crate) title: String,
    pub(crate) description: Option<String>,
    pub(crate) icon_url: Option<String>,
    // Modrinth search returns author/categories as arrays.
    pub(crate) author: Option<String>,
    pub(crate) categories: Option<Vec<String>>,
    pub(crate) downloads: Option<i64>,
    pub(crate) project_type: Option<String>,
    pub(crate) follows: Option<i64>,
    pub(crate) date_created: Option<String>,
    pub(crate) date_modified: Option<String>,
    pub(crate) versions: Option<Vec<String>>,
    pub(crate) license: Option<String>,
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
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModrinthSort {
    #[default]
    Relevance,
    Downloads,
    Follows,
    Newest,
    Updated,
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
    /// When None the facet is omitted, returning results from all project types.
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
pub(crate) struct ModrinthVersion {
    pub(crate) id: String,
    pub(crate) name: Option<String>,
    pub(crate) version_number: String,
    pub(crate) date_published: Option<String>,
    pub(crate) game_versions: Option<Vec<String>>,
    pub(crate) loaders: Option<Vec<String>>,
    pub(crate) files: Vec<ModrinthVersionFile>,
    #[serde(default)]
    pub(crate) dependencies: Vec<ModrinthApiDep>,
    #[serde(default)]
    pub(crate) changelog: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ModrinthVersionFile {
    pub(crate) url: String,
    pub(crate) filename: String,
    pub(crate) primary: bool,
    /// Modrinth publishes both sha1 and sha512 hashes for each file.
    /// Per Â§6.3 the launcher verifies against the SHA-1 hash published by
    /// Modrinth's API.
    pub(crate) hashes: Option<ModrinthFileHashes>,
    #[serde(default)]
    pub(crate) size: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ModrinthFileHashes {
    pub(crate) sha1: Option<String>,
    /// Modrinth also publishes sha512; retained for documentation / future use.
    #[allow(dead_code)]
    pub(crate) sha512: Option<String>,
}

// --- Private API types for resolve_modrinth_file_metadata (self-contained) ---

#[derive(Debug, Deserialize)]
pub(crate) struct ModrinthVersionFileRaw {
    url: String,
    filename: String,
    primary: Option<bool>,
    size: Option<u64>,
    hashes: Option<ModrinthFileHashesRaw>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ModrinthFileHashesRaw {
    sha1: Option<String>,
    sha512: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ModrinthVersionRaw {
    #[serde(default)]
    files: Vec<ModrinthVersionFileRaw>,
}

/// Internal API dependency type (snake_case, matching Modrinth v2 API).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ModrinthApiDep {
    project_id: Option<String>,
    version_id: Option<String>,
    dependency_type: String,
}

/// A dependency declared in a raw Modrinth version.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawModrinthDependency {
    pub project_id: Option<String>,
    pub version_id: Option<String>,
    pub dependency_type: String,
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
///   - multiple categories â†’ AND (the mod must have every category)
///   - multiple loaders    â†’ OR  (mod supports any selected loader)
///   - multiple game versions â†’ OR
///   - If `project_type` is set it is applied as a facet; when `None` the facet is omitted (all project types).
///
/// Returns a [`ModrinthSearchPage`] including `total_hits` so the frontend
/// can drive infinite scroll / "load more" paging.
pub async fn search_modrinth(
    conn: &rusqlite::Connection,
    params: &ModrinthSearchParams,
) -> LauncherResult<ModrinthSearchPage> {
    // Sync DB checks â€” connection only needed here
    {
        if !db::is_network_enabled(conn, "network_modrinth_enabled") {
            return Err(LauncherError::Generic {
                code: "ERR_NETWORK_DISABLED".into(),
                message: "Modrinth catalog API is disabled in Privacy settings.".into(),
            });
        }
        require_modrinth_enabled(conn)?;
    }
    // Proceed with async HTTP â€” connection no longer borrowed
    search_modrinth_http(params).await
}

/// Async HTTP-only search â€” no DB connection needed. Callers that already
/// validated the modrinth_enabled + network_enabled settings can call this
/// directly to avoid holding a `!Send` Connection across `.await` points.
pub async fn search_modrinth_http(
    params: &ModrinthSearchParams,
) -> LauncherResult<ModrinthSearchPage> {
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
    // When None, omit the facet so Modrinth returns all project types.
    if let Some(pt) = &params.project_type {
        facet_groups.push(vec![format!("project_type:{}", pt)]);
    }

    let facets_json = serde_json::to_string(&facet_groups).unwrap_or_else(|_| "[]".to_string());

    let url = format!(
        "https://api.modrinth.com/v2/search?query={query}&limit={limit}&offset={offset}&index={index}&facets={facets}",
        query = urlencoding::encode(q),
        limit = limit,
        offset = offset,
        index = sort.as_str(),
        facets = urlencoding::encode(&facets_json),
    );

    let clients = HttpClients::new()?;
    let resp: ModrinthSearchResponse =
        http_client::checked_get_json(&clients, ClientCategory::Modrinth, &url).await?;

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
    conn: &rusqlite::Connection,
) -> LauncherResult<Vec<ModrinthCategoryInfo>> {
    if !db::is_network_enabled(conn, "network_modrinth_enabled") {
        return Err(LauncherError::Generic {
            code: "ERR_NETWORK_DISABLED".into(),
            message: "Modrinth catalog API is disabled in Privacy settings.".into(),
        });
    }
    require_modrinth_enabled(conn)?;
    modrinth_get_json::<Vec<ModrinthCategoryTag>>("https://api.modrinth.com/v2/tag/category")
        .await
        .map(|tags| {
            tags.into_iter()
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
    conn: &rusqlite::Connection,
) -> LauncherResult<Vec<ModrinthLoaderInfo>> {
    if !db::is_network_enabled(conn, "network_modrinth_enabled") {
        return Err(LauncherError::Generic {
            code: "ERR_NETWORK_DISABLED".into(),
            message: "Modrinth catalog API is disabled in Privacy settings.".into(),
        });
    }
    require_modrinth_enabled(conn)?;
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
    conn: &rusqlite::Connection,
) -> LauncherResult<Vec<ModrinthGameVersionInfo>> {
    if !db::is_network_enabled(conn, "network_modrinth_enabled") {
        return Err(LauncherError::Generic {
            code: "ERR_NETWORK_DISABLED".into(),
            message: "Modrinth catalog API is disabled in Privacy settings.".into(),
        });
    }
    require_modrinth_enabled(conn)?;
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
    let clients = HttpClients::new()?;
    http_client::checked_get_json(&clients, ClientCategory::Modrinth, url).await
}

/// Internal: GET a JSON endpoint using a caller-provided client.
/// Used by the catalog source to share the host's HTTP client.
pub(crate) async fn modrinth_get_json_with_client<T: serde::de::DeserializeOwned>(
    clients: &HttpClients,
    url: &str,
) -> LauncherResult<T> {
    http_client::checked_get_json(clients, ClientCategory::Modrinth, url).await
}

/// Fetch a single Modrinth project's full details including the body (markdown
/// description) via `GET /v2/project/{id}`.
pub async fn fetch_project_full(
    conn: &rusqlite::Connection,
    project_id: &str,
) -> LauncherResult<ModrinthProjectFull> {
    if !db::is_network_enabled(conn, "network_modrinth_enabled") {
        return Err(LauncherError::Generic {
            code: "ERR_NETWORK_DISABLED".into(),
            message: "Modrinth catalog API is disabled in Privacy settings.".into(),
        });
    }
    require_modrinth_enabled(conn)?;

    let url = format!(
        "https://api.modrinth.com/v2/project/{pid}",
        pid = urlencoding::encode(project_id),
    );

    let clients = HttpClients::new()?;
    let resp: ModrinthProjectFullRaw =
        http_client::checked_get_json(&clients, ClientCategory::Modrinth, &url).await?;

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
    let clients = HttpClients::new().ok()?;

    let url = format!(
        "https://api.modrinth.com/v2/project/{pid}/version",
        pid = urlencoding::encode(project_id),
    );

    let versions: Vec<ModrinthVersionRaw> =
        http_client::checked_get_json(&clients, ClientCategory::Modrinth, &url)
            .await
            .ok()?;

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

/// Internal HTTP-only version of list_raw_modrinth_versions.
///
/// Same as the public [`list_raw_modrinth_versions`] but without the DB connection
/// parameter. Callers must validate `network_modrinth_enabled` and
/// `modrinth_enabled` before calling this function.
pub(crate) async fn list_raw_modrinth_versions_http(
    instance: Option<&InstanceRow>,
    project_id: &str,
    project_type: Option<&str>,
) -> LauncherResult<Vec<RawModrinthVersionCandidate>> {
    let mut url = format!(
        "https://api.modrinth.com/v2/project/{pid}/version",
        pid = urlencoding::encode(project_id),
    );

    if let Some(inst) = instance {
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

    let clients = HttpClients::new()?;
    let versions: Vec<ModrinthVersion> =
        http_client::checked_get_json(&clients, ClientCategory::Modrinth, &url).await?;

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
                    .map(|d| RawModrinthDependency {
                        project_id: d.project_id,
                        version_id: d.version_id,
                        dependency_type: d.dependency_type,
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
/// based on `project_type` (mods â†’ `mods/`, shaders â†’ `shaderpacks/`,
/// resourcepacks â†’ `resourcepacks/`, datapacks â†’ `datapacks/`).
/// The manifest entry uses `source: "modrinth_raw"` per the spec (Â§6.3 / Â§6.5).
///
/// Parameters:
/// - `instance_dir` â€” the instance root directory (e.g. `instances/my-instance`)
/// - `instance_id` â€” the instance ID for manifest path resolution
/// - `project_id` â€” the Modrinth project ID
/// - `candidate` â€” version candidate with download URL and SHA-1
/// - `project_type` â€” "mod", "shader", "resourcepack", or "datapack"
/// - `download_mod_bytes` â€” async closure to download bytes from a URL
/// - `available_disk_space_bytes` â€” sync closure to check free disk space
/// - `parse_jar_metadata` â€” sync closure to extract JAR metadata for the manifest
/// - `app_data_dir` â€” the app data directory (for manifest path resolution)
#[allow(clippy::too_many_arguments)]
pub async fn install_raw_modrinth(
    instance_dir: &Path,
    instance_id: &str,
    project_id: &str,
    candidate: &RawModrinthVersionCandidate,
    project_type: &str,
    download_mod_bytes: impl Fn(
            &str,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = LauncherResult<Vec<u8>>> + Send>>
        + Send
        + Sync,
    available_disk_space_bytes: impl Fn(&std::path::Path) -> Option<u64> + Send + Sync,
    parse_jar_metadata: impl Fn(&std::path::Path) -> crate::dependency_ops::JarDeps + Send + Sync,
    app_data_dir: &Path,
) -> LauncherResult<InstalledMod> {
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

    // Pre-check free disk space (Â§7.1.2).
    if let Some(free) = available_disk_space_bytes(instance_dir) {
        const MIN_DISK_SPACE_BYTES: u64 = 500_000_000;
        if free < MIN_DISK_SPACE_BYTES {
            return Err(LauncherError::DiskFull);
        }
    }

    // Download from Modrinth CDN (allowlist + redirect-safe policy enforced).
    let bytes = download_mod_bytes(&candidate.download_url).await?;

    // Verify SHA-1 against the Modrinth-published hash.
    let actual_sha1 = crate::download::sha1_hex(&bytes);
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
    let manifest_path = crate::paths::instance_manifest_path(app_data_dir, instance_id)
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
    let metadata = parse_jar_metadata(&mod_path);
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
        content_type: "mod".to_string(),
    };

    manifest.mods.push(installed_mod.clone());

    let tmp_path = manifest_path.with_extension("json.tmp");
    let text =
        serde_json::to_string_pretty(&manifest).map_err(|_| LauncherError::InstanceCreateFailed)?;
    std::fs::write(&tmp_path, text).map_err(|_| LauncherError::InstanceCreateFailed)?;
    std::fs::rename(&tmp_path, &manifest_path).map_err(|_| LauncherError::InstanceCreateFailed)?;

    Ok(installed_mod)
}

// ---------------------------------------------------------------------------
// ModrinthService — setting-gated facade
// ---------------------------------------------------------------------------

/// Core-owned service that gates every Modrinth API call behind the
/// `modrinth_enabled` toggle and the `network_modrinth_enabled` privacy
/// setting.
///
/// Desktop and CLI adapters should use this service rather than calling
/// the free functions directly, to avoid duplicating settings checks or
/// building Modrinth HTTP requests on their own.
#[derive(Clone)]
pub struct ModrinthService {
    ctx: Ctx,
}

impl ModrinthService {
    pub fn new(ctx: Ctx) -> Self {
        Self { ctx }
    }

    fn connection(&self) -> LauncherResult<rusqlite::Connection> {
        crate::db::local_state_connection(&self.ctx.paths.local_state_db()).map_err(|e| {
            LauncherError::Generic {
                code: "ERR_LOCAL_STATE_FAILED".into(),
                message: e.to_string(),
            }
        })
    }

    /// Check both the `network_modrinth_enabled` privacy setting and the
    /// `modrinth_enabled` feature toggle. Returns `Err(ModrinthDisabled)` when
    /// the toggle is off or the DB cannot be read (secure default: closed).
    pub fn check_enabled(&self) -> LauncherResult<()> {
        let conn = self.connection()?;
        if !db::is_network_enabled(&conn, "network_modrinth_enabled") {
            return Err(LauncherError::Generic {
                code: "ERR_NETWORK_DISABLED".into(),
                message: "Modrinth catalog API is disabled in Privacy settings.".into(),
            });
        }
        require_modrinth_enabled(&conn)
    }

    /// Whether the `modrinth_enabled` toggle is currently on.
    /// Returns `false` on any read failure (secure default: closed).
    pub fn is_modrinth_enabled(&self) -> bool {
        self.connection()
            .ok()
            .and_then(|conn| {
                db::get_setting(&conn, "modrinth_enabled")
                    .ok()
                    .flatten()
                    .and_then(|v| {
                        if v.as_bool() == Some(true) {
                            Some(true)
                        } else {
                            None
                        }
                    })
            })
            .unwrap_or(false)
    }

    /// Live Modrinth search (see [`search_modrinth_http`]).
    pub async fn search_modrinth(
        &self,
        params: &ModrinthSearchParams,
    ) -> LauncherResult<ModrinthSearchPage> {
        self.check_enabled()?;
        search_modrinth_http(params).await
    }

    /// List Modrinth category tags.
    pub async fn list_modrinth_categories(&self) -> LauncherResult<Vec<ModrinthCategoryInfo>> {
        self.check_enabled()?;
        modrinth_get_json::<Vec<ModrinthCategoryTag>>("https://api.modrinth.com/v2/tag/category")
            .await
            .map(|tags| {
                tags.into_iter()
                    .map(|t| ModrinthCategoryInfo {
                        name: t.name,
                        project_type: t.project_type,
                        header: t.header,
                    })
                    .collect()
            })
    }

    /// List Modrinth loader tags.
    pub async fn list_modrinth_loaders(&self) -> LauncherResult<Vec<ModrinthLoaderInfo>> {
        self.check_enabled()?;
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

    /// List Modrinth game version tags.
    pub async fn list_modrinth_game_versions(
        &self,
    ) -> LauncherResult<Vec<ModrinthGameVersionInfo>> {
        self.check_enabled()?;
        modrinth_get_json::<Vec<ModrinthGameVersionTag>>(
            "https://api.modrinth.com/v2/tag/game_version",
        )
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

    /// Fetch full project details.
    pub async fn fetch_project_full(
        &self,
        project_id: &str,
    ) -> LauncherResult<ModrinthProjectFull> {
        self.check_enabled()?;

        let url = format!(
            "https://api.modrinth.com/v2/project/{pid}",
            pid = urlencoding::encode(project_id),
        );

        let clients = HttpClients::new()?;
        let resp: ModrinthProjectFullRaw =
            http_client::checked_get_json(&clients, ClientCategory::Modrinth, &url).await?;

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

    /// Resolve Modrinth-published per-file metadata (URL + sha1 + sha512 + size).
    pub async fn resolve_modrinth_file_metadata(
        &self,
        project_id: &str,
        filename: &str,
    ) -> Option<ModrinthFileMetadata> {
        resolve_modrinth_file_metadata(project_id, filename).await
    }

    /// List versions for a raw Modrinth project, optionally scoped to an
    /// instance's MC version and loader.
    pub async fn list_raw_modrinth_versions(
        &self,
        instance_id: Option<&str>,
        project_id: &str,
        project_type: Option<&str>,
    ) -> LauncherResult<Vec<RawModrinthVersionCandidate>> {
        self.check_enabled()?;

        let instance = match instance_id {
            Some(iid) => {
                let svc = InstanceService::new(self.ctx.clone());
                Some(
                    svc.get(iid)?
                        .ok_or_else(|| LauncherError::Generic {
                            code: "ERR_INSTANCE_NOT_FOUND".to_string(),
                            message: format!("Instance '{iid}' not found."),
                        })?
                        .row,
                )
            }
            None => None,
        };

        list_raw_modrinth_versions_http(instance.as_ref(), project_id, project_type).await
    }

    /// Install a raw Modrinth mod file into an instance.
    ///
    /// Validates the gate, rejects modpack installs, requires SHA-1, then
    /// delegates to [`InstallService::install_artifact`].
    pub async fn install_raw_modrinth(
        &self,
        instance_id: &str,
        project_id: &str,
        candidate: &RawModrinthVersionCandidate,
        project_type: &str,
    ) -> LauncherResult<InstalledMod> {
        self.check_enabled()?;

        if project_type == "modpack" {
            return Err(LauncherError::Generic {
                code: "ERR_USE_PACK_IMPORT".to_string(),
                message: "Modpacks must be imported via the pack import flow, not installed as a single file."
                    .to_string(),
            });
        }

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

        let content_type = if project_type.is_empty() || project_type == "modpack" {
            "mod"
        } else {
            project_type
        };

        let svc = InstallService::new(self.ctx.clone());
        svc.install_artifact(
            instance_id,
            &candidate.filename,
            content_type,
            &candidate.download_url,
            None,
            Some(project_id),
            "modrinth_raw",
            Some(&candidate.version),
            Some(&expected_sha1),
            None,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_ctx() -> (Ctx, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "agora-modrinth-svc-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let ctx = Ctx::for_testing(root.clone());
        crate::db::init_local_state_db(&ctx.paths.local_state_db()).unwrap();
        let conn = crate::db::local_state_connection(&ctx.paths.local_state_db()).unwrap();
        // modrinth_enabled must be explicitly true (default is off)
        crate::db::set_setting(&conn, "modrinth_enabled", &serde_json::Value::Bool(true)).unwrap();
        // network_modrinth_enabled defaults to true; ensure it's on
        crate::db::set_setting(
            &conn,
            "network_modrinth_enabled",
            &serde_json::Value::Bool(true),
        )
        .unwrap();
        conn.close().unwrap();
        (ctx, root)
    }

    #[test]
    fn service_check_enabled_ok_when_both_on() {
        let (ctx, root) = temp_ctx();
        let svc = ModrinthService::new(ctx);
        assert!(svc.check_enabled().is_ok());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn service_check_enabled_fails_when_modrinth_disabled() {
        let root = std::env::temp_dir().join(format!(
            "agora-modrinth-disabled-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let ctx = Ctx::for_testing(root.clone());
        crate::db::init_local_state_db(&ctx.paths.local_state_db()).unwrap();
        // Never set modrinth_enabled -> defaults to off
        let svc = ModrinthService::new(ctx);
        assert!(svc.check_enabled().is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn service_check_enabled_fails_when_network_disabled() {
        let root = std::env::temp_dir().join(format!(
            "agora-modrinth-netoff-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let ctx = Ctx::for_testing(root.clone());
        crate::db::init_local_state_db(&ctx.paths.local_state_db()).unwrap();
        let conn = crate::db::local_state_connection(&ctx.paths.local_state_db()).unwrap();
        crate::db::set_setting(&conn, "modrinth_enabled", &serde_json::Value::Bool(true)).unwrap();
        crate::db::set_setting(
            &conn,
            "network_modrinth_enabled",
            &serde_json::Value::Bool(false),
        )
        .unwrap();
        conn.close().unwrap();
        let svc = ModrinthService::new(ctx);
        let err = svc.check_enabled().unwrap_err();
        assert_eq!(err.code(), "ERR_NETWORK_DISABLED");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn service_is_modrinth_enabled_returns_true() {
        let (ctx, root) = temp_ctx();
        let svc = ModrinthService::new(ctx);
        assert!(svc.is_modrinth_enabled());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn service_is_modrinth_enabled_returns_false_when_missing() {
        let root = std::env::temp_dir().join(format!(
            "agora-modrinth-off-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let ctx = Ctx::for_testing(root.clone());
        crate::db::init_local_state_db(&ctx.paths.local_state_db()).unwrap();
        let svc = ModrinthService::new(ctx);
        assert!(!svc.is_modrinth_enabled());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn raw_modrinth_dependency_serde_roundtrip() {
        let dep = RawModrinthDependency {
            project_id: Some("abc".into()),
            version_id: None,
            dependency_type: "required".into(),
        };
        let json = serde_json::to_string(&dep).unwrap();
        let parsed: RawModrinthDependency = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.project_id, Some("abc".into()));
        assert_eq!(parsed.version_id, None);
        assert_eq!(parsed.dependency_type, "required");
    }

    #[test]
    fn raw_modrinth_version_candidate_deserialize() {
        let json = r#"{
            "version": "1.0.0",
            "version_id": "v1",
            "name": "Test Mod",
            "filename": "test-mod.jar",
            "download_url": "https://cdn.modrinth.com/test.jar",
            "sha1": "da39a3ee5e6b4b0d3255bfef95601890afd80709",
            "sha512": "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e",
            "size": 12345,
            "dependencies": [{"projectId": "dep1", "dependencyType": "required"}],
            "mc_versions": ["1.21"],
            "loaders": ["fabric"],
            "release_date": "2024-01-01",
            "primary": true,
            "changelog": "Initial release"
        }"#;
        let candidate: RawModrinthVersionCandidate = serde_json::from_str(json).unwrap();
        assert_eq!(candidate.version, "1.0.0");
        assert_eq!(candidate.sha512.as_deref(), Some("cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"));
        assert_eq!(candidate.size, Some(12345));
        assert_eq!(candidate.dependencies.len(), 1);
        assert_eq!(
            candidate.dependencies[0].project_id.as_deref(),
            Some("dep1")
        );
    }
}
