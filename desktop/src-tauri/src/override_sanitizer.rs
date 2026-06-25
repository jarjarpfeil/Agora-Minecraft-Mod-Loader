use crate::error::{LauncherError, LauncherResult};
use serde::Serialize;
use std::io::Read;
use std::path::Path;

/// Hard limits for zip extraction (§7.2.1).
const MAX_ZIP_SIZE: u64 = 500 * 1024 * 1024; // 500MB compressed
const MAX_EXTRACTED_SIZE: u64 = 2 * 1024 * 1024 * 1024; // 2GB total extracted
const MAX_FILE_COUNT: usize = 5000; // 5000 files max

/// Directory whitelist (§7.2.2). Only files under these prefixes are extracted.
const ALLOWED_PREFIXES: &[&str] = &[
    "config/",
    "defaultconfigs/",
    "resourcepacks/",
    "shaderpacks/",
    "datapacks/",
    "kubejs/",
];

/// Banned extensions (§7.2.2). Hard-banned even inside whitelisted directories.
const BANNED_EXTENSIONS: &[&str] = &[
    ".jar", ".class", ".exe", ".bat", ".cmd", ".sh", ".ps1",
    ".dll", ".so", ".dylib", ".msi", ".dmg",
];

/// Result of an override extraction.
#[derive(Debug, Clone, Serialize)]
pub struct ExtractionResult {
    pub extracted: Vec<String>,
    pub skipped: Vec<String>,
    pub total_bytes_written: u64,
}

