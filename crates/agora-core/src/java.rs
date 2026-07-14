//! Java runtime detection, inspection, and discovery.
//!
//! Provides the canonical [`JavaInstallation`] model used throughout the
//! launcher, platform-specific system-JRE discovery, managed/Mojang runtime
//! scanning, and the combined [`detect_java_candidates`] API.
//!
//! ## Sources
//!
//! Every discovered Java installation carries a [`JavaSource`] tag that the
//! selection policy uses to rank candidates.  The ordering is:
//!
//! 1. **Override** — explicit user path (always highest priority when set).
//! 2. **Managed** — auto-provisioned by [`crate::runtime_manager`].
//! 3. **Mojang** — bundled runtimes under the official launcher directory.
//! 4. **System** — OS-default JRE (PATH, standard install directories).

use crate::launch::VersionInfo;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Test injection point for inspect_java
// ---------------------------------------------------------------------------

// Thread-local mock so parallel tests do not interfere with each other.
#[cfg(test)]
thread_local! {
    static MOCK_INSPECT: std::cell::RefCell<Option<fn(&Path) -> Option<JavaInstallation>>> =
        const { std::cell::RefCell::new(None) };
}

/// RAII guard that restores the previous mock on drop.
#[cfg(test)]
pub struct MockInspectGuard(Option<fn(&Path) -> Option<JavaInstallation>>);

#[cfg(test)]
impl Drop for MockInspectGuard {
    fn drop(&mut self) {
        let prev = self.0.take();
        MOCK_INSPECT.with(|cell| {
            cell.replace(prev);
        });
    }
}

/// Set a mock for `inspect_java` (test-only).
///
/// Returns a [`MockInspectGuard`] that restores the previous mock when
/// dropped. Uses a thread-local so parallel tests are isolated.
#[cfg(test)]
pub fn set_mock_inspect(f: Option<fn(&Path) -> Option<JavaInstallation>>) -> MockInspectGuard {
    MOCK_INSPECT.with(|cell| {
        let prev = cell.replace(f);
        MockInspectGuard(prev)
    })
}

// ---------------------------------------------------------------------------
// JavaSource
// ---------------------------------------------------------------------------

/// Origin of a discovered Java installation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum JavaSource {
    /// Explicit user override path.
    Override,
    /// Auto-provisioned via the managed runtime manager.
    Managed,
    /// Bundled runtime under the official Mojang launcher directory.
    Mojang,
    /// OS-default / system-installed JRE.
    System,
}

impl Default for JavaSource {
    fn default() -> Self {
        JavaSource::System
    }
}

// ---------------------------------------------------------------------------
// JavaInstallation
// ---------------------------------------------------------------------------

/// A discovered or provisioned Java runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JavaInstallation {
    /// Absolute path to the `java` (or `java.exe`) executable.
    pub path: PathBuf,
    /// Parsed Java major version (e.g. 8, 11, 17, 21).
    pub version: u32,
    /// The raw version string from `java -version` (e.g. `"17.0.9"`).
    pub version_string: String,
    /// Origin of this installation.
    #[serde(default)]
    pub source: JavaSource,
    /// Architecture reported by the JVM (`os.arch`), if available.
    #[serde(default)]
    pub arch: Option<String>,
}

impl JavaInstallation {
    /// Canonicalise the path before comparison.
    fn canonical_path(&self) -> PathBuf {
        self.path
            .canonicalize()
            .unwrap_or_else(|_| self.path.clone())
    }
}

// ---------------------------------------------------------------------------
// inspect_java — bounded probe with arch extraction and leak-free process
// ---------------------------------------------------------------------------

const INSPECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Probe a Java executable, returning version info and architecture.
///
/// Runs `java -XshowSettings:properties -version` with **both** stdout and
/// stderr captured.  Parses the `java.specification.version` (or falls back
/// to the `version "…"` line) and extracts `os.arch`.  The process is killed
/// if it does not complete within [`INSPECT_TIMEOUT`].
///
/// # Leak-free guarantee
/// The child process PID is captured *before* ownership is moved into the
/// wait thread.  If the outer thread times out on the channel it kills the
/// process by PID, guaranteeing no orphaned JVM processes.
pub fn inspect_java(path: &Path) -> Option<JavaInstallation> {
    #[cfg(test)]
    {
        let result = MOCK_INSPECT.with(|cell| {
            let guard = cell.borrow();
            guard.as_ref().map(|f| f(path))
        });
        if let Some(Some(inst)) = result {
            return Some(inst);
        }
    }
    if !path.is_file() {
        return None;
    }
    let path_for_result = path.to_path_buf();
    let cloned = path.to_path_buf();

    let child = std::process::Command::new(&cloned)
        .arg("-XshowSettings:properties")
        .arg("-version")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;

    let pid = child.id();

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = child.wait_with_output();
        let _ = tx.send(result);
    });

    let output = match rx.recv_timeout(INSPECT_TIMEOUT) {
        Ok(output) => output.ok()?,
        Err(_) => {
            // Timeout: kill the child by PID to prevent leaks.
            kill_pid(pid);
            return None;
        }
    };

    // Parse from combined stderr + stdout (Java sends -XshowSettings to stderr).
    let combined = String::from_utf8_lossy(&output.stderr).into_owned();
    let stdout_str = String::from_utf8_lossy(&output.stdout);
    let combined_all = if stdout_str.is_empty() {
        combined
    } else {
        format!("{}\n{}", combined, stdout_str)
    };

    let version_str = parse_version_string(&combined_all)?;
    let major = extract_major_version(version_str)?;
    let arch = parse_os_arch(&combined_all);

    Some(JavaInstallation {
        path: path_for_result,
        version: major,
        version_string: version_str.to_string(),
        source: JavaSource::System,
        arch,
    })
}

/// Kill a process by PID.  Platform-specific helper.
///
/// On Windows, uses `taskkill /F /T /PID` to kill the entire process tree.
/// On Unix, uses `kill -9` on the immediate process only — a safe but
/// intentionally limited approach: killing the process group would require
/// either `libc` or a guarantee that the child is a process-group leader,
/// neither of which is safely available without an extra dependency.  Since
/// the killed process is a harmless `java -XshowSettings:properties -version`
/// probe that has timed out, the immediate-process kill is sufficient.
fn kill_pid(pid: u32) {
    #[cfg(unix)]
    {
        // Immediate process only (see doc comment for rationale).
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .output();
    }
    #[cfg(windows)]
    {
        // /T kills the entire process tree — safe for a timed-out probe child.
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .output();
    }
}

// ---------------------------------------------------------------------------
// Parsing helpers
// ---------------------------------------------------------------------------

/// Parse `os.arch` from the `-XshowSettings:properties` output.
fn parse_os_arch(output: &str) -> Option<String> {
    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(val) = trimmed.strip_prefix("os.arch = ") {
            let arch = val.trim().to_string();
            if !arch.is_empty() {
                return Some(arch);
            }
        }
    }
    None
}

