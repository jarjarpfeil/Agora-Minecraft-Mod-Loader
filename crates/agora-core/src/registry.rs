use crate::error::{LauncherError, LauncherResult};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json;


/// Canonical column list for `registry_items` selects that feed `row_to_item`.
///
/// Single source of truth: `browse_items` and `for_you_items` both select this
/// exact list (with the `ri.` alias) so a schema column addition only needs
/// to be reflected here + in `row_to_item`'s positional reads, not in each query.
/// `get_item_by_id` is a single-table query and uses unprefixed names, so it
/// is kept in sync separately.
pub const REGISTRY_ITEM_COLUMNS: &str = "ri.id, ri.name, ri.content_type, ri.download_strategy,
        ri.source_identifier, ri.sha256, ri.upvotes, ri.downvotes,
        ri.net_score, ri.velocity, ri.status, ri.is_immune,
        ri.immunity_reason, ri.allow_comments, ri.icon_url,
        ri.gallery_urls_json, ri.date_added, ri.compatible_versions_json,
        ri.description, ri.body_markdown, ri.page_url, ri.license_id,
        ri.source_updated_at, ri.modrinth_id";

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
        "SELECT {REGISTRY_ITEM_COLUMNS}
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

/// A row from the `audit_log` transparency table (§4.6).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub id: i64,
    pub timestamp: String,
    pub action: String,
    pub details: Option<String>,
}

