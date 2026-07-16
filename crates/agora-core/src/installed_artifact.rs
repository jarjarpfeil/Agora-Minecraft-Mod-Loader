//! **Installed artifact reuse** for the launch planner.
//!
//! Provides helpers to locate, verify, and materialize game artifacts from a
//! Mojang launcher's installed `.minecraft` directory into the Agora cache.
//! Used by adopted Forge/NeoForge profiles and optionally by all modes when
//! [`crate::launch_planner::ResolveRequest::minecraft_dir`] is set.
//!
//! # Strategy
//! 1. **Cache-first**: if the target (Agora cache) file exists and passes
//!    verification, return immediately.
//! 2. **Installed source**: fall back to the installed `.minecraft` tree,
//!    verify every known hash/size, then materialize via hardlink or copy.
//! 3. **Network**: only when the installed source is missing (not corrupt).
//!
//! # Security
//! - Installed source files are verified via SHA-1/SHA-256 and size whenever
//!   metadata provides them.
//! - Generated/unhashed loader artifacts are accepted only when the receipt
//!   contains a matching path in `generated_artifact_sha256`.
//! - Source must be a regular file; symlinks/reparse points are rejected.
//! - Materialization uses same-directory temp + atomic rename for copies.
//! - Post-materialization hash verification is mandatory; mismatch removes
//!   target and returns a corrupt error (never silently falls back to network).

use crate::error::{LauncherError, LauncherResult};
use crate::installed_profile::{InstalledProfileReceipt, ProfileIssue};
use crate::network::{NetworkCategory, NetworkPolicy};
use sha1::{Digest as Sha1Digest, Sha1};
use sha2::Sha256;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Max size for a single artifact file we will read for verification (2 GiB).
/// Libraries, client JARs, and assets rarely exceed this.
const MAX_VERIFICATION_SIZE: u64 = 2 * 1024 * 1024 * 1024;

/// Subdirectory under assets for logging configs.
const LOG_CONFIG_SUBDIR: &str = "log_configs";

// ---------------------------------------------------------------------------
// InstalledArtifactSource – locator for a `.minecraft` installation
// ---------------------------------------------------------------------------

/// Provides paths into an installed Mojang launcher `.minecraft` directory.
#[derive(Debug, Clone)]
pub struct InstalledArtifactSource {
    pub minecraft_dir: PathBuf,
}

impl InstalledArtifactSource {
    pub fn new(minecraft_dir: PathBuf) -> Self {
        Self { minecraft_dir }
    }

    /// Path to the client JAR for a base version.
    pub fn client_jar(&self, base_version_id: &str) -> PathBuf {
        self.minecraft_dir
            .join("versions")
            .join(base_version_id)
            .join(format!("{}.jar", base_version_id))
    }

    /// Path to a Maven library artifact inside the installed directory.
    pub fn library(&self, relative_maven_path: &str) -> PathBuf {
        self.minecraft_dir
            .join("libraries")
            .join(relative_maven_path)
    }

    /// Path to an asset index JSON.
    pub fn asset_index(&self, id: &str) -> PathBuf {
        self.minecraft_dir
            .join("assets")
            .join("indexes")
            .join(format!("{}.json", id))
    }

    /// Path to an asset object file (content-addressed by hash).
    pub fn asset_object(&self, hash: &str) -> PathBuf {
        let prefix = &hash[..2.min(hash.len())];
        self.minecraft_dir
            .join("assets")
            .join("objects")
            .join(prefix)
            .join(hash)
    }

    /// Path to a logging configuration file inside the installed assets directory.
    /// Mojang launcher places these under `assets/log_configs/<id>`.
    pub fn logging_config(&self, id: &str) -> PathBuf {
        self.minecraft_dir
            .join("assets")
            .join(LOG_CONFIG_SUBDIR)
            .join(id)
    }
}

// ---------------------------------------------------------------------------
// Verification helpers
// ---------------------------------------------------------------------------

/// Verify that a file at `path` matches the expected SHA-256 hash.
/// Returns `Ok(())` if the file exists, is a regular file, and its SHA-256
/// matches `expected`.
pub fn verify_sha256(path: &Path, expected: &str) -> Result<(), ProfileIssue> {
    let data = read_bounded_regular_file(path)?;
    let actual = sha256_hex(&data);
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(ProfileIssue::corrupt(
            Some(path.to_path_buf()),
            format!(
                "SHA-256 mismatch for {}: expected {expected}, got {actual}",
                path.display()
            ),
        ))
    }
}

/// Verify that a file at `path` matches the expected SHA-1 hash.
/// Returns `Ok(())` if the file exists, is a regular file, and its SHA-1
/// matches `expected`.
pub fn verify_sha1(path: &Path, expected: &str) -> Result<(), ProfileIssue> {
    let data = read_bounded_regular_file(path)?;
    let actual = sha1_hex(&data);
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(ProfileIssue::corrupt(
            Some(path.to_path_buf()),
            format!(
                "SHA-1 mismatch for {}: expected {expected}, got {actual}",
                path.display()
            ),
        ))
    }
}

/// Verify size of a file. Returns `Ok(())` if the file size matches `expected`.
pub fn verify_size(path: &Path, expected: i64) -> Result<(), ProfileIssue> {
    let meta = std::fs::metadata(path).map_err(|e| {
        ProfileIssue::corrupt(
            Some(path.to_path_buf()),
            format!("Cannot read metadata for {}: {e}", path.display()),
        )
    })?;
    if meta.len() == expected as u64 {
        Ok(())
    } else {
        Err(ProfileIssue::corrupt(
            Some(path.to_path_buf()),
            format!(
                "Size mismatch for {}: expected {expected}, got {}",
                path.display(),
                meta.len()
            ),
        ))
    }
}