/// Extract a zip file into an instance directory with full sanitization.
///
/// This is the main entry point for §7.2. It:
/// 1. Checks compressed size against MAX_ZIP_SIZE.
/// 2. Pre-scans all entries for total uncompressed size and file count.
/// 3. Validates each entry path against the directory whitelist.
/// 4. Rejects banned extensions even within whitelisted directories.
/// 5. Prevents Zip Slip (path traversal) by rejecting `..` and absolute paths.
/// 6. Tracks actual bytes written and aborts mid-stream if limits are exceeded.
/// 7. On any security violation, deletes partially extracted files.
pub fn extract_overrides(
    zip_path: &Path,
    dest_dir: &Path,
) -> LauncherResult<ExtractionResult> {
    // Pre-check: compressed file size.
    let zip_size = zip_path
        .metadata()
        .map_err(|_| LauncherError::Generic {
            code: "ERR_OVERRIDE_FAILED".to_string(),
            message: "Could not read zip file metadata.".to_string(),
        })?
        .len();

    if zip_size > MAX_ZIP_SIZE {
        return Err(LauncherError::Generic {
            code: "ERR_ZIP_TOO_LARGE".to_string(),
            message: format!(
                "Zip file is {}MB, exceeds the {}MB limit.",
                zip_size / (1024 * 1024),
                MAX_ZIP_SIZE / (1024 * 1024)
            ),
        });
    }

    let file = std::fs::File::open(zip_path).map_err(|_| LauncherError::Generic {
        code: "ERR_OVERRIDE_FAILED".to_string(),
        message: "Could not open zip file.".to_string(),
    })?;

    let mut archive = zip::ZipArchive::new(file).map_err(|_| LauncherError::Generic {
        code: "ERR_OVERRIDE_FAILED".to_string(),
        message: "Invalid or corrupt zip file.".to_string(),
    })?;

    // Phase 1: Pre-scan all entries for size and count limits.
    let mut total_uncompressed: u64 = 0;
    let mut entry_count: usize = 0;

    for i in 0..archive.len() {
        let entry = archive
            .by_index(i)
            .map_err(|_| LauncherError::Generic {
                code: "ERR_OVERRIDE_FAILED".to_string(),
                message: "Could not read zip entry.".to_string(),
            })?;

        total_uncompressed = total_uncompressed
            .saturating_add(entry.size());
        entry_count += 1;

        if total_uncompressed > MAX_EXTRACTED_SIZE {
            return Err(LauncherError::Generic {
                code: "ERR_ZIP_BOMB".to_string(),
                message: format!(
                    "Total uncompressed size exceeds the {}GB limit. Possible zip bomb.",
                    MAX_EXTRACTED_SIZE / (1024 * 1024 * 1024)
                ),
            });
        }

        if entry_count > MAX_FILE_COUNT {
            return Err(LauncherError::Generic {
                code: "ERR_TOO_MANY_FILES".to_string(),
                message: format!(
                    "Zip contains more than {} files. Limit exceeded.",
                    MAX_FILE_COUNT
                ),
            });
        }
    }

    // Phase 2: Extract with path validation.
    let mut extracted: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut bytes_written: u64 = 0;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|_| LauncherError::Generic {
                code: "ERR_OVERRIDE_FAILED".to_string(),
                message: "Could not read zip entry during extraction.".to_string(),
            })?;

        let raw_name = entry.name().to_string();

        // Sanitize the path: reject absolute paths and parent traversal.
        let safe_name = match sanitize_path(&raw_name) {
            Some(name) => name,
            None => {
                // Path traversal attempt — abort entire extraction.
                cleanup_partial(&dest_dir, &extracted);
                return Err(LauncherError::Generic {
                    code: "ERR_ZIP_SLIP".to_string(),
                    message: format!(
                        "Path traversal detected in zip entry: '{}'. Extraction aborted.",
                        raw_name
                    ),
                });
            }
        };

        // Skip directories (they'll be created by their files).
        if entry.is_dir() {
            continue;
        }

        // Check directory whitelist.
        if !is_whitelisted(&safe_name) {
            skipped.push(safe_name.clone());
            continue;
        }

        // Check banned extensions.
        if has_banned_extension(&safe_name) {
            cleanup_partial(&dest_dir, &extracted);
            return Err(LauncherError::Generic {
                code: "ERR_SECURITY_VIOLATION".to_string(),
                message: format!(
                    "Security Violation: Pack overrides cannot contain executable files or mods. \
                     Banned file type detected: '{}'. \
                     All mods must be routed through the platform manifest.",
                     safe_name
                ),
            });
        }

        // Build the destination path and verify it's inside the sandbox.
        let dest_path = dest_dir.join(&safe_name);
        if !dest_path.starts_with(dest_dir) {
            cleanup_partial(&dest_dir, &extracted);
            return Err(LauncherError::Generic {
                code: "ERR_ZIP_SLIP".to_string(),
                message: format!(
                    "Resolved path escapes the instance directory: '{}'. Extraction aborted.",
                    raw_name
                ),
            });
        }

        // Create parent directories.
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent).map_err(|_| LauncherError::Generic {
                code: "ERR_OVERRIDE_FAILED".to_string(),
                message: "Could not create directory for extracted file.".to_string(),
            })?;
        }

        // Write the file, tracking actual bytes.
        let mut file_data = Vec::new();
        entry
            .read_to_end(&mut file_data)
            .map_err(|_| LauncherError::Generic {
                code: "ERR_OVERRIDE_FAILED".to_string(),
                message: "Could not read file data from zip.".to_string(),
            })?;

        let file_len = file_data.len() as u64;
        bytes_written = bytes_written.saturating_add(file_len);

        // Mid-stream check: abort if actual bytes exceed limit.
        if bytes_written > MAX_EXTRACTED_SIZE {
            cleanup_partial(&dest_dir, &extracted);
            return Err(LauncherError::Generic {
                code: "ERR_ZIP_BOMB".to_string(),
                message: "Actual extracted size exceeds the 2GB limit. Aborting.".to_string(),
            });
        }

        std::fs::write(&dest_path, &file_data).map_err(|_| LauncherError::Generic {
            code: "ERR_OVERRIDE_FAILED".to_string(),
            message: format!("Could not write extracted file: '{}'.", safe_name),
        })?;

        extracted.push(safe_name);
    }

    Ok(ExtractionResult {
        extracted,
        skipped,
        total_bytes_written: bytes_written,
    })
}

