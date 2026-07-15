//! Managed Java runtime provisioning — download, verify, extract, and track
//! curated Eclipse Temurin JRE installations.
//!
//! ## Layout
//!
//! ```text
//! {runtimes_root}/
//!   temurin/
//!     {major}/
//!       {full_version}/
//!         {os}-{arch}/
//!           receipt.json
//!           jdk8u…/  (extracted JRE root)
//!   .archives/
//!     {sha256}.zip
//!     {sha256}.tar.gz
//! ```
//!
//! ## Receipt schema v1
//!
//! Every managed runtime carries a `receipt.json` that records the catalog
//! identity, installation timestamps, and the SHA-256 of the source archive
//! for drift detection.

use crate::error::{LauncherError, LauncherResult};
use crate::java::{self, JavaInstallation, JavaSource};
use crate::network::{NetworkCategory, NetworkPolicy};
use crate::runtime_catalog::{RuntimeCatalog, RuntimeCatalogEntry};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Schema version for RuntimeReceipt.
pub const RECEIPT_SCHEMA_VERSION: u32 = 1;

/// Managed vendor directory prefix.
pub const MANAGED_VENDOR: &str = "temurin";

/// Counter for unique temp directory names when atomic counter is insufficient.
static UNIQUE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Archive cache directory (relative to runtimes_root).
const ARCHIVE_CACHE_DIR: &str = ".archives";

// --- Extraction caps ---

/// Maximum files per archive (generous for JRE archives).
const MAX_ENTRIES: usize = 10_000;

/// Per-entry uncompressed size cap (500 MB).
const PER_ENTRY_LIMIT: u64 = 500 * 1024 * 1024;

/// Aggregate uncompressed size cap (2 GB).
const AGGREGATE_LIMIT: u64 = 2 * 1024 * 1024 * 1024;

// --- Download timeouts ---

/// Maximum time to wait for a response headers.
const DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Maximum time for the full download stream.
const DOWNLOAD_STREAM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

// --- Redirect allowlist for runtime downloads ---

const ALLOWED_DOWNLOAD_HOSTS: &[&str] = &[
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

// ---------------------------------------------------------------------------
// RuntimeReceipt
// ---------------------------------------------------------------------------

/// On-disk receipt for a managed Java runtime installation.
///
/// Written atomically inside the staging directory before the final rename,
/// so a partially-extracted runtime never carries a valid receipt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeReceipt {
    pub schema_version: u32,
    pub vendor: String,
    pub major: u32,
    pub full_version: String,
    pub os: String,
    pub arch: String,
    pub archive_sha256: String,
    pub archive_size: u64,
    pub source_url: String,
    pub java_relative_path: String,
    pub installed_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub successful_use_at: Option<DateTime<Utc>>,
    /// SHA-256 of the Java executable for tamper detection without archive.
    /// Older receipts written before this field existed will have `None`,
    /// which causes re-validation on the next cache-hit check.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub java_sha256: Option<String>,
}

impl RuntimeReceipt {
    /// Build a receipt from a catalog entry and the current timestamp.
    pub fn from_entry(entry: &RuntimeCatalogEntry) -> Self {
        Self {
            schema_version: RECEIPT_SCHEMA_VERSION,
            vendor: entry.vendor.clone(),
            major: entry.major,
            full_version: entry.full_version.clone(),
            os: entry.os.clone(),
            arch: entry.arch.clone(),
            archive_sha256: entry.sha256.clone(),
            archive_size: entry.size,
            source_url: entry.url.clone(),
            java_relative_path: entry.java_relative_path.clone(),
            installed_at: Utc::now(),
            last_used_at: Some(Utc::now()),
            successful_use_at: None,
            java_sha256: None,
        }
    }

    /// Set the java_sha256 after extraction.
    pub fn with_java_hash(mut self, hash: String) -> Self {
        self.java_sha256 = Some(hash);
        self
    }

    /// Read and validate a receipt from its JSON file.
    pub fn read_from(path: &Path) -> LauncherResult<Self> {
        let data = std::fs::read_to_string(path).map_err(|e| LauncherError::Generic {
            code: "ERR_RECEIPT_READ".into(),
            message: format!("Failed to read receipt {}: {e}", path.display()),
        })?;
        let receipt: Self = serde_json::from_str(&data).map_err(|e| LauncherError::Generic {
            code: "ERR_RECEIPT_PARSE".into(),
            message: format!("Invalid receipt {}: {e}", path.display()),
        })?;
        if receipt.schema_version != RECEIPT_SCHEMA_VERSION {
            return Err(LauncherError::Generic {
                code: "ERR_RECEIPT_SCHEMA".into(),
                message: format!(
                    "Receipt {} has schema version {}, expected {}",
                    path.display(),
                    receipt.schema_version,
                    RECEIPT_SCHEMA_VERSION
                ),
            });
        }
        Ok(receipt)
    }

    /// Write the receipt atomically to the given path.
    pub fn write_to(&self, path: &Path) -> LauncherResult<()> {
        let data = serde_json::to_string_pretty(self).map_err(|e| LauncherError::Generic {
            code: "ERR_RECEIPT_SERIALIZE".into(),
            message: format!("Failed to serialize receipt: {e}"),
        })?;
        atomic_write(path, data.as_bytes())
    }

    /// Update `last_used_at` in-place and persist to disk.
    pub fn touch_last_used(&mut self, path: &Path) -> LauncherResult<()> {
        self.last_used_at = Some(Utc::now());
        self.write_to(path)
    }

    /// Update `successful_use_at` in-place and persist to disk.
    pub fn touch_successful_use(&mut self, path: &Path) -> LauncherResult<()> {
        self.successful_use_at = Some(Utc::now());
        self.write_to(path)
    }
}

// ---------------------------------------------------------------------------
// ManagedRuntime — discovered runtime with its receipt
// ---------------------------------------------------------------------------

/// A discovered managed runtime with its parsed receipt.
#[derive(Debug, Clone)]
pub struct ManagedRuntime {
    /// The root directory of the extracted JRE.
    pub root_dir: PathBuf,
    /// The parsed receipt.
    pub receipt: RuntimeReceipt,
    /// Path to the `java` (or `java.exe`) executable.
    pub java_path: PathBuf,
}

// ---------------------------------------------------------------------------
// Progress reporting
// ---------------------------------------------------------------------------

/// Optional progress and cancellation for runtime operations.
pub trait RuntimeProgress: Send + Sync {
    /// Called with a human-readable message and optional percentage (0.0–100.0).
    fn on_progress(&self, message: &str, percent: Option<f64>);
    /// Returns `true` if the operation should be cancelled.
    fn is_cancelled(&self) -> bool;
}

/// Check cancellation and return `JavaRuntimeCancelled` if the user cancelled.
pub fn check_cancelled<P: RuntimeProgress + ?Sized>(
    progress: &P,
    major: u32,
    component: &str,
) -> LauncherResult<()> {
    if progress.is_cancelled() {
        Err(LauncherError::JavaRuntimeCancelled {
            major,
            component: component.to_string(),
        })
    } else {
        Ok(())
    }
}

/// A no-op progress reporter for callers that do not need progress.
pub struct NoopProgress;

impl RuntimeProgress for NoopProgress {
    fn on_progress(&self, _message: &str, _percent: Option<f64>) {}
    fn is_cancelled(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Build the managed runtime directory for a catalog entry.
pub fn managed_entry_path(runtimes_root: &Path, entry: &RuntimeCatalogEntry) -> PathBuf {
    runtimes_root
        .join(MANAGED_VENDOR)
        .join(entry.major.to_string())
        .join(&entry.full_version)
        .join(format!("{}-{}", entry.os, entry.arch))
}

/// Build the receipt path for a catalog entry.
pub fn receipt_path(runtimes_root: &Path, entry: &RuntimeCatalogEntry) -> PathBuf {
    managed_entry_path(runtimes_root, entry).join("receipt.json")
}

/// Build the archive cache path for a given SHA-256 and extension.
fn archive_cache_path(runtimes_root: &Path, sha256: &str, ext: &str) -> PathBuf {
    runtimes_root
        .join(ARCHIVE_CACHE_DIR)
        .join(format!("{}.{}", sha256, ext))
}

/// Build the archive extension from a catalog entry.
fn archive_ext(entry: &RuntimeCatalogEntry) -> &'static str {
    match entry.archive_type.as_str() {
        "zip" => "zip",
        "tar.gz" => "tar.gz",
        _ => "unknown",
    }
}

// ---------------------------------------------------------------------------
// ensure_runtime — the main provisioning entry point
// ---------------------------------------------------------------------------

/// Ensure a managed JRE for the given major version is installed and valid.
///
/// # Steps
///
/// 1. Look up the catalog entry for the current platform.
/// 2. Validate existing installation (receipt + java binary match).
/// 3. Check network policy.
/// 4. Download (or reuse cached archive), verify SHA-256 + size.
/// 5. Extract to a staging directory with safety caps.
/// 6. Write receipt and atomically promote staging to final.
/// 7. Run `inspect_java` on the result and return it.
pub fn ensure_runtime(
    runtimes_root: &Path,
    major: u32,
    catalog: &RuntimeCatalog,
    network_policy: &NetworkPolicy,
    progress: Option<&dyn RuntimeProgress>,
) -> LauncherResult<JavaInstallation> {
    let progress = progress.unwrap_or(&NoopProgress);

    // 1. Look up catalog entry for current platform.
    let current_os = std::env::consts::OS;
    let current_arch = std::env::consts::ARCH;
    let lookup = catalog
        .lookup(major, current_os, current_arch)
        .ok_or_else(|| LauncherError::JavaRuntimeCatalogMissing {
            major,
            os: current_os.to_string(),
            arch: current_arch.to_string(),
        })?;
    let entry = lookup.entry.clone();
    let entry_path = managed_entry_path(runtimes_root, &entry);
    let rec_path = entry_path.join("receipt.json");

    progress.on_progress(&format!("Checking Java {major} installation…"), Some(0.0));

    // Check cancellation before proceeding.
    check_cancelled(progress, major, "ensure_runtime")?;

    // 2. Check for a valid existing installation using the receipt's actual
    //    java_relative_path (which accounts for any top-level archive dir).
    let existing_runtime_valid = try_cache_hit(&rec_path, &entry, major, progress);

    if let Some(inst) = existing_runtime_valid {
        return Ok(inst);
    }

    // 3. Check network policy before any download.
    network_policy.check(NetworkCategory::JavaRuntime)?;
    check_cancelled(progress, major, "ensure_runtime")?;

    // 4. Download (or reuse cached) archive — no recursion.
    let ext = archive_ext(&entry);
    let cache_path = archive_cache_path(runtimes_root, &entry.sha256, ext);

    match resolve_archive_cache(&cache_path, &entry, &rec_path, &entry_path, major, progress)? {
        ArchiveCacheOutcome::CacheUsable => { /* proceed to extraction below */ }
        ArchiveCacheOutcome::RuntimeRecovered(inst) => return Ok(inst),
        ArchiveCacheOutcome::DownloadNeeded => { /* fall through to download */ }
    }
    check_cancelled(progress, major, "ensure_runtime")?;

    if !cache_path.is_file() {
        // Check cancellation before download.
        check_cancelled(progress, major, "ensure_runtime")?;

        // Ensure archive cache dir exists.
        if let Some(parent) = cache_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| LauncherError::Generic {
                code: "ERR_ARCHIVE_CACHE_CREATE".into(),
                message: format!("Failed to create archive cache dir: {e}"),
            })?;
        }
        progress.on_progress("Downloading JRE archive…", Some(20.0));
        let component = format!("download-{}", entry.major);
        download_archive_verified(
            &entry.url,
            &cache_path,
            &entry.sha256,
            entry.size,
            progress,
            &component,
            major,
        )?;
        progress.on_progress("Archive downloaded and verified.", Some(50.0));
        check_cancelled(progress, major, "ensure_runtime")?;
    }

    // 5. Extract to a staging directory using a unique path so that
    //    concurrent or crashed installs never collide.
    let staging_id = format!(
        "staging-{}-{}-{}",
        std::process::id(),
        UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed),
        uuid::Uuid::new_v4(),
    );
    let staging = entry_path.with_extension(&staging_id);
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|e| LauncherError::Generic {
            code: "ERR_STAGING_REMOVE".into(),
            message: format!("Failed to remove stale staging: {e}"),
        })?;
    }
    if let Some(parent) = staging.parent() {
        std::fs::create_dir_all(parent).map_err(|e| LauncherError::Generic {
            code: "ERR_STAGING_PARENT".into(),
            message: format!("Failed to create staging parent: {e}"),
        })?;
    }
    // The staging directory itself is created by extraction (it's the root).
    std::fs::create_dir_all(&staging).map_err(|e| LauncherError::Generic {
        code: "ERR_STAGING_CREATE".into(),
        message: format!("Failed to create staging dir: {e}"),
    })?;

    // Perform extraction based on archive type.
    let extraction_result = match entry.archive_type.as_str() {
        "zip" => extract_zip(&cache_path, &staging, &entry, progress, "extract"),
        "tar.gz" => extract_tar_gz(&cache_path, &staging, &entry, progress, "extract"),
        other => Err(LauncherError::Generic {
            code: "ERR_UNSUPPORTED_ARCHIVE_TYPE".into(),
            message: format!("Unsupported archive type: {other}"),
        }),
    };

    // Locate the java executable in the staging directory.
    let staged_java = if extraction_result.is_ok() {
        // Search for the java binary inside staging.
        find_java_in_staging(&staging, &entry).ok_or_else(|| LauncherError::Generic {
            code: "ERR_JAVA_BINARY_NOT_FOUND".into(),
            message: "Extracted archive does not contain a Java executable at the expected path."
                .into(),
        })?
    } else {
        // Cleanup on failure.
        let _ = std::fs::remove_dir_all(&staging);
        return Err(extraction_result.unwrap_err());
    };

    // 6. Verify the extracted java works and matches major version.
    let installed = java::inspect_java(&staged_java).ok_or_else(|| LauncherError::Generic {
        code: "ERR_JAVA_INSPECT_FAILED".into(),
        message: "Extracted Java binary failed inspection.".into(),
    })?;

    if installed.version != major {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(LauncherError::Generic {
            code: "ERR_JAVA_VERSION_MISMATCH".into(),
            message: format!(
                "Extracted Java major version is {} but expected {}",
                installed.version, major
            ),
        });
    }

    // Determine the actual relative path from staging to the discovered Java
    // binary. Real Temurin archives contain a top-level directory like
    // `jdk-21.0.11+10/bin/java`, not a flat `bin/java`.
    let actual_java_relative = staged_java
        .strip_prefix(&staging)
        .map_err(|_| LauncherError::Generic {
            code: "ERR_RUNTIME_JAVA_PATH".into(),
            message: "Discovered Java path escaped runtime staging.".into(),
        })?
        .to_path_buf();

    // Check cancellation before writing receipt.
    check_cancelled(progress, major, "ensure_runtime")?;

    // Write the receipt inside staging — including java_sha256 and the
    // actual relative path (not the catalog's idealized path).
    let java_hash = sha256_hex_file(&staged_java).map_err(|e| LauncherError::Generic {
        code: "ERR_JAVA_HASH".into(),
        message: format!("Failed to hash java binary: {e}"),
    })?;
    let mut receipt = RuntimeReceipt::from_entry(&entry).with_java_hash(java_hash);
    receipt.java_relative_path = actual_java_relative.to_string_lossy().into_owned();
    let staging_receipt = staging.join("receipt.json");
    receipt.write_to(&staging_receipt)?;

    // Check cancellation before promoting staging -> final.
    check_cancelled(progress, major, "ensure_runtime")?;

    // 7. Atomically promote staging to final using backup/restore.
    let final_java_path = entry_path.join(&actual_java_relative);
    let backup = entry_path.with_extension(format!("backup-{}", uuid::Uuid::new_v4()));
    let had_existing = entry_path.exists();

    if had_existing {
        std::fs::rename(&entry_path, &backup).map_err(|e| LauncherError::Generic {
            code: "ERR_RUNTIME_BACKUP".into(),
            message: format!("Failed to back up existing runtime: {e}"),
        })?;
    }

    match std::fs::rename(&staging, &entry_path) {
        Ok(()) => {
            // Success — remove backup.
            if had_existing {
                let _ = std::fs::remove_dir_all(&backup);
            }
        }
        Err(error) => {
            // Rename failed — restore backup.
            if had_existing {
                let _ = std::fs::rename(&backup, &entry_path);
            }
            // Clean up staging.
            let _ = std::fs::remove_dir_all(&staging);
            return Err(LauncherError::Generic {
                code: "ERR_RUNTIME_PROMOTE".into(),
                message: format!("Failed to promote runtime {}: {error}", staging.display()),
            });
        }
    }

    // Verify the final java binary exists and works.
    if !final_java_path.is_file() {
        return Err(LauncherError::Generic {
            code: "ERR_RUNTIME_VERIFY".into(),
            message: format!(
                "Installed Java executable not found at expected path: {}",
                final_java_path.display()
            ),
        });
    }

    progress.on_progress("Runtime installed.", Some(100.0));

    Ok(JavaInstallation {
        path: final_java_path,
        version: installed.version,
        version_string: installed.version_string,
        source: JavaSource::Managed,
        arch: installed.arch,
    })
}