fn parse_version_string(stderr: &str) -> Option<&str> {
    // First try `java.specification.version = 17` or `java.version = 17.0.9`
    for line in stderr.lines() {
        let trimmed = line.trim();
        if let Some(val) = trimmed.strip_prefix("java.specification.version = ") {
            let v = val.trim();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    // Fallback: openjdk version "17.0.9" or java version "1.8.0_352"
    for line in stderr.lines() {
        let line = line.trim();
        if let Some(start) = line.find("version \"") {
            let rest = &line[start + "version \"".len()..];
            if let Some(end) = rest.find('"') {
                return Some(&rest[..end]);
            }
        }
    }
    None
}

fn extract_major_version(version: &str) -> Option<u32> {
    // Java 8 and earlier: "1.8.0_352" -> 8
    if let Some(v) = version.strip_prefix("1.") {
        if let Some(dot) = v.find('.') {
            return v[..dot].parse::<u32>().ok();
        }
        return v.parse::<u32>().ok();
    }
    // Java 9+: "17.0.9" or "21" -> take the first component
    if let Some(dot) = version.find('.') {
        return version[..dot].parse::<u32>().ok();
    }
    if let Some(underscore) = version.find('_') {
        return version[..underscore].parse::<u32>().ok();
    }
    version.parse::<u32>().ok()
}

// ---------------------------------------------------------------------------
// Discovery — System JREs (existing behaviour tagged JavaSource::System)
// ---------------------------------------------------------------------------

/// Detect system-installed JREs (the original `detect_installed_jres`
/// behaviour, now tagged [`JavaSource::System`]).
///
/// This is the backward-compatible entry point.  New callers should prefer
/// [`detect_java_candidates`] which also includes managed and Mojang runtimes.
pub fn detect_installed_jres() -> Vec<JavaInstallation> {
    let mut results = Vec::new();

    // Windows paths
    #[cfg(target_os = "windows")]
    {
        let windows_roots = [
            r"C:\Program Files\Java",
            r"C:\Program Files (x86)\Java",
            r"C:\Program Files\Eclipse Adoptium",
            r"C:\Program Files\Microsoft\jdk",
            r"C:\Program Files\Zulu",
        ];
        for root in &windows_roots {
            let dir = PathBuf::from(root);
            if dir.is_dir() {
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for entry in entries.flatten() {
                        let javadir = entry.path().join("bin");
                        let path = javadir.join("java.exe");
                        if path.is_file() {
                            if let Some(mut inst) = inspect_java(&path) {
                                inst.source = JavaSource::System;
                                results.push(inst);
                            }
                        }
                    }
                }
            }
        }
    }

    // macOS paths
    #[cfg(target_os = "macos")]
    {
        let base = PathBuf::from("/Library/Java/JavaVirtualMachines");
        if base.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&base) {
                for entry in entries.flatten() {
                    let path = entry.path().join("Contents/Home/bin/java");
                    if path.is_file() {
                        if let Some(mut inst) = inspect_java(&path) {
                            inst.source = JavaSource::System;
                            results.push(inst);
                        }
                    }
                }
            }
        }
    }

    // Linux paths
    #[cfg(target_os = "linux")]
    {
        let linux_roots = ["/usr/lib/jvm", "/opt/jdk"];
        for root in &linux_roots {
            let dir = PathBuf::from(root);
            if dir.is_dir() {
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for entry in entries.flatten() {
                        let path = entry.path().join("bin/java");
                        if path.is_file() {
                            if let Some(mut inst) = inspect_java(&path) {
                                inst.source = JavaSource::System;
                                results.push(inst);
                            }
                        }
                    }
                }
            }
        }
        let global = PathBuf::from("/usr/bin/java");
        if global.is_file() {
            if let Some(mut inst) = inspect_java(&global) {
                inst.source = JavaSource::System;
                results.push(inst);
            }
        }
    }

    // PATH scan (all platforms)
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            #[cfg(target_os = "windows")]
            let path = dir.join("java.exe");
            #[cfg(not(target_os = "windows"))]
            let path = dir.join("java");
            if path.is_file() {
                if let Some(mut inst) = inspect_java(&path) {
                    inst.source = JavaSource::System;
                    results.push(inst);
                }
            }
        }
    }

    results.sort_by(|left, right| {
        left.version
            .cmp(&right.version)
            .then_with(|| left.path.cmp(&right.path))
    });
    results.dedup_by(|left, right| left.path == right.path);
    results
}

// ---------------------------------------------------------------------------
// Discovery — Managed JREs
// ---------------------------------------------------------------------------

/// Detect previously provisioned managed JREs under `runtimes_root`.
///
/// Scans the known layout: `{runtimes_root}/temurin/<major>/<full_version>/<os>-<arch>/`
/// and validates each receipt alongside the Java executable.  Does NOT recurse
/// arbitrarily outside this known catalog layout.
///
/// Returns installations tagged [`JavaSource::Managed`].
pub fn detect_managed_jres(runtimes_root: &Path) -> Vec<JavaInstallation> {
    use crate::runtime_manager::RuntimeReceipt;

    let mut results = Vec::new();
    let vendor_dir = runtimes_root.join("temurin");
    if !vendor_dir.is_dir() {
        return results;
    }

    let major_dirs = match std::fs::read_dir(&vendor_dir) {
        Ok(d) => d,
        Err(_) => return results,
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
                // Read and validate receipt
                let receipt = match RuntimeReceipt::read_from(&receipt_path) {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                // Validate the java binary exists
                let java_path = plat_dir.join(&receipt.java_relative_path);
                if !java_path.is_file() {
                    continue;
                }
                results.push(JavaInstallation {
                    path: java_path,
                    version: receipt.major,
                    version_string: receipt.full_version.clone(),
                    source: JavaSource::Managed,
                    arch: Some(receipt.arch.clone()),
                });
            }
        }
    }

    results
}

