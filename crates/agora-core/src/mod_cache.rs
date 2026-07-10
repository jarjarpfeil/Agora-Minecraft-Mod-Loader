use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedJar {
    pub sha256: String,
    pub filename: String,
    pub size_bytes: u64,
    pub cached_at: String,
}

/// Content-addressed mod cache.
pub struct ModCache {
    cache_dir: PathBuf,
}

impl ModCache {
    pub fn new(cache_dir: PathBuf) -> Self {
        ModCache { cache_dir }
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Store a JAR in the cache. Copies the file, verifies SHA-256, returns the cache path.
    /// If already cached (same hash), returns existing path without re-copying.
    pub fn store_jar(&self, jar_path: &Path, expected_sha256: &str) -> Result<PathBuf, String> {
        let data = fs::read(jar_path).map_err(|e| format!("failed to read jar: {}", e))?;
        let actual_sha256 = crate::download::sha256_hex(&data);
        if actual_sha256 != expected_sha256 {
            return Err(format!(
                "SHA-256 mismatch: expected {} got {}",
                expected_sha256, actual_sha256
            ));
        }

        let cache_path = self.cache_path_for(&actual_sha256);
        if cache_path.exists() {
            return Ok(cache_path);
        }

        let parent = cache_path.parent().ok_or_else(|| "Invalid cache path".to_string())?;
        fs::create_dir_all(parent).map_err(|e| format!("failed to create cache dir: {}", e))?;
        fs::copy(jar_path, &cache_path).map_err(|e| format!("failed to copy to cache: {}", e))?;

        let metadata = CachedJar {
            sha256: actual_sha256.clone(),
            filename: jar_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string(),
            size_bytes: data.len() as u64,
            cached_at: chrono::Utc::now().to_rfc3339(),
        };
        // Metadata belongs to the content-addressed hash directory, alongside
        // `file.jar`. `list_cached` reads this canonical filename; the old
        // `file.json` location made newly cached jars invisible to cache
        // management and size accounting.
        let meta_path = cache_path
            .parent()
            .ok_or_else(|| "Invalid cache path".to_string())?
            .join("metadata.json");
        fs::write(
            &meta_path,
            serde_json::to_string_pretty(&metadata).unwrap(),
        )
        .map_err(|e| format!("failed to write metadata: {}", e))?;

        Ok(cache_path)
    }

    /// Hardlink (or copy) a cached JAR into a target directory with the given filename.
    /// Returns the created path. Falls back to copy if hardlink fails (cross-device, FAT).
    pub fn resolve_and_link(
        &self,
        sha256: &str,
        target_dir: &Path,
        filename: &str,
    ) -> Result<PathBuf, String> {
        let cache_path = self.cache_path_for(sha256);
        if !cache_path.exists() {
            return Err(format!("cached jar not found for hash {}", sha256));
        }

        let target_path = target_dir.join(filename);
        fs::create_dir_all(target_dir)
            .map_err(|e| format!("failed to create target dir: {}", e))?;

        if fs::hard_link(&cache_path, &target_path).is_err() {
            fs::copy(&cache_path, &target_path)
                .map_err(|e| format!("failed to copy jar: {}", e))?;
        }

        Ok(target_path)
    }

    /// Check if a JAR is in the cache.
    pub fn has_jar(&self, sha256: &str) -> bool {
        self.cache_path_for(sha256).exists()
    }

    /// List all cached JARs.
    pub fn list_cached(&self) -> Result<Vec<CachedJar>, String> {
        let mut jars = Vec::new();
        if !self.cache_dir.exists() {
            return Ok(jars);
        }

        let entries =
            fs::read_dir(&self.cache_dir).map_err(|e| format!("failed to read cache dir: {}", e))?;

        for entry in entries {
            let entry = entry.map_err(|e| format!("failed to read entry: {}", e))?;
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }

            let hash_entries = fs::read_dir(&entry.path())
                .map_err(|e| format!("failed to read hash dir: {}", e))?;

            for hash_entry in hash_entries {
                let hash_entry = hash_entry.map_err(|e| format!("failed to read hash entry: {}", e))?;
                if !hash_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }

                let metadata_path = hash_entry.path().join("metadata.json");
                // Keep existing player caches discoverable after correcting
                // the old writer, which placed metadata beside file.jar as
                // file.json.
                let meta_path = if metadata_path.exists() {
                    metadata_path
                } else {
                    hash_entry.path().join("file.json")
                };
                if meta_path.exists() {
                    let content = fs::read_to_string(&meta_path)
                        .map_err(|e| format!("failed to read metadata: {}", e))?;
                    let jar: CachedJar = serde_json::from_str(&content)
                        .map_err(|e| format!("failed to parse metadata: {}", e))?;
                    jars.push(jar);
                }
            }
        }