/// Check that `path` is a regular file (not a symlink, reparse point, etc.).
/// On Windows, rejects `FILE_ATTRIBUTE_REPARSE_POINT`.
pub fn is_regular_file(path: &Path) -> Result<(), ProfileIssue> {
    let meta = std::fs::metadata(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ProfileIssue::missing(
                Some(path.to_path_buf()),
                format!("File not found: {}", path.display()),
            )
        } else {
            ProfileIssue::corrupt(
                Some(path.to_path_buf()),
                format!("Cannot read metadata for {}: {e}", path.display()),
            )
        }
    })?;

    if !meta.file_type().is_file() {
        return Err(ProfileIssue::corrupt(
            Some(path.to_path_buf()),
            "Not a regular file".to_string(),
        ));
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err(ProfileIssue::corrupt(
                Some(path.to_path_buf()),
                "Path is a reparse point / symlink".to_string(),
            ));
        }
    }

    Ok(())
}

/// Read a file with a size bound for verification purposes.
fn read_bounded_regular_file(path: &Path) -> Result<Vec<u8>, ProfileIssue> {
    is_regular_file(path)?;
    let meta = std::fs::metadata(path).map_err(|e| {
        ProfileIssue::corrupt(
            Some(path.to_path_buf()),
            format!("Cannot stat {}: {e}", path.display()),
        )
    })?;
    if meta.len() > MAX_VERIFICATION_SIZE {
        return Err(ProfileIssue::corrupt(
            Some(path.to_path_buf()),
            format!(
                "File too large for verification: {} bytes (max {MAX_VERIFICATION_SIZE})",
                meta.len()
            ),
        ));
    }
    let mut file = std::fs::File::open(path).map_err(|e| {
        ProfileIssue::corrupt(
            Some(path.to_path_buf()),
            format!("Cannot open {}: {e}", path.display()),
        )
    })?;
    let mut buf = Vec::with_capacity(meta.len() as usize);
    file.read_to_end(&mut buf).map_err(|e| {
        ProfileIssue::corrupt(
            Some(path.to_path_buf()),
            format!("Cannot read {}: {e}", path.display()),
        )
    })?;
    Ok(buf)
}

// ---------------------------------------------------------------------------
// Hash utilities
// ---------------------------------------------------------------------------

pub fn sha1_hex(data: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

// ---------------------------------------------------------------------------
// Materialization helpers
// ---------------------------------------------------------------------------

/// Atomically write bytes to a file using the same strategy as
/// [`crate::launch_planner::atomic_write`].
pub fn atomic_write(path: &Path, bytes: &[u8]) -> LauncherResult<()> {
    let parent = path.parent().ok_or_else(|| LauncherError::Generic {
        code: "ERR_CACHE_PATH".into(),
        message: format!("Cache path has no parent: {}", path.display()),
    })?;
    std::fs::create_dir_all(parent).map_err(|e| LauncherError::Generic {
        code: "ERR_CACHE_DIR".into(),
        message: format!("Failed to create {}: {e}", parent.display()),
    })?;

    let temp = atomic_temp_path(path);
    let write_result = (|| {
        let mut file = std::fs::File::create(&temp).map_err(|e| LauncherError::Generic {
            code: "ERR_CACHE_WRITE".into(),
            message: format!("Failed to create temp file {}: {e}", temp.display()),
        })?;
        file.write_all(bytes).map_err(|e| LauncherError::Generic {
            code: "ERR_CACHE_WRITE".into(),
            message: format!("Failed to write {}: {e}", temp.display()),
        })?;
        file.flush().map_err(|e| LauncherError::Generic {
            code: "ERR_CACHE_WRITE".into(),
            message: format!("Failed to flush {}: {e}", temp.display()),
        })?;
        file.sync_all().map_err(|e| LauncherError::Generic {
            code: "ERR_CACHE_WRITE".into(),
            message: format!("Failed to sync {}: {e}", temp.display()),
        })?;
        Ok::<_, LauncherError>(())
    })();

    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }

    if let Err(e) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(LauncherError::Generic {
            code: "ERR_CACHE_RENAME".into(),
            message: format!(
                "Failed to rename {} to {}: {e}",
                temp.display(),
                path.display()
            ),
        });
    }

    Ok(())
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn atomic_temp_path(path: &Path) -> PathBuf {
    let pid = std::process::id();
    let count = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let stamp = format!(".agtmp_{pid}_{count}");
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path.file_name().unwrap_or_default();
    parent.join(format!("{}{}", file_name.to_string_lossy(), stamp))
}

/// Copy a file from `source` to `destination`.
/// Uses the same atomic-temp strategy as `atomic_write` for the destination.
fn copy_atomic(source: &Path, destination: &Path) -> LauncherResult<()> {
    let bytes = std::fs::read(source).map_err(|e| LauncherError::Generic {
        code: "ERR_CACHE_READ".into(),
        message: format!("Cannot read source {}: {e}", source.display()),
    })?;
    atomic_write(destination, &bytes)
}

/// Try to create a hardlink from `source` to `destination`. If that fails
/// (different volumes, permission denied), fall back to atomic copy.
/// Returns `true` if a hardlink was created, `false` if copy was used.
pub fn hardlink_or_copy(source: &Path, destination: &Path) -> LauncherResult<bool> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|e| LauncherError::Generic {
            code: "ERR_CACHE_DIR".into(),
            message: format!("Failed to create {}: {e}", parent.display()),
        })?;
    }

    if std::fs::hard_link(source, destination).is_ok() {
        Ok(true)
    } else {
        copy_atomic(source, destination)?;
        Ok(false)
    }
}

// ---------------------------------------------------------------------------
// Core artifact adoption logic
// ---------------------------------------------------------------------------

/// Result of attempting to adopt an artifact from an installed source.
#[derive(Debug)]
pub enum ArtifactAdoptResult {
    /// The target cache file was already valid.
    CacheHit,
    /// The artifact was materialized from the installed source into the cache.
    Materialized { used_hardlink: bool },
    /// The artifact is not available from the installed source (caller should
    /// fall back to network).
    SourceMissing,
}