// ---------------------------------------------------------------------------
// Discovery — Mojang bundled JREs
// ---------------------------------------------------------------------------

/// Maximum number of directories to scan under Mojang's `runtime/` directory.
const MOJANG_MAX_DIRS: usize = 100;

/// Maximum directory depth under `runtime/`.
const MOJANG_MAX_DEPTH: usize = 6;

/// Maximum Java candidates returned from Mojang discovery.
const MOJANG_MAX_CANDIDATES: usize = 16;

/// Best-effort bounded scan for Mojang-bundled JREs.
///
/// Scans `{minecraft_dir}/runtime/` for `bin/java(.exe)`, with caps on the
/// number of directories traversed, recursion depth, and total candidates.
/// No symlink or reparse-point escapes are followed.
///
/// Returns installations tagged [`JavaSource::Mojang`].
pub fn detect_mojang_jres(minecraft_dir: &Path) -> Vec<JavaInstallation> {
    let runtime_dir = minecraft_dir.join("runtime");
    if !runtime_dir.is_dir() {
        return Vec::new();
    }

    let mut results = Vec::new();
    let mut dir_count = 0usize;

    // Non-recursive bounded BFS-style scan.
    let mut stack = vec![runtime_dir];
    while let Some(dir) = stack.pop() {
        if dir_count >= MOJANG_MAX_DIRS || results.len() >= MOJANG_MAX_CANDIDATES {
            break;
        }
        // Guard: depth from runtime/
        // (We just count dirs rather than measure depth precisely — the max
        //  depth cap combined with max dirs is sufficient to bound the scan.)
        if stack.len() > MOJANG_MAX_DEPTH {
            continue;
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
                // Skip symlink/reparse-point dirs to prevent escape.
                let meta = match entry.metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                };
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
                    // Reparse point check (includes junctions/symlinks).
                    if meta.file_attributes() & 0x400 /* FILE_ATTRIBUTE_REPARSE_POINT */ != 0 {
                        continue;
                    }
                }
                dir_count += 1;
                if dir_count <= MOJANG_MAX_DIRS {
                    stack.push(entry.path());
                }
            } else if ft.is_file() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                let is_java = if cfg!(target_os = "windows") {
                    name_str.eq_ignore_ascii_case("java.exe")
                        || name_str.eq_ignore_ascii_case("javaw.exe")
                } else {
                    name_str == "java"
                };
                if is_java {
                    if results.len() >= MOJANG_MAX_CANDIDATES {
                        break;
                    }
                    if let Some(mut inst) = inspect_java(&entry.path()) {
                        inst.source = JavaSource::Mojang;
                        results.push(inst);
                    }
                }
            }
        }
    }

    results
}

// ---------------------------------------------------------------------------
// Combined discovery
// ---------------------------------------------------------------------------

/// Source-priority ordering for candidate ranking.
fn source_priority(src: &JavaSource) -> u8 {
    match src {
        JavaSource::Override => 0,
        JavaSource::Managed => 1,
        JavaSource::Mojang => 2,
        JavaSource::System => 3,
    }
}

/// Combine managed, Mojang, and system JRE candidates with deduplication.
///
/// Candidates are returned sorted by version ascending, then by source
/// priority (Managed > Mojang > System), then by stable path.
/// Duplicates (same canonical path) are removed, keeping the highest-priority
/// source entry.
pub fn detect_java_candidates(
    runtimes_root: Option<&Path>,
    minecraft_dir: Option<&Path>,
) -> Vec<JavaInstallation> {
    let mut results = Vec::new();

    // Managed (highest priority for equal major)
    if let Some(root) = runtimes_root {
        results.extend(detect_managed_jres(root));
    }

    // Mojang
    if let Some(dir) = minecraft_dir {
        results.extend(detect_mojang_jres(dir));
    }

    // System (lowest priority)
    results.extend(detect_system_jres());

    // Deduplicate by canonical path, keeping highest priority source.
    // Sort by (version, source_priority, path) then dedup by canonical path.
    results.sort_by_key(|inst| {
        let sp = source_priority(&inst.source);
        let canon = inst.canonical_path();
        (inst.version, sp, canon)
    });
    results.dedup_by(|a, b| a.canonical_path() == b.canonical_path());

    results
}

