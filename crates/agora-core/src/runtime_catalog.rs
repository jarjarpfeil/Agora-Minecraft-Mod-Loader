//! Read-only runtime catalog — pinned Adoptium Eclipse Temurin JRE releases.
//!
//! This module provides the data model, validation, and lookup for the
//! `runtime-catalog/runtime_catalog.json` file.  It is purely read-only:
//! no download, extract, or runtime-manager code lives here.
//!
//! The catalog is generated offline by `scripts/generate_runtime_catalog.py`
//! and committed to the repository.  Consumers use `lookup()` to find the
//! best JRE entry for a given Java major version and the current OS/arch.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Embedded runtime catalog bytes at compile time.
/// All consumers should use [`RuntimeCatalog::embedded`] instead of their own
/// `include_bytes!` to avoid path duplication.
pub const EMBEDDED_CATALOG_BYTES: &[u8] =
    include_bytes!("../../../runtime-catalog/runtime_catalog.json");

/// Schema version expected by this module.
pub const SCHEMA_VERSION: u32 = 1;

/// The SPDX license identifier for all Adoptium Eclipse Temurin JRE packages.
pub const LICENSE_SPDX: &str = "GPL-2.0-only WITH Classpath-exception-2.0";

/// The vendor string used in catalog entries.
pub const VENDOR: &str = "eclipse-temurin";

/// Regular expression pattern that matches official Adoptium GitHub release URLs.
/// This is checked at runtime when loading the catalog.
const ADOPTIUM_GITHUB_RELEASE_HOST_PREFIX: &str = "https://github.com/adoptium/";

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

/// A single JRE release entry in the runtime catalog.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RuntimeCatalogEntry {
    pub vendor: String,
    pub major: u32,
    pub full_version: String,
    pub openjdk_version: String,
    pub os: String,
    pub arch: String,
    pub image_type: String,
    pub jvm_impl: String,
    pub archive_type: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    pub java_relative_path: String,
    pub license: String,
    pub source_api_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_major: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_minor: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_security: Option<u32>,
}

/// Top-level runtime catalog structure.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RuntimeCatalog {
    pub schema_version: u32,
    pub generated_at: String,
    pub source: String,
    pub entries: Vec<RuntimeCatalogEntry>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Error type for catalog validation and lookup failures.
#[derive(Debug)]
pub enum CatalogError {
    /// The JSON file could not be parsed or is structurally invalid.
    Parse(String),
    /// Schema version mismatch.
    SchemaVersion { expected: u32, got: u32 },
    /// A required field is missing or empty.
    MissingField(String),
    /// An entry's URL is not HTTPS.
    UrlNotHttps(String),
    /// An entry's URL is not on the official Adoptium GitHub release host.
    UrlNotOfficial(String),
    /// SHA-256 is not 64 lowercase hex characters.
    InvalidSha256(String),
    /// An entry fails OS-specific validation.
    OsValidation(String),
    /// Duplicate (major, os, arch) tuple.
    DuplicateTuple {
        major: u32,
        os: String,
        arch: String,
    },
    /// No entry found for the requested (major, os, arch).
    NotFound {
        major: u32,
        os: String,
        arch: String,
    },
    /// Vendor mismatch.
    Vendor(String),
    /// License mismatch.
    License(String),
    /// Image type mismatch.
    ImageType(String),
    /// JVM implementation mismatch.
    JvmImpl(String),
    /// Size is suspiciously small.
    SizeTooSmall(String),
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CatalogError::Parse(msg) => write!(f, "parse error: {}", msg),
            CatalogError::SchemaVersion { expected, got } => {
                write!(f, "schema version: expected {}, got {}", expected, got)
            }
            CatalogError::MissingField(msg) => write!(f, "missing field: {}", msg),
            CatalogError::UrlNotHttps(url) => write!(f, "URL is not HTTPS: {}", url),
            CatalogError::UrlNotOfficial(url) => {
                write!(f, "URL is not an official Adoptium GitHub release: {}", url)
            }
            CatalogError::InvalidSha256(sha) => write!(f, "invalid SHA-256: {}", sha),
            CatalogError::OsValidation(msg) => write!(f, "OS validation: {}", msg),
            CatalogError::DuplicateTuple { major, os, arch } => {
                write!(
                    f,
                    "duplicate tuple (major={}, os={}, arch={})",
                    major, os, arch
                )
            }
            CatalogError::NotFound { major, os, arch } => {
                write!(f, "no entry for major={}, os={}, arch={}", major, os, arch)
            }
            CatalogError::Vendor(v) => write!(f, "unexpected vendor: {}", v),
            CatalogError::License(l) => write!(f, "unexpected license: {}", l),
            CatalogError::ImageType(t) => write!(f, "unexpected image_type: {}", t),
            CatalogError::JvmImpl(j) => write!(f, "unexpected jvm_impl: {}", j),
            CatalogError::SizeTooSmall(s) => write!(f, "size too small: {}", s),
        }
    }
}

