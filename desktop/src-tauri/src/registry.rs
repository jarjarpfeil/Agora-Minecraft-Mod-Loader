use crate::error::{LauncherError, LauncherResult};
use crate::paths;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// A registry item row for browsing (§6.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryItem {
    pub id: String,
    pub name: String,
    pub content_type: String,
    pub download_strategy: String,
    pub source_identifier: String,
    pub sha256: String,
    pub upvotes: i64,
    pub downvotes: i64,
    pub net_score: i64,
    pub velocity: f64,
    pub status: String,
    pub is_immune: bool,
    pub immunity_reason: Option<String>,
    pub allow_comments: bool,
    pub icon_url: Option<String>,
    pub gallery_urls_json: Option<String>,
    pub date_added: Option<String>,
    pub compatible_versions_json: Option<String>,
    /// Short tagline hydrated from the upstream source (e.g. Modrinth).
    pub description: Option<String>,
    /// Full markdown body hydrated from the upstream source. Community-authored
    /// markdown — must be rendered without `dangerouslySetInnerHTML`.
    pub body_markdown: Option<String>,
    /// Canonical upstream page URL (e.g. https://modrinth.com/mod/<slug>).
    pub page_url: Option<String>,
    /// SPDX-ish license id hydrated from the upstream source.
    pub license_id: Option<String>,
    /// ISO timestamp of the upstream project's last update (nightly snapshot).
    pub source_updated_at: Option<String>,
    /// Optional Modrinth project id (github_release/direct_hash mods may carry
    /// one when the project also exists on Modrinth). Used as the
    /// version-resolution fallback when the primary source fails.
    pub modrinth_id: Option<String>,
}

/// Valid sort options for browsing (§6.2).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortOption {
    NetScore,
    Velocity,
    MostDownvoted,
    Newest,
    MostUpvoted,
}

impl Default for SortOption {
    fn default() -> Self {
        Self::NetScore
    }
}

impl SortOption {
    fn order_clause(&self) -> &'static str {
        match self {
            Self::NetScore => "ORDER BY net_score DESC",
            Self::Velocity => "ORDER BY velocity DESC",
            Self::MostDownvoted => "ORDER BY downvotes DESC",
            Self::Newest => "ORDER BY date_added DESC",
            Self::MostUpvoted => "ORDER BY upvotes DESC",
        }
    }
}

/// Open the cached registry.db read-only.
pub fn open_registry<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> LauncherResult<Connection> {
    let path = paths::registry_db_path(app).map_err(|_| LauncherError::RegistryMissing)?;
    if !path.exists() {
        return Err(LauncherError::RegistryMissing);
    }
    let conn = Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| LauncherError::RegistryMissing)?;
    conn.pragma_update(None, "query_only", "ON")
        .map_err(|_| LauncherError::RegistryMissing)?;
    Ok(conn)
}

/// Browse registry items with optional filters (§6.2).
///
/// When `modrinth_enabled` is false, modrinth-sourced items are excluded.
/// `mc_version` and `loader` filter against `compatible_versions_json` using
/// SQLite's JSON1 functions (same approach as the web `browseItems`).
pub fn browse_items(
    conn: &Connection,
    content_type: Option<&str>,
    category: Option<&str>,
    sort: &SortOption,
    modrinth_enabled: bool,
    mc_version: Option<&str>,
    loader: Option<&str>,
    limit: i64,
) -> LauncherResult<Vec<RegistryItem>> {
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    let mut where_parts: Vec<String> = Vec::new();

    if let Some(ct) = content_type {
        where_parts.push("ri.content_type = ?".to_string());
        params.push(Box::new(ct.to_string()));
    }
    if let Some(cat) = category {
        where_parts.push("ic.category_id = ?".to_string());
        params.push(Box::new(cat.to_string()));
    }
    if !modrinth_enabled {
        where_parts.push("ri.download_strategy != 'modrinth_id'".to_string());
    }
    if let Some(mv) = mc_version {
        // Only items declaring a compatible_version for this MC version match.
        // json_each over an empty/NULL array yields no rows -> the item is
        // excluded, which is the desired behaviour (no declared support).
        where_parts.push(
            "ri.id IN (SELECT r.id FROM registry_items r, json_each(r.compatible_versions_json) cv \
             WHERE json_extract(cv.value, '$.mc_version') = ?)"
                .to_string(),
        );
        params.push(Box::new(mv.to_string()));
    }
    if let Some(ld) = loader {
        if ld != "all" {
            where_parts.push(
                "ri.id IN (SELECT r.id FROM registry_items r, json_each(r.compatible_versions_json) cv \
                 WHERE json_extract(cv.value, '$.loader') = ?)"
                    .to_string(),
            );
            params.push(Box::new(ld.to_string()));
        }
    }

    let where_clause = if where_parts.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", where_parts.join(" AND "))
    };

    let join = if category.is_some() {
        " JOIN item_categories ic ON ri.id = ic.item_id"
    } else {
        ""
    };

    let sql = format!(
        "SELECT ri.id, ri.name, ri.content_type, ri.download_strategy,
                ri.source_identifier, ri.sha256, ri.upvotes, ri.downvotes,
                ri.net_score, ri.velocity, ri.status, ri.is_immune,
                ri.immunity_reason, ri.allow_comments, ri.icon_url,
                ri.gallery_urls_json, ri.date_added, ri.compatible_versions_json,
                ri.description, ri.body_markdown, ri.page_url, ri.license_id,
                ri.source_updated_at, ri.modrinth_id
         FROM registry_items ri{join}{where_clause} {order} LIMIT ?",
        join = join,
        where_clause = where_clause,
        order = sort.order_clause(),
    );

    params.push(Box::new(limit));

    let mut stmt = conn.prepare(&sql).map_err(|e| LauncherError::Generic {
        code: "ERR_INVALID_QUERY".to_string(),
        message: e.to_string(),
    })?;

    let rows = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), row_to_item)
        .map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?;

    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?);
    }
    Ok(out)
}

