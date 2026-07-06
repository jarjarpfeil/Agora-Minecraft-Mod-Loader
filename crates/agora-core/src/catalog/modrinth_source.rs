use std::path::Path;

use crate::catalog::source::{
    CatalogItem, CatalogSource, DepGraph, Hashes, ProjectRef, ReleaseType, SearchQuery, Version,
};
use crate::ctx::Ctx;
use crate::db;
use crate::download;
use crate::error::{LauncherError, LauncherResult};
use crate::modrinth;
use crate::paths;

/// A concrete [`CatalogSource`] backed by the live Modrinth API.
///
/// Wraps the existing [`modrinth`] module patterns and maps their
/// Modrinth-specific types into the unified catalog types.
///
/// Design: all DB work (settings checks) is done synchronously before
/// any `.await`. HTTP calls use `ctx.client` via
/// `modrinth_get_json_with_client` to avoid holding a
/// `rusqlite::Connection` (which is `!Sync`) across an async boundary.
pub struct ModrinthSource;

impl ModrinthSource {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ModrinthSource {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn open_conn(ctx: &Ctx) -> Result<rusqlite::Connection, LauncherError> {
    let path = paths::local_state_db_path(&ctx.app_data_dir).map_err(|_| LauncherError::LocalStateFailed)?;
    db::local_state_connection(&path).map_err(|_| LauncherError::LocalStateFailed)
}

fn check_modrinth_enabled(conn: &rusqlite::Connection) -> LauncherResult<()> {
    match db::get_setting(conn, "modrinth_enabled") {
        Ok(Some(v)) if v.as_bool() == Some(true) => Ok(()),
        _ => Err(LauncherError::ModrinthDisabled),
    }
}

/// Build the search URL from a `SearchQuery`.
fn build_search_url(q: &SearchQuery) -> String {
    let limit = q.limit.clamp(1, 100);
    let q_str = match q.query.as_deref() {
        Some(s) if !s.trim().is_empty() => s.trim(),
        _ => "*",
    };

    // Build facets: loaders map to categories facet, categories are AND,
    // game versions are OR.
    let mut facet_groups: Vec<Vec<String>> = Vec::new();

    if let Some(ref loader) = q.loader {
        if !loader.is_empty() {
            facet_groups.push(vec![format!("categories:{}", loader)]);
        }
    }

    for cat in &q.categories {
        if !cat.is_empty() {
            facet_groups.push(vec![format!("categories:{}", cat)]);
        }
    }

    if let Some(ref gv) = q.game_version {
        if !gv.is_empty() {
            facet_groups.push(vec![format!("versions:{}", gv)]);
        }
    }

    facet_groups.push(vec!["project_type:mod".to_string()]);

    let facets_json = serde_json::to_string(&facet_groups).unwrap_or_else(|_| "[]".to_string());

    format!(
        "https://api.modrinth.com/v2/search?query={}&limit={}&offset={}&index=relevance&facets={}",
        urlencoding::encode(q_str),
        limit,
        q.offset,
        urlencoding::encode(&facets_json),
    )
}

/// Build the project-full-details URL.
fn build_project_url(project_id: &str) -> String {
    format!(
        "https://api.modrinth.com/v2/project/{}",
        urlencoding::encode(project_id)
    )
}

/// Build the versions list URL, optionally filtered by MC version + loader.
fn build_versions_url(project_id: &str, game_version: Option<&str>, loader: Option<&str>) -> String {
    let mut url = format!(
        "https://api.modrinth.com/v2/project/{}/version",
        urlencoding::encode(project_id)
    );

    if let Some(gv) = game_version {
        let gv_json = serde_json::to_string(&[gv]).unwrap_or_else(|_| "[]".to_string());
        url.push_str("?game_versions=");
        url.push_str(&urlencoding::encode(&gv_json));
        if let Some(l) = loader {
            let l_json = serde_json::to_string(&[l]).unwrap_or_else(|_| "[]".to_string());
            url.push_str("&loaders=");
            url.push_str(&urlencoding::encode(&l_json));
        }
    }

    url
}

// ---------------------------------------------------------------------------
// Type mappings
// ---------------------------------------------------------------------------

fn search_result_to_item(r: modrinth::ModrinthSearchResult) -> CatalogItem {
    CatalogItem {
        project_ref: ProjectRef::Modrinth(r.project_id),
        name: r.title,
        slug: r.slug,
        author: r.author,
        description: r.description,
        icon_url: r.icon_url,
        downloads: r.downloads as u64,
        follows: Some(r.follows as u64),
        categories: r.categories,
        loader: None,
        game_versions: r.versions,
    }
}

fn project_full_to_item(project_id: &str, full: &modrinth::ModrinthProjectFull) -> CatalogItem {
    let slug = full
        .page_url
        .as_ref()
        .and_then(|url| url.split('/').last().map(|s| s.to_string()))
        .unwrap_or_default();

    CatalogItem {
        project_ref: ProjectRef::Modrinth(project_id.to_string()),
        name: full.title.clone(),
        slug,
        author: String::new(),
        description: full.description.clone(),
        icon_url: full.icon_url.clone(),
        downloads: 0,
        follows: None,
        categories: vec![],
        loader: None,
        game_versions: vec![],
    }
}

// ---------------------------------------------------------------------------
// CatalogSource impl
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl CatalogSource for ModrinthSource {
    fn name(&self) -> &str {
        "Modrinth"
    }

    fn is_enabled(&self, ctx: &Ctx) -> bool {
        match open_conn(ctx) {
            Ok(conn) => {
                match db::get_setting(&conn, "modrinth_enabled") {
                    Ok(Some(v)) => v.as_bool().unwrap_or(true),
                    _ => true,
                }
            }
            Err(_) => true,
        }
    }

    async fn search(
        &self,
        ctx: &Ctx,
        q: &SearchQuery,
    ) -> Result<Vec<CatalogItem>, LauncherError> {
        // Synchronous DB check — connection dropped before any .await.
        let conn = open_conn(ctx)?;
        check_modrinth_enabled(&conn)?;
        drop(conn);

        let url = build_search_url(q);
        let resp: modrinth::ModrinthSearchResponse =
            modrinth::modrinth_get_json_with_client(&ctx.client, &url).await?;

        Ok(resp.hits.into_iter().map(|h| modrinth::ModrinthSearchResult {
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
        }).map(search_result_to_item).collect())
    }

    async fn project(
        &self,
        ctx: &Ctx,
        id: &ProjectRef,
    ) -> Result<CatalogItem, LauncherError> {
        // Synchronous DB check.
        let conn = open_conn(ctx)?;
        check_modrinth_enabled(&conn)?;
        drop(conn);

        match id {
            ProjectRef::Modrinth(project_id) => {
                let url = build_project_url(project_id);
                let full: modrinth::ModrinthProjectFullRaw =
                    modrinth::modrinth_get_json_with_client(&ctx.client, &url).await?;

                let page_url = full.slug.as_ref().map(|slug| {
                    format!("https://modrinth.com/{}/{}", full.project_type, slug)
                });

                let mapped = modrinth::ModrinthProjectFull {
                    id: full.id,
                    title: full.title,
                    description: full.description,
                    body: full.body,
                    icon_url: full.icon_url,
                    page_url,
                    license_id: full.license.and_then(|l| l.id),
                    source_updated_at: full.updated,
                    gallery_urls: full
                        .gallery
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|g| g.url.filter(|u| u.starts_with("https://")))
                        .collect(),
                };

                Ok(project_full_to_item(&id.as_modrinth_id(), &mapped))
            }
            ProjectRef::Agora(slug) => {
                // Search by slug and return first match.
                let url = build_search_url(&SearchQuery {
                    query: Some(slug.clone()),
                    limit: 1,
                    ..Default::default()
                });
                let resp: modrinth::ModrinthSearchResponse =
                    modrinth::modrinth_get_json_with_client(&ctx.client, &url).await?;

                if let Some(hit) = resp.hits.into_iter().next() {
                    let result = modrinth::ModrinthSearchResult {
                        project_id: hit.project_id,
                        slug: hit.slug.unwrap_or_default(),
                        title: hit.title,
                        description: hit.description.unwrap_or_default(),
                        icon_url: hit.icon_url,
                        author: hit.author.unwrap_or_default(),
                        categories: hit.categories.unwrap_or_default(),
                        downloads: hit.downloads.unwrap_or(0),
                        follows: hit.follows.unwrap_or(0),
                        project_type: hit.project_type.unwrap_or_else(|| "mod".to_string()),
                        date_created: hit.date_created,
                        date_modified: hit.date_modified,
                        versions: hit.versions.unwrap_or_default(),
                        license: hit.license,
                    };
                    Ok(search_result_to_item(result))
                } else {
                    Err(LauncherError::VersionNotFound)
                }
            }
            ProjectRef::GithubRelease(_) => Err(LauncherError::VersionNotFound),
        }
    }