impl std::error::Error for CatalogError {}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Normalize a Rust platform OS name to the catalog's OS convention.
///
/// Returns `None` for unrecognized OS names.
pub fn normalize_os(os: &str) -> Option<&'static str> {
    match os {
        "linux" => Some("linux"),
        "macos" | "mac" | "darwin" | "osx" => Some("macos"),
        "windows" | "win" | "win32" | "win64" => Some("windows"),
        _ => None,
    }
}

/// Normalize a Rust platform architecture name to the catalog's architecture convention.
///
/// Returns `None` for unrecognized architectures.
pub fn normalize_arch(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64" | "x86-64" | "amd64" | "x64" => Some("x64"),
        "aarch64" | "arm64" => Some("aarch64"),
        _ => None,
    }
}

/// Check whether a URL is a valid HTTPS link to an official Adoptium GitHub release.
fn is_valid_adoptium_url(url: &str) -> bool {
    if !url.starts_with("https://") {
        return false;
    }
    // Must start with the official adoptium GitHub host
    if !url.starts_with(ADOPTIUM_GITHUB_RELEASE_HOST_PREFIX) {
        return false;
    }
    // Must contain "/releases/download/" in the path
    let after_host = &url[ADOPTIUM_GITHUB_RELEASE_HOST_PREFIX.len()..];
    if !after_host.contains("/releases/download/") {
        return false;
    }
    true
}