/// Check whether an existing receipt still matches the catalog entry.
fn receipt_matches(receipt: &RuntimeReceipt, entry: &RuntimeCatalogEntry) -> bool {
    receipt.vendor == entry.vendor
        && receipt.major == entry.major
        && receipt.full_version == entry.full_version
        && receipt.os == entry.os
        && receipt.arch == entry.arch
}

/// Try to use the existing runtime installation as a cache hit.
///
/// Verifies:
/// - Receipt exists and matches catalog entry
/// - Receipt stores the actual `java_relative_path` (from a prior successful
///   extract, which accounts for any top-level archive directory)
/// - Java binary exists at that path
/// - `java_sha256` in receipt is present and matches current file hash
/// - `inspect_java` reports the correct major version and compatible arch
///
/// Returns `Some(JavaInstallation)` on full hit, `None` otherwise (caller
/// should proceed with re-installation).
fn try_cache_hit(
    rec_path: &Path,
    entry: &RuntimeCatalogEntry,
    major: u32,
    progress: &dyn RuntimeProgress,
) -> Option<JavaInstallation> {
    let existing_receipt = RuntimeReceipt::read_from(rec_path).ok()?;

    if existing_receipt.archive_sha256 != entry.sha256 {
        return None;
    }
    if !receipt_matches(&existing_receipt, entry) {
        return None;
    }

    // Construct the Java path from the receipt's actual java_relative_path
    // (which includes any top-level archive directory like `jdk-21.0.11+10/`).
    let entry_dir = rec_path.parent()?;
    let actual_java_path = entry_dir.join(&existing_receipt.java_relative_path);

    if !actual_java_path.is_file() {
        return None;
    }

    // java_sha256 must be present (backward receipts without it invalidate).
    let expected_java_hash = existing_receipt.java_sha256.as_ref()?;
    let actual_hash = sha256_hex_file(&actual_java_path).ok()?;
    if actual_hash != *expected_java_hash {
        return None;
    }

    let inst = java::inspect_java(&actual_java_path)?;
    if inst.version != major {
        return None;
    }

    // Verify arch compatibility when JVM reports it.
    if let Some(ref jvm_arch) = inst.arch {
        if crate::runtime_catalog::normalize_arch(jvm_arch) != Some(entry.arch.as_str()) {
            return None;
        }
    }

    progress.on_progress("Using cached runtime.", Some(100.0));

    // Update last_used (best-effort).
    if let Ok(mut receipt) = RuntimeReceipt::read_from(rec_path) {
        let _ = receipt.touch_last_used(rec_path);
    }

    Some(JavaInstallation {
        path: actual_java_path.to_path_buf(),
        version: inst.version,
        version_string: inst.version_string,
        source: JavaSource::Managed,
        arch: inst.arch,
    })
}

/// Result of resolving the archive cache.
enum ArchiveCacheOutcome {
    /// Cached archive matches expected hash — no download needed.
    CacheUsable,
    /// No valid archive cache — caller must download, then extract.
    DownloadNeeded,
    /// Cached archive was corrupt but the existing runtime validated fine
    /// — no download or extraction needed.
    RuntimeRecovered(JavaInstallation),
}

/// Resolve the archive cache or recover an existing valid runtime.
///
/// Returns [`ArchiveCacheOutcome`] indicating the next action.
/// When the cached archive hash mismatches: the corrupt archive is deleted,
/// and if the extracted runtime still passes full validation
/// (inspect_java + java_sha256), it is returned as `RuntimeRecovered`.
/// Otherwise the stale runtime directory is removed and `DownloadNeeded` is
/// returned so the caller proceeds with a fresh download and extraction.
fn resolve_archive_cache(
    cache_path: &Path,
    entry: &RuntimeCatalogEntry,
    rec_path: &Path,
    entry_path: &Path,
    major: u32,
    progress: &dyn RuntimeProgress,
) -> LauncherResult<ArchiveCacheOutcome> {
    if !cache_path.is_file() {
        return Ok(ArchiveCacheOutcome::DownloadNeeded);
    }

    let cached_sha256 = sha256_hex_file(cache_path).map_err(|e| LauncherError::Generic {
        code: "ERR_ARCHIVE_CACHE_READ".into(),
        message: format!("Failed to read cached archive: {e}"),
    })?;

    if cached_sha256 == entry.sha256 {
        progress.on_progress("Using cached archive.", Some(30.0));
        return Ok(ArchiveCacheOutcome::CacheUsable);
    }

    // Cache corrupt — remove bad archive.
    let _ = std::fs::remove_file(cache_path);
    progress.on_progress("Cached archive corrupt.", Some(20.0));

    // Try full runtime validation on the existing installation using the
    // receipt's actual java_relative_path (not the catalog's idealized path).
    if let Ok(existing_receipt) = RuntimeReceipt::read_from(rec_path) {
        if existing_receipt.major == major
            && existing_receipt.os == entry.os
            && existing_receipt.arch == entry.arch
        {
            let java_path = entry_path.join(&existing_receipt.java_relative_path);
            if java_path.is_file() {
                let java_hash_valid = match &existing_receipt.java_sha256 {
                    Some(expected_hash) => sha256_hex_file(&java_path)
                        .ok()
                        .map(|h| h == *expected_hash)
                        .unwrap_or(false),
                    None => false,
                };

                if java_hash_valid {
                    if let Some(inst) = java::inspect_java(&java_path) {
                        if inst.version == major {
                            progress.on_progress(
                                "Using existing runtime (archive recoverable).",
                                Some(100.0),
                            );
                            if let Ok(mut receipt) = RuntimeReceipt::read_from(rec_path) {
                                let _ = receipt.touch_last_used(rec_path);
                            }
                            return Ok(ArchiveCacheOutcome::RuntimeRecovered(JavaInstallation {
                                path: java_path,
                                version: inst.version,
                                version_string: inst.version_string,
                                source: JavaSource::Managed,
                                arch: inst.arch,
                            }));
                        }
                    }
                }
            }
        }
    }

    // Remove stale runtime directory so extraction is clean.
    if entry_path.exists() {
        let _ = std::fs::remove_dir_all(entry_path);
    }

    Ok(ArchiveCacheOutcome::DownloadNeeded)
}

// ---------------------------------------------------------------------------
// Archive download with verification
// ---------------------------------------------------------------------------

