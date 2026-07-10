//! Registry read operations — pure SQL queries and re-exports.
//!
//! The pure, `&Connection`-based functions live in `agora_core::registry`
//! and are re-exported here for backward-compatible `crate::registry::*`
//! resolution in callers (commands.rs, mcp.rs, mod_install.rs, etc.).
//!
//! Desktop-specific functions that need `&AppHandle` remain in this module.

use crate::error::{LauncherError, LauncherResult};
use crate::models::InstanceManifest;
use crate::paths;
use rusqlite::Connection;
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Re-exports: pure registry functions + types (from agora-core)
// ---------------------------------------------------------------------------

pub use agora_core::registry::{
    browse_items, get_item_by_id, list_categories, pack_mods_for_pack, list_audit_log,
    list_under_review_items, list_recent_resolutions, list_mod_reviews,
    get_manifest_dependencies, get_all_mod_aliases, resolve_alias, get_known_conflicts,
    get_curated_annotation,
    row_to_item, REGISTRY_ITEM_COLUMNS,
    RegistryItem, SortOption, CategoryInfo, PackModRow, AuditLogEntry, UnderReviewItem,
    ModReview, KnownConflict, ManifestDeps, CuratedAnnotation,
};

// ---------------------------------------------------------------------------
// Desktop-only functions (require tauri::AppHandle)
// ---------------------------------------------------------------------------

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

/// Collect the set of installed-mod registry ids across all local instance
/// manifests. Used by the "For You" ranking (§6.2) to suppress already-installed
/// items and to derive the user's category interests. Best-effort: malformed
/// manifests are silently skipped. O(1) dedup via `HashSet`.
fn collect_installed_registry_ids<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> LauncherResult<HashSet<String>> {
    let dir = paths::instances_dir(app).map_err(|_| LauncherError::LocalStateFailed)?;
    let mut ids: HashSet<String> = HashSet::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Ok(ids),
    };
    for entry in entries.flatten() {
        let manifest_path = entry.path().join("instance_manifest.json");
        if let Ok(text) = std::fs::read_to_string(&manifest_path) {
            if let Ok(manifest) = serde_json::from_str::<InstanceManifest>(&text) {
                for m in &manifest.mods {
                    if let Some(rid) = &m.registry_id {
                        let rid = rid.trim();
                        if !rid.is_empty() {
                            ids.insert(rid.to_string());
                        }
                    }
                }
            }
        }
    }
    Ok(ids)
}

/// "For You" items (§6.2): boost uninstalled mods whose categories overlap
/// with the categories of the user's installed mods.
///
/// Ranking: items matching MORE of the user's interest categories rank higher
/// (`COUNT(ic.category_id) DESC`), with `net_score DESC` as a tiebreak.
/// Already-installed items are excluded. Degrades to a plain `net_score`
/// ordering when the user has no installed mods with registry ids, or when
/// those items expose no resolvable categories.
pub fn for_you_items<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    modrinth_enabled: bool,
    mc_version: Option<&str>,
    loader: Option<&str>,
    limit: i64,
    modrinth_categories: Option<&[String]>,
) -> LauncherResult<Vec<RegistryItem>> {
    let installed = collect_installed_registry_ids(app)?;
    let conn = open_registry(app)?;

    // Derive interest categories from installed items.
    let mut interest: Vec<String> = Vec::new();
    if !installed.is_empty() {
        let installed_ph = installed.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let interest_sql = format!(
            "SELECT DISTINCT category_id FROM item_categories WHERE item_id IN ({installed_ph})"
        );
        let mut stmt = conn.prepare(&interest_sql).map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?;
        let interest_params: Vec<Box<dyn rusqlite::ToSql>> =
            installed.iter().map(|s| Box::new(s.clone()) as Box<dyn rusqlite::ToSql>).collect();
        let interest_rows = stmt
            .query_map(rusqlite::params_from_iter(interest_params.iter()), |row| {
                let cat: String = row.get(0)?;
                Ok(cat)
            })
            .map_err(|e| LauncherError::Generic {
                code: "ERR_INVALID_QUERY".to_string(),
                message: e.to_string(),
            })?;
        for r in interest_rows {
            interest.push(r.map_err(|e| LauncherError::Generic {
                code: "ERR_INVALID_QUERY".to_string(),
                message: e.to_string(),
            })?);
        }
    }

    // Merge Modrinth category facets from the Browse page filter state.
    if let Some(mr_cats) = modrinth_categories {
        for cat in mr_cats {
            if !interest.contains(cat) {
                interest.push(cat.clone());
            }
        }
    }

    // No interest signal at all → degrade to net_score browse.
    if interest.is_empty() {
        let sort = SortOption::NetScore;
        return browse_items(&conn, None, None, &sort, modrinth_enabled, mc_version, loader, None, limit);
    }

    // Candidate items: uninstalled items sharing >=1 interest category, ranked
    // by number of interest categories matched then net_score.
    // Normalized score formula: score = overlap_count * 10 + net_score * 0.5 + log(downloads + 1) * 0.3
    // (downloads term requires a future schema addition)
    let interest_ph = interest.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let mut where_parts: Vec<String> = Vec::new();
    if !modrinth_enabled {
        where_parts.push("ri.download_strategy != 'modrinth_id'".to_string());
    }
    // Compatibility filters mirror browse_items so "For You" only recommends
    // items that declare support for the user's selected MC version / loader.
    if mc_version.is_some() {
        where_parts.push(
            "ri.id IN (SELECT r.id FROM registry_items r, json_each(r.compatible_versions_json) cv \
             WHERE json_extract(cv.value, '$.mc_version') = ?)"
                .to_string(),
        );
    }
    if let Some(ld) = loader {
        if ld != "all" {
            where_parts.push(
                "ri.id IN (SELECT r.id FROM registry_items r, json_each(r.compatible_versions_json) cv \
                 WHERE json_extract(cv.value, '$.loader') = ?)"
                    .to_string(),
            );
        }
    }
    let where_clause = if where_parts.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", where_parts.join(" AND "))
    };

    let sql = format!(
        "SELECT {REGISTRY_ITEM_COLUMNS}
         FROM registry_items ri
         LEFT JOIN item_categories ic ON ri.id = ic.item_id AND ic.category_id IN ({interest_ph})
         {where_clause}
         GROUP BY ri.id
         ORDER BY COUNT(ic.category_id) DESC, ri.net_score DESC
         LIMIT ?"
    );

    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    for cat in &interest {
        params.push(Box::new(cat.clone()));
    }
    if let Some(mv) = mc_version {
        params.push(Box::new(mv.to_string()));
    }
    if let Some(ld) = loader {
        if ld != "all" {
            params.push(Box::new(ld.to_string()));
        }
    }
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