/// Strip absolute paths and `../` sequences. Returns None if the path is
/// purely traversal (no valid path remains).
fn sanitize_path(raw: &str) -> Option<String> {
    // Replace backslashes with forward slashes for Windows compatibility.
    let normalized = raw.replace('\\', "/");

    // Reject absolute paths (Unix and Windows drive letters).
    if normalized.starts_with('/') || normalized.matches(':').count() > 0 {
        return None;
    }

    // Split on '/' and rebuild, rejecting any '..' component.
    let mut parts: Vec<&str> = Vec::new();
    for part in normalized.split('/') {
        if part == ".." {
            return None; // Zip Slip attempt
        }
        if part == "." || part.is_empty() {
            continue;
        }
        parts.push(part);
    }

    if parts.is_empty() {
        return None;
    }

    Some(parts.join("/"))
}

/// Check if a path starts with one of the whitelisted directory prefixes.
fn is_whitelisted(path: &str) -> bool {
    ALLOWED_PREFIXES.iter().any(|prefix| path.starts_with(prefix))
}

/// Check if a filename has a banned extension.
fn has_banned_extension(path: &str) -> bool {
    let lower = path.to_lowercase();
    BANNED_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

/// Delete partially extracted files on security violation or error.
fn cleanup_partial(dest_dir: &Path, extracted: &[String]) {
    for file in extracted {
        let path = dest_dir.join(file);
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_path_rejects_traversal() {
        assert!(sanitize_path("../../evil.exe").is_none());
        assert!(sanitize_path("config/../../evil.exe").is_none());
        assert!(sanitize_path("/etc/passwd").is_none());
        assert!(sanitize_path("C:/windows/system32/evil.dll").is_none());
    }

    #[test]
    fn test_sanitize_path_normalizes_backslashes() {
        assert_eq!(
            sanitize_path("config\\mod\\settings.toml").unwrap(),
            "config/mod/settings.toml"
        );
    }

    #[test]
    fn test_sanitize_path_strips_dot_segments() {
        assert_eq!(
            sanitize_path("./config/./mod.toml").unwrap(),
            "config/mod.toml"
        );
    }

    #[test]
    fn test_whitelist_allows_config() {
        assert!(is_whitelisted("config/mod.toml"));
        assert!(is_whitelisted("defaultconfigs/server.toml"));
        assert!(is_whitelisted("resourcepacks/mypack.zip"));
        assert!(is_whitelisted("kubejs/server_scripts/script.js"));
    }

    #[test]
    fn test_whitelist_rejects_mods() {
        assert!(!is_whitelisted("mods/evil.jar"));
        assert!(!is_whitelisted("saves/world/level.dat"));
        assert!(!is_whitelisted("README.txt"));
    }

    #[test]
    fn test_banned_extensions() {
        assert!(has_banned_extension("config/setup.exe"));
        assert!(has_banned_extension("config/lib.dll"));
        assert!(has_banned_extension("kubejs/evil.sh"));
        assert!(!has_banned_extension("config/mod.toml"));
        assert!(!has_banned_extension("kubejs/script.js"));
    }

    #[test]
    fn test_whitelist_allows_shaderpacks() {
        assert!(is_whitelisted("shaderpacks/ComplementaryShaders.zip"));
        assert!(is_whitelisted("datapacks/custom_loot.zip"));
    }

    #[test]
    fn test_whitelist_rejects_shaderpacks_jar() {
        // .jar in shaderpacks is still banned — it should go through mods/
        assert!(has_banned_extension("shaderpacks/evil.jar"));
        // .zip is fine
        assert!(!has_banned_extension("shaderpacks/ComplementaryShaders.zip"));
    }
}