    async fn versions(
        &self,
        ctx: &Ctx,
        id: &ProjectRef,
    ) -> Result<Vec<Version>, LauncherError> {
        // Synchronous DB check.
        let conn = open_conn(ctx)?;
        check_modrinth_enabled(&conn)?;
        drop(conn);

        let project_id = match id {
            ProjectRef::Modrinth(pid) => pid.clone(),
            ProjectRef::Agora(slug) => slug.clone(),
            ProjectRef::GithubRelease(_) => return Err(LauncherError::VersionNotFound),
        };

        let url = build_versions_url(&project_id, None, None);
        let versions: Vec<modrinth::ModrinthVersion> =
            modrinth::modrinth_get_json_with_client(&ctx.client, &url).await?;

        Ok(versions
            .into_iter()
            .flat_map(|v| {
                let files = v.files.clone();
                files.into_iter().filter_map(move |f| {
                    let hashes = f.hashes.as_ref();
                    let sha1 = hashes.and_then(|h| h.sha1.clone()).unwrap_or_default();
                    let sha512 = hashes.and_then(|h| h.sha512.clone()).unwrap_or_default();
                    if sha1.is_empty() {
                        return None;
                    }
                    let release_type = if v.version_number.contains("-alpha") || v.version_number.contains("-a.") {
                        ReleaseType::Alpha
                    } else if v.version_number.contains("-beta") || v.version_number.contains("-b.") || v.version_number.contains("-rc") {
                        ReleaseType::Beta
                    } else {
                        ReleaseType::Release
                    };

                    Some(Version {
                        project_ref: id.clone(),
                        version_number: v.version_number.clone(),
                        name: v.name.clone().unwrap_or_default(),
                        filename: f.filename,
                        download_url: f.url,
                        hashes: Hashes {
                            sha1: if sha1.is_empty() { None } else { Some(sha1) },
                            sha512: if sha512.is_empty() { None } else { Some(sha512) },
                            sha256: None,
                        },
                        loaders: v.loaders.clone().unwrap_or_default().into_iter().map(|l| l.to_lowercase()).collect(),
                        game_versions: v.game_versions.clone().unwrap_or_default(),
                        release_type,
                        dependencies: vec![],
                    })
                })
            })
            .collect())
    }

