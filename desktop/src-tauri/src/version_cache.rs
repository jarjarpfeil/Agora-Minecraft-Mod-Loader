use crate::models::ModVersionCandidate;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;
use tokio::sync::RwLock;

pub const VERSION_PAGE_SIZE: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModVersionPage {
    pub items: Vec<ModVersionCandidate>,
    pub has_more: bool,
    pub total: usize,
}

#[derive(Debug, Clone)]
pub struct VersionListEntry {
    pub item_id: String,
    pub mc_version: String,
    pub loader: String,
    pub source_identifier: String,
    pub download_strategy: String,
    pub versions: Vec<ModVersionCandidate>,
    pub total_pages: u32,
    pub pages_fetched: BTreeSet<u32>,
}

#[derive(Debug, Default)]
pub struct VersionCacheInner {
    pub entry: Option<VersionListEntry>,
}

pub type SharedVersionCache = Arc<RwLock<VersionCacheInner>>;

pub fn new_cache() -> SharedVersionCache {
    Arc::new(RwLock::new(VersionCacheInner::default()))
}

/// Replace the cache entry with a fresh set of versions (first load).
pub async fn load_versions(
    cache: &SharedVersionCache,
    item_id: &str,
    mc_version: &str,
    loader: &str,
    source_identifier: &str,
    download_strategy: &str,
    versions: Vec<ModVersionCandidate>,
    total_pages: u32,
    pages_fetched: BTreeSet<u32>,
) {
    let mut c = cache.write().await;
    c.entry = Some(VersionListEntry {
        item_id: item_id.to_string(),
        mc_version: mc_version.to_string(),
        loader: loader.to_string(),
        source_identifier: source_identifier.to_string(),
        download_strategy: download_strategy.to_string(),
        versions,
        total_pages,
        pages_fetched,
    });
}

/// Append more candidates (from additional GitHub pages) into an existing
/// cache entry and re-sort by compatibility.  Returns the new total.
pub async fn extend_versions(
    cache: &SharedVersionCache,
    item_id: &str,
    mc_version: &str,
    loader: &str,
    mut more: Vec<ModVersionCandidate>,
    page_numbers: &[u32],
) -> Option<usize> {
    let mut c = cache.write().await;
    if let Some(ref mut entry) = c.entry {
        if entry.item_id == item_id && entry.mc_version == mc_version && entry.loader == loader {
            entry.versions.append(&mut more);
            // Re-sort so compatibles stay on top after new data arrives
            entry.versions.sort_by(|a, b| {
                let tier = |v: &ModVersionCandidate| -> u8 {
                    match v.version_compat.as_str() {
                        "compatible" => 0,
                        "major_match" => 1,
                        _ => 2,
                    }
                };
                tier(a).cmp(&tier(b)).then_with(|| {
                    b.release_date
                        .as_deref()
                        .unwrap_or("")
                        .cmp(a.release_date.as_deref().unwrap_or(""))
                })
            });
            for &p in page_numbers {
                entry.pages_fetched.insert(p);
            }
            return Some(entry.versions.len());
        }
    }
    None
}

pub async fn get_page(
    cache: &SharedVersionCache,
    item_id: &str,
    mc_version: &str,
    loader: &str,
    page: usize,
) -> Option<ModVersionPage> {
    let c = cache.read().await;
    if let Some(ref entry) = c.entry {
        if entry.item_id == item_id && entry.mc_version == mc_version && entry.loader == loader {
            let total = entry.versions.len();
            let start = page * VERSION_PAGE_SIZE;
            let end = std::cmp::min(start + VERSION_PAGE_SIZE, total);
            let items = if start < total {
                entry.versions[start..end].to_vec()
            } else {
                Vec::new()
            };
            let all_fetched = entry.pages_fetched.len() as u32 >= entry.total_pages;
            let has_more = if all_fetched {
                end < total
            } else {
                // There are still unfetched GitHub pages — we may have more data
                true
            };
            return Some(ModVersionPage {
                items,
                has_more,
                total,
            });
        }
    }
    None
}

/// Return a clone of the cache entry metadata, or `None` if no entry or the
/// key doesn't match.
pub async fn get_entry(
    cache: &SharedVersionCache,
    item_id: &str,
    mc_version: &str,
    loader: &str,
) -> Option<VersionListEntry> {
    let c = cache.read().await;
    if let Some(ref entry) = c.entry {
        if entry.item_id == item_id && entry.mc_version == mc_version && entry.loader == loader {
            return Some(entry.clone());
        }
    }
    None
}

pub async fn invalidate(cache: &SharedVersionCache) {
    let mut c = cache.write().await;
    c.entry = None;
}
