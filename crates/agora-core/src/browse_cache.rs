use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::registry::RegistryItem;
use crate::modrinth::ModrinthSearchResult;

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
    pub items: Vec<BrowseItem>,
    pub total: usize,
    pub filters: BrowseFilters,
    pub modrinth_offset: usize,
    pub has_more_modrinth: bool,
}

impl Default for BrowseCache {
    fn default() -> Self {
        Self {
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

    let mr_len = modrinth_results.len();
    let mut matched_count = 0u32;
    let mut modrinth_only_count = 0u32;
    for mr in modrinth_results {
        if let Some(matched) = registry_by_modrinth_id.get(&mr.project_id) {
            matched_count += 1;
            matched_ids.insert(matched.id.clone());
            merged.push(BrowseItem {
                id: matched.id.clone(),
                source: "curated".to_string(),
                registry_item: Some(matched.clone()),
                modrinth_result: Some(mr.clone()),
                name: matched.name.clone(),
                icon_url: matched.icon_url.clone().or(mr.icon_url.clone()),
                description: matched.description.clone().or_else(|| Some(mr.description.clone())),
                content_type: matched.content_type.clone(),
            });
        } else {
            modrinth_only_count += 1;
            merged.push(BrowseItem {
                id: mr.project_id.clone(),
                source: "modrinth".to_string(),
                registry_item: None,
                modrinth_result: Some(mr.clone()),
                name: mr.title.clone(),
                icon_url: mr.icon_url.clone(),
                description: Some(mr.description.clone()),
                content_type: mr.project_type.clone(),
            });
        }
    }

    eprintln!("[MERGE] modrinth_results={}: matched={} modrinth-only={}", mr_len, matched_count, modrinth_only_count);

    let reg_len = registry_items.len();
    let mut remaining_count = 0u32;
    for ri in registry_items {
        if !matched_ids.contains(&ri.id) {
            remaining_count += 1;
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

    eprintln!("[MERGE] registry_items={}: remaining-curated={} total-merged={}", reg_len, remaining_count, merged.len());

    merged
}

/// Load the first page of browse results into the cache.
pub async fn load_initial(
    cache: &SharedBrowseCache,
    registry_items: Vec<RegistryItem>,
    modrinth_results: Vec<ModrinthSearchResult>,
    filters: BrowseFilters,
    modrinth_offset: usize,
    has_more_modrinth: bool,
) {
    let merged = merge_items(registry_items, modrinth_results);
    let total = merged.len();
    let mut c = cache.write().await;
    c.items = merged;
    c.total = total;
    c.filters = filters;
    c.modrinth_offset = modrinth_offset;
    c.has_more_modrinth = has_more_modrinth;
}

/// Append more Modrinth items to the cache (deduplicating by id).
pub async fn append_items(
    cache: &SharedBrowseCache,
    new_items: Vec<BrowseItem>,
    new_offset: usize,
    has_more: bool,
) {
    let mut c = cache.write().await;
    let existing_ids: std::collections::HashSet<String> = c.items.iter().map(|i| i.id.clone()).collect();
    for item in new_items {
        if !existing_ids.contains(&item.id) {
            c.items.push(item);
        }
    }
    c.total = c.items.len();
    c.modrinth_offset = new_offset;
    c.has_more_modrinth = has_more;
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