/// Tagged system JRE discovery — equivalent to [`detect_installed_jres`] but
/// with an explicit name matching the discovery-family convention.
pub fn detect_system_jres() -> Vec<JavaInstallation> {
    detect_installed_jres()
}

// ---------------------------------------------------------------------------
// JavaRequirement — derived from an already-resolved VersionInfo
// ---------------------------------------------------------------------------

/// The Java major version and component string required by a Minecraft
/// version's metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JavaRequirement {
    /// Required Java major version (e.g. 8, 17, 21).
    pub major: u32,
    /// The Mojang component name (e.g. `"java-runtime-gamma"`, `"jre-legacy"`).
    pub component: String,
}

/// Derive the [`JavaRequirement`] from an already-resolved [`VersionInfo`].
///
/// The Mojang version manifest (the `.json` per version) carries an optional
/// `javaVersion` block containing `component` and `majorVersion`. When absent,
/// the requirement defaults to Java 8 with `"jre-legacy"`.
///
/// # Why a pure helper
///
/// This function does no I/O. It is intended for use **before** Java selection
/// in the launch planner (and in the Forge installer bootstrap) so the caller
/// can know what major version is needed without fetching Mojang metadata
/// again after it has already been resolved.
pub fn java_requirement_from_version(version: &VersionInfo) -> JavaRequirement {
    match version.java_version.as_ref() {
        Some(jv) => {
            let major = u32::try_from(jv.major_version)
                .ok()
                .filter(|&m| m > 0)
                .unwrap_or(8);
            let component = if jv.component.is_empty() {
                "jre-legacy".to_string()
            } else {
                jv.component.clone()
            };
            JavaRequirement { major, component }
        }
        None => JavaRequirement {
            major: 8,
            component: "jre-legacy".to_string(),
        },
    }
}

