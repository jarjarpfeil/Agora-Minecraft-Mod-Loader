use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::modrinth::ModrinthSearchResult;
use crate::registry::RegistryItem;

pub const PAGE_SIZE: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseItem {
    pub id: String,
    pub source: String, // "curated" | "modrinth"
    pub registry_item: Option<RegistryItem>,
    pub modrinth_result: Option<ModrinthSearchResult>,
    pub name: String,
    pub icon_url: Option<String>,
    pub description: Option<String>,
    pub content_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowsePage {
    pub items: Vec<BrowseItem>,
    pub total: usize,
    pub page: usize,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BrowseFilters {
    pub query: String,
    pub content_type: Option<String>,
    pub category: Option<String>,
    pub sort: String,
    pub mc_version: Option<String>,
    pub loader: Option<String>,
    pub modrinth_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowseCache {
    /// Immutable identity of the filters that produced this cache.
    pub query_key: String,
    pub items: Vec<BrowseItem>,
    pub total: usize,
    pub filters: BrowseFilters,
    pub modrinth_offset: usize,
    pub has_more_modrinth: bool,
}

impl Default for BrowseCache {
    fn default() -> Self {
        Self {
            query_key: String::new(),
            items: Vec::new(),
            total: 0,
            filters: BrowseFilters::default(),
            modrinth_offset: 0,
            has_more_modrinth: true,
        }
    }
}

pub type SharedBrowseCache = Arc<RwLock<BrowseCache>>;

pub fn new_cache() -> SharedBrowseCache {
    Arc::new(RwLock::new(BrowseCache::default()))
}

pub fn normalize_modrinth_content_type(project_type: &str) -> &str {
    match project_type {
        "modpack" => "pack",
        "minecraft_java_server" => "server",
        other => other,
    }
}

/// Merge registry items and Modrinth results, deduplicating by modrinth_id.
pub fn merge_items(
    registry_items: Vec<RegistryItem>,
    modrinth_results: Vec<ModrinthSearchResult>,
) -> Vec<BrowseItem> {
    let mut registry_by_modrinth_id = HashMap::new();
    for ri in &registry_items {
        if let Some(ref mid) = ri.modrinth_id {
            registry_by_modrinth_id.insert(mid.clone(), ri.clone());
        }
    }

    let mut matched_ids = std::collections::HashSet::new();
    let mut merged = Vec::new();

    for mr in modrinth_results {
        if let Some(matched) = registry_by_modrinth_id.get(&mr.project_id) {
            matched_ids.insert(matched.id.clone());
            merged.push(BrowseItem {
                id: matched.id.clone(),
                source: "curated".to_string(),
                registry_item: Some(matched.clone()),
                modrinth_result: Some(mr.clone()),
                name: matched.name.clone(),
                icon_url: matched.icon_url.clone().or(mr.icon_url.clone()),
                description: matched
                    .description
                    .clone()
                    .or_else(|| Some(mr.description.clone())),
                content_type: matched.content_type.clone(),
            });
        } else {
            merged.push(BrowseItem {
                id: mr.project_id.clone(),
                source: "modrinth".to_string(),
                registry_item: None,
                modrinth_result: Some(mr.clone()),
                name: mr.title.clone(),
                icon_url: mr.icon_url.clone(),
                description: Some(mr.description.clone()),
                // Normalize Modrinth project types to app-internal content_type values.
                content_type: normalize_modrinth_content_type(&mr.project_type).to_string(),
            });
        }
    }

    for ri in registry_items {
        if !matched_ids.contains(&ri.id) {
            merged.push(BrowseItem {
                id: ri.id.clone(),
                source: "curated".to_string(),
                registry_item: Some(ri.clone()),
                modrinth_result: None,
                name: ri.name.clone(),
                icon_url: ri.icon_url.clone(),
                description: ri.description.clone(),
                content_type: ri.content_type.clone(),
            });
        }
    }

    merged
}

/// Load the first page of browse results into the cache.
pub async fn load_initial(
    cache: &SharedBrowseCache,
    query_key: String,
    registry_items: Vec<RegistryItem>,
    modrinth_results: Vec<ModrinthSearchResult>,
    filters: BrowseFilters,
    modrinth_offset: usize,
    has_more_modrinth: bool,
) {
    let merged = merge_items(registry_items, modrinth_results);
    let total = merged.len();
    let mut c = cache.write().await;
    c.query_key = query_key;
    c.items = merged;
    c.total = total;
    c.filters = filters;
    c.modrinth_offset = modrinth_offset;
    c.has_more_modrinth = has_more_modrinth;
}

/// Append more Modrinth items only when the cache still belongs to the
/// expected query. Returns false when a newer query replaced the cache.
pub async fn append_items(
    cache: &SharedBrowseCache,
    expected_query_key: &str,
    new_items: Vec<BrowseItem>,
    new_offset: usize,
    has_more: bool,
) -> bool {
    let mut c = cache.write().await;
    if c.query_key != expected_query_key {
        return false;
    }
    let mut existing_ids: std::collections::HashSet<(String, String)> = c
        .items
        .iter()
        .map(|i| (i.source.clone(), i.id.clone()))
        .collect();
    for item in new_items {
        if existing_ids.insert((item.source.clone(), item.id.clone())) {
            c.items.push(item);
        }
    }
    c.total = c.items.len();
    c.modrinth_offset = new_offset;
    c.has_more_modrinth = has_more;
    true
}

/// Get a page of results from the cache.
pub async fn get_page(cache: &SharedBrowseCache, page: usize) -> BrowsePage {
    let c = cache.read().await;
    let start = page * PAGE_SIZE;
    let end = std::cmp::min(start + PAGE_SIZE, c.items.len());
    let items = if start < c.items.len() {
        c.items[start..end].to_vec()
    } else {
        Vec::new()
    };
    BrowsePage {
        items,
        total: c.total,
        page,
        has_more: end < c.items.len(),
    }
}

/// Get all cached items (for CLI/MCP use).
pub async fn get_all(cache: &SharedBrowseCache) -> Vec<BrowseItem> {
    let c = cache.read().await;
    c.items.clone()
}

/// Invalidate the cache (e.g., on filter change).
pub async fn invalidate(cache: &SharedBrowseCache) {
    let mut c = cache.write().await;
    *c = BrowseCache::default();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: usize) -> BrowseItem {
        BrowseItem {
            id: format!("item-{id}"),
            source: "curated".into(),
            registry_item: None,
            modrinth_result: None,
            name: format!("Item {id}"),
            icon_url: None,
            description: None,
            content_type: "mod".into(),
        }
    }

    #[test]
    fn normalizes_modrinth_project_types_for_browse() {
        assert_eq!(normalize_modrinth_content_type("modpack"), "pack");
        assert_eq!(
            normalize_modrinth_content_type("minecraft_java_server"),
            "server"
        );
        assert_eq!(normalize_modrinth_content_type("shader"), "shader");
    }

    #[tokio::test]
    async fn requested_pages_do_not_skip_cached_items() {
        let cache = new_cache();
        {
            let mut state = cache.write().await;
            state.query_key = "query-a".into();
            state.items = (0..95).map(item).collect();
            state.total = state.items.len();
            state.has_more_modrinth = false;
        }
        let page_one = get_page(&cache, 1).await;
        assert_eq!(
            page_one.items.first().map(|i| i.id.as_str()),
            Some("item-20")
        );
        assert_eq!(
            page_one.items.last().map(|i| i.id.as_str()),
            Some("item-39")
        );
    }

    #[tokio::test]
    async fn stale_query_append_is_rejected() {
        let cache = new_cache();
        cache.write().await.query_key = "query-b".into();
        let appended = append_items(&cache, "query-a", vec![item(1)], PAGE_SIZE, false).await;
        assert!(!appended);
        assert!(cache.read().await.items.is_empty());
    }

    #[tokio::test]
    async fn append_deduplicates_by_source_and_id() {
        let cache = new_cache();
        cache.write().await.query_key = "query-a".into();
        let duplicate = item(1);
        assert!(
            append_items(
                &cache,
                "query-a",
                vec![duplicate.clone(), duplicate],
                PAGE_SIZE,
                false,
            )
            .await
        );
        assert_eq!(cache.read().await.items.len(), 1);
    }
}