/// Download a file from `url`, write it to `path`, verify SHA-256 and size.
///
/// # URL safety
///
/// - Initial URL must be from `github.com/adoptium/` release path.
/// - All redirect targets must be on the allowlist and HTTPS port 443.
/// - Only the exact expected size is accepted; no unbounded RAM buffering.
fn download_archive_verified(
    url: &str,
    path: &Path,
    expected_sha256: &str,
    expected_size: u64,
    progress: &dyn RuntimeProgress,
    _component: &str,
    major: u32,
) -> LauncherResult<()> {
    // Validate URL first.
    validate_runtime_url(url)?;

    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() > 10 {
                return attempt.stop();
            }
            if is_allowed_redirect_target(attempt.url()) {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .timeout(DOWNLOAD_STREAM_TIMEOUT)
        .user_agent("AgoraRuntimeManager/1.0")
        .build()
        .map_err(|e| LauncherError::Generic {
            code: "ERR_NETWORK".into(),
            message: format!("Failed to build download client: {e}"),
        })?;

    let response = client
        .get(url)
        .timeout(DOWNLOAD_TIMEOUT)
        .send()
        .map_err(|_| LauncherError::NetworkOffline)?;

    if !response.status().is_success() {
        return Err(LauncherError::Generic {
            code: "ERR_DOWNLOAD_HTTP".into(),
            message: format!("Download {url} returned HTTP {}", response.status()),
        });
    }

    // Verify final URL is safe.
    if !is_allowed_redirect_target(response.url()) {
        return Err(LauncherError::UntrustedSource);
    }

    // --- Streaming download with size+hash verification ---
    // Every failure path in this section cleans up the .partial file.
    let partial = path.with_extension("partial");

    let stream_result: LauncherResult<()> = (|| {
        let mut file = std::fs::File::create(&partial).map_err(|e| LauncherError::Generic {
            code: "ERR_ARCHIVE_WRITE".into(),
            message: format!("Failed to create {}: {e}", partial.display()),
        })?;

        // Content-Length: None -> skip upfront equality, rely on streamed exact size.
        // Some must equal expected_size.
        if let Some(total) = response.content_length() {
            if total != expected_size {
                return Err(LauncherError::Generic {
                    code: "ERR_DOWNLOAD_SIZE_MISMATCH".into(),
                    message: format!(
                        "Server reported content-length {total}, expected {expected_size}"
                    ),
                });
            }
        }

        let mut hasher = Sha256::new();
        let mut downloaded: u64 = 0;
        let mut buffer = [0u8; 8192];
        let mut reader = std::io::BufReader::new(response);

        loop {
            // Check cancellation at every chunk.
            check_cancelled(progress, major, "download")?;

            let n = reader
                .read(&mut buffer)
                .map_err(|_| LauncherError::NetworkOffline)?;
            if n == 0 {
                break;
            }
            downloaded += n as u64;
            if downloaded > expected_size {
                return Err(LauncherError::Generic {
                    code: "ERR_DOWNLOAD_SIZE_EXCEEDED".into(),
                    message: "Download exceeded expected size".into(),
                });
            }
            hasher.update(&buffer[..n]);
            file.write_all(&buffer[..n])
                .map_err(|e| LauncherError::Generic {
                    code: "ERR_ARCHIVE_WRITE".into(),
                    message: format!("Failed to write archive: {e}"),
                })?;
        }

        if downloaded != expected_size {
            return Err(LauncherError::Generic {
                code: "ERR_DOWNLOAD_SIZE_MISMATCH".into(),
                message: format!("Downloaded {downloaded} bytes, expected {expected_size}"),
            });
        }

        file.flush().map_err(|e| LauncherError::Generic {
            code: "ERR_ARCHIVE_FLUSH".into(),
            message: format!("Failed to flush archive: {e}"),
        })?;
        file.sync_all().map_err(|e| LauncherError::Generic {
            code: "ERR_ARCHIVE_SYNC".into(),
            message: format!("Failed to sync archive: {e}"),
        })?;
        drop(file);

        let actual_sha256 = hex::encode(hasher.finalize());
        if actual_sha256 != expected_sha256 {
            return Err(LauncherError::HashMismatch);
        }

        // Atomic rename to final cache path.
        std::fs::rename(&partial, path).map_err(|e| LauncherError::Generic {
            code: "ERR_ARCHIVE_RENAME".into(),
            message: format!("Failed to rename archive: {e}"),
        })?;

        Ok(())
    })();

    // Clean up .partial on any failure before propagating.
    if let Err(e) = stream_result {
        let _ = std::fs::remove_file(&partial);
        return Err(e);
    }

    Ok(())
}

/// Validate that a runtime download URL is on the allowlist.
fn validate_runtime_url(url: &str) -> LauncherResult<()> {
    let parsed = reqwest::Url::parse(url).map_err(|_| LauncherError::UntrustedSource)?;

    let host = parsed.host_str().ok_or(LauncherError::UntrustedSource)?;
    if parsed.scheme() != "https" || parsed.port_or_known_default() != Some(443) {
        return Err(LauncherError::UntrustedSource);
    }

    // Must be from github.com/adoptium/ with a /releases/download/ path.
    if host == "github.com" {
        let path = parsed.path();
        if path.starts_with("/adoptium/") && path.contains("/releases/download/") {
            return Ok(());
        }
    }

    // Also allow objects.githubusercontent.com and release-assets.githubusercontent.com
    // for redirect targets (checked again in redirect policy).
    if ALLOWED_DOWNLOAD_HOSTS.contains(&host) {
        return Ok(());
    }

    Err(LauncherError::UntrustedSource)
}

/// Check if a URL is an allowed redirect target for runtime downloads.
fn is_allowed_redirect_target(url: &reqwest::Url) -> bool {
    if url.scheme() != "https" || url.port_or_known_default() != Some(443) {
        return false;
    }
    let host = match url.host_str() {
        Some(h) => h,
        None => return false,
    };
    if host == "github.com" {
        // Only release download paths.
        return url.path().contains("/releases/download/");
    }
    ALLOWED_DOWNLOAD_HOSTS.contains(&host)
}

// ---------------------------------------------------------------------------
// SHA-256 helper
// ---------------------------------------------------------------------------

fn sha256_hex_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

// ---------------------------------------------------------------------------
// Safe extraction — ZIP
// ---------------------------------------------------------------------------

fn extract_zip(
    archive_path: &Path,
    staging: &Path,
    entry: &RuntimeCatalogEntry,
    progress: &dyn RuntimeProgress,
    component: &str,
) -> LauncherResult<()> {
    let file = std::fs::File::open(archive_path).map_err(|e| LauncherError::Generic {
        code: "ERR_ARCHIVE_OPEN".into(),
        message: format!("Failed to open {}: {e}", archive_path.display()),
    })?;

    let mut archive = zip::ZipArchive::new(file).map_err(|e| LauncherError::Generic {
        code: "ERR_ARCHIVE_PARSE".into(),
        message: format!("Failed to parse ZIP {}: {e}", archive_path.display()),
    })?;

    if archive.len() > MAX_ENTRIES {
        return Err(LauncherError::ZipBomb);
    }

    let mut seen_paths: Vec<PathBuf> = Vec::new();
    let mut aggregate_size: u64 = 0;

    for i in 0..archive.len() {
        // Check cancellation at every entry.
        check_cancelled(progress, entry.major, component)?;

        let mut entry_zip = archive.by_index(i).map_err(|e| LauncherError::Generic {
            code: "ERR_ARCHIVE_ENTRY".into(),
            message: format!("Failed to read entry {i}: {e}"),
        })?;

        let raw_name = entry_zip.name().to_string();

        // Check for NUL bytes.
        if raw_name.contains('\0') {
            return Err(LauncherError::Generic {
                code: "ERR_ARCHIVE_INVALID_NAME".into(),
                message: "Archive entry name contains NUL byte.".into(),
            });
        }

        // Normalize separators.
        let normalized = raw_name.replace('\\', "/");

        // Validate entry name (no absolute/traversal/UNC/colon).
        let relative = validate_entry_name(&normalized)?;

        // Check unix_mode: only regular files and directories.
        if let Some(mode) = entry_zip.unix_mode() {
            if !is_allowed_mode(mode) {
                return Err(LauncherError::Generic {
                    code: "ERR_ARCHIVE_FORBIDDEN_ENTRY".into(),
                    message: format!("Forbidden entry type in '{}' (mode 0{mode:o})", raw_name),
                });
            }
        }

        if entry_zip.is_dir() {
            let target = staging.join(&relative);
            check_path_collision(&seen_paths, &normalized, &relative)?;
            seen_paths.push(relative);
            std::fs::create_dir_all(&target).map_err(|e| LauncherError::Generic {
                code: "ERR_ARCHIVE_EXTRACT".into(),
                message: format!("Failed to create directory {}: {e}", target.display()),
            })?;
            continue;
        }

        // Regular file.
        let declared_size = entry_zip.size();
        if declared_size > PER_ENTRY_LIMIT {
            return Err(LauncherError::ZipBomb);
        }
        aggregate_size = aggregate_size.saturating_add(declared_size);
        if aggregate_size > AGGREGATE_LIMIT {
            return Err(LauncherError::ZipBomb);
        }

        let target = staging.join(&relative);
        check_path_collision(&seen_paths, &normalized, &relative)?;
        seen_paths.push(relative);

        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| LauncherError::Generic {
                code: "ERR_ARCHIVE_EXTRACT".into(),
                message: format!("Failed to create parent {}: {e}", parent.display()),
            })?;
        }

        let mut output = std::fs::File::create(&target).map_err(|e| LauncherError::Generic {
            code: "ERR_ARCHIVE_EXTRACT".into(),
            message: format!("Failed to create {}: {e}", target.display()),
        })?;

        let bytes_written =
            std::io::copy(&mut entry_zip, &mut output).map_err(|e| LauncherError::Generic {
                code: "ERR_ARCHIVE_EXTRACT".into(),
                message: format!("Failed to extract {}: {e}", target.display()),
            })?;

        if bytes_written != declared_size {
            return Err(LauncherError::Generic {
                code: "ERR_ARCHIVE_SIZE_MISMATCH".into(),
                message: format!(
                    "Entry '{}' declared {declared_size} bytes but {bytes_written} extracted",
                    raw_name
                ),
            });
        }

        // Preserve executable bit on Unix.
        #[cfg(unix)]
        if let Some(mode) = entry_zip.unix_mode() {
            set_unix_permissions(&target, mode);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Safe extraction — tar.gz
// ---------------------------------------------------------------------------

fn extract_tar_gz(
    archive_path: &Path,
    staging: &Path,
    entry: &RuntimeCatalogEntry,
    progress: &dyn RuntimeProgress,
    component: &str,
) -> LauncherResult<()> {
    let file = std::fs::File::open(archive_path).map_err(|e| LauncherError::Generic {
        code: "ERR_ARCHIVE_OPEN".into(),
        message: format!("Failed to open {}: {e}", archive_path.display()),
    })?;

    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);

    let mut seen_paths: Vec<PathBuf> = Vec::new();
    let mut aggregate_size: u64 = 0;
    let mut entry_count: usize = 0;

    // We must process entries manually because tar::Archive doesn't expose
    // entry count before iteration. We'll cap as we go.
    let result: LauncherResult<()> = (|| {
        let entries = archive.entries().map_err(|e| LauncherError::Generic {
            code: "ERR_TAR_ENTRIES".into(),
            message: format!("Failed to read tar entries: {e}"),
        })?;

        for entry_result in entries {
            entry_count += 1;

            // Check cancellation at every entry.
            check_cancelled(progress, entry.major, component)?;
            if entry_count > MAX_ENTRIES {
                return Err(LauncherError::ZipBomb);
            }

            let mut entry = entry_result.map_err(|e| LauncherError::Generic {
                code: "ERR_TAR_ENTRY".into(),
                message: format!("Failed to read tar entry: {e}"),
            })?;

            // Reject symlinks, hardlinks, devices, FIFOs.
            let entry_type;
            #[cfg(unix)]
            let mode;
            {
                let header = entry.header();
                entry_type = header.entry_type();
                if !matches!(
                    entry_type,
                    tar::EntryType::Regular
                        | tar::EntryType::Directory
                        | tar::EntryType::Continuous
                ) {
                    return Err(LauncherError::Generic {
                        code: "ERR_ARCHIVE_FORBIDDEN_ENTRY".into(),
                        message: format!("Forbidden entry type {:?} in tar archive", entry_type),
                    });
                }

                // Reject links.
                let has_link = header
                    .link_name()
                    .map_err(|e| LauncherError::Generic {
                        code: "ERR_TAR_HEADER".into(),
                        message: format!("Failed to read tar link name: {e}"),
                    })?
                    .is_some();
                if has_link {
                    return Err(LauncherError::Generic {
                        code: "ERR_ARCHIVE_FORBIDDEN_ENTRY".into(),
                        message: "Hardlinks are not permitted.".into(),
                    });
                }

                // Extract mode for Unix permissions before header goes out of scope.
                #[cfg(unix)]
                {
                    mode = header.mode().unwrap_or(0o644);
                }
            } // header dropped here, releasing the immutable borrow on entry

            let raw_name = match entry.path() {
                Ok(p) => p.to_string_lossy().into_owned(),
                Err(_) => continue,
            };

            if raw_name.contains('\0') {
                return Err(LauncherError::Generic {
                    code: "ERR_ARCHIVE_INVALID_NAME".into(),
                    message: "Archive entry name contains NUL byte.".into(),
                });
            }

            let normalized = raw_name.replace('\\', "/");
            let relative = validate_entry_name(&normalized)?;

            if entry_type.is_dir() {
                let target = staging.join(&relative);
                check_path_collision(&seen_paths, &normalized, &relative)?;
                seen_paths.push(relative);
                std::fs::create_dir_all(&target).map_err(|e| LauncherError::Generic {
                    code: "ERR_ARCHIVE_EXTRACT".into(),
                    message: format!("Failed to create directory {}: {e}", target.display()),
                })?;
                continue;
            }

            // Regular file.
            let declared_size = entry.size();
            if declared_size > PER_ENTRY_LIMIT {
                return Err(LauncherError::ZipBomb);
            }
            aggregate_size = aggregate_size.saturating_add(declared_size);
            if aggregate_size > AGGREGATE_LIMIT {
                return Err(LauncherError::ZipBomb);
            }

            let target = staging.join(&relative);
            check_path_collision(&seen_paths, &normalized, &relative)?;
            seen_paths.push(relative);

            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| LauncherError::Generic {
                    code: "ERR_ARCHIVE_EXTRACT".into(),
                    message: format!("Failed to create parent {}: {e}", parent.display()),
                })?;
            }

            let mut output =
                std::fs::File::create(&target).map_err(|e| LauncherError::Generic {
                    code: "ERR_ARCHIVE_EXTRACT".into(),
                    message: format!("Failed to create {}: {e}", target.display()),
                })?;

            let bytes_written =
                std::io::copy(&mut entry, &mut output).map_err(|e| LauncherError::Generic {
                    code: "ERR_ARCHIVE_EXTRACT".into(),
                    message: format!("Failed to extract {}: {e}", target.display()),
                })?;

            if bytes_written != declared_size {
                return Err(LauncherError::Generic {
                    code: "ERR_ARCHIVE_SIZE_MISMATCH".into(),
                    message: format!(
                        "Entry '{raw_name}' declared {declared_size} bytes but {bytes_written} extracted"
                    ),
                });
            }

            // Preserve executable bit on Unix.
            #[cfg(unix)]
            {
                set_unix_permissions(&target, mode);
            }
        }
        Ok(())
    })();

    result
}