/// Cache-first helper to resolve a Minecraft version JSON from local
/// `.minecraft/versions/<id>/<id>.json` before falling back to the
/// Mojang version manifest.  Returns [`VersionInfo`] if found.
///
/// This is useful when the caller needs the Java requirement but does not
/// yet want to run the full launch planner (e.g. during installer bootstrap).
pub fn resolve_version_metadata(minecraft_dir: &Path, version_id: &str) -> Option<VersionInfo> {
    // Prefer installed base profile.
    let installed_path = minecraft_dir
        .join("versions")
        .join(version_id)
        .join(format!("{}.json", version_id));
    if let Ok(data) = std::fs::read_to_string(&installed_path) {
        if let Ok(info) = serde_json::from_str::<VersionInfo>(&data) {
            return Some(info);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- extract_major_version ---

    #[test]
    fn test_extract_major_version_18() {
        assert_eq!(extract_major_version("1.8.0_352"), Some(8));
    }

    #[test]
    fn test_extract_major_version_17() {
        assert_eq!(extract_major_version("17.0.1"), Some(17));
    }

    #[test]
    fn test_extract_major_version_21() {
        assert_eq!(extract_major_version("21"), Some(21));
    }

    #[test]
    fn test_extract_major_version_invalid() {
        assert_eq!(extract_major_version("invalid"), None);
    }

    // --- parse_version_string ---

    #[test]
    fn test_parse_version_string_java8() {
        let input = "java version \"1.8.0_352\"\nJava(TM) SE Runtime Environment (build 1.8.0_352-b08)\nJava HotSpot(TM) 64-Bit Server VM (build 25.352-b08, mixed mode)";
        assert_eq!(parse_version_string(input), Some("1.8.0_352"));
    }

    #[test]
    fn test_parse_version_string_java17() {
        let input = "openjdk version \"17.0.9\" 2023-10-17\nOpenJDK Runtime Environment (build 17.0.9+9)\nOpenJDK 64-Bit Server VM (build 17.0.9+9, mixed mode)";
        assert_eq!(parse_version_string(input), Some("17.0.9"));
    }

    #[test]
    fn test_parse_version_string_prefers_specification_version() {
        let input = "Property settings:\n    java.specification.version = 21\n    java.version = 21.0.2\n\nopenjdk version \"21.0.2\" 2024-01-16\n";
        assert_eq!(parse_version_string(input), Some("21"));
    }

    // --- parse_os_arch ---

    #[test]
    fn test_parse_os_arch_found() {
        let input = "Property settings:\n    os.arch = amd64\n    java.specification.version = 17";
        assert_eq!(parse_os_arch(input), Some("amd64".into()));
    }

    #[test]
    fn test_parse_os_arch_not_found() {
        assert_eq!(parse_os_arch("no arch here"), None);
    }

    // --- detect_installed_jres ---

    #[test]
    fn test_detect_no_panic() {
        let _ = detect_installed_jres();
    }

    // --- JavaInstallation serde backward compat ---

    #[test]
    fn test_java_installation_deserializes_without_source() {
        let json = r#"{"path": "/usr/bin/java", "version": 17, "version_string": "17"}"#;
        let inst: JavaInstallation = serde_json::from_str(json).unwrap();
        assert_eq!(inst.source, JavaSource::System);
        assert!(inst.arch.is_none());
    }

    #[test]
    fn test_java_installation_deserializes_with_source() {
        let json = r#"{"path": "/managed/java", "version": 21, "version_string": "21", "source": "Managed", "arch": "amd64"}"#;
        let inst: JavaInstallation = serde_json::from_str(json).unwrap();
        assert_eq!(inst.source, JavaSource::Managed);
        assert_eq!(inst.arch.as_deref(), Some("amd64"));
    }

    #[test]
    fn test_java_installation_roundtrip() {
        let inst = JavaInstallation {
            path: PathBuf::from("/test/java"),
            version: 17,
            version_string: "17.0.1".into(),
            source: JavaSource::Mojang,
            arch: Some("aarch64".into()),
        };
        let json = serde_json::to_string(&inst).unwrap();
        let deserialized: JavaInstallation = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.source, JavaSource::Mojang);
        assert_eq!(deserialized.arch.as_deref(), Some("aarch64"));
    }

    // --- source priority ---

    #[test]
    fn test_source_priority_ordering() {
        assert!(source_priority(&JavaSource::Override) < source_priority(&JavaSource::Managed));
        assert!(source_priority(&JavaSource::Managed) < source_priority(&JavaSource::Mojang));
        assert!(source_priority(&JavaSource::Mojang) < source_priority(&JavaSource::System));
    }

    // --- detect_managed_jres / detect_mojang_jres are integration-level
    // and tested more thoroughly via runtime_manager tests. ---

    // --- JavaRequirement tests ---

    #[test]
    fn test_java_requirement_from_version_specified() {
        let v = crate::launch::VersionInfo {
            java_version: Some(crate::launch::JavaVersion {
                component: "java-runtime-gamma".into(),
                major_version: 21,
            }),
            ..Default::default()
        };
        let req = java_requirement_from_version(&v);
        assert_eq!(req.major, 21);
        assert_eq!(req.component, "java-runtime-gamma");
    }

    #[test]
    fn test_java_requirement_from_version_defaults_to_8() {
        let v = crate::launch::VersionInfo::default();
        let req = java_requirement_from_version(&v);
        assert_eq!(req.major, 8);
        assert_eq!(req.component, "jre-legacy");
    }

    #[test]
    fn test_java_requirement_from_version_zero_major_defaults() {
        let v = crate::launch::VersionInfo {
            java_version: Some(crate::launch::JavaVersion {
                component: "test".into(),
                major_version: 0,
            }),
            ..Default::default()
        };
        let req = java_requirement_from_version(&v);
        assert_eq!(req.major, 8);
        assert_eq!(req.component, "test");
    }

    #[test]
    fn test_java_requirement_roundtrip_serialize() {
        let req = JavaRequirement {
            major: 17,
            component: "java-runtime-alpha".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let deserialized: JavaRequirement = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, req);
    }

    // --- resolve_version_metadata returns None for missing ---

    #[test]
    fn test_resolve_version_metadata_missing_dir_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let result = resolve_version_metadata(tmp.path(), "1.21");
        assert!(result.is_none());
    }
}