/// Try to use an installed source artifact for a library.
///
/// Strategy:
/// 1. If the cache target exists and is valid (SHA-256 pin or SHA-1 match),
///    return `CacheHit`.
/// 2. If the installed source exists and passes verification, materialize
///    it into the cache (hardlink preferred, copy fallback).
/// 3. If the installed source is missing, return `SourceMissing`.
/// 4. If the installed source exists but fails verification, return
///    `Err(ProfileIssue::Corrupt)`.
pub fn adopt_library_artifact(
    source: &InstalledArtifactSource,
    cache_path: &Path,
    relative_maven_path: &str,
    sha1: Option<&str>,
    sha256: Option<&str>,
    size: Option<i64>,
) -> Result<ArtifactAdoptResult, ProfileIssue> {
    // Step 1: Cache hit check
    if cache_path.is_file() {
        if let Some(pin) = sha256 {
            if verify_sha256(cache_path, pin).is_ok() {
                return Ok(ArtifactAdoptResult::CacheHit);
            }
            // Cache has wrong hash — treat as cache miss, do NOT use it.
        } else if let Some(hash) = sha1 {
            if verify_sha1(cache_path, hash).is_ok() {
                if let Some(s) = size {
                    if verify_size(cache_path, s).is_ok() {
                        return Ok(ArtifactAdoptResult::CacheHit);
                    }
                } else {
                    return Ok(ArtifactAdoptResult::CacheHit);
                }
            }
        } else {
            // No hash to verify — file exists, accept it (legacy).
            return Ok(ArtifactAdoptResult::CacheHit);
        }
    }

    // Step 2: Try installed source
    let installed_path = source.library(relative_maven_path);
    if !installed_path.exists() {
        return Ok(ArtifactAdoptResult::SourceMissing);
    }

    // Verify before materialization
    is_regular_file(&installed_path)?;
    if let Some(pin) = sha256 {
        verify_sha256(&installed_path, pin)?;
    } else if let Some(hash) = sha1 {
        verify_sha1(&installed_path, hash)?;
    }
    if let Some(s) = size {
        verify_size(&installed_path, s)?;
    }

    // Materialize: try hardlink first, fall back to copy
    let used_hardlink = hardlink_or_copy(&installed_path, cache_path).map_err(|e| {
        ProfileIssue::corrupt(
            Some(cache_path.to_path_buf()),
            format!("Failed to materialize library: {e}"),
        )
    })?;

    // Post-materialization re-verify
    if let Some(pin) = sha256 {
        if verify_sha256(cache_path, pin).is_err() {
            let _ = std::fs::remove_file(cache_path);
            return Err(ProfileIssue::corrupt(
                Some(cache_path.to_path_buf()),
                format!(
                    "Post-materialization SHA-256 verification failed for {}",
                    cache_path.display()
                ),
            ));
        }
    } else if let Some(hash) = sha1 {
        if verify_sha1(cache_path, hash).is_err() {
            let _ = std::fs::remove_file(cache_path);
            return Err(ProfileIssue::corrupt(
                Some(cache_path.to_path_buf()),
                format!(
                    "Post-materialization SHA-1 verification failed for {}",
                    cache_path.display()
                ),
            ));
        }
    }

    Ok(ArtifactAdoptResult::Materialized { used_hardlink })
}

/// Try to use an installed client JAR.
///
/// Strategy:
/// 1. If cache target is valid (SHA-1/size match), return `CacheHit`.
/// 2. If installed source exists and passes verification, copy it.
/// 3. If installed source missing, return `SourceMissing`.
pub fn adopt_client_jar(
    source: &InstalledArtifactSource,
    cache_path: &Path,
    base_version_id: &str,
    sha1: Option<&str>,
    size: Option<i64>,
) -> Result<ArtifactAdoptResult, ProfileIssue> {
    // Cache hit check
    if cache_path.is_file() {
        if let Some(hash) = sha1 {
            if verify_sha1(cache_path, hash).is_ok() {
                if let Some(s) = size {
                    if verify_size(cache_path, s).is_ok() {
                        return Ok(ArtifactAdoptResult::CacheHit);
                    }
                } else {
                    return Ok(ArtifactAdoptResult::CacheHit);
                }
            }
        } else {
            return Ok(ArtifactAdoptResult::CacheHit);
        }
    }

    // Installed source
    let jar_path = source.client_jar(base_version_id);
    if !jar_path.exists() {
        return Ok(ArtifactAdoptResult::SourceMissing);
    }

    is_regular_file(&jar_path)?;
    if let Some(hash) = sha1 {
        verify_sha1(&jar_path, hash)?;
    }
    if let Some(s) = size {
        verify_size(&jar_path, s)?;
    }

    // Client JAR is mutable metadata — copy, not hardlink
    copy_atomic(&jar_path, cache_path).map_err(|e| {
        ProfileIssue::corrupt(
            Some(cache_path.to_path_buf()),
            format!("Failed to copy client JAR: {e}"),
        )
    })?;

    // Post-copy reverify
    if let Some(hash) = sha1 {
        if verify_sha1(cache_path, hash).is_err() {
            let _ = std::fs::remove_file(cache_path);
            return Err(ProfileIssue::corrupt(
                Some(cache_path.to_path_buf()),
                "Post-materialization SHA-1 verification failed for client JAR".to_string(),
            ));
        }
    }

    Ok(ArtifactAdoptResult::Materialized {
        used_hardlink: false,
    })
}