/// List audit log entries from the registry DB (newest first by id).
///
/// Defensively returns an empty vec if the `audit_log` table does not exist
/// (older registry.db builds predate this table) so the UI degrades gracefully.
pub fn list_audit_log(conn: &Connection, limit: i64) -> LauncherResult<Vec<AuditLogEntry>> {
    let has_table: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='audit_log'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !has_table {
        return Ok(Vec::new());
    }

    let mut stmt = conn
        .prepare("SELECT id, timestamp, action, details FROM audit_log ORDER BY id DESC LIMIT ?1")
        .map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?;
    let rows = stmt
        .query_map([limit], |row| {
            Ok(AuditLogEntry {
                id: row.get(0)?,
                timestamp: row.get(1)?,
                action: row.get(2)?,
                details: row.get(3)?,
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

pub fn row_to_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<RegistryItem> {
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

/// An under-review registry item for the Triage Center.
#[derive(Debug, Clone, Serialize)]
pub struct UnderReviewItem {
    pub id: String,
    pub name: String,
    pub content_type: String,
    pub icon_url: Option<String>,
    pub net_score: i64,
}

/// List registry items whose status is `under_review`, ordered by net_score
/// ascending (lowest scores first for triage).
pub fn list_under_review_items(conn: &Connection) -> LauncherResult<Vec<UnderReviewItem>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, content_type, icon_url, net_score \
             FROM registry_items WHERE status = 'under_review' \
             ORDER BY net_score ASC",
        )
        .map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?;

    let rows = stmt
        .query_map([], |row| {
            Ok(UnderReviewItem {
                id: row.get(0)?,
                name: row.get(1)?,
                content_type: row.get(2)?,
                icon_url: row.get(3)?,
                net_score: row.get(4)?,
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

/// List recent triage resolutions from the audit log.
///
/// Defensively returns an empty vec if the `audit_log` table does not exist.
pub fn list_recent_resolutions(
    conn: &Connection,
    limit: u32,
) -> LauncherResult<Vec<AuditLogEntry>> {
    let has_table: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='audit_log'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !has_table {
        return Ok(Vec::new());
    }

    let mut stmt = conn
        .prepare(
            "SELECT id, timestamp, action, details FROM audit_log \
             WHERE action IN ('triage_archive','triage_keep',
                              'organic_under_review','raid_breaker_offenders') \
             ORDER BY id DESC LIMIT ?",
        )
        .map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?;
    let rows = stmt
        .query_map([limit as i64], |row| {
            Ok(AuditLogEntry {
                id: row.get(0)?,
                timestamp: row.get(1)?,
                action: row.get(2)?,
                details: row.get(3)?,
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

/// A parsed curator review from a `curator_reviews.top_reviews_json` cell.
#[derive(Debug, Clone, Serialize)]
pub struct ModReview {
    pub author: Option<String>,
    pub text: String,
    pub issue_number: i64,
    pub created_at: Option<String>,
}

/// Load parsed curator reviews for a single item.
///
/// Reads the JSON array from `curator_reviews.top_reviews_json`, parses it,
/// and returns the resulting `ModReview` list. On NULL/empty/missing row or
/// parse error, returns `Ok(vec![])`.
pub fn list_mod_reviews(conn: &Connection, item_id: String) -> LauncherResult<Vec<ModReview>> {
    let mut stmt = conn
        .prepare("SELECT top_reviews_json FROM curator_reviews WHERE item_id = ?1")
        .map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?;

    let raw: Option<String> = stmt
        .query_row([item_id], |row| row.get(0))
        .ok();

    let json_str = match raw {
        Some(s) if !s.is_empty() => s,
        _ => return Ok(Vec::new()),
    };

    let parsed: Vec<serde_json::Value> = match serde_json::from_str(&json_str) {
        Ok(v) => v,
        Err(_) => return Ok(Vec::new()),
    };

    let mut out = Vec::new();
    for val in parsed {
        out.push(ModReview {
            author: val.get("author").and_then(|a| a.as_str()).map(String::from),
            text: val
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string(),
            issue_number: val
                .get("issue_number")
                .and_then(|i| i.as_i64())
                .unwrap_or(0),
            created_at: val
                .get("created_at")
                .and_then(|c| c.as_str())
                .map(String::from),
        });
    }
    Ok(out)
}

/// A known conflict between two mods (§4.6).
#[derive(Debug, Clone, Serialize)]
pub struct KnownConflict {
    pub mod_a_id: String,
    pub mod_b_id: String,
    pub severity: String,
    pub mitigated_by: Vec<String>,
    pub notes: Option<String>,
}

/// A parsed mod dependency set from `mod_manual_dependencies`.
#[derive(Debug, Clone, Serialize)]
pub struct ManifestDeps {
    pub required: Vec<String>,
    pub optional: Vec<String>,
    pub incompatible: Vec<String>,
}

/// Return manual dependency information for a single item from the
/// `mod_manual_dependencies` table.
///
/// Defensively returns `Ok(None)` if the `mod_manual_dependencies` table does
/// not exist (older registry.db builds predate this table) so the UI degrades
/// gracefully. Parses each JSON column string into `Vec<String>`; NULL/empty/
/// parse errors yield an empty vec.
pub fn get_manifest_dependencies(conn: &Connection, item_id: String) -> LauncherResult<Option<ManifestDeps>> {
    let has_table: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='mod_manual_dependencies'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !has_table {
        return Ok(None);
    }

    let mut stmt = conn
        .prepare(
            "SELECT required_json, optional_json, incompatible_json \
             FROM mod_manual_dependencies WHERE item_id = ?",
        )
        .map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?;

    let mut row = stmt
        .query_map([item_id], |row| {
            let required_json: Option<String> = row.get(0)?;
            let optional_json: Option<String> = row.get(1)?;
            let incompatible_json: Option<String> = row.get(2)?;

            let required: Vec<String> = match required_json {
                Some(s) if !s.is_empty() => match serde_json::from_str(&s) {
                    Ok(v) => v,
                    Err(_) => Vec::new(),
                },
                _ => Vec::new(),
            };

            let optional: Vec<String> = match optional_json {
                Some(s) if !s.is_empty() => match serde_json::from_str(&s) {
                    Ok(v) => v,
                    Err(_) => Vec::new(),
                },
                _ => Vec::new(),
            };

            let incompatible: Vec<String> = match incompatible_json {
                Some(s) if !s.is_empty() => match serde_json::from_str(&s) {
                    Ok(v) => v,
                    Err(_) => Vec::new(),
                },
                _ => Vec::new(),
            };

            Ok(ManifestDeps {
                required,
                optional,
                incompatible,
            })
        })
        .map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?;

    match row.next() {
        Some(Ok(deps)) => Ok(Some(deps)),
        Some(Err(e)) => Err(LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        }),
        None => Ok(None),
    }
}

/// Return manual dependency information for ALL items that have curated
/// dependencies in the `mod_manual_dependencies` table.
///
/// Defensively returns an empty map if the table does not exist (older
/// registry.db builds predate this table).
pub fn get_all_manifest_dependencies(
    conn: &Connection,
) -> LauncherResult<std::collections::HashMap<String, ManifestDeps>> {
    let has_table: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='mod_manual_dependencies'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !has_table {
        return Ok(std::collections::HashMap::new());
    }

    let mut stmt = conn
        .prepare(
            "SELECT item_id, required_json, optional_json, incompatible_json \
             FROM mod_manual_dependencies",
        )
        .map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?;

    let rows = stmt
        .query_map([], |row| {
            let item_id: String = row.get(0)?;
            let required_json: Option<String> = row.get(1)?;
            let optional_json: Option<String> = row.get(2)?;
            let incompatible_json: Option<String> = row.get(3)?;

            let required: Vec<String> = match required_json {
                Some(s) if !s.is_empty() => match serde_json::from_str(&s) {
                    Ok(v) => v,
                    Err(_) => Vec::new(),
                },
                _ => Vec::new(),
            };
            let optional: Vec<String> = match optional_json {
                Some(s) if !s.is_empty() => match serde_json::from_str(&s) {
                    Ok(v) => v,
                    Err(_) => Vec::new(),
                },
                _ => Vec::new(),
            };
            let incompatible: Vec<String> = match incompatible_json {
                Some(s) if !s.is_empty() => match serde_json::from_str(&s) {
                    Ok(v) => v,
                    Err(_) => Vec::new(),
                },
                _ => Vec::new(),
            };

            Ok((item_id, ManifestDeps { required, optional, incompatible }))
        })
        .map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?;

    let mut out = std::collections::HashMap::new();
    for r in rows {
        if let Ok((item_id, deps)) = r {
            out.insert(item_id, deps);
        }
    }
    Ok(out)
}

/// List all mod JAR aliases from the `mod_jar_aliases` table.
///
/// Returns `(registry_id, alias)` tuples. Defensively returns an empty vec
/// if the `mod_jar_aliases` table does not exist (older registry.db builds
/// predate this table) so the UI degrades gracefully.
pub fn get_all_mod_aliases(conn: &Connection) -> LauncherResult<Vec<(String, String)>> {
    let has_table: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='mod_jar_aliases'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !has_table {
        return Ok(Vec::new());
    }

    let mut stmt = conn
        .prepare("SELECT registry_id, alias FROM mod_jar_aliases")
        .map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?;

    let rows = stmt
        .query_map([], |row| {
            let registry_id: String = row.get(0)?;
            let alias: String = row.get(1)?;
            Ok((registry_id, alias))
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

/// Resolve a mod JAR alias to its registry item ID.
///
/// Returns `Ok(Some(registry_id))` when the alias is found, `Ok(None)` when
/// it is not. Defensively returns `Ok(None)` if the `mod_jar_aliases` table
/// does not exist (older registry.db builds predate this table).
pub fn resolve_alias(conn: &Connection, alias: &str) -> LauncherResult<Option<String>> {
    let has_table: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='mod_jar_aliases'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !has_table {
        return Ok(None);
    }

    let mut stmt = conn
        .prepare(
            "SELECT registry_id FROM mod_jar_aliases WHERE alias = ?",
        )
        .map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?;

    let mut rows = stmt
        .query_map([alias], |row| row.get(0))
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

/// A lightweight annotation for Modrinth projects that have a curated entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CuratedAnnotation {
    pub id: String,
    pub name: String,
    pub curator_note: Option<String>,
    pub net_score: Option<f64>,
    pub is_immune: bool,
    pub base_categories: Vec<String>,
}

/// Look up a curated annotation for a Modrinth project.
///
/// Queries `registry_items` by `modrinth_id`, then fetches the item's
/// categories from `item_categories`. Returns `None` when no curated entry
/// exists for the given Modrinth project ID.
pub fn get_curated_annotation(
    conn: &Connection,
    modrinth_id: &str,
) -> LauncherResult<Option<CuratedAnnotation>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, net_score, is_immune, description \
             FROM registry_items WHERE modrinth_id = ?1",
        )
        .map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?;

    let mut rows = stmt
        .query_map([modrinth_id], |row| {
            let id: String = row.get(0)?;
            let name: String = row.get(1)?;
            let net_score: i64 = row.get(2)?;
            let is_immune: bool = row.get(3)?;
            let curator_note: Option<String> = row.get(4)?;
            Ok((id, name, net_score, is_immune, curator_note))
        })
        .map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?;

    let (id, name, net_score, is_immune, curator_note) = match rows.next() {
        Some(Ok(r)) => r,
        Some(Err(e)) => {
            return Err(LauncherError::Generic {
                code: "ERR_INVALID_QUERY".to_string(),
                message: e.to_string(),
            })
        }
        None => return Ok(None),
    };

    // Fetch base categories for this curated item.
    let mut cat_stmt = conn
        .prepare("SELECT category_id FROM item_categories WHERE item_id = ?1")
        .map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?;
    let cat_rows = cat_stmt
        .query_map([&id], |row| row.get::<_, String>(0))
        .map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?;

    let mut base_categories: Vec<String> = Vec::new();
    for r in cat_rows {
        base_categories.push(r.map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?);
    }

    Ok(Some(CuratedAnnotation {
        id,
        name,
        curator_note,
        net_score: Some(net_score as f64),
        is_immune,
        base_categories,
    }))
}

/// List known conflicts from the `known_conflicts` table.
///
/// Defensively returns an empty vec if the `known_conflicts` table does not
/// exist (older registry.db builds predate this table) so the UI degrades
/// gracefully. Parses `mitigated_by_json` (a JSON array string, may be
/// NULL/empty) into `Vec<String>`; parse errors yield an empty vec.
pub fn get_known_conflicts(conn: &Connection) -> LauncherResult<Vec<KnownConflict>> {
    let has_table: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='known_conflicts'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !has_table {
        return Ok(Vec::new());
    }

    let mut stmt = conn
        .prepare(
            "SELECT mod_a_id, mod_b_id, severity, mitigated_by_json, notes \
             FROM known_conflicts",
        )
        .map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?;

    let rows = stmt
        .query_map([], |row| {
            let mitigated_by_json: Option<String> = row.get(3)?;
            let mitigated_by: Vec<String> = match mitigated_by_json {
                Some(s) if !s.is_empty() => match serde_json::from_str(&s) {
                    Ok(v) => v,
                    Err(_) => Vec::new(),
                },
                _ => Vec::new(),
            };
            Ok(KnownConflict {
                mod_a_id: row.get(0)?,
                mod_b_id: row.get(1)?,
                severity: row.get(2)?,
                mitigated_by,
                notes: row.get(4)?,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::registry_connection;
    use std::fs;
    use tempfile::TempDir;

    fn temp_registry_db() -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("registry.db");

        // Create a minimal registry database with all tables the tests need.
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "
            CREATE TABLE registry_items (
                id TEXT PRIMARY KEY,
                name TEXT,
                content_type TEXT,
                download_strategy TEXT,
                source_identifier TEXT,
                sha256 TEXT,
                upvotes INTEGER,
                downvotes INTEGER,
                net_score INTEGER,
                velocity REAL,
                status TEXT,
                is_immune INTEGER,
                immunity_reason TEXT,
                allow_comments INTEGER,
                icon_url TEXT,
                gallery_urls_json TEXT,
                date_added TEXT,
                compatible_versions_json TEXT,
                description TEXT,
                body_markdown TEXT,
                page_url TEXT,
                license_id TEXT,
                source_updated_at TEXT,
                modrinth_id TEXT
            );

            CREATE TABLE categories (
                id TEXT PRIMARY KEY,
                display_name TEXT,
                is_community INTEGER
            );

            CREATE TABLE item_categories (
                item_id TEXT,
                category_id TEXT
            );

            CREATE TABLE pack_mods (
                pack_id TEXT,
                mod_id TEXT,
                source TEXT,
                version TEXT,
                status TEXT,
                description TEXT
            );

            CREATE TABLE audit_log (
                id INTEGER PRIMARY KEY,
                timestamp TEXT,
                action TEXT,
                details TEXT
            );

            CREATE TABLE curator_reviews (
                item_id TEXT PRIMARY KEY,
                top_reviews_json TEXT
            );

            CREATE TABLE mod_manual_dependencies (
                item_id TEXT PRIMARY KEY,
                required_json TEXT,
                optional_json TEXT,
                incompatible_json TEXT
            );

            CREATE TABLE mod_jar_aliases (
                registry_id TEXT,
                alias TEXT
            );

            CREATE TABLE known_conflicts (
                mod_a_id TEXT,
                mod_b_id TEXT,
                severity TEXT,
                mitigated_by_json TEXT,
                notes TEXT
            );

            INSERT INTO registry_items VALUES (
                'test-mod-1', 'Test Mod 1', 'mod', 'github_release',
                'owner/repo', 'abc123', 10, 2, 8, 1.5, 'approved',
                0, NULL, 1, 'https://example.com/icon.png', NULL,
                '2024-01-01T00:00:00Z', '[{\"mc_version\":\"1.20.1\",\"loader\":\"fabric\"}]',
                'A test mod', NULL, 'https://example.com/mod1', 'MIT',
                '2024-01-01T00:00:00Z', NULL
            );

            INSERT INTO categories VALUES ('fabric', 'Fabric', 0);
            INSERT INTO item_categories VALUES ('test-mod-1', 'fabric');

            INSERT INTO pack_mods VALUES ('test-pack', 'test-mod-1', 'modrinth', '1.0.0', 'installed', 'A pack mod');

            INSERT INTO audit_log VALUES (1, '2024-01-01T00:00:00Z', 'triage_keep', 'Kept item');

            INSERT INTO curator_reviews VALUES ('test-mod-1', '[{\"author\":\"curator1\",\"text\":\"Great mod!\",\"issue_number\":42,\"created_at\":\"2024-01-01\"}]');

            INSERT INTO mod_manual_dependencies VALUES ('test-mod-1', '[\"dep-1\"]', '[\"opt-1\"]', '[\"incomp-1\"]');

            INSERT INTO mod_jar_aliases VALUES ('test-mod-1', 'testmod.jar');

            INSERT INTO known_conflicts VALUES ('mod-a', 'mod-b', 'hard', '[\"workaround\"]', 'Known issue');
            ",
        )
        .unwrap();

        dir
    }

    #[test]
    fn test_get_item_by_id() {
        let dir = temp_registry_db();
        let conn = registry_connection(&dir.path().join("registry.db")).unwrap();
        let item = get_item_by_id(&conn, "test-mod-1").unwrap().unwrap();
        assert_eq!(item.name, "Test Mod 1");
        assert_eq!(item.content_type, "mod");
        assert_eq!(item.net_score, 8);
    }

    #[test]
    fn test_get_item_by_id_not_found() {
        let dir = temp_registry_db();
        let conn = registry_connection(&dir.path().join("registry.db")).unwrap();
        let item = get_item_by_id(&conn, "nonexistent").unwrap();
        assert!(item.is_none());
    }

    #[test]
    fn test_list_categories() {
        let dir = temp_registry_db();
        let conn = registry_connection(&dir.path().join("registry.db")).unwrap();
        let cats = list_categories(&conn).unwrap();
        assert_eq!(cats.len(), 1);
        assert_eq!(cats[0].display_name, "Fabric");
    }

    #[test]
    fn test_pack_mods_for_pack() {
        let dir = temp_registry_db();
        let conn = registry_connection(&dir.path().join("registry.db")).unwrap();
        let mods = pack_mods_for_pack(&conn, "test-pack").unwrap();
        assert_eq!(mods.len(), 1);
        assert_eq!(mods[0].mod_id, "test-mod-1");
    }

    #[test]
    fn test_list_audit_log() {
        let dir = temp_registry_db();
        let conn = registry_connection(&dir.path().join("registry.db")).unwrap();
        let entries = list_audit_log(&conn, 10).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, "triage_keep");
    }

    #[test]
    fn test_list_under_review_items_empty() {
        let dir = temp_registry_db();
        let conn = registry_connection(&dir.path().join("registry.db")).unwrap();
        let items = list_under_review_items(&conn).unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn test_list_recent_resolutions() {
        let dir = temp_registry_db();
        let conn = registry_connection(&dir.path().join("registry.db")).unwrap();
        let entries = list_recent_resolutions(&conn, 10).unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn test_list_mod_reviews() {
        let dir = temp_registry_db();
        let conn = registry_connection(&dir.path().join("registry.db")).unwrap();
        let reviews = list_mod_reviews(&conn, "test-mod-1".to_string()).unwrap();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].author, Some("curator1".to_string()));
        assert_eq!(reviews[0].text, "Great mod!");
    }

    #[test]
    fn test_list_mod_reviews_empty() {
        let dir = temp_registry_db();
        let conn = registry_connection(&dir.path().join("registry.db")).unwrap();
        let reviews = list_mod_reviews(&conn, "nonexistent".to_string()).unwrap();
        assert!(reviews.is_empty());
    }

    #[test]
    fn test_get_manifest_dependencies() {
        let dir = temp_registry_db();
        let conn = registry_connection(&dir.path().join("registry.db")).unwrap();
        let deps = get_manifest_dependencies(&conn, "test-mod-1".to_string()).unwrap().unwrap();
        assert_eq!(deps.required, vec!["dep-1"]);
        assert_eq!(deps.optional, vec!["opt-1"]);
        assert_eq!(deps.incompatible, vec!["incomp-1"]);
    }

    #[test]
    fn test_get_manifest_dependencies_none() {
        let dir = temp_registry_db();
        let conn = registry_connection(&dir.path().join("registry.db")).unwrap();
        let deps = get_manifest_dependencies(&conn, "nonexistent".to_string()).unwrap();
        assert!(deps.is_none());
    }

    #[test]
    fn test_get_all_mod_aliases() {
        let dir = temp_registry_db();
        let conn = registry_connection(&dir.path().join("registry.db")).unwrap();
        let aliases = get_all_mod_aliases(&conn).unwrap();
        assert_eq!(aliases.len(), 1);
        assert_eq!(aliases[0].0, "test-mod-1");
        assert_eq!(aliases[0].1, "testmod.jar");
    }

    #[test]
    fn test_resolve_alias() {
        let dir = temp_registry_db();
        let conn = registry_connection(&dir.path().join("registry.db")).unwrap();
        let resolved = resolve_alias(&conn, "testmod.jar").unwrap().unwrap();
        assert_eq!(resolved, "test-mod-1");
    }

    #[test]
    fn test_resolve_alias_not_found() {
        let dir = temp_registry_db();
        let conn = registry_connection(&dir.path().join("registry.db")).unwrap();
        let resolved = resolve_alias(&conn, "nonexistent.jar").unwrap();
        assert!(resolved.is_none());
    }

    #[test]
    fn test_get_known_conflicts() {
        let dir = temp_registry_db();
        let conn = registry_connection(&dir.path().join("registry.db")).unwrap();
        let conflicts = get_known_conflicts(&conn).unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].mod_a_id, "mod-a");
        assert_eq!(conflicts[0].mod_b_id, "mod-b");
        assert_eq!(conflicts[0].severity, "hard");
        assert_eq!(conflicts[0].mitigated_by, vec!["workaround".to_string()]);
    }

    #[test]
    fn test_browse_items_basic() {
        let dir = temp_registry_db();
        let conn = registry_connection(&dir.path().join("registry.db")).unwrap();
        let sort = SortOption::NetScore;
        let items = browse_items(&conn, None, None, &sort, true, None, None, 100).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "Test Mod 1");
    }

    #[test]
    fn test_browse_items_with_content_type() {
        let dir = temp_registry_db();
        let conn = registry_connection(&dir.path().join("registry.db")).unwrap();
        let sort = SortOption::NetScore;
        let items = browse_items(&conn, Some("mod"), None, &sort, true, None, None, 100).unwrap();
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn test_browse_items_with_category() {
        let dir = temp_registry_db();
        let conn = registry_connection(&dir.path().join("registry.db")).unwrap();
        let sort = SortOption::NetScore;
        let items = browse_items(&conn, None, Some("fabric"), &sort, true, None, None, 100).unwrap();
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn test_sort_option_default() {
        let sort = SortOption::default();
        assert!(matches!(sort, SortOption::NetScore));
    }

    #[test]
    fn test_get_curated_annotation_found() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("registry.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "
            CREATE TABLE registry_items (
                id TEXT PRIMARY KEY, name TEXT, content_type TEXT,
                download_strategy TEXT, source_identifier TEXT, sha256 TEXT,
                upvotes INTEGER, downvotes INTEGER, net_score INTEGER,
                velocity REAL, status TEXT, is_immune INTEGER,
                immunity_reason TEXT, allow_comments INTEGER, icon_url TEXT,
                gallery_urls_json TEXT, date_added TEXT,
                compatible_versions_json TEXT, description TEXT,
                body_markdown TEXT, page_url TEXT, license_id TEXT,
                source_updated_at TEXT, modrinth_id TEXT
            );
            CREATE TABLE item_categories (
                item_id TEXT, category_id TEXT
            );
            INSERT INTO registry_items VALUES (
                'curated-mod', 'Curated Mod', 'mod', 'modrinth_id',
                'mr-id-123', 'abc', 100, 5, 85, 2.0, 'approved',
                1, NULL, 1, NULL, NULL, '2024-06-01T00:00:00Z', NULL,
                'A curated mod description', NULL, NULL, NULL, NULL, 'mr-id-123'
            );
            INSERT INTO item_categories VALUES ('curated-mod', 'fabric');
            INSERT INTO item_categories VALUES ('curated-mod', 'adventure');
            ",
        )
        .unwrap();

        let annotation = get_curated_annotation(&conn, "mr-id-123")
            .unwrap()
            .expect("Expected some annotation");
        assert_eq!(annotation.id, "curated-mod");
        assert_eq!(annotation.name, "Curated Mod");
        assert!(annotation.is_immune);
        assert_eq!(annotation.net_score, Some(85.0));
        assert_eq!(annotation.curator_note, Some("A curated mod description".to_string()));
        assert_eq!(annotation.base_categories.len(), 2);
        assert!(annotation.base_categories.contains(&"fabric".to_string()));
        assert!(annotation.base_categories.contains(&"adventure".to_string()));
    }

    #[test]
    fn test_get_curated_annotation_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("registry.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "
            CREATE TABLE registry_items (
                id TEXT PRIMARY KEY, name TEXT, content_type TEXT,
                download_strategy TEXT, source_identifier TEXT, sha256 TEXT,
                upvotes INTEGER, downvotes INTEGER, net_score INTEGER,
                velocity REAL, status TEXT, is_immune INTEGER,
                immunity_reason TEXT, allow_comments INTEGER, icon_url TEXT,
                gallery_urls_json TEXT, date_added TEXT,
                compatible_versions_json TEXT, description TEXT,
                body_markdown TEXT, page_url TEXT, license_id TEXT,
                source_updated_at TEXT, modrinth_id TEXT
            );
            CREATE TABLE item_categories (
                item_id TEXT, category_id TEXT
            );
            ",
        )
        .unwrap();

        let result = get_curated_annotation(&conn, "nonexistent-id").unwrap();
        assert!(result.is_none());
    }
}