        Ok(jars)
    }

    /// Total cache size in bytes.
    pub fn total_size(&self) -> Result<u64, String> {
        let jars = self.list_cached()?;
        Ok(jars.iter().map(|j| j.size_bytes).sum())
    }

    fn cache_path_for(&self, sha256: &str) -> PathBuf {
        let prefix = &sha256[..2.min(sha256.len())];
        self.cache_dir
            .join(prefix)
            .join(sha256)
            .join("file.jar")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn store_jar_and_has_jar() {
        let tmp = TempDir::new().unwrap();
        let cache = ModCache::new(tmp.path().join("cache"));

        let jar_dir = tmp.path().join("input");
        fs::create_dir_all(&jar_dir).unwrap();
        let jar_path = jar_dir.join("test.jar");
        fs::write(&jar_path, b"hello world").unwrap();

        let expected = crate::download::sha256_hex(b"hello world");
        let stored = cache.store_jar(&jar_path, &expected).unwrap();
        assert!(stored.exists());
        assert!(cache.has_jar(&expected));
    }

    #[test]
    fn store_jar_deduplicates() {
        let tmp = TempDir::new().unwrap();
        let cache = ModCache::new(tmp.path().join("cache"));

        let jar_dir = tmp.path().join("input");
        fs::create_dir_all(&jar_dir).unwrap();
        let jar_path = jar_dir.join("test.jar");
        fs::write(&jar_path, b"same content").unwrap();

        let expected = crate::download::sha256_hex(b"same content");
        let first = cache.store_jar(&jar_path, &expected).unwrap();
        let second = cache.store_jar(&jar_path, &expected).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn store_jar_rejects_hash_mismatch() {
        let tmp = TempDir::new().unwrap();
        let cache = ModCache::new(tmp.path().join("cache"));

        let jar_dir = tmp.path().join("input");
        fs::create_dir_all(&jar_dir).unwrap();
        let jar_path = jar_dir.join("test.jar");
        fs::write(&jar_path, b"content").unwrap();

        let result = cache.store_jar(&jar_path, "deadbeef");
        assert!(result.is_err());
    }

    #[test]
    fn resolve_and_link_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let cache = ModCache::new(tmp.path().join("cache"));

        let jar_dir = tmp.path().join("input");
        fs::create_dir_all(&jar_dir).unwrap();
        let jar_path = jar_dir.join("test.jar");
        fs::write(&jar_path, b"mod data").unwrap();

        let expected = crate::download::sha256_hex(b"mod data");
        cache.store_jar(&jar_path, &expected).unwrap();

        let target_dir = tmp.path().join("mods");
        let linked = cache.resolve_and_link(&expected, &target_dir, "mymod.jar").unwrap();
        assert!(linked.exists());
        assert_eq!(fs::read(&linked).unwrap(), b"mod data");
    }

    #[test]
    fn list_cached_returns_stored_jars() {
        let tmp = TempDir::new().unwrap();
        let cache = ModCache::new(tmp.path().join("cache"));

        let jar_dir = tmp.path().join("input");
        fs::create_dir_all(&jar_dir).unwrap();
        let jar_path = jar_dir.join("test.jar");
        fs::write(&jar_path, b"list test").unwrap();

        let expected = crate::download::sha256_hex(b"list test");
        cache.store_jar(&jar_path, &expected).unwrap();

        let jars = cache.list_cached().unwrap();
        assert_eq!(jars.len(), 1);
        assert_eq!(jars[0].sha256, expected);
    }

    #[test]
    fn total_size_matches_content_length() {
        let tmp = TempDir::new().unwrap();
        let cache = ModCache::new(tmp.path().join("cache"));

        let jar_dir = tmp.path().join("input");
        fs::create_dir_all(&jar_dir).unwrap();
        let jar_path = jar_dir.join("test.jar");
        fs::write(&jar_path, b"size check").unwrap();

        let expected = crate::download::sha256_hex(b"size check");
        cache.store_jar(&jar_path, &expected).unwrap();

        assert_eq!(cache.total_size().unwrap(), 10);
    }
}