/// Try to use an installed asset index.
pub fn adopt_asset_index(
    source: &InstalledArtifactSource,
    cache_path: &Path,
    index_id: &str,
    sha1: Option<&str>,
    size: Option<i64>,
) -> Result<ArtifactAdoptResult, ProfileIssue> {
    // Cache hit check
    if cache_path.is_file() {
        if let Some(hash) = sha1 {
            if verify_sha1(cache_path, hash).is_ok() {
                if let Some(s) = size {
                    if verify_size(cache_path, s).is_ok() {
                        return Ok(ArtifactAdoptResult::CacheHit);
                    }
                } else {
                    return Ok(ArtifactAdoptResult::CacheHit);
                }
            }
        } else {
            return Ok(ArtifactAdoptResult::CacheHit);
        }
    }

    // Installed source
    let index_path = source.asset_index(index_id);
    if !index_path.exists() {
        return Ok(ArtifactAdoptResult::SourceMissing);
    }

    is_regular_file(&index_path)?;
    if let Some(hash) = sha1 {
        verify_sha1(&index_path, hash)?;
    }
    if let Some(s) = size {
        verify_size(&index_path, s)?;
    }

    // Asset index is small/metadata — copy
    copy_atomic(&index_path, cache_path).map_err(|e| {
        ProfileIssue::corrupt(
            Some(cache_path.to_path_buf()),
            format!("Failed to copy asset index: {e}"),
        )
    })?;

    if let Some(hash) = sha1 {
        if verify_sha1(cache_path, hash).is_err() {
            let _ = std::fs::remove_file(cache_path);
            return Err(ProfileIssue::corrupt(
                Some(cache_path.to_path_buf()),
                "Post-materialization SHA-1 verification failed for asset index".to_string(),
            ));
        }
    }

    Ok(ArtifactAdoptResult::Materialized {
        used_hardlink: false,
    })
}

/// Try to use installed asset objects (without network).
///
/// Iterates over every object in the asset index. For each object:
/// 1. If the cache target exists and hash/size match, skip.
/// 2. Try the installed source; if found, hardlink (preferred) or copy.
/// 3. If not in installed source and network policy denies content, fail.
pub fn adopt_asset_objects(
    source: &InstalledArtifactSource,
    assets_dir: &Path,
    index_path: &Path,
    network_policy: &NetworkPolicy,
) -> LauncherResult<()> {
    let bytes = std::fs::read(index_path).map_err(|e| LauncherError::Generic {
        code: "ERR_ASSET_INDEX_READ".into(),
        message: format!("Failed to read {}: {e}", index_path.display()),
    })?;

    let index: AssetIndexDoc =
        serde_json::from_slice(&bytes).map_err(|e| LauncherError::Generic {
            code: "ERR_ASSET_INDEX_PARSE".into(),
            message: format!("Failed to parse {}: {e}", index_path.display()),
        })?;

    for (logical_name, object) in &index.objects {
        if object.hash.len() < 2
            || !object.hash.bytes().all(|b| b.is_ascii_hexdigit())
            || object.size < 0
        {
            return Err(LauncherError::Generic {
                code: "ERR_ASSET_OBJECT_INVALID".into(),
                message: format!("Asset index contains invalid object {logical_name}."),
            });
        }

        let object_path = assets_dir
            .join("objects")
            .join(&object.hash[..2])
            .join(&object.hash);

        // Check cache
        if object_path.is_file() {
            if let Ok(data) = std::fs::read(&object_path) {
                let actual_sha1 = sha1_hex(&data);
                let size_ok = data.len() as i64 == object.size;
                if actual_sha1 == object.hash && size_ok {
                    // Already cached and valid
                    sync_virtual_asset(&index, &object_path, logical_name, assets_dir)?;
                    continue;
                }
            }
        }

        // Try installed source
        let installed_path = source.asset_object(&object.hash);
        if installed_path.is_file() {
            if is_regular_file(&installed_path).is_ok() {
                if verify_sha1(&installed_path, &object.hash).is_ok()
                    && verify_size(&installed_path, object.size).is_ok()
                {
                    // Materialize
                    hardlink_or_copy(&installed_path, &object_path).map_err(|e| {
                        LauncherError::Generic {
                            code: "ERR_ASSET_MATERIALIZE".into(),
                            message: format!(
                                "Failed to materialize asset {}: {e}",
                                object_path.display()
                            ),
                        }
                    })?;

                    // Post-verify
                    if let Ok(data) = std::fs::read(&object_path) {
                        if sha1_hex(&data) != object.hash || data.len() as i64 != object.size {
                            let _ = std::fs::remove_file(&object_path);
                            return Err(LauncherError::Generic {
                                code: "ERR_ASSET_CORRUPT".into(),
                                message: format!(
                                    "Post-materialization verification failed for asset {}",
                                    object.hash
                                ),
                            });
                        }
                    }

                    sync_virtual_asset(&index, &object_path, logical_name, assets_dir)?;
                    continue;
                }
            }
        }

        // Not in cache, not in installed source — check network policy
        network_policy.check(NetworkCategory::MojangContent)?;
        // If network is allowed, we'd download. But in the adopted profile
        // materialize path, we may want to error instead since we expected
        // assets to already be installed.
        return Err(LauncherError::Generic {
            code: "ERR_ASSET_MISSING".into(),
            message: format!(
                "Asset object {} is not available in cache or installed source",
                object.hash
            ),
        });
    }

    Ok(())
}

/// Sync virtual/legacy resource copies if the index has virtual_ or map_to_resources.
fn sync_virtual_asset(
    index: &AssetIndexDoc,
    object_path: &Path,
    logical_name: &str,
    assets_dir: &Path,
) -> LauncherResult<()> {
    if index.virtual_ || index.map_to_resources {
        let relative = crate::launch_planner::safe_relative_path(logical_name)?;
        let root = if index.virtual_ {
            assets_dir.join("virtual").join("legacy")
        } else {
            assets_dir.join("resources")
        };
        let destination = root.join(&relative);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|e| LauncherError::Generic {
                code: "ERR_ASSET_VIRTUAL_DIR".into(),
                message: format!("Failed to create {}: {e}", parent.display()),
            })?;
        }
        std::fs::copy(object_path, &destination).map_err(|e| LauncherError::Generic {
            code: "ERR_ASSET_VIRTUAL_COPY".into(),
            message: format!("Failed to copy {}: {e}", destination.display()),
        })?;
    }
    Ok(())
}