    async fn resolve_dependencies(
        &self,
        _ctx: &Ctx,
        _v: &Version,
    ) -> Result<DepGraph, LauncherError> {
        // Modrinth's version API doesn't expose dependency metadata.
        // Dependency resolution is handled at a higher layer.
        Ok(DepGraph::default())
    }

    async fn download(
        &self,
        ctx: &Ctx,
        v: &Version,
        dest: &Path,
    ) -> Result<Hashes, LauncherError> {
        let url = &v.download_url;
        let resp = ctx
            .client
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

        let bytes = resp
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|_| LauncherError::NetworkOffline)?;

        // Verify SHA-1 first (Modrinth always publishes it).
        if let Some(ref expected_sha1) = v.hashes.sha1 {
            let actual_sha1 = download::sha1_hex(&bytes);
            if actual_sha1 != expected_sha1.to_lowercase() {
                return Err(LauncherError::HashMismatch);
            }
        }

        // Verify SHA-256 if available.
        if let Some(ref expected_sha256) = v.hashes.sha256 {
            let actual_sha256 = download::sha256_hex(&bytes);
            if actual_sha256 != expected_sha256.to_lowercase() {
                return Err(LauncherError::HashMismatch);
            }
        }

        // Write to destination.
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|_| LauncherError::InstanceCreateFailed)?;
        }
        std::fs::write(dest, &bytes).map_err(|_| LauncherError::InstanceCreateFailed)?;

        Ok(Hashes {
            sha1: Some(download::sha1_hex(&bytes)),
            sha256: Some(download::sha256_hex(&bytes)),
            sha512: v.hashes.sha512.clone(),
        })
    }

    async fn verify(&self, file: &Path, expected: &Hashes) -> Result<(), LauncherError> {
        let data = std::fs::read(file).map_err(|_| LauncherError::Generic {
            code: "ERR_FILE_READ".to_string(),
            message: format!("Failed to read file: {}", file.display()),
        })?;

        if let Some(ref expected_sha1) = expected.sha1 {
            let actual = download::sha1_hex(&data);
            if actual != expected_sha1.to_lowercase() {
                return Err(LauncherError::HashMismatch);
            }
        }

        if let Some(ref expected_sha256) = expected.sha256 {
            let actual = download::sha256_hex(&data);
            if actual != expected_sha256.to_lowercase() {
                return Err(LauncherError::HashMismatch);
            }
        }

        if let Some(ref expected_sha512) = expected.sha512 {
            use sha2::{Digest, Sha512};
            let mut hasher = Sha512::new();
            hasher.update(&data);
            let actual = hex::encode(hasher.finalize());
            if actual != expected_sha512.to_lowercase() {
                return Err(LauncherError::HashMismatch);
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helper trait for extracting project ID from ProjectRef
// ---------------------------------------------------------------------------

trait ProjectRefExt {
    fn as_modrinth_id(&self) -> String;
}

impl ProjectRefExt for ProjectRef {
    fn as_modrinth_id(&self) -> String {
        match self {
            ProjectRef::Modrinth(id) => id.clone(),
            _ => String::new(),
        }
    }
}