/// Fetch a single registry item by ID.
pub fn get_item_by_id(conn: &Connection, item_id: &str) -> LauncherResult<Option<RegistryItem>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, content_type, download_strategy,
                    source_identifier, sha256, upvotes, downvotes,
                    net_score, velocity, status, is_immune,
                    immunity_reason, allow_comments, icon_url,
                    gallery_urls_json, date_added, compatible_versions_json,
                    description, body_markdown, page_url, license_id,
                    source_updated_at, modrinth_id
             FROM registry_items WHERE id = ?1",
        )
        .map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?;

    let mut rows = stmt
        .query_map([item_id], row_to_item)
        .map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?;

    if let Some(r) = rows.next() {
        Ok(Some(r.map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?))
    } else {
        Ok(None)
    }
}

/// Category info for the browse filter UI.
#[derive(Debug, Clone, Serialize)]
pub struct CategoryInfo {
    pub id: String,
    pub display_name: String,
    pub is_community: bool,
}

/// List all categories from the registry.
pub fn list_categories(conn: &Connection) -> LauncherResult<Vec<CategoryInfo>> {
    let mut stmt = conn
        .prepare("SELECT id, display_name, is_community FROM categories ORDER BY display_name")
        .map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?;

    let rows = stmt
        .query_map([], |row| {
            Ok(CategoryInfo {
                id: row.get(0)?,
                display_name: row.get(1)?,
                is_community: row.get(2)?,
            })
        })
        .map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?;

    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?);
    }
    Ok(out)
}

/// A row from the `pack_mods` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackModRow {
    pub pack_id: String,
    pub mod_id: String,
    pub source: String,
    pub version: Option<String>,
    pub status: String,
    pub description: Option<String>,
}

/// List all mods in a pack, ordered by mod_id.
pub fn pack_mods_for_pack(
    conn: &Connection,
    pack_id: &str,
) -> LauncherResult<Vec<PackModRow>> {
    let mut stmt = conn
        .prepare(
            "SELECT pack_id, mod_id, source, version, status, description \
             FROM pack_mods WHERE pack_id = ?1 ORDER BY mod_id ASC",
        )
        .map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?;

    let rows = stmt
        .query_map([pack_id], |row| {
            Ok(PackModRow {
                pack_id: row.get(0)?,
                mod_id: row.get(1)?,
                source: row.get(2)?,
                version: row.get(3)?,
                status: row.get(4)?,
                description: row.get(5)?,
            })
        })
        .map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?;

    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?);
    }
    Ok(out)
}

fn row_to_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<RegistryItem> {
    Ok(RegistryItem {
        id: row.get(0)?,
        name: row.get(1)?,
        content_type: row.get(2)?,
        download_strategy: row.get(3)?,
        source_identifier: row.get(4)?,
        sha256: row.get(5)?,
        upvotes: row.get(6)?,
        downvotes: row.get(7)?,
        net_score: row.get(8)?,
        velocity: row.get(9)?,
        status: row.get(10)?,
        is_immune: row.get(11)?,
        immunity_reason: row.get(12)?,
        allow_comments: row.get(13)?,
        icon_url: row.get(14)?,
        gallery_urls_json: row.get(15)?,
        date_added: row.get(16)?,
        compatible_versions_json: row.get(17)?,
        description: row.get(18)?,
        body_markdown: row.get(19)?,
        page_url: row.get(20)?,
        license_id: row.get(21)?,
        source_updated_at: row.get(22)?,
        modrinth_id: row.get(23)?,
    })
}