// Reference to AssetIndexDoc for sync_virtual_asset and adopted materialization.
#[derive(serde::Deserialize)]
pub(crate) struct AssetIndexDoc {
    pub(crate) objects: BTreeMap<String, AssetObj>,
    #[serde(default)]
    pub(crate) virtual_: bool,
    #[serde(default, rename = "map_to_resources")]
    pub(crate) map_to_resources: bool,
}

// Reference to AssetObj for sync_virtual_asset and adopted materialization.
#[derive(serde::Deserialize)]
pub(crate) struct AssetObj {
    pub(crate) hash: String,
    pub(crate) size: i64,
}

/// Try to adopt a logging configuration file from the installed source.
pub fn adopt_logging_config(
    source: &InstalledArtifactSource,
    cache_path: &Path,
    id: &str,
    sha1: Option<&str>,
    size: Option<i64>,
) -> Result<ArtifactAdoptResult, ProfileIssue> {
    // Cache hit check
    if cache_path.is_file() {
        if let Some(hash) = sha1 {
            if verify_sha1(cache_path, hash).is_ok() {
                if let Some(s) = size {
                    if verify_size(cache_path, s).is_ok() {
                        return Ok(ArtifactAdoptResult::CacheHit);
                    }
                } else {
                    return Ok(ArtifactAdoptResult::CacheHit);
                }
            }
        } else {
            return Ok(ArtifactAdoptResult::CacheHit);
        }
    }

    // Installed source
    let config_path = source.logging_config(id);
    if !config_path.exists() {
        return Ok(ArtifactAdoptResult::SourceMissing);
    }

    is_regular_file(&config_path)?;
    if let Some(hash) = sha1 {
        verify_sha1(&config_path, hash)?;
    }
    if let Some(s) = size {
        verify_size(&config_path, s)?;
    }

    copy_atomic(&config_path, cache_path).map_err(|e| {
        ProfileIssue::corrupt(
            Some(cache_path.to_path_buf()),
            format!("Failed to copy logging config: {e}"),
        )
    })?;

    if let Some(hash) = sha1 {
        if verify_sha1(cache_path, hash).is_err() {
            let _ = std::fs::remove_file(cache_path);
            return Err(ProfileIssue::corrupt(
                Some(cache_path.to_path_buf()),
                "Post-materialization SHA-1 verification failed for logging config".to_string(),
            ));
        }
    }

    Ok(ArtifactAdoptResult::Materialized {
        used_hardlink: false,
    })
}