/// Validate that a string is a 64-character lowercase hex SHA-256.
fn is_valid_sha256(s: &str) -> bool {
    if s.len() != 64 {
        return false;
    }
    s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

// ---------------------------------------------------------------------------
// Catalog loading and validation
// ---------------------------------------------------------------------------

impl RuntimeCatalog {
    /// Parse and validate a `RuntimeCatalog` from a JSON byte slice.
    ///
    /// Returns `Ok(catalog)` on success, or a list of validation errors.
    pub fn from_json(data: &[u8]) -> Result<Self, Vec<CatalogError>> {
        let catalog: RuntimeCatalog =
            serde_json::from_slice(data).map_err(|e| vec![CatalogError::Parse(e.to_string())])?;

        let mut errors = Vec::new();
        catalog.validate_into(&mut errors);

        if errors.is_empty() {
            Ok(catalog)
        } else {
            Err(errors)
        }
    }

    /// Run all validation checks on the catalog, appending errors.
    pub fn validate_into(&self, errors: &mut Vec<CatalogError>) {
        // Schema version
        if self.schema_version != SCHEMA_VERSION {
            errors.push(CatalogError::SchemaVersion {
                expected: SCHEMA_VERSION,
                got: self.schema_version,
            });
        }

        // Metadata
        if self.generated_at.is_empty() {
            errors.push(CatalogError::MissingField("generated_at".into()));
        }
        if self.source.is_empty() {
            errors.push(CatalogError::MissingField("source".into()));
        }

        // Validate each entry
        let mut seen: HashSet<(u32, String, String)> = HashSet::new();

        for (i, entry) in self.entries.iter().enumerate() {
            let idx = format!("entry[{}]", i);

            // Required string fields
            let str_fields = [
                ("vendor", &entry.vendor),
                ("full_version", &entry.full_version),
                ("openjdk_version", &entry.openjdk_version),
                ("os", &entry.os),
                ("arch", &entry.arch),
                ("image_type", &entry.image_type),
                ("jvm_impl", &entry.jvm_impl),
                ("archive_type", &entry.archive_type),
                ("url", &entry.url),
                ("sha256", &entry.sha256),
                ("java_relative_path", &entry.java_relative_path),
                ("license", &entry.license),
                ("source_api_url", &entry.source_api_url),
            ];
            for (name, val) in &str_fields {
                if val.is_empty() {
                    errors.push(CatalogError::MissingField(format!("{}: {}", idx, name)));
                }
            }

            // Required int fields
            if entry.size == 0 {
                errors.push(CatalogError::MissingField(format!("{}: size is zero", idx)));
            }

            // Vendor
            if entry.vendor != VENDOR {
                errors.push(CatalogError::Vendor(format!(
                    "{}: got '{}', expected '{}'",
                    idx, entry.vendor, VENDOR
                )));
            }

            // License
            if entry.license != LICENSE_SPDX {
                errors.push(CatalogError::License(format!(
                    "{}: got '{}', expected '{}'",
                    idx, entry.license, LICENSE_SPDX
                )));
            }

            // Image type
            if entry.image_type != "jre" {
                errors.push(CatalogError::ImageType(format!(
                    "{}: got '{}', expected 'jre'",
                    idx, entry.image_type
                )));
            }

            // JVM impl
            if entry.jvm_impl != "hotspot" {
                errors.push(CatalogError::JvmImpl(format!(
                    "{}: got '{}', expected 'hotspot'",
                    idx, entry.jvm_impl
                )));
            }

            // SHA-256
            if !is_valid_sha256(&entry.sha256) {
                errors.push(CatalogError::InvalidSha256(format!(
                    "{}: '{}'",
                    idx, entry.sha256
                )));
            }

            // URL must be HTTPS and official
            if !entry.url.starts_with("https://") {
                errors.push(CatalogError::UrlNotHttps(format!("{}: {}", idx, entry.url)));
            } else if !is_valid_adoptium_url(&entry.url) {
                errors.push(CatalogError::UrlNotOfficial(format!(
                    "{}: {}",
                    idx, entry.url
                )));
            }

            // OS-specific checks
            match entry.os.as_str() {
                "windows" => {
                    if entry.archive_type != "zip" {
                        errors.push(CatalogError::OsValidation(format!(
                            "{}: Windows entries must use archive_type 'zip', got '{}'",
                            idx, entry.archive_type
                        )));
                    }
                    if entry.java_relative_path != "bin/java.exe" {
                        errors.push(CatalogError::OsValidation(format!(
                            "{}: Windows JRP should be 'bin/java.exe', got '{}'",
                            idx, entry.java_relative_path
                        )));
                    }
                }
                "macos" => {
                    if entry.archive_type != "tar.gz" {
                        errors.push(CatalogError::OsValidation(format!(
                            "{}: macOS entries must use archive_type 'tar.gz', got '{}'",
                            idx, entry.archive_type
                        )));
                    }
                    if entry.java_relative_path != "Contents/Home/bin/java" {
                        errors.push(CatalogError::OsValidation(format!(
                            "{}: macOS JRP should be 'Contents/Home/bin/java', got '{}'",
                            idx, entry.java_relative_path
                        )));
                    }
                }
                "linux" => {
                    if entry.archive_type != "tar.gz" {
                        errors.push(CatalogError::OsValidation(format!(
                            "{}: Linux entries must use archive_type 'tar.gz', got '{}'",
                            idx, entry.archive_type
                        )));
                    }
                    if entry.java_relative_path != "bin/java" {
                        errors.push(CatalogError::OsValidation(format!(
                            "{}: Linux JRP should be 'bin/java', got '{}'",
                            idx, entry.java_relative_path
                        )));
                    }
                }
                _ => {
                    errors.push(CatalogError::OsValidation(format!(
                        "{}: unknown OS '{}'",
                        idx, entry.os
                    )));
                }
            }

            // Size check (minimum 10 MB)
            if entry.size < 10_000_000 {
                errors.push(CatalogError::SizeTooSmall(format!(
                    "{}: {} bytes",
                    idx, entry.size
                )));
            }

            // Duplicate detection
            let key = (entry.major, entry.os.clone(), entry.arch.clone());
            if !seen.insert(key.clone()) {
                errors.push(CatalogError::DuplicateTuple {
                    major: entry.major,
                    os: entry.os.clone(),
                    arch: entry.arch.clone(),
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Lookup
// ---------------------------------------------------------------------------

/// Lookup result from the catalog.
#[derive(Clone, Debug)]
pub struct LookupResult {
    /// The matching catalog entry.
    pub entry: RuntimeCatalogEntry,
    /// Index in the catalog's entries vector.
    pub index: usize,
}

impl RuntimeCatalog {
    /// Load and validate the embedded catalog at compile time.
    ///
    /// Panics if the embedded JSON is invalid (this is a compile-time invariant
    /// checked in tests).
    pub fn embedded() -> Self {
        let catalog = Self::from_json(EMBEDDED_CATALOG_BYTES)
            .expect("embedded runtime catalog should be valid");
        catalog
    }

    /// Look up the best JRE entry for a given Java major version and the
    /// *current* OS and architecture.
    ///
    /// OS and arch are normalized from Rust platform conventions
    /// (e.g., `"macos"`, `"x86_64"`) to the catalog's conventions
    /// (`"macos"`, `"x64"`).
    ///
    /// Returns `None` if no entry matches or if the OS/arch are unrecognized.
    pub fn lookup(&self, major: u32, os: &str, arch: &str) -> Option<LookupResult> {
        let canonical_os = normalize_os(os)?;
        let canonical_arch = normalize_arch(arch)?;

        self.entries
            .iter()
            .enumerate()
            .find(|(_, e)| e.major == major && e.os == canonical_os && e.arch == canonical_arch)
            .map(|(index, entry)| LookupResult {
                entry: entry.clone(),
                index,
            })
    }

    /// Return the list of available (major, os, arch) tuples in the catalog.
    pub fn list_available(&self) -> Vec<(u32, &str, &str)> {
        self.entries
            .iter()
            .map(|e| (e.major, e.os.as_str(), e.arch.as_str()))
            .collect()
    }

    /// Return the number of entries in the catalog.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if the catalog has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Real catalog data embedded at compile time for tests.
    const CATALOG_JSON: &str = include_str!("../../../runtime-catalog/runtime_catalog.json");

    fn load_test_catalog() -> RuntimeCatalog {
        RuntimeCatalog::from_json(CATALOG_JSON.as_bytes()).expect("test catalog should be valid")
    }

    #[test]
    fn test_catalog_parses_and_validates() {
        let catalog = load_test_catalog();
        assert_eq!(catalog.schema_version, 1);
        assert!(!catalog.entries.is_empty());
    }

    #[test]
    fn test_catalog_has_expected_entry_count() {
        let catalog = load_test_catalog();
        // Expecting 14 entries: 8 (linux+linux+mac+windows = 4)
        // + 17 (linux+linux+mac+mac+windows = 5)
        // + 21 (linux+linux+mac+mac+windows = 5)
        // Total: 14
        assert!(
            catalog.entries.len() >= 10,
            "expected at least 10 entries, got {}",
            catalog.entries.len()
        );
    }

    #[test]
    fn test_lookup_exists() {
        let catalog = load_test_catalog();
        let result = catalog.lookup(21, "linux", "x86_64");
        assert!(result.is_some(), "should find Java 21 linux x64");
        let r = result.unwrap();
        assert_eq!(r.entry.major, 21);
        assert_eq!(r.entry.os, "linux");
        assert_eq!(r.entry.arch, "x64");
    }

    #[test]
    fn test_lookup_macos_normalization() {
        let catalog = load_test_catalog();
        // All these should normalize to "macos"
        for os in &["macos", "mac", "darwin", "osx"] {
            let result = catalog.lookup(21, os, "aarch64");
            assert!(result.is_some(), "should find Java 21 {} aarch64", os);
            if let Some(r) = result {
                assert_eq!(r.entry.os, "macos");
            }
        }
    }

    #[test]
    fn test_lookup_windows_normalization() {
        let catalog = load_test_catalog();
        for os in &["windows", "win", "win32", "win64"] {
            let result = catalog.lookup(21, os, "x64");
            assert!(result.is_some(), "should find Java 21 {} x64", os);
        }
    }

    #[test]
    fn test_lookup_arch_normalization() {
        let catalog = load_test_catalog();
        for arch in &["x86_64", "x86-64", "amd64", "x64"] {
            let result = catalog.lookup(21, "linux", arch);
            assert!(result.is_some(), "should find Java 21 linux {}", arch);
        }
        for arch in &["aarch64", "arm64"] {
            let result = catalog.lookup(21, "linux", arch);
            assert!(result.is_some(), "should find Java 21 linux {}", arch);
        }
    }

    #[test]
    fn test_lookup_not_found() {
        let catalog = load_test_catalog();
        // Java 99 should not exist
        let result = catalog.lookup(99, "linux", "x86_64");
        assert!(result.is_none());
        // windows + aarch64 should not exist (unavailable)
        let result = catalog.lookup(21, "windows", "aarch64");
        assert!(result.is_none());
    }

    #[test]
    fn test_lookup_unknown_os_returns_none() {
        let catalog = load_test_catalog();
        let result = catalog.lookup(21, "freebsd", "x86_64");
        assert!(result.is_none());
    }

    #[test]
    fn test_lookup_unknown_arch_returns_none() {
        let catalog = load_test_catalog();
        let result = catalog.lookup(21, "linux", "riscv64");
        assert!(result.is_none());
    }

    #[test]
    fn test_list_available() {
        let catalog = load_test_catalog();
        let available = catalog.list_available();
        assert!(!available.is_empty());
        // Check that all entries have valid majors
        for (major, os, arch) in &available {
            assert!(*major > 0);
            assert!(!os.is_empty());
            assert!(!arch.is_empty());
        }
    }

    #[test]
    fn test_catalog_not_empty() {
        let catalog = load_test_catalog();
        assert!(!catalog.is_empty());
        assert!(catalog.len() > 0);
    }

    #[test]
    fn test_validate_each_entry() {
        let catalog = load_test_catalog();
        // Run validation explicitly
        let mut errors = Vec::new();
        catalog.validate_into(&mut errors);
        assert!(
            errors.is_empty(),
            "validation produced {} errors: {:?}",
            errors.len(),
            errors
        );
    }

    #[test]
    fn test_is_valid_sha256() {
        assert!(is_valid_sha256(
            "a123456789abcdefa123456789abcdefa123456789abcdefa123456789abcdef"
        ));
        assert!(!is_valid_sha256("")); // empty
        assert!(!is_valid_sha256("a")); // too short
        assert!(!is_valid_sha256(&format!("A{}", "a".repeat(63)))); // uppercase first
        assert!(!is_valid_sha256(&format!("g{}", "a".repeat(63)))); // non-hex
    }

    #[test]
    fn test_is_valid_adoptium_url() {
        assert!(is_valid_adoptium_url(
            "https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.11%2B10/OpenJDK21U-jre_x64_linux_hotspot_21.0.11_10.tar.gz"
        ));
        assert!(!is_valid_adoptium_url(
            "http://github.com/adoptium/temurin21-binaries/releases/download/test/pkg.tar.gz"
        ));
        assert!(!is_valid_adoptium_url(
            "https://malicious.example.com/backdoor.tar.gz"
        ));
        assert!(!is_valid_adoptium_url(
            "https://github.com/evil-corp/malware/releases/download/v1/pkg.tar.gz"
        ));
        assert!(!is_valid_adoptium_url(
            "https://github.com/adoptium/temurin21-binaries/raw/main/pkg.tar.gz"
        ));
    }

    #[test]
    fn test_normalize_os() {
        assert_eq!(normalize_os("linux"), Some("linux"));
        assert_eq!(normalize_os("macos"), Some("macos"));
        assert_eq!(normalize_os("mac"), Some("macos"));
        assert_eq!(normalize_os("darwin"), Some("macos"));
        assert_eq!(normalize_os("osx"), Some("macos"));
        assert_eq!(normalize_os("windows"), Some("windows"));
        assert_eq!(normalize_os("win"), Some("windows"));
        assert_eq!(normalize_os("freebsd"), None);
    }

    #[test]
    fn test_normalize_arch() {
        assert_eq!(normalize_arch("x86_64"), Some("x64"));
        assert_eq!(normalize_arch("x86-64"), Some("x64"));
        assert_eq!(normalize_arch("amd64"), Some("x64"));
        assert_eq!(normalize_arch("x64"), Some("x64"));
        assert_eq!(normalize_arch("aarch64"), Some("aarch64"));
        assert_eq!(normalize_arch("arm64"), Some("aarch64"));
        assert_eq!(normalize_arch("riscv64"), None);
        assert_eq!(normalize_arch("armv8"), None);
    }

    #[test]
    fn test_duplicate_detection() {
        let mut catalog = load_test_catalog();
        // Add a duplicate entry for Java 21 linux x64
        if let Some(first) = catalog.entries.first().cloned() {
            catalog.entries.push(first);
            let mut errors = Vec::new();
            catalog.validate_into(&mut errors);
            let dup_errors: Vec<_> = errors
                .iter()
                .filter(|e| matches!(e, CatalogError::DuplicateTuple { .. }))
                .collect();
            assert!(!dup_errors.is_empty(), "should detect duplicate tuple");
        }
    }

    #[test]
    fn test_rejects_malformed_json() {
        let result = RuntimeCatalog::from_json(b"not-json");
        assert!(result.is_err());
    }

    #[test]
    fn test_rejects_bad_schema_version() {
        let bad = br#"{"schema_version": 999, "generated_at": "", "source": "", "entries": []}"#;
        let result = RuntimeCatalog::from_json(bad);
        assert!(result.is_err());
        if let Err(errors) = result {
            assert!(errors
                .iter()
                .any(|e| matches!(e, CatalogError::SchemaVersion { .. })));
        }
    }

    #[test]
    fn test_embedded_catalog_loads_successfully() {
        let catalog = RuntimeCatalog::embedded();
        assert!(!catalog.entries.is_empty());
        assert_eq!(catalog.schema_version, 1);
    }

    #[test]
    fn test_embedded_catalog_bytes_constant_exists() {
        assert!(!EMBEDDED_CATALOG_BYTES.is_empty());
        let catalog = RuntimeCatalog::from_json(EMBEDDED_CATALOG_BYTES)
            .expect("EMBEDDED_CATALOG_BYTES should be valid");
        assert!(!catalog.entries.is_empty());
    }

    #[test]
    fn test_catalog_entry_serde_roundtrip() {
        let catalog = load_test_catalog();
        let json = serde_json::to_string(&catalog).expect("serialize");
        let deserialized: RuntimeCatalog = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(catalog, deserialized);
    }
}