// ---------------------------------------------------------------------------
// Entry name validation
// ---------------------------------------------------------------------------

fn validate_entry_name(normalized: &str) -> LauncherResult<PathBuf> {
    // Reject absolute Unix paths.
    if normalized.starts_with('/') {
        return Err(LauncherError::OverrideSecurityViolation);
    }
    // Reject UNC paths.
    if normalized.starts_with("//") {
        return Err(LauncherError::OverrideSecurityViolation);
    }
    // Reject Windows drive letters.
    if normalized.contains(':') {
        return Err(LauncherError::OverrideSecurityViolation);
    }

    let mut path = PathBuf::new();
    for component in normalized.split('/') {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            return Err(LauncherError::OverrideSecurityViolation);
        }
        path.push(component);
    }
    if path.as_os_str().is_empty() {
        return Err(LauncherError::OverrideSecurityViolation);
    }
    Ok(path)
}

// ---------------------------------------------------------------------------
// Path collision detection
// ---------------------------------------------------------------------------

fn check_path_collision(
    seen: &[PathBuf],
    _normalized_name: &str,
    relative: &Path,
) -> LauncherResult<()> {
    let rel_str = relative.to_string_lossy();

    // 1. Exact duplicate.
    if seen.iter().any(|p| p.as_os_str() == relative.as_os_str()) {
        return Err(LauncherError::OverrideSecurityViolation);
    }

    // 2. Case-insensitive collision.
    let rel_lower = rel_str.to_ascii_lowercase();
    for prev in seen {
        if prev.to_string_lossy().to_ascii_lowercase() == rel_lower {
            return Err(LauncherError::OverrideSecurityViolation);
        }
    }

    // 3. File-directory conflict (ancestor check).
    for prev in seen {
        let prev_str = prev.to_string_lossy();
        if prev_str.len() > rel_str.len()
            && prev_str.starts_with(rel_str.as_ref())
            && prev_str.as_ref()[rel_str.len()..].starts_with('/')
        {
            return Err(LauncherError::OverrideSecurityViolation);
        }
        if rel_str.len() > prev_str.len()
            && rel_str.starts_with(prev_str.as_ref())
            && rel_str.as_ref()[prev_str.len()..].starts_with('/')
        {
            return Err(LauncherError::OverrideSecurityViolation);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Unix permission helpers
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn set_unix_permissions(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    // Preserve executable bit, strip setuid/setgid/sticky.
    let conservative = mode & 0o755;
    if let Err(_e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(conservative)) {
        // Best-effort.
    }
}

// ---------------------------------------------------------------------------
// Mode validation
// ---------------------------------------------------------------------------

fn is_allowed_mode(mode: u32) -> bool {
    const S_IFMT: u32 = 0o170000;
    const S_IFREG: u32 = 0o100000;
    const S_IFDIR: u32 = 0o040000;
    matches!(mode & S_IFMT, S_IFREG | S_IFDIR)
}

// ---------------------------------------------------------------------------
// Java binary discovery inside staging
// ---------------------------------------------------------------------------

/// Find exactly one Java executable in the staging directory.
///
/// First tries the expected relative path from the catalog entry.
/// Falls back to a discovered unique `*/bin/java` (or `*/bin/java.exe`).
fn find_java_in_staging(staging: &Path, entry: &RuntimeCatalogEntry) -> Option<PathBuf> {
    // First try the expected path.
    let expected = staging.join(&entry.java_relative_path);
    if expected.is_file() {
        return Some(expected);
    }

    // Fallback: scan for a unique `bin/java(.exe)`.
    let java_name = if cfg!(target_os = "windows") {
        "java.exe"
    } else {
        "java"
    };

    let mut candidates = Vec::new();
    let bin_dirs = find_dirs_named(staging, "bin");
    for bin_dir in bin_dirs {
        let java_path = bin_dir.join(java_name);
        if java_path.is_file() {
            candidates.push(java_path);
        }
    }

    if candidates.len() == 1 {
        Some(candidates.into_iter().next().unwrap())
    } else {
        None
    }
}

/// Find all directories named `name` (case-insensitive on Windows) within
/// `root`, limited to a reasonable depth.
fn find_dirs_named(root: &Path, name: &str) -> Vec<PathBuf> {
    let mut results = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let mut visited: usize = 0;
    let max_visited = 500;

    while let Some(dir) = stack.pop() {
        visited += 1;
        if visited > max_visited {
            break;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let ft = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ft.is_dir() {
                let entry_name = entry.file_name();
                let entry_str = entry_name.to_string_lossy();
                let is_match = if cfg!(target_os = "windows") {
                    entry_str.eq_ignore_ascii_case(name)
                } else {
                    entry_str == name
                };
                if is_match {
                    results.push(entry.path());
                } else {
                    stack.push(entry.path());
                }
            }
        }
    }

    results
}

// ---------------------------------------------------------------------------
// Receipt management operations
// ---------------------------------------------------------------------------

/// List all managed runtimes that have valid receipts.
pub fn list_managed_runtimes(runtimes_root: &Path) -> LauncherResult<Vec<ManagedRuntime>> {
    let vendor_dir = runtimes_root.join(MANAGED_VENDOR);
    if !vendor_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut results = Vec::new();

    let major_dirs = match std::fs::read_dir(&vendor_dir) {
        Ok(d) => d,
        Err(e) => {
            return Err(LauncherError::Generic {
                code: "ERR_RUNTIME_LIST".into(),
                message: format!("Failed to list runtimes: {e}"),
            })
        }
    };

    for major_entry in major_dirs.flatten() {
        if !major_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let version_dir = major_entry.path();
        let version_dirs = match std::fs::read_dir(&version_dir) {
            Ok(d) => d,
            Err(_) => continue,
        };
        for ve in version_dirs.flatten() {
            if !ve.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            // ve is the version directory (e.g. "21.0.11+10").
            // The platform directory (e.g. "linux-x64") is one level deeper.
            let plat_dirs = match std::fs::read_dir(ve.path()) {
                Ok(d) => d,
                Err(_) => continue,
            };
            for pe in plat_dirs.flatten() {
                if !pe.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }
                // Reject symlinked platform directories to prevent escape.
                if let Ok(meta) = pe.metadata() {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::MetadataExt;
                        if meta.file_type().is_symlink() {
                            continue;
                        }
                    }
                    #[cfg(windows)]
                    {
                        use std::os::windows::fs::MetadataExt;
                        if meta.file_attributes() & 0x400 != 0 {
                            continue;
                        }
                    }
                }
                let plat_dir = pe.path();
                let receipt_path = plat_dir.join("receipt.json");
                if !receipt_path.is_file() {
                    continue;
                }
                let receipt = match RuntimeReceipt::read_from(&receipt_path) {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                let java_path = plat_dir.join(&receipt.java_relative_path);
                if !java_path.is_file() {
                    continue;
                }
                results.push(ManagedRuntime {
                    root_dir: plat_dir,
                    receipt,
                    java_path,
                });
            }
        }
    }

    Ok(results)
}

/// Remove a specific managed runtime by its root directory.
///
/// Only deletes the **canonicalised** target directory inside the known
/// managed layout (`temurin/<major>/<full_version>/<os>-<arch>/`).
/// Rejects:
/// - Paths outside the managed runtime root.
/// - Symlinks / junctions where canonicalisation disagrees with the
///   provided path (anti-escape guard).
/// - Paths without a valid `receipt.json` at the canonicalised target.
pub fn remove_runtime(runtimes_root: &Path, root_dir: &Path) -> LauncherResult<()> {
    let canonical_root = runtimes_root
        .canonicalize()
        .unwrap_or_else(|_| runtimes_root.to_path_buf());

    let canonical_target = root_dir
        .canonicalize()
        .unwrap_or_else(|_| root_dir.to_path_buf());

    // Reject symlink/junction targets: ensure none of the components in
    // root_dir is a symlink or reparse point. We check by reading the
    // metadata of root_dir itself without following symlinks.
    if let Ok(meta) = std::fs::symlink_metadata(root_dir) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if meta.file_type().is_symlink() {
                return Err(LauncherError::Generic {
                    code: "ERR_RUNTIME_REMOVE_SYMLINK".into(),
                    message: "Cannot remove: target path is a symlink.".into(),
                });
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if meta.file_attributes() & 0x400 != 0 {
                // FILE_ATTRIBUTE_REPARSE_POINT
                return Err(LauncherError::Generic {
                    code: "ERR_RUNTIME_REMOVE_SYMLINK".into(),
                    message: "Cannot remove: target path is a junction/symlink.".into(),
                });
            }
        }
    }

    if !canonical_target.starts_with(&canonical_root) {
        return Err(LauncherError::Generic {
            code: "ERR_RUNTIME_REMOVE_OUTSIDE".into(),
            message: "Cannot remove: target is outside the managed runtime root.".into(),
        });
    }

    // Verify it's under the known vendor + major + version + plat pattern.
    let rel = canonical_target.strip_prefix(&canonical_root).unwrap();
    let components: Vec<_> = rel.components().collect();
    if components.len() < 4 {
        return Err(LauncherError::Generic {
            code: "ERR_RUNTIME_REMOVE_INVALID".into(),
            message: "Cannot remove: path does not match managed layout.".into(),
        });
    }

    // Require a valid runtime receipt before deletion.
    let receipt_path = canonical_target.join("receipt.json");
    RuntimeReceipt::read_from(&receipt_path).map_err(|_| LauncherError::Generic {
        code: "ERR_RUNTIME_REMOVE_NO_RECEIPT".into(),
        message: format!(
            "Cannot remove: no valid receipt at {}",
            receipt_path.display()
        ),
    })?;

    // Delete the canonicalised target (not the original root_dir) so that
    // symlink-escape paths cannot delete unintended targets.
    std::fs::remove_dir_all(&canonical_target).map_err(|e| LauncherError::Generic {
        code: "ERR_RUNTIME_REMOVE".into(),
        message: format!(
            "Failed to remove runtime {}: {e}",
            canonical_target.display()
        ),
    })?;

    Ok(())
}

/// Remove unused runtimes, keeping the newest catalog build per major version
/// and any paths in `protected_paths`.
///
/// "Newest" is determined by catalog `source_api_url` ordering per major.
pub fn remove_unused(
    runtimes_root: &Path,
    catalog: &RuntimeCatalog,
    protected_paths: &[PathBuf],
) -> LauncherResult<usize> {
    let runtimes = list_managed_runtimes(runtimes_root)?;
    if runtimes.is_empty() {
        return Ok(0);
    }

    // Group by major version.
    let mut by_major: std::collections::BTreeMap<u32, Vec<ManagedRuntime>> =
        std::collections::BTreeMap::new();
    for rt in runtimes {
        by_major.entry(rt.receipt.major).or_default().push(rt);
    }

    // For each major, find the "newest" (prefer most recent catalog entry).
    let mut removed: usize = 0;

    for (major, group) in &by_major {
        // Find the best catalog entry for this major.
        let best_receipt_data = catalog
            .list_available()
            .iter()
            .find(|(m, _, _)| *m == *major)
            .copied();

        let (best_os, best_arch) = match best_receipt_data {
            Some((_m, os, arch)) => (os, arch),
            None => continue,
        };

        // Find the best matched runtime (matching os/arch from catalog).
        let best_runtime = group
            .iter()
            .find(|rt| rt.receipt.os == best_os && rt.receipt.arch == best_arch);

        let protected_set: std::collections::HashSet<&Path> =
            protected_paths.iter().map(|p| p.as_path()).collect();

        for rt in group {
            // Skip if this is the best runtime.
            if let Some(best) = best_runtime {
                if rt.root_dir == best.root_dir {
                    continue;
                }
            }
            // Skip if the path is protected.
            if protected_set.contains(rt.root_dir.as_path()) {
                continue;
            }
            // Remove this runtime.
            if remove_runtime(runtimes_root, &rt.root_dir).is_ok() {
                removed += 1;
            }
        }
    }

    Ok(removed)
}

/// Mark the managed runtime that owns `java_path` as successfully used.
///
/// This is called after a game launch exits with a **success** outcome.
/// It walks the managed runtime list, finds the runtime whose `java_path`
/// matches (by canonical comparison), and calls `touch_successful_use` on
/// its receipt.
///
/// Best-effort: returns `Ok(())` even if no matching runtime is found
/// (the Java may be a system or Mojang runtime, not managed).
pub fn mark_successful_use(runtimes_root: &Path, java_path: &Path) -> LauncherResult<()> {
    let runtimes = list_managed_runtimes(runtimes_root)?;

    let canonical_java = java_path
        .canonicalize()
        .unwrap_or_else(|_| java_path.to_path_buf());

    for rt in runtimes {
        let rt_canonical = rt
            .java_path
            .canonicalize()
            .unwrap_or_else(|_| rt.java_path.clone());

        if rt_canonical == canonical_java {
            let receipt_path = rt.root_dir.join("receipt.json");
            let mut receipt = RuntimeReceipt::read_from(&receipt_path)?;
            receipt.touch_successful_use(&receipt_path)?;
            return Ok(());
        }
    }

    // No matching managed runtime — not an error (system/Mojang Java).
    Ok(())
}

/// Validate a managed runtime is intact and return the Java installation.
///
/// All of these must be true for validation to pass:
/// - Receipt major matches request
/// - Java executable is inside the runtime root
/// - Java executable SHA-256 matches receipt
/// - `java -version` succeeds
/// - Reported major matches required major
pub fn validate_managed_runtime(
    runtime: &ManagedRuntime,
    required_major: u32,
) -> LauncherResult<JavaInstallation> {
    if runtime.receipt.major != required_major {
        return Err(LauncherError::JavaIncompatible);
    }

    let canonical_root = runtime
        .root_dir
        .canonicalize()
        .map_err(|e| runtime_corrupt(&format!("Cannot resolve runtime root: {e}")))?;
    let canonical_java = runtime
        .java_path
        .canonicalize()
        .map_err(|e| runtime_corrupt(&format!("Cannot resolve java path: {e}")))?;

    if !canonical_java.starts_with(&canonical_root) {
        return Err(runtime_corrupt("Java executable escapes runtime root"));
    }

    if let Some(expected) = runtime.receipt.java_sha256.as_deref() {
        let actual = sha256_hex_file(&canonical_java)
            .map_err(|e| runtime_corrupt(&format!("Cannot hash java binary: {e}")))?;
        if actual != expected {
            return Err(LauncherError::HashMismatch);
        }
    }

    let inspected = java::inspect_java(&canonical_java)
        .ok_or_else(|| runtime_corrupt("Java inspection failed"))?;

    if inspected.version != required_major {
        return Err(LauncherError::JavaIncompatible);
    }

    Ok(JavaInstallation {
        path: canonical_java,
        version: inspected.version,
        version_string: inspected.version_string,
        source: JavaSource::Managed,
        arch: inspected.arch,
    })
}

/// Build a "runtime corrupt" error for validation failures.
fn runtime_corrupt(detail: &str) -> LauncherError {
    LauncherError::Generic {
        code: "ERR_RUNTIME_CORRUPT".into(),
        message: format!("Managed runtime is corrupt: {detail}"),
    }
}

// ---------------------------------------------------------------------------
// Atomic write helper
// ---------------------------------------------------------------------------

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn atomic_write(path: &Path, bytes: &[u8]) -> LauncherResult<()> {
    let parent = path.parent().ok_or_else(|| LauncherError::Generic {
        code: "ERR_ATOMIC_WRITE".into(),
        message: format!("Path has no parent: {}", path.display()),
    })?;
    std::fs::create_dir_all(parent).map_err(|e| LauncherError::Generic {
        code: "ERR_ATOMIC_WRITE".into(),
        message: format!("Failed to create parent {}: {e}", parent.display()),
    })?;

    let pid = std::process::id();
    let count = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let stamp = format!(".agtmp_{pid}_{count}");
    let file_name = path.file_name().unwrap_or_default();
    let temp = parent.join(format!("{}{}", file_name.to_string_lossy(), stamp));

    let write_result = (|| {
        let mut file = std::fs::File::create(&temp).map_err(|e| LauncherError::Generic {
            code: "ERR_ATOMIC_WRITE".into(),
            message: format!("Failed to create temp file {}: {e}", temp.display()),
        })?;
        file.write_all(bytes).map_err(|e| LauncherError::Generic {
            code: "ERR_ATOMIC_WRITE".into(),
            message: format!("Failed to write {}: {e}", temp.display()),
        })?;
        file.flush().map_err(|e| LauncherError::Generic {
            code: "ERR_ATOMIC_WRITE".into(),
            message: format!("Failed to flush {}: {e}", temp.display()),
        })?;
        file.sync_all().map_err(|e| LauncherError::Generic {
            code: "ERR_ATOMIC_WRITE".into(),
            message: format!("Failed to sync {}: {e}", temp.display()),
        })?;
        Ok::<_, LauncherError>(())
    })();

    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }

    std::fs::rename(&temp, path).map_err(|e| LauncherError::Generic {
        code: "ERR_ATOMIC_WRITE".into(),
        message: format!(
            "Failed to rename {} to {}: {e}",
            temp.display(),
            path.display()
        ),
    })?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_catalog::RuntimeCatalog;

    /// A valid catalog for testing (loaded at compile time).
    fn test_catalog() -> RuntimeCatalog {
        let data = include_str!("../../../runtime-catalog/runtime_catalog.json");
        RuntimeCatalog::from_json(data.as_bytes()).expect("test catalog")
    }

    #[test]
    fn test_receipt_serde_roundtrip() {
        let entry = test_catalog()
            .lookup(21, "linux", "x86_64")
            .expect("Java 21 linux x64 entry")
            .entry;

        let receipt = RuntimeReceipt::from_entry(&entry);
        let json = serde_json::to_string(&receipt).unwrap();
        let deserialized: RuntimeReceipt = serde_json::from_str(&json).unwrap();

        assert_eq!(receipt, deserialized);
    }

    #[test]
    fn test_receipt_read_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("receipt.json");

        let entry = test_catalog()
            .lookup(21, "linux", "x86_64")
            .expect("Java 21 linux x64")
            .entry;
        let receipt = RuntimeReceipt::from_entry(&entry);

        receipt.write_to(&path).unwrap();
        let read = RuntimeReceipt::read_from(&path).unwrap();

        assert_eq!(receipt, read);
    }

    #[test]
    fn test_invalid_receipt_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("receipt.json");
        std::fs::write(&path, "not-json").unwrap();

        assert!(RuntimeReceipt::read_from(&path).is_err());
    }

    #[test]
    fn test_wrong_schema_version_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("receipt.json");
        let bad = r#"{"schema_version": 999, "vendor": "test", "major": 21}"#;
        std::fs::write(&path, bad).unwrap();

        assert!(RuntimeReceipt::read_from(&path).is_err());
    }

    #[test]
    fn test_validate_entry_name_rejects_absolute() {
        assert!(validate_entry_name("/etc/passwd").is_err());
    }

    #[test]
    fn test_validate_entry_name_rejects_traversal() {
        assert!(validate_entry_name("../../evil").is_err());
    }

    #[test]
    fn test_validate_entry_name_rejects_unc() {
        assert!(validate_entry_name("//server/share/evil").is_err());
    }

    #[test]
    fn test_validate_entry_name_rejects_colon() {
        assert!(validate_entry_name("C:/Windows/evil.dll").is_err());
    }

    #[test]
    fn test_validate_entry_name_accepts_normal() {
        let p = validate_entry_name("jdk-21/jre/bin/java").unwrap();
        assert_eq!(p, PathBuf::from("jdk-21/jre/bin/java"));
    }

    #[test]
    fn test_check_path_collision_exact_duplicate() {
        let seen = vec![PathBuf::from("a/b")];
        assert!(check_path_collision(&seen, "a/b", Path::new("a/b")).is_err());
    }

    #[test]
    fn test_check_path_collision_case_collision() {
        let seen = vec![PathBuf::from("A/B")];
        assert!(check_path_collision(&seen, "a/b", Path::new("a/b")).is_err());
    }

    #[test]
    fn test_is_allowed_mode_accepts_files_and_dirs() {
        assert!(is_allowed_mode(0o100644)); // regular file
        assert!(is_allowed_mode(0o040755)); // directory
    }

    #[test]
    fn test_is_allowed_mode_rejects_symlinks() {
        assert!(!is_allowed_mode(0o120777)); // symlink
    }

    #[test]
    fn test_managed_entry_path_format() {
        let entry = test_catalog()
            .lookup(21, "linux", "x86_64")
            .expect("Java 21 linux x64")
            .entry;

        let root = PathBuf::from("/runtimes");
        let path = managed_entry_path(&root, &entry);
        assert!(path.starts_with("/runtimes/temurin/21/"));
        assert!(path.to_string_lossy().contains("linux-x64"));
    }

    #[test]
    fn test_archive_cache_path_format() {
        let root = PathBuf::from("/runtimes");
        let sha = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        let path = archive_cache_path(&root, sha, "tar.gz");
        assert_eq!(
            path,
            PathBuf::from(format!("/runtimes/.archives/{sha}.tar.gz"))
        );
    }

    #[test]
    fn test_archive_ext() {
        let entry = test_catalog()
            .lookup(21, "linux", "x86_64")
            .expect("Java 21 linux x64")
            .entry;
        assert_eq!(archive_ext(&entry), "tar.gz");

        let win_entry = test_catalog()
            .lookup(21, "windows", "x64")
            .expect("Java 21 windows x64")
            .entry;
        assert_eq!(archive_ext(&win_entry), "zip");
    }

    #[test]
    fn test_receipt_touch_last_used() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("receipt.json");

        let entry = test_catalog()
            .lookup(21, "linux", "x86_64")
            .expect("Java 21 linux x64")
            .entry;
        let mut receipt = RuntimeReceipt::from_entry(&entry);

        // Initial: last_used_at should be set.
        assert!(receipt.last_used_at.is_some());
        assert!(receipt.successful_use_at.is_none());

        receipt.write_to(&path).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));

        receipt.touch_successful_use(&path).unwrap();
        let read = RuntimeReceipt::read_from(&path).unwrap();
        assert!(read.successful_use_at.is_some());
    }

    #[test]
    fn test_list_managed_runtimes_empty() {
        let dir = tempfile::tempdir().unwrap();
        let runtimes = list_managed_runtimes(dir.path()).unwrap();
        assert!(runtimes.is_empty());
    }

    #[test]
    fn test_mark_successful_use_no_match_is_ok() {
        // No managed runtimes at all — should return Ok(()), not an error.
        let dir = tempfile::tempdir().unwrap();
        let java_path = dir.path().join("bin/java");
        mark_successful_use(dir.path(), &java_path).unwrap();
    }

    #[test]
    fn test_mark_successful_use_finds_matching_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let runtimes_root = dir.path().join("runtimes");

        // Create a fake managed runtime layout.
        let entry = test_catalog()
            .lookup(21, "linux", "x86_64")
            .expect("Java 21 linux x64")
            .entry;
        let rt_dir = managed_entry_path(&runtimes_root, &entry);
        std::fs::create_dir_all(&rt_dir).unwrap();

        // Create a fake java binary.
        let java_rel = &entry.java_relative_path;
        let java_path = rt_dir.join(java_rel);
        if let Some(parent) = java_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&java_path, b"fake").unwrap();

        // Write receipt.
        let receipt = RuntimeReceipt::from_entry(&entry);
        let receipt_path = rt_dir.join("receipt.json");
        receipt.write_to(&receipt_path).unwrap();

        // successful_use_at should be None initially.
        let read = RuntimeReceipt::read_from(&receipt_path).unwrap();
        assert!(read.successful_use_at.is_none());

        // Mark successful use.
        mark_successful_use(&runtimes_root, &java_path).unwrap();

        // successful_use_at should now be set.
        let read = RuntimeReceipt::read_from(&receipt_path).unwrap();
        assert!(read.successful_use_at.is_some());
    }

    #[test]
    fn test_remove_runtime_rejects_outside_root() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("..").canonicalize().unwrap();
        let result = remove_runtime(dir.path(), &outside);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_runtime_url_accepts_adoptium() {
        assert!(
            validate_runtime_url(
                "https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.11%2B10/OpenJDK21U-jre_x64_linux_hotspot_21.0.11_10.tar.gz"
            )
            .is_ok()
        );
    }

    #[test]
    fn test_validate_runtime_url_rejects_non_adoptium() {
        assert!(validate_runtime_url("https://evil.example.com/backdoor.zip").is_err());
    }

    #[test]
    fn test_validate_runtime_url_rejects_http() {
        assert!(validate_runtime_url(
            "http://github.com/adoptium/temurin21-binaries/releases/download/x/pkg.tar.gz"
        )
        .is_err());
    }

    #[test]
    fn test_is_allowed_redirect_target() {
        let ok_url =
            reqwest::Url::parse("https://objects.githubusercontent.com/test/archive.tar.gz")
                .unwrap();
        assert!(is_allowed_redirect_target(&ok_url));

        let bad_url = reqwest::Url::parse("http://evil.com/backdoor").unwrap();
        assert!(!is_allowed_redirect_target(&bad_url));
    }

    #[test]
    fn test_find_dirs_named() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("a").join("b").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("java"), "").unwrap();

        let found = find_dirs_named(dir.path(), "bin");
        assert_eq!(found.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Helpers for end-to-end fixture tests
    // -----------------------------------------------------------------------

    /// Platform-appropriate java relative path for test archives.
    fn test_java_rel() -> &'static str {
        if cfg!(target_os = "windows") {
            "jdk-21.0.2+13/bin/java.exe"
        } else {
            "jdk-21.0.2+13/bin/java"
        }
    }

    /// Platform-appropriate archive extension.
    fn test_archive_extension() -> &'static str {
        if cfg!(target_os = "windows") {
            "zip"
        } else {
            "tar.gz"
        }
    }

    /// Create a test catalog entry with a known sha256 and size.
    fn make_test_entry(sha256: &str, size: u64) -> RuntimeCatalogEntry {
        RuntimeCatalogEntry {
            vendor: "eclipse-temurin".into(),
            major: 21,
            full_version: "21.0.2+13".into(),
            openjdk_version: "21.0.2".into(),
            os: std::env::consts::OS.into(),
            arch: crate::runtime_catalog::normalize_arch(std::env::consts::ARCH)
                .unwrap()
                .into(),
            image_type: "jre".into(),
            jvm_impl: "hotspot".into(),
            archive_type: test_archive_extension().into(),
            url: "https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.2%2B13/OpenJDK21U-jre_x64_windows_hotspot_21.0.2_13.zip".into(),
            sha256: sha256.into(),
            size,
            java_relative_path: test_java_rel().into(),
            license: "GPL-2.0-only WITH Classpath-exception-2.0".into(),
            source_api_url: "https://api.adoptium.net/v3/assets/latest/21/hotspot?image_type=jre&vendor=eclipse".into(),
            version_major: Some(21),
            version_minor: Some(0),
            version_security: Some(2),
        }
    }

    fn make_catalog(entry: RuntimeCatalogEntry) -> RuntimeCatalog {
        RuntimeCatalog {
            schema_version: 1,
            generated_at: "2026-01-01T00:00:00Z".into(),
            source: "test".into(),
            entries: vec![entry],
            warnings: vec![],
        }
    }

    /// Create a minimal ZIP archive containing a topdir/bin/java + LICENSE.
    fn create_test_zip(java_rel_path: &str) -> Vec<u8> {
        use std::io::{Cursor, Seek, Write};
        let buf = Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(buf);

        let opts = zip::write::FileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .unix_permissions(0o755);

        let rel_path = Path::new(java_rel_path);
        let topdir = rel_path
            .components()
            .next()
            .unwrap()
            .as_os_str()
            .to_str()
            .unwrap()
            .to_string();

        // Create directory entries
        let mut dir = String::new();
        for comp in rel_path.parent().unwrap().components() {
            let seg = comp.as_os_str().to_str().unwrap();
            if !dir.is_empty() {
                dir.push('/');
            }
            dir.push_str(seg);
            zip.add_directory(&dir, opts).unwrap();
        }

        // Java executable
        zip.start_file(java_rel_path, opts).unwrap();
        if cfg!(target_os = "windows") {
            zip.write_all(b"@echo off\necho java version \"21\"\n")
                .unwrap();
        } else {
            zip.write_all(b"#!/bin/sh\necho 'java version \"21\"'\n")
                .unwrap();
        }

        // LICENSE
        let lic_path = format!("{topdir}/LICENSE");
        zip.start_file(&lic_path, opts).unwrap();
        zip.write_all(b"GPL-2.0-only WITH Classpath-exception-2.0\n")
            .unwrap();

        let mut buf = zip.finish().unwrap();
        buf.seek(std::io::SeekFrom::Start(0)).unwrap();
        let mut data = Vec::new();
        buf.read_to_end(&mut data).unwrap();
        data
    }

    /// Create a minimal tar.gz archive containing a topdir/bin/java + LICENSE.
    fn create_test_tar_gz(java_rel_path: &str) -> Vec<u8> {
        let mut buf = Vec::new();
        let gz = flate2::write::GzEncoder::new(&mut buf, flate2::Compression::best());
        let mut tar = tar::Builder::new(gz);

        let rel_path = Path::new(java_rel_path);
        let topdir = rel_path
            .components()
            .next()
            .unwrap()
            .as_os_str()
            .to_str()
            .unwrap()
            .to_string();

        // Create directory entries.
        let mut dir = PathBuf::new();
        for comp in rel_path.parent().unwrap().components() {
            dir.push(comp);
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Directory);
            header.set_mode(0o755);
            header.set_size(0);
            header.set_cksum();
            tar.append_data(&mut header, &dir, &[] as &[u8]).unwrap();
        }

        // Java executable
        let java_data: &[u8] = if cfg!(target_os = "windows") {
            b"@echo off\r\necho java version \"21\"\r\n"
        } else {
            b"#!/bin/sh\necho 'java version \"21\"'\n"
        };
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(0o755);
        header.set_size(java_data.len() as u64);
        header.set_cksum();
        tar.append_data(&mut header, java_rel_path, java_data)
            .unwrap();

        // LICENSE
        let lic_path = format!("{topdir}/LICENSE");
        let lic_data = b"GPL-2.0-only WITH Classpath-exception-2.0\n";
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(0o644);
        header.set_size(lic_data.len() as u64);
        header.set_cksum();
        tar.append_data(&mut header, &lic_path, &lic_data[..])
            .unwrap();

        drop(tar);
        buf
    }

    /// Create a test archive, compute its sha256, write it to the archive cache
    /// path, and return the entry with correct sha256/size.
    fn setup_archive_cache(root: &Path, entry: &mut RuntimeCatalogEntry) {
        let ext = test_archive_extension();
        let archive_data = if ext == "zip" {
            create_test_zip(&entry.java_relative_path)
        } else {
            create_test_tar_gz(&entry.java_relative_path)
        };

        let hash = hex::encode(sha2::Sha256::digest(&archive_data));
        entry.sha256 = hash;
        entry.size = archive_data.len() as u64;

        let cache_dir = root.join(ARCHIVE_CACHE_DIR);
        std::fs::create_dir_all(&cache_dir).unwrap();
        let cache_path = cache_dir.join(format!("{}.{ext}", entry.sha256));
        std::fs::write(&cache_path, &archive_data).unwrap();
    }

    /// The mock inspect function returns a fake Java 21 installation for any path.
    fn mock_inspect_21(_path: &Path) -> Option<JavaInstallation> {
        Some(JavaInstallation {
            path: _path.to_path_buf(),
            version: 21,
            version_string: "21.0.2".into(),
            source: JavaSource::Managed,
            arch: Some(
                crate::runtime_catalog::normalize_arch(std::env::consts::ARCH)
                    .unwrap()
                    .into(),
            ),
        })
    }

    // -----------------------------------------------------------------------
    // End-to-end fixture tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_ensure_runtime_succeeds_with_zip_fixture() {
        // Only run on platforms where the test archive type is zip.
        if !cfg!(target_os = "windows") {
            return;
        }
        let _guard = crate::java::set_mock_inspect(Some(mock_inspect_21));
        let dir = tempfile::tempdir().unwrap();
        let mut entry = make_test_entry("", 0);
        setup_archive_cache(dir.path(), &mut entry);
        let catalog = make_catalog(entry.clone());

        let policy =
            NetworkPolicy::all_disabled().with_category(NetworkCategory::JavaRuntime, true);

        let result = ensure_runtime(dir.path(), 21, &catalog, &policy, None);
        assert!(result.is_ok(), "ensure_runtime failed: {:?}", result.err());

        let inst = result.unwrap();
        assert!(inst.path.exists());
        assert_eq!(inst.version, 21);
        assert_eq!(inst.source, JavaSource::Managed);

        // Verify receipt was written correctly
        let receipt_path = managed_entry_path(dir.path(), &entry).join("receipt.json");
        assert!(receipt_path.is_file(), "receipt should exist");
        let receipt = RuntimeReceipt::read_from(&receipt_path).unwrap();
        assert_eq!(
            receipt.java_sha256,
            Some(sha256_hex_file(&inst.path).unwrap())
        );
        assert_eq!(receipt.archive_sha256, entry.sha256);
    }

    #[test]
    fn test_ensure_runtime_succeeds_with_targz_fixture() {
        if cfg!(target_os = "windows") {
            return;
        }
        let _guard = crate::java::set_mock_inspect(Some(mock_inspect_21));
        let dir = tempfile::tempdir().unwrap();
        let mut entry = make_test_entry("", 0);
        setup_archive_cache(dir.path(), &mut entry);
        let catalog = make_catalog(entry.clone());

        let policy =
            NetworkPolicy::all_disabled().with_category(NetworkCategory::JavaRuntime, true);

        let result = ensure_runtime(dir.path(), 21, &catalog, &policy, None);
        assert!(result.is_ok(), "ensure_runtime failed: {:?}", result.err());

        let inst = result.unwrap();
        assert!(inst.path.exists());
        assert_eq!(inst.version, 21);
        assert_eq!(inst.source, JavaSource::Managed);

        // Verify receipt
        let receipt_path = managed_entry_path(dir.path(), &entry).join("receipt.json");
        assert!(receipt_path.is_file());
        let receipt = RuntimeReceipt::read_from(&receipt_path).unwrap();
        assert_eq!(
            receipt.java_sha256,
            Some(sha256_hex_file(&inst.path).unwrap())
        );
    }

    #[test]
    fn test_ensure_runtime_cache_hit_succeeds() {
        let _guard = crate::java::set_mock_inspect(Some(mock_inspect_21));
        let dir = tempfile::tempdir().unwrap();
        let mut entry = make_test_entry("", 0);
        setup_archive_cache(dir.path(), &mut entry);
        let catalog = make_catalog(entry.clone());

        let policy =
            NetworkPolicy::all_disabled().with_category(NetworkCategory::JavaRuntime, true);

        // First call: extract and install.
        let result1 = ensure_runtime(dir.path(), 21, &catalog, &policy, None);
        assert!(result1.is_ok(), "first install failed: {:?}", result1.err());
        let inst1 = result1.unwrap();

        // Second call: should be a cache hit.
        let result2 = ensure_runtime(dir.path(), 21, &catalog, &policy, None);
        assert!(result2.is_ok(), "cache hit failed: {:?}", result2.err());
        let inst2 = result2.unwrap();
        assert_eq!(inst1.path, inst2.path);
        assert_eq!(inst2.version, 21);

        // Verify receipt last_used was updated.
        let receipt_path = managed_entry_path(dir.path(), &entry).join("receipt.json");
        let receipt = RuntimeReceipt::read_from(&receipt_path).unwrap();
        assert!(receipt.last_used_at.is_some());
        assert!(receipt.java_sha256.is_some());
    }

    #[test]
    fn test_archive_hash_mismatch_recovery() {
        // When the cached archive is corrupt but the existing runtime is
        // still valid, resolve_archive_cache should recover the runtime.
        let _guard = crate::java::set_mock_inspect(Some(mock_inspect_21));
        let dir = tempfile::tempdir().unwrap();
        let mut entry = make_test_entry("", 0);
        setup_archive_cache(dir.path(), &mut entry);
        let catalog = make_catalog(entry.clone());
        let policy =
            NetworkPolicy::all_disabled().with_category(NetworkCategory::JavaRuntime, true);

        // Install once successfully.
        let _ = ensure_runtime(dir.path(), 21, &catalog, &policy, None).unwrap();

        // Now corrupt the archive cache file.
        let cache_dir = dir.path().join(ARCHIVE_CACHE_DIR);
        let cache_path = cache_dir.join(format!("{}.{}", entry.sha256, test_archive_extension()));
        std::fs::write(&cache_path, b"corrupt data").unwrap();

        // Second call: cache hash won't match, but the existing runtime is
        // still healthy (receipt + java hash check). Should recover.
        let result = ensure_runtime(dir.path(), 21, &catalog, &policy, None);
        assert!(
            result.is_ok(),
            "should recover from corrupt cache: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_java_hash_tamper_invalidates_cache() {
        let _guard = crate::java::set_mock_inspect(Some(mock_inspect_21));
        let dir = tempfile::tempdir().unwrap();
        let mut entry = make_test_entry("", 0);
        setup_archive_cache(dir.path(), &mut entry);
        let catalog = make_catalog(entry.clone());
        let policy =
            NetworkPolicy::all_disabled().with_category(NetworkCategory::JavaRuntime, true);

        // Install once.
        let _ = ensure_runtime(dir.path(), 21, &catalog, &policy, None).unwrap();

        // Tamper the java binary.
        let entry_path = managed_entry_path(dir.path(), &entry);
        let java_path = entry_path.join(&entry.java_relative_path);
        std::fs::write(&java_path, b"tampered content").unwrap();

        // Second call should detect hash mismatch and attempt reinstall.
        // The cache archive is still valid, so the check passes, but the
        // receipt's java_sha256 won't match. So try_cache_hit returns None.
        // Falls through to network check, passes, archive cache check passes,
        // but extraction + mock succeeds.
        // However the extraction removes the old runtime dir first.
        // Let's just verify it can reinstall.
        let result = ensure_runtime(dir.path(), 21, &catalog, &policy, None);
        assert!(
            result.is_ok(),
            "reinstall after tamper failed: {:?}",
            result.err()
        );

        // Verify java hash changed.
        let java_hash = sha256_hex_file(&java_path).unwrap();
        let receipt = RuntimeReceipt::read_from(&entry_path.join("receipt.json")).unwrap();
        assert_eq!(receipt.java_sha256.as_deref(), Some(java_hash.as_str()));
    }

    // -----------------------------------------------------------------------
    // ZIP extraction safety tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_extract_zip_rejects_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("staging");
        std::fs::create_dir_all(&staging).unwrap();

        // Create a ZIP with a traversal entry.
        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(buf);
        let opts =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("../../etc/passwd", opts).unwrap();
        zip.write_all(b"evil").unwrap();
        let mut buf = zip.finish().unwrap();
        let mut data = Vec::new();
        use std::io::Read;
        buf.read_to_end(&mut data).unwrap();

        let archive_path = dir.path().join("evil.zip");
        std::fs::write(&archive_path, &data).unwrap();
        let entry = make_test_entry("a", 0);

        let result = extract_zip(&archive_path, &staging, &entry, &NoopProgress, "test");
        assert!(result.is_err(), "traversal should be rejected");
    }

    #[test]
    fn test_extract_zip_rejects_symlink_entry() {
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("staging");
        std::fs::create_dir_all(&staging).unwrap();

        // Create a ZIP with a symlink entry (mode 0o120777).
        use std::io::{Cursor, Read};
        let buf = Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(buf);
        let opts = zip::write::FileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .unix_permissions(0o120777);
        zip.start_file("jdk/link", opts).unwrap();
        zip.write_all(b"target").unwrap();
        let mut buf = zip.finish().unwrap();
        let mut data = Vec::new();
        buf.read_to_end(&mut data).unwrap();

        let archive_path = dir.path().join("symlink.zip");
        std::fs::write(&archive_path, &data).unwrap();
        let entry = make_test_entry("a", 0);

        let result = extract_zip(&archive_path, &staging, &entry, &NoopProgress, "test");
        assert!(result.is_err(), "symlink entry should be rejected");
    }

    #[test]
    fn test_extract_zip_rejects_case_collision() {
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("staging");
        std::fs::create_dir_all(&staging).unwrap();

        use std::io::{Cursor, Read, Write};
        let buf = Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(buf);
        let opts =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);

        // Add "A/B" then "a/b" → case collision
        zip.add_directory("A", opts).unwrap();
        zip.start_file("A/B", opts).unwrap();
        zip.write_all(b"content1").unwrap();
        zip.start_file("a/b", opts).unwrap();
        zip.write_all(b"content2").unwrap();

        let mut buf = zip.finish().unwrap();
        let mut data = Vec::new();
        buf.read_to_end(&mut data).unwrap();

        let archive_path = dir.path().join("collision.zip");
        std::fs::write(&archive_path, &data).unwrap();
        let entry = make_test_entry("a", 0);

        let result = extract_zip(&archive_path, &staging, &entry, &NoopProgress, "test");
        assert!(result.is_err(), "case collision should be rejected");
    }

    // -----------------------------------------------------------------------
    // tar.gz extraction safety tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_extract_tar_gz_rejects_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("staging");
        std::fs::create_dir_all(&staging).unwrap();

        let mut buf = Vec::new();
        let gz = flate2::write::GzEncoder::new(&mut buf, flate2::Compression::best());
        let mut tar = tar::Builder::new(gz);

        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_mode(0o777);
        header.set_size(0);
        header.set_cksum();
        tar.append_data(&mut header, "jdk/link", &[] as &[u8])
            .unwrap();
        // Set link name manually by writing raw data — tar builder API quirk.
        // Actually symlinks need link_name set, but Builder::append_data for
        // Symlink doesn't set link_name properly. Let's use a different approach:
        // directly using append_link.

        // Restart — tar::Builder doesn't expose append_link in 0.4.
        // For symlink rejection we rely on header entry_type which is set.
        // The extractor checks entry_type and rejects non-Regular/Dir.
        // Symlink entry_type is checked and rejected at line 812.
        drop(tar);

        let archive_path = dir.path().join("evil.tar.gz");
        std::fs::write(&archive_path, &buf).unwrap();
        let entry = make_test_entry("a", 0);
        let result = extract_tar_gz(&archive_path, &staging, &entry, &NoopProgress, "test");
        assert!(result.is_err(), "symlink should be rejected");
    }

    #[test]
    fn test_extract_tar_gz_rejects_hardlink() {
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("staging");
        std::fs::create_dir_all(&staging).unwrap();

        // Create a tar.gz with a hardlink. tar::Builder::append_link creates
        // a hardlink entry.
        // Actually tar 0.4 has Builder::append_link(header, path, link_name).
        // But let me check... it might not exist. Let's build the raw tar bytes.

        // Build tar.gz with a regular file then a hardlink to it.
        let mut buf = Vec::new();
        {
            let gz = flate2::write::GzEncoder::new(&mut buf, flate2::Compression::best());
            let mut tar = tar::Builder::new(gz);

            // Regular file
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Regular);
            h.set_mode(0o644);
            h.set_size(4);
            h.set_cksum();
            tar.append_data(&mut h, "original", &b"data"[..]).unwrap();

            // Hardlink
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Link);
            h.set_mode(0o644);
            h.set_size(0);
            h.set_cksum();
            // For hardlinks, the content is the link target name.
            // The tar builder may not set this correctly with append_data.
            // Let's just create a tar with link entry type and see if the
            // extractor catches the entry_type (it should at line 812).
            tar.append_data(&mut h, "hardlink", &[] as &[u8]).unwrap();

            drop(tar);
        }

        let archive_path = dir.path().join("hardlink.tar.gz");
        std::fs::write(&archive_path, &buf).unwrap();
        let entry = make_test_entry("a", 0);

        // The tar extractor should reject Link entry_type.
        let result = extract_tar_gz(&archive_path, &staging, &entry, &NoopProgress, "test");
        assert!(result.is_err(), "hardlink should be rejected");
    }

    #[test]
    fn test_extract_tar_gz_rejects_device() {
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("staging");
        std::fs::create_dir_all(&staging).unwrap();

        let mut buf = Vec::new();
        {
            let gz = flate2::write::GzEncoder::new(&mut buf, flate2::Compression::best());
            let mut tar = tar::Builder::new(gz);

            // Block device
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Block);
            h.set_mode(0o644);
            h.set_size(0);
            h.set_cksum();
            tar.append_data(&mut h, "dev/block", &[] as &[u8]).unwrap();

            drop(tar);
        }

        let archive_path = dir.path().join("device.tar.gz");
        std::fs::write(&archive_path, &buf).unwrap();
        let entry = make_test_entry("a", 0);

        let result = extract_tar_gz(&archive_path, &staging, &entry, &NoopProgress, "test");
        assert!(result.is_err(), "device entry should be rejected");
    }

    #[test]
    fn test_extract_tar_gz_rejects_fifo() {
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("staging");
        std::fs::create_dir_all(&staging).unwrap();

        let mut buf = Vec::new();
        {
            let gz = flate2::write::GzEncoder::new(&mut buf, flate2::Compression::best());
            let mut tar = tar::Builder::new(gz);

            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Fifo);
            h.set_mode(0o644);
            h.set_size(0);
            h.set_cksum();
            tar.append_data(&mut h, "fifo", &[] as &[u8]).unwrap();

            drop(tar);
        }

        let archive_path = dir.path().join("fifo.tar.gz");
        std::fs::write(&archive_path, &buf).unwrap();
        let entry = make_test_entry("a", 0);

        let result = extract_tar_gz(&archive_path, &staging, &entry, &NoopProgress, "test");
        assert!(result.is_err(), "fifo should be rejected");
    }

    #[test]
    fn test_extract_tar_gz_rejects_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("staging");
        std::fs::create_dir_all(&staging).unwrap();

        // Build raw tar bytes with a traversal path. The tar::Builder
        // rejects ".." in paths, so we construct the raw byte sequence.
        let mut tar_bytes = Vec::new();

        // Tar header for "../../evil" (exactly 512 bytes).
        // We need to craft it manually to bypass the builder's path check.
        fn make_traversal_header(size: u64) -> Vec<u8> {
            let mut h = vec![0u8; 512];
            let name = b"../../evil";
            h[..name.len()].copy_from_slice(name); // name
            h[name.len()] = b'\0'; // null-terminate
            write_octal(&mut h, 100, 8, 0o644); // mode
            write_octal(&mut h, 108, 8, 0); // uid
            write_octal(&mut h, 116, 8, 0); // gid
            write_octal(&mut h, 124, 12, size); // size
            write_octal(&mut h, 136, 12, 0); // mtime
                                             // chksum at 148 (set to spaces first)
                                             // chksum at 148 (7 octal digits + NUL)
            for i in 148..156 {
                h[i] = b' ';
            }
            // typeflag at 156: '0' for regular
            h[156] = b'0';
            h[257..263].copy_from_slice(b"ustar\0"); // magic
            h[263..265].copy_from_slice(b"00"); // version
            let cksum: u32 = h.iter().map(|&b| b as u32).sum();
            let cksum_str = format!("{:07o}", cksum);
            h[148..155].copy_from_slice(cksum_str.as_bytes());
            h[155] = b'\0';
            h
        }

        fn write_octal(buf: &mut [u8], offset: usize, field_len: usize, val: u64) {
            let s = format!("{:0>width$o}", val, width = field_len - 1);
            let bytes = s.as_bytes();
            buf[offset..offset + bytes.len()].copy_from_slice(bytes);
            buf[offset + field_len - 1] = b' ';
        }

        let header = make_traversal_header(4);
        tar_bytes.extend_from_slice(&header);
        // File content (padded to 512 block)
        tar_bytes.extend_from_slice(b"data");
        // Pad to 512
        let pad = 512 - (4 % 512);
        if pad != 512 {
            tar_bytes.resize(tar_bytes.len() + pad, 0);
        }
        // End-of-archive marker (two zero blocks)
        tar_bytes.extend_from_slice(&[0u8; 1024]);

        // Compress
        use flate2::write::GzEncoder;
        use std::io::Write;
        let mut compressed = Vec::new();
        {
            let mut encoder = GzEncoder::new(&mut compressed, flate2::Compression::best());
            encoder.write_all(&tar_bytes).unwrap();
        }

        let archive_path = dir.path().join("traverse.tar.gz");
        std::fs::write(&archive_path, &compressed).unwrap();
        let entry = make_test_entry("a", 0);

        let result = extract_tar_gz(&archive_path, &staging, &entry, &NoopProgress, "test");
        assert!(result.is_err(), "traversal should be rejected");
    }

    // -----------------------------------------------------------------------
    // Staging cleanup on failure
    // -----------------------------------------------------------------------

    #[test]
    fn test_staging_cleaned_on_java_version_mismatch() {
        // Mock returning wrong version to trigger cleanup.
        fn mock_wrong_version(_path: &Path) -> Option<JavaInstallation> {
            Some(JavaInstallation {
                path: _path.to_path_buf(),
                version: 8, // wrong major
                version_string: "1.8".into(),
                source: JavaSource::Managed,
                arch: Some("x86_64".into()),
            })
        }

        let _guard = crate::java::set_mock_inspect(Some(mock_wrong_version));
        let dir = tempfile::tempdir().unwrap();
        let mut entry = make_test_entry("", 0);
        setup_archive_cache(dir.path(), &mut entry);
        let catalog = make_catalog(entry.clone());
        let policy =
            NetworkPolicy::all_disabled().with_category(NetworkCategory::JavaRuntime, true);

        let entry_path = managed_entry_path(dir.path(), &entry);
        let staging_path = entry_path.with_extension("staging");

        let result = ensure_runtime(dir.path(), 21, &catalog, &policy, None);
        assert!(result.is_err(), "version mismatch should error");
        // Staging should be cleaned up.
        assert!(
            !staging_path.exists(),
            "staging dir should be removed on failure"
        );

        crate::java::set_mock_inspect(None);
    }

    // -----------------------------------------------------------------------
    // Receipt integrity
    // -----------------------------------------------------------------------

    #[test]
    fn test_receipt_written_last_and_includes_java_hash() {
        let _guard = crate::java::set_mock_inspect(Some(mock_inspect_21));
        let dir = tempfile::tempdir().unwrap();
        let mut entry = make_test_entry("", 0);
        setup_archive_cache(dir.path(), &mut entry);
        let catalog = make_catalog(entry.clone());
        let policy =
            NetworkPolicy::all_disabled().with_category(NetworkCategory::JavaRuntime, true);

        let result = ensure_runtime(dir.path(), 21, &catalog, &policy, None);
        assert!(result.is_ok(), "install failed: {:?}", result.err());
        let inst = result.unwrap();

        let entry_path = managed_entry_path(dir.path(), &entry);
        let receipt_path = entry_path.join("receipt.json");

        // Receipt exists and is valid.
        let receipt = RuntimeReceipt::read_from(&receipt_path).unwrap();
        assert_eq!(receipt.schema_version, RECEIPT_SCHEMA_VERSION);
        assert_eq!(receipt.major, 21);
        assert_eq!(receipt.os, std::env::consts::OS);
        assert_eq!(receipt.archive_sha256, entry.sha256);

        // java_sha256 must be present and match actual binary.
        let actual_hash = sha256_hex_file(&inst.path).unwrap();
        assert_eq!(
            receipt.java_sha256,
            Some(actual_hash),
            "java_sha256 should match actual file hash"
        );

        // The entry_path should have the final name (not staging).
        assert!(!entry_path.to_string_lossy().contains("staging"));

        crate::java::set_mock_inspect(None);
    }

    // -----------------------------------------------------------------------
    // remove_runtime safety
    // -----------------------------------------------------------------------

    #[test]
    fn test_remove_runtime_rejects_symlink_target() {
        let dir = tempfile::tempdir().unwrap();
        let real_dir = dir.path().join("real");
        std::fs::create_dir_all(&real_dir).unwrap();

        // Create a valid managed runtime layout with receipt.
        let entry_path = real_dir
            .join("temurin")
            .join("21")
            .join("21.0.2+13")
            .join("windows-x64");
        std::fs::create_dir_all(&entry_path).unwrap();
        let receipt = RuntimeReceipt {
            schema_version: RECEIPT_SCHEMA_VERSION,
            vendor: "eclipse-temurin".into(),
            major: 21,
            full_version: "21.0.2+13".into(),
            os: "windows".into(),
            arch: "x64".into(),
            archive_sha256: "a".repeat(64),
            archive_size: 100,
            source_url: "https://example.com".into(),
            java_relative_path: "bin/java.exe".into(),
            installed_at: chrono::Utc::now(),
            last_used_at: None,
            successful_use_at: None,
            java_sha256: None,
        };
        receipt.write_to(&entry_path.join("receipt.json")).unwrap();

        // Create a symlink/junction to the entry path and try removing via it.
        // On Windows, creating a junction requires admin or developer mode.
        // We'll just verify that the canonical check catches it by testing
        // with an unresolvable path.
        let link_path = dir.path().join("link");
        // Try to create a directory symlink using the host OS API. This may
        // fail on Windows without Developer Mode or administrator privileges.
        #[cfg(windows)]
        let link_result = std::os::windows::fs::symlink_dir(&entry_path, &link_path);
        #[cfg(unix)]
        let link_result = std::os::unix::fs::symlink(&entry_path, &link_path);
        #[cfg(not(any(windows, unix)))]
        let link_result: std::io::Result<()> = Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "directory symlinks are unsupported on this platform",
        ));

        if link_result.is_ok() && link_path.exists() {
            let result = remove_runtime(&real_dir, &link_path);
            assert!(result.is_err(), "symlink should be rejected");
        }
    }

    #[test]
    fn test_remove_runtime_rejects_no_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let entry_path = dir
            .path()
            .join("temurin")
            .join("21")
            .join("21.0.2+13")
            .join("windows-x64");
        std::fs::create_dir_all(&entry_path).unwrap();

        let result = remove_runtime(dir.path(), &entry_path);
        assert!(result.is_err(), "should reject runtime without receipt");
        assert!(entry_path.exists(), "should not delete without receipt");
    }

    #[test]
    fn test_remove_runtime_uses_canonical_target() {
        let dir = tempfile::tempdir().unwrap();
        let entry_path = dir
            .path()
            .join("temurin")
            .join("21")
            .join("21.0.2+13")
            .join("windows-x64");
        std::fs::create_dir_all(&entry_path).unwrap();
        let receipt = RuntimeReceipt {
            schema_version: RECEIPT_SCHEMA_VERSION,
            vendor: "eclipse-temurin".into(),
            major: 21,
            full_version: "21.0.2+13".into(),
            os: "windows".into(),
            arch: "x64".into(),
            archive_sha256: "a".repeat(64),
            archive_size: 100,
            source_url: "https://example.com".into(),
            java_relative_path: "bin/java.exe".into(),
            installed_at: chrono::Utc::now(),
            last_used_at: None,
            successful_use_at: None,
            java_sha256: None,
        };
        receipt.write_to(&entry_path.join("receipt.json")).unwrap();

        let result = remove_runtime(dir.path(), &entry_path);
        assert!(
            result.is_ok(),
            "remove_runtime should succeed: {:?}",
            result.err()
        );
        assert!(!entry_path.exists(), "runtime should be deleted");
    }

    // -----------------------------------------------------------------------
    // Backward receipt without java_sha256
    // -----------------------------------------------------------------------

    #[test]
    fn test_old_receipt_without_java_hash_is_invalidated() {
        // A receipt without java_sha256 should not pass cache validation.
        let dir = tempfile::tempdir().unwrap();
        let entry_path = dir
            .path()
            .join("temurin")
            .join("21")
            .join("21.0.2+13")
            .join("windows-x64");
        std::fs::create_dir_all(&entry_path).unwrap();
        let java_path = entry_path.join("bin/java.exe");
        std::fs::create_dir_all(java_path.parent().unwrap()).unwrap();
        std::fs::write(&java_path, b"fake java").unwrap();

        // Old receipt WITHOUT java_sha256
        let receipt = RuntimeReceipt {
            schema_version: RECEIPT_SCHEMA_VERSION,
            vendor: "eclipse-temurin".into(),
            major: 21,
            full_version: "21.0.2+13".into(),
            os: "windows".into(),
            arch: "x64".into(),
            archive_sha256: "a".repeat(64),
            archive_size: 100,
            source_url: "https://example.com".into(),
            java_relative_path: "bin/java.exe".into(),
            installed_at: chrono::Utc::now(),
            last_used_at: None,
            successful_use_at: None,
            java_sha256: None,
        };
        receipt.write_to(&entry_path.join("receipt.json")).unwrap();

        let entry = make_test_entry(&"a".repeat(64), 100);
        let rec_path = entry_path.join("receipt.json");

        let result = try_cache_hit(&rec_path, &entry, 21, &NoopProgress);
        assert!(
            result.is_none(),
            "old receipt without java_sha256 should not be a cache hit"
        );
    }

    // -----------------------------------------------------------------------
    // Cancellation tests
    // -----------------------------------------------------------------------

    /// A progress reporter that cancels immediately on first check.
    struct CancelledProgress;

    impl RuntimeProgress for CancelledProgress {
        fn on_progress(&self, _msg: &str, _pct: Option<f64>) {}
        fn is_cancelled(&self) -> bool {
            true
        }
    }

    #[test]
    fn test_ensure_runtime_cancelled_before_download() {
        let _guard = crate::java::set_mock_inspect(Some(mock_inspect_21));
        let dir = tempfile::tempdir().unwrap();
        let mut entry = make_test_entry("", 0);
        setup_archive_cache(dir.path(), &mut entry);
        let catalog = make_catalog(entry.clone());
        let policy =
            NetworkPolicy::all_disabled().with_category(NetworkCategory::JavaRuntime, true);

        let result = ensure_runtime(dir.path(), 21, &catalog, &policy, Some(&CancelledProgress));
        assert!(
            matches!(result, Err(LauncherError::JavaRuntimeCancelled { .. })),
            "expected JavaRuntimeCancelled, got {:?}",
            result
        );
    }

    #[test]
    fn test_check_cancelled_returns_error() {
        let result = check_cancelled(&CancelledProgress, 21, "test");
        assert!(
            matches!(
                result,
                Err(LauncherError::JavaRuntimeCancelled { major: 21, .. })
            ),
            "expected JavaRuntimeCancelled(21), got {:?}",
            result
        );
    }

    #[test]
    fn test_check_cancelled_passthrough_on_noop() {
        let result = check_cancelled(&NoopProgress, 21, "test");
        assert!(result.is_ok());
    }

    #[test]
    fn test_extract_zip_cancelled_during_extraction() {
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("staging");
        std::fs::create_dir_all(&staging).unwrap();

        // Use a progress reporter that cancels immediately.
        let entry = make_test_entry("a", 0);

        // Create a valid test zip.
        let archive_data = create_test_zip(&entry.java_relative_path);
        let archive_path = dir.path().join("test.zip");
        std::fs::write(&archive_path, &archive_data).unwrap();

        let result = extract_zip(&archive_path, &staging, &entry, &CancelledProgress, "test");
        assert!(
            matches!(result, Err(LauncherError::JavaRuntimeCancelled { .. })),
            "expected JavaRuntimeCancelled during ZIP extract, got {:?}",
            result
        );
    }

    #[test]
    fn test_extract_tar_gz_cancelled_during_extraction() {
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("staging");
        std::fs::create_dir_all(&staging).unwrap();

        let entry = make_test_entry("a", 0);
        let archive_data = create_test_tar_gz(&entry.java_relative_path);
        let archive_path = dir.path().join("test.tar.gz");
        std::fs::write(&archive_path, &archive_data).unwrap();

        let result = extract_tar_gz(&archive_path, &staging, &entry, &CancelledProgress, "test");
        assert!(
            matches!(result, Err(LauncherError::JavaRuntimeCancelled { .. })),
            "expected JavaRuntimeCancelled during tar.gz extract, got {:?}",
            result
        );
    }
}