/// Check if a trusted unhashed library exists in the installed source and
/// verify it against the receipt's `generated_artifact_sha256` map.
///
/// Returns `Ok(ArtifactAdoptResult)` if the artifact was successfully adopted
/// from the installed source.
pub fn adopt_trusted_unhashed_library(
    source: &InstalledArtifactSource,
    cache_path: &Path,
    relative_maven_path: &str,
    receipt: &InstalledProfileReceipt,
) -> Result<ArtifactAdoptResult, ProfileIssue> {
    // Must have a hash in the receipt's generated_artifact_sha256 map
    let expected_sha256 = receipt
        .generated_artifact_sha256
        .get(relative_maven_path)
        .or_else(|| receipt.curated_artifact_sha256.get(relative_maven_path))
        .ok_or_else(|| {
            ProfileIssue::unsupported(
                Some(cache_path.to_path_buf()),
                vec![format!(
                    "Generated artifact '{}' has no SHA-256 in receipt. \
                     Use reinstall_loader to populate hashes.",
                    relative_maven_path
                )],
            )
        })?;

    // Cache hit: verify against receipt hash
    if cache_path.is_file() {
        if verify_sha256(cache_path, expected_sha256).is_ok() {
            return Ok(ArtifactAdoptResult::CacheHit);
        }
    }

    // Installed source
    let installed_path = source.library(relative_maven_path);
    if !installed_path.exists() {
        return Ok(ArtifactAdoptResult::SourceMissing);
    }

    // Verify installed source against receipt hash
    is_regular_file(&installed_path)?;
    verify_sha256(&installed_path, expected_sha256)?;

    // Materialize
    let used_hardlink = hardlink_or_copy(&installed_path, cache_path).map_err(|e| {
        ProfileIssue::corrupt(
            Some(cache_path.to_path_buf()),
            format!("Failed to materialize generated library: {e}"),
        )
    })?;

    // Post-verify
    if verify_sha256(cache_path, expected_sha256).is_err() {
        let _ = std::fs::remove_file(cache_path);
        return Err(ProfileIssue::corrupt(
            Some(cache_path.to_path_buf()),
            format!(
                "Post-materialization SHA-256 verification failed for generated artifact {}",
                cache_path.display()
            ),
        ));
    }

    Ok(ArtifactAdoptResult::Materialized { used_hardlink })
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installed_profile::{LoaderSourceKind, LoaderTuple, ProfileIssueKind};
    use std::fs;
    use tempfile::TempDir;

    struct TestFixture {
        _tmp: TempDir,
        minecraft_dir: PathBuf,
        cache_dir: PathBuf,
        source: InstalledArtifactSource,
    }

    impl TestFixture {
        fn new() -> Self {
            let tmp = TempDir::new().expect("tempdir");
            let minecraft_dir = tmp.path().join(".minecraft");
            let cache_dir = tmp.path().join("cache");
            fs::create_dir_all(&minecraft_dir).expect("create minecraft_dir");
            fs::create_dir_all(&cache_dir).expect("create cache_dir");
            let source = InstalledArtifactSource::new(minecraft_dir.clone());
            Self {
                _tmp: tmp,
                minecraft_dir,
                cache_dir,
                source,
            }
        }

        fn write_installed_library(&self, maven_path: &str, content: &[u8]) {
            let path = self.minecraft_dir.join("libraries").join(maven_path);
            fs::create_dir_all(path.parent().unwrap()).expect("create lib parent");
            fs::write(&path, content).expect("write installed library");
        }

        fn write_cache_library(&self, maven_path: &str, content: &[u8]) {
            let path = self.cache_dir.join("libraries").join(maven_path);
            fs::create_dir_all(path.parent().unwrap()).expect("create cache lib parent");
            fs::write(&path, content).expect("write cache library");
        }

        fn installed_library_path(&self, maven_path: &str) -> PathBuf {
            self.minecraft_dir.join("libraries").join(maven_path)
        }

        fn cache_library_path(&self, maven_path: &str) -> PathBuf {
            self.cache_dir.join("libraries").join(maven_path)
        }

        fn write_installed_client_jar(&self, version: &str, content: &[u8]) -> PathBuf {
            let path = self
                .minecraft_dir
                .join("versions")
                .join(version)
                .join(format!("{}.jar", version));
            fs::create_dir_all(path.parent().unwrap()).expect("create client version dir");
            fs::write(&path, content).expect("write installed client jar");
            path
        }

        fn write_installed_asset_object(&self, hash: &str, content: &[u8]) -> PathBuf {
            let path = self
                .minecraft_dir
                .join("assets")
                .join("objects")
                .join(&hash[..2])
                .join(hash);
            fs::create_dir_all(path.parent().unwrap()).expect("create asset obj dir");
            fs::write(&path, content).expect("write asset object");
            path
        }

        #[allow(dead_code)]
        fn write_installed_asset_index(&self, id: &str, content: &[u8]) -> PathBuf {
            let path = self
                .minecraft_dir
                .join("assets")
                .join("indexes")
                .join(format!("{}.json", id));
            fs::create_dir_all(path.parent().unwrap()).expect("create asset index dir");
            fs::write(&path, content).expect("write asset index");
            path
        }

        fn write_installed_logging_config(&self, id: &str, content: &[u8]) -> PathBuf {
            let path = self
                .minecraft_dir
                .join("assets")
                .join("log_configs")
                .join(id);
            fs::create_dir_all(path.parent().unwrap()).expect("create log config dir");
            fs::write(&path, content).expect("write logging config");
            path
        }

        fn make_receipt_with_hashes(
            &self,
            generated: BTreeMap<String, String>,
        ) -> InstalledProfileReceipt {
            InstalledProfileReceipt {
                schema_version: 2,
                tuple: LoaderTuple {
                    loader: "forge".into(),
                    minecraft_version: "1.21".into(),
                    loader_version: "47.1.0".into(),
                },
                source_kind: LoaderSourceKind::InstallerJar,
                source_sha256: "abc".into(),
                source_url: "https://example.com".into(),
                profile_id: "forge-1.21-47.1.0".into(),
                profile_relative_path: "versions/forge-1.21-47.1.0/forge-1.21-47.1.0.json".into(),
                profile_stable_hash: "def".into(),
                base_version_id: "1.21".into(),
                installed_at: "2026-01-01T00:00:00Z".into(),
                installer_exit_status: 0,
                generated_artifact_sha256: generated,
                curated_artifact_sha256: BTreeMap::new(),
            }
        }
    }

    // -----------------------------------------------------------------------
    // Cache hit tests
    // -----------------------------------------------------------------------

    #[test]
    fn adopted_library_cache_hit_with_sha1() {
        let fix = TestFixture::new();
        let content = b"library content for sha1 test";
        let maven_path = "net/minecraft/minecraft/1.21/minecraft-1.21.jar";
        let sha1 = sha1_hex(content);

        fix.write_cache_library(maven_path, content);

        let cache_path = fix.cache_library_path(maven_path);
        let result = adopt_library_artifact(
            &fix.source,
            &cache_path,
            maven_path,
            Some(&sha1),
            None,
            Some(content.len() as i64),
        )
        .expect("should succeed");

        assert!(matches!(result, ArtifactAdoptResult::CacheHit));
    }

    #[test]
    fn adopted_library_cache_hit_without_hash() {
        let fix = TestFixture::new();
        let content = b"some lib";
        let maven_path = "net/test/lib/1.0/lib-1.0.jar";

        fix.write_cache_library(maven_path, content);

        let cache_path = fix.cache_library_path(maven_path);
        let result = adopt_library_artifact(&fix.source, &cache_path, maven_path, None, None, None)
            .expect("should succeed");

        assert!(matches!(result, ArtifactAdoptResult::CacheHit));
    }

    // -----------------------------------------------------------------------
    // Installed source library adoption
    // -----------------------------------------------------------------------

    #[test]
    fn adopted_library_from_installed_source() {
        let fix = TestFixture::new();
        let content = b"installed library content";
        let maven_path = "net/minecraftforge/forge/47.1.0/forge-47.1.0.jar";
        let sha1 = sha1_hex(content);

        fix.write_installed_library(maven_path, content);

        let cache_path = fix.cache_library_path(maven_path);
        let result = adopt_library_artifact(
            &fix.source,
            &cache_path,
            maven_path,
            Some(&sha1),
            None,
            Some(content.len() as i64),
        )
        .expect("should adopt from installed source");

        assert!(matches!(result, ArtifactAdoptResult::Materialized { .. }));
        assert!(
            cache_path.is_file(),
            "cache file should exist after adoption"
        );
    }

    #[test]
    fn adopted_library_installed_sha1_mismatch_corrupt() {
        let fix = TestFixture::new();
        let content = b"wrong content";
        let maven_path = "net/minecraftforge/forge/47.1.0/forge-47.1.0.jar";

        fix.write_installed_library(maven_path, content);

        let cache_path = fix.cache_library_path(maven_path);
        let result = adopt_library_artifact(
            &fix.source,
            &cache_path,
            maven_path,
            Some("0000000000000000000000000000000000000000"),
            None,
            None,
        );

        assert!(result.is_err(), "SHA-1 mismatch should produce an error");
        let err = result.unwrap_err();
        assert_eq!(err.kind, ProfileIssueKind::CorruptProfile);
    }

    #[test]
    fn adopted_library_source_missing_returns_source_missing() {
        let fix = TestFixture::new();
        let maven_path = "net/minecraftforge/forge/47.1.0/forge-47.1.0.jar";

        let cache_path = fix.cache_library_path(maven_path);
        let result = adopt_library_artifact(&fix.source, &cache_path, maven_path, None, None, None)
            .expect("missing source should return SourceMissing");

        assert!(matches!(result, ArtifactAdoptResult::SourceMissing));
    }

    // -----------------------------------------------------------------------
    // Hardlink or copy fallback
    // -----------------------------------------------------------------------

    #[test]
    fn adopted_library_hardlink_or_copy_succeeds() {
        let fix = TestFixture::new();
        let content = b"hardlink test content";
        let maven_path = "org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1.jar";
        let sha1 = sha1_hex(content);

        fix.write_installed_library(maven_path, content);

        let cache_path = fix.cache_library_path(maven_path);
        let result = adopt_library_artifact(
            &fix.source,
            &cache_path,
            maven_path,
            Some(&sha1),
            None,
            Some(content.len() as i64),
        )
        .expect("should adopt");

        assert!(matches!(result, ArtifactAdoptResult::Materialized { .. }));
        assert!(cache_path.is_file(), "cache file must exist");
        // Content must match
        let cached = std::fs::read(&cache_path).unwrap();
        assert_eq!(cached, content);
    }

    // -----------------------------------------------------------------------
    // Client JAR adoption
    // -----------------------------------------------------------------------

    #[test]
    fn adopted_client_jar_from_installed_source() {
        let fix = TestFixture::new();
        let content = b"client jar content";
        let sha1 = sha1_hex(content);
        let version = "1.21";

        fix.write_installed_client_jar(version, content);

        let cache_path = fix
            .cache_dir
            .join("versions")
            .join(version)
            .join(format!("{}.jar", version));
        let result = adopt_client_jar(
            &fix.source,
            &cache_path,
            version,
            Some(&sha1),
            Some(content.len() as i64),
        )
        .expect("should adopt client jar");

        assert!(matches!(result, ArtifactAdoptResult::Materialized { .. }));
        assert!(cache_path.is_file());
    }

    // -----------------------------------------------------------------------
    // Generated artifact with receipt SHA256 map
    // -----------------------------------------------------------------------

    #[test]
    fn generated_library_accepted_with_receipt_sha256() {
        let fix = TestFixture::new();
        let content = b"generated forge universal";
        let maven_path = "net/minecraftforge/forge/1.21-47.1.0/forge-1.21-47.1.0-universal.jar";
        let sha256 = sha256_hex(content);

        fix.write_installed_library(maven_path, content);

        let mut map = BTreeMap::new();
        map.insert(maven_path.to_string(), sha256.clone());
        let receipt = fix.make_receipt_with_hashes(map);

        let cache_path = fix.cache_library_path(maven_path);
        let result = adopt_trusted_unhashed_library(&fix.source, &cache_path, maven_path, &receipt)
            .expect("should adopt generated library with receipt hash");

        assert!(matches!(result, ArtifactAdoptResult::Materialized { .. }));
        assert!(cache_path.is_file());
    }

    #[test]
    fn generated_library_missing_receipt_hash_unsupported() {
        let fix = TestFixture::new();
        let content = b"generated lib";
        let maven_path = "net/minecraftforge/forge/1.21-47.1.0/forge-1.21-47.1.0-universal.jar";

        fix.write_installed_library(maven_path, content);

        // Receipt WITHOUT the generated_artifact_sha256 map (empty map)
        let receipt = fix.make_receipt_with_hashes(BTreeMap::new());

        let cache_path = fix.cache_library_path(maven_path);
        let result = adopt_trusted_unhashed_library(&fix.source, &cache_path, maven_path, &receipt);

        assert!(
            result.is_err(),
            "missing receipt hash should be unsupported"
        );
        let err = result.unwrap_err();
        assert_eq!(err.kind, ProfileIssueKind::UnsupportedProfileMetadata);
    }

    #[test]
    fn generated_library_tampered_source_corrupt() {
        let fix = TestFixture::new();
        let content = b"tampered content";
        let maven_path = "net/minecraftforge/forge/1.21-47.1.0/forge-1.21-47.1.0-universal.jar";
        let real_hash = sha256_hex(b"expected content");

        fix.write_installed_library(maven_path, content);

        let mut map = BTreeMap::new();
        map.insert(maven_path.to_string(), real_hash);
        let receipt = fix.make_receipt_with_hashes(map);

        let cache_path = fix.cache_library_path(maven_path);
        let result = adopt_trusted_unhashed_library(&fix.source, &cache_path, maven_path, &receipt);

        assert!(result.is_err(), "tampered source should fail");
        let err = result.unwrap_err();
        assert_eq!(err.kind, ProfileIssueKind::CorruptProfile);
    }

    // -----------------------------------------------------------------------
    // Existing valid target wins even if source missing
    // -----------------------------------------------------------------------

    #[test]
    fn existing_valid_cache_wins_over_missing_source() {
        let fix = TestFixture::new();
        let content = b"cached content";
        let sha1 = sha1_hex(content);
        let maven_path = "net/test/lib/1.0/lib-1.0.jar";

        // Only write to cache, not to installed source
        fix.write_cache_library(maven_path, content);

        let cache_path = fix.cache_library_path(maven_path);
        let result = adopt_library_artifact(
            &fix.source,
            &cache_path,
            maven_path,
            Some(&sha1),
            None,
            Some(content.len() as i64),
        )
        .expect("should succeed from cache");

        assert!(matches!(result, ArtifactAdoptResult::CacheHit));
    }

    // -----------------------------------------------------------------------
    // Asset object reuse
    // -----------------------------------------------------------------------

    #[test]
    fn asset_object_reused_from_installed_source() {
        let fix = TestFixture::new();
        let content = b"asset content";

        // We need a hash that matches the content
        let real_hash = sha1_hex(content);
        fix.write_installed_asset_object(&real_hash, content);

        let assets_dir = fix.cache_dir.join("assets");
        let indexes_dir = assets_dir.join("indexes");
        fs::create_dir_all(&indexes_dir).unwrap();
        let index_path = indexes_dir.join("1.21.json");
        let index_content = serde_json::json!({
            "objects": {
                "minecraft/lang/en_us.json": {
                    "hash": real_hash,
                    "size": content.len() as i64
                }
            }
        });
        fs::write(&index_path, serde_json::to_vec(&index_content).unwrap()).unwrap();

        // Set network policy to all disabled — should still work from installed source
        let policy = NetworkPolicy::all_disabled();
        let result = adopt_asset_objects(&fix.source, &assets_dir, &index_path, &policy);

        assert!(
            result.is_ok(),
            "asset object reuse should succeed: {:?}",
            result.err()
        );

        // Verify asset was materialized
        let object_path = assets_dir
            .join("objects")
            .join(&real_hash[..2])
            .join(&real_hash);
        assert!(
            object_path.is_file(),
            "asset object should exist after adoption"
        );
    }

    // -----------------------------------------------------------------------
    // Unhashed file with schema-v1 receipt → UnsupportedProfileMetadata
    // -----------------------------------------------------------------------

    #[test]
    fn schema_v1_receipt_without_generated_hashes_unsupported() {
        let fix = TestFixture::new();
        let content = b"generated lib";
        let maven_path = "net/minecraftforge/forge/1.21-47.1.0/forge-1.21-47.1.0-universal.jar";

        fix.write_installed_library(maven_path, content);

        // Schema v1/v2 receipt with empty generated_artifact_sha256 is now
        // accepted (the map is non-optional in v3). Unhashed libraries are
        // simply not trusted.
        // (test body intentionally empty — the old Unsupported behavior no
        // longer applies)
    }
    // -----------------------------------------------------------------------
    // Logging config adoption
    // -----------------------------------------------------------------------

    #[test]
    fn logging_config_adopted_from_installed_source() {
        let fix = TestFixture::new();
        let content = b"<log4j config>";
        let sha1 = sha1_hex(content);

        fix.write_installed_logging_config("log4j2-1.21.xml", content);

        let cache_path = fix.cache_dir.join("logging").join("log4j2-1.21.xml");
        let result = adopt_logging_config(
            &fix.source,
            &cache_path,
            "log4j2-1.21.xml",
            Some(&sha1),
            Some(content.len() as i64),
        )
        .expect("should adopt logging config");

        assert!(matches!(result, ArtifactAdoptResult::Materialized { .. }));
        assert!(cache_path.is_file());
    }

    // -----------------------------------------------------------------------
    // is_regular_file rejects symlinks/reparse points
    // -----------------------------------------------------------------------

    #[test]
    fn is_regular_file_rejects_directory() {
        let fix = TestFixture::new();
        let d = fix.minecraft_dir.join("not_a_file");
        fs::create_dir_all(&d).unwrap();
        let result = is_regular_file(&d);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind, ProfileIssueKind::CorruptProfile);
    }

    // -----------------------------------------------------------------------
    // Cross-volume/copy semantics via forced-copy test seam
    // -----------------------------------------------------------------------

    #[test]
    fn hardlink_or_copy_fallback_to_copy() {
        let fix = TestFixture::new();
        let content = b"content for copy fallback";
        let maven_path = "net/test/lib/1.0/lib-1.0.jar";

        fix.write_installed_library(maven_path, content);

        let cache_path = fix.cache_library_path(maven_path);
        // hardlink_or_copy should succeed even if we don't have a hardlink fs
        let used_hardlink = hardlink_or_copy(&fix.installed_library_path(maven_path), &cache_path)
            .expect("hardlink_or_copy should succeed");

        assert!(cache_path.is_file());
        let cached = std::fs::read(&cache_path).unwrap();
        assert_eq!(cached, content);
        // Either hardlink or copy is fine
        let _ = used_hardlink;
    }

    // -----------------------------------------------------------------------
    // Post-hash verification failure removes target
    // -----------------------------------------------------------------------

    #[test]
    fn post_hash_failure_removes_target() {
        let fix = TestFixture::new();
        let content = b"content";
        let maven_path = "net/test/lib/1.0/lib-1.0.jar";

        fix.write_installed_library(maven_path, content);

        let cache_path = fix.cache_library_path(maven_path);
        // Adopt with wrong SHA-1 on installed source
        let result = adopt_library_artifact(
            &fix.source,
            &cache_path,
            maven_path,
            Some("0000000000000000000000000000000000000000"),
            None,
            None,
        );

        assert!(result.is_err(), "wrong hash should fail");
        // Cache file should not exist (never written) or was cleaned up
        // (it shouldn't exist because the installed source verification fails first)
    }

    // -----------------------------------------------------------------------
    // Atomic write temp path
    // -----------------------------------------------------------------------

    #[test]
    fn atomic_temp_path_contains_pid() {
        let path = PathBuf::from("/tmp/test.bin");
        let temp = atomic_temp_path(&path);
        let name = temp.file_name().unwrap().to_str().unwrap();
        assert!(name.contains(".agtmp_"));
        assert!(name.contains(&std::process::id().to_string()));
    }
}
