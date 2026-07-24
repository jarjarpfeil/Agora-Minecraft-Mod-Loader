use crate::error::{LauncherError, LauncherResult};
use crate::event_sink::{OperationId, ProgressEvent, ProgressPhase, ProgressSink};
use crate::models::{InstalledMod, InstanceManifest};
use crate::paths;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;
use zip::ZipArchive;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    pub instance_id: String,
    pub name: String,
    pub minecraft_version: String,
    pub loader: String,
    pub loader_version: String,
    pub imported_mods: usize,
    pub linked_saves: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DetectedLauncher {
    pub launcher_type: String,
    pub instances_dir: PathBuf,
    pub instance_count: usize,
}

#[derive(Deserialize)]
struct MrpackIndex {
    #[serde(default)]
    name: String,
    dependencies: Option<serde_json::Value>,
    files: Vec<MrpackFile>,
    #[serde(default)]
    overrides: String,
}

#[derive(Deserialize)]
struct MrpackFile {
    path: String,
    #[serde(default)]
    downloads: Vec<String>,
    #[serde(default)]
    hashes: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
struct ImportedModrinthFile {
    project_id: String,
    download_url: String,
}

#[derive(Deserialize)]
struct PrismPackJson {
    #[serde(default)]
    components: Vec<PrismComponent>,
}

#[derive(Deserialize)]
struct PrismComponent {
    uid: String,
    version: Option<String>,
}

fn sanitize(name: &str) -> String {
    paths::sanitize_id(name)
}

const MAX_MRPACK_FILE_BYTES: usize = 500 * 1024 * 1024;

const MRPACK_DOWNLOAD_ALLOWLIST: &[&str] = &[
    "cdn.modrinth.com",
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

fn validate_mrpack_download_url(raw_url: &str) -> LauncherResult<reqwest::Url> {
    let url = reqwest::Url::parse(raw_url).map_err(|_| LauncherError::UntrustedSource)?;
    let host = url.host_str().ok_or(LauncherError::UntrustedSource)?;
    if url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !MRPACK_DOWNLOAD_ALLOWLIST.contains(&host)
    {
        return Err(LauncherError::UntrustedSource);
    }
    Ok(url)
}

fn verify_mrpack_file_hash(file: &MrpackFile, bytes: &[u8]) -> LauncherResult<()> {
    let expected = file.hashes.get("sha1").ok_or_else(|| {
        import_error(
            "ERR_IMPORT_HASH_MISSING",
            format!("Pack entry '{}' has no SHA-1 integrity hash.", file.path),
        )
    })?;
    let actual = crate::download::sha1_hex(bytes);
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(import_error(
            "ERR_HASH_MISMATCH",
            format!(
                "Pack entry '{}' failed integrity verification: expected {} got {}.",
                file.path, expected, actual
            ),
        ));
    }
    Ok(())
}

/// Extract a Modrinth project ID only from the canonical Modrinth CDN path.
/// Pack entries may also point at GitHub or contain no download URL, so those
/// entries remain pack-managed without claiming a Modrinth project identity.
fn modrinth_project_id_from_download_url(raw_url: &str) -> Option<String> {
    let url = reqwest::Url::parse(raw_url).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("cdn.modrinth.com")
        || url.port_or_known_default() != Some(443)
    {
        return None;
    }
    let segments: Vec<_> = url.path_segments()?.collect();
    if segments.len() < 5 || segments[0] != "data" || segments[2] != "versions" {
        return None;
    }
    let project_id = segments[1];
    if project_id.is_empty()
        || !project_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return None;
    }
    Some(project_id.to_string())
}

fn modrinth_file_identity(file: &MrpackFile) -> Option<ImportedModrinthFile> {
    file.downloads.iter().find_map(|download_url| {
        modrinth_project_id_from_download_url(download_url).map(|project_id| ImportedModrinthFile {
            project_id,
            download_url: download_url.clone(),
        })
    })
}

fn manifest_path_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn inventory_pack_content(
    instance_dir: &Path,
    directory: &str,
    content_type: &str,
    modrinth_files: &BTreeMap<String, ImportedModrinthFile>,
) -> LauncherResult<Vec<InstalledMod>> {
    let root = instance_dir.join(directory);
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut installed = Vec::new();
    for entry in fs::read_dir(&root).map_err(|e| {
        import_error(
            "ERR_IMPORT_READ",
            format!("Cannot inventory imported {directory}: {e}"),
        )
    })? {
        let entry = entry.map_err(|e| import_error("ERR_IMPORT_READ", e.to_string()))?;
        if !entry
            .file_type()
            .map_err(|e| import_error("ERR_IMPORT_READ", e.to_string()))?
            .is_file()
        {
            continue;
        }
        let bytes = fs::read(entry.path()).map_err(|e| {
            import_error(
                "ERR_IMPORT_READ",
                format!(
                    "Cannot hash imported file '{}': {e}",
                    entry.path().display()
                ),
            )
        })?;
        let identity_key = format!("{directory}/{}", entry.file_name().to_string_lossy());
        let modrinth_file = if content_type == "mod" {
            modrinth_files.get(&identity_key)
        } else {
            None
        };
        installed.push(InstalledMod {
            filename: entry.file_name().to_string_lossy().into_owned(),
            registry_id: None,
            modrinth_id: modrinth_file.map(|file| file.project_id.clone()),
            source: "modrinth-pack".into(),
            source_url: modrinth_file.map(|file| file.download_url.clone()),
            version: None,
            sha256: crate::download::sha256_hex(&bytes),
            installed_at: chrono::Utc::now().to_rfc3339(),
            java_packages: Vec::new(),
            mod_jar_id: None,
            provided_mod_ids: Vec::new(),
            enabled: true,
            content_type: content_type.into(),
            depends_on: Vec::new(),
            optional_deps: Vec::new(),
            incompatible_deps: Vec::new(),
        });
    }
    installed.sort_by(|a, b| a.filename.cmp(&b.filename));
    Ok(installed)
}

/// A freshly allocated destination for an import.  Every importer writes into
/// `staging_dir` first and only exposes the instance after a successful rename.
struct ImportTarget {
    instance_id: String,
    final_dir: PathBuf,
    staging_dir: PathBuf,
}

fn import_error(code: &str, message: impl Into<String>) -> LauncherError {
    LauncherError::Generic {
        code: code.into(),
        message: message.into(),
    }
}

fn is_reserved_windows_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && matches!(upper.as_bytes()[3], b'1'..=b'9'))
}

fn instance_id_for_import(name: &str) -> LauncherResult<String> {
    let instance_id = sanitize(name);
    if instance_id.is_empty()
        || instance_id == "."
        || instance_id == ".."
        || is_reserved_windows_name(&instance_id)
    {
        return Err(import_error(
            "ERR_INVALID_INSTANCE_ID",
            "The imported instance name cannot be used as a safe folder name. Rename the source instance and try again.",
        ));
    }
    Ok(instance_id)
}

/// Allocate an isolated, same-volume staging directory below the instances
/// root.  Existing instances are never overwritten by an import.
fn prepare_import_target(instances_root: &Path, name: &str) -> LauncherResult<ImportTarget> {
    fs::create_dir_all(instances_root).map_err(|e| {
        import_error(
            "ERR_IMPORT_MKDIR",
            format!("Cannot create the instances folder: {e}"),
        )
    })?;

    let instance_id = instance_id_for_import(name)?;
    let final_dir = instances_root.join(&instance_id);
    if final_dir.exists() {
        return Err(import_error(
            "ERR_INSTANCE_EXISTS",
            format!(
                "An instance named '{instance_id}' already exists. Choose a different name before importing so no existing saves or settings are overwritten."
            ),
        ));
    }

    let staging_dir =
        instances_root.join(format!(".agora-import-{instance_id}-{}", Uuid::new_v4()));
    fs::create_dir(&staging_dir).map_err(|e| {
        import_error(
            "ERR_IMPORT_MKDIR",
            format!("Cannot create a safe staging folder for the import: {e}"),
        )
    })?;

    Ok(ImportTarget {
        instance_id,
        final_dir,
        staging_dir,
    })
}

fn cleanup_staging(target: &ImportTarget) {
    let _ = fs::remove_dir_all(&target.staging_dir);
}

/// Publish a completed staged import.  `rename` is atomic because both paths
/// are direct children of the same instances root.
fn finalize_import(target: &ImportTarget) -> LauncherResult<()> {
    if target.final_dir.exists() {
        return Err(import_error(
            "ERR_INSTANCE_EXISTS",
            format!(
                "An instance named '{}' was created while this import was running. No existing instance was changed.",
                target.instance_id
            ),
        ));
    }
    fs::rename(&target.staging_dir, &target.final_dir).map_err(|e| {
        import_error(
            "ERR_IMPORT_FINALIZE",
            format!(
                "Could not finalize the imported instance. Your existing instances were not changed: {e}"
            ),
        )
    })
}

/// Normalize archive-provided separators before applying the path safety
/// check. This makes Windows-style paths safe on every supported OS.
fn relative_archive_path(raw: &str) -> PathBuf {
    PathBuf::from(raw.replace('\\', "/"))
}

/// Validate that a candidate path stays within the base directory.
/// Rejects absolute paths, parent traversal, and existing symlink escapes.
fn assert_safe_path(base: &Path, candidate: &Path) -> LauncherResult<()> {
    let canonical_base = fs::canonicalize(base).map_err(|e| {
        import_error(
            "ERR_IMPORT_PATH",
            format!("Cannot resolve import destination: {e}"),
        )
    })?;
    let mut probe = base.to_path_buf();
    let mut has_normal_component = false;

    for component in candidate.components() {
        match component {
            Component::Normal(part) => {
                has_normal_component = true;
                probe.push(part);
                // A pre-existing symlink can redirect a future write outside
                // the staging directory even when the final path does not yet
                // exist. Check every existing ancestor, not only the leaf.
                if probe.exists() {
                    let canonical_probe = fs::canonicalize(&probe).map_err(|e| {
                        import_error(
                            "ERR_IMPORT_PATH",
                            format!("Cannot resolve import path '{}': {e}", candidate.display()),
                        )
                    })?;
                    if !canonical_probe.starts_with(&canonical_base) {
                        return Err(import_error(
                            "ERR_PATH_TRAVERSAL",
                            format!("Path traversal detected: {}", candidate.display()),
                        ));
                    }
                }
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(import_error(
                    "ERR_PATH_TRAVERSAL",
                    format!("Path traversal detected: {}", candidate.display()),
                ));
            }
        }
    }

    if !has_normal_component {
        return Err(import_error(
            "ERR_PATH_TRAVERSAL",
            "Archive entry does not name a file inside the instance.",
        ));
    }
    Ok(())
}

fn copy_or_symlink(src: &Path, dst: &Path, symlink: bool) -> LauncherResult<()> {
    let io_err = |e: io::Error| LauncherError::Generic {
        code: "ERR_COPY".into(),
        message: format!("{}", e),
    };
    if symlink {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(src, dst).map_err(io_err)?;
            return Ok(());
        }
        #[cfg(windows)]
        {
            if src.is_dir() {
                std::os::windows::fs::symlink_dir(src, dst).map_err(io_err)?;
            } else {
                std::os::windows::fs::symlink_file(src, dst).map_err(io_err)?;
            }
            return Ok(());
        }
        #[cfg(not(any(unix, windows)))]
        {}
    }
    if src.is_dir() {
        fs::create_dir_all(dst).map_err(io_err)?;
        for entry in fs::read_dir(src).map_err(io_err)? {
            let entry = entry.map_err(io_err)?;
            let file_type = entry.file_type().map_err(io_err)?;
            if file_type.is_symlink() {
                continue;
            }
            let child_src = entry.path();
            let child_dst = dst.join(entry.file_name());
            copy_or_symlink(&child_src, &child_dst, false)?;
        }
    } else {
        fs::copy(src, dst).map_err(io_err)?;
    }
    Ok(())
}

/// Copy an instance directory while leaving one top-level directory for a
/// separately-created save symlink. Source symlinks are never traversed.
fn copy_directory_excluding_top_level(
    src: &Path,
    dst: &Path,
    excluded_name: &str,
) -> LauncherResult<()> {
    let io_err = |e: io::Error| LauncherError::Generic {
        code: "ERR_COPY".into(),
        message: e.to_string(),
    };
    fs::create_dir_all(dst).map_err(io_err)?;
    for entry in fs::read_dir(src).map_err(io_err)? {
        let entry = entry.map_err(io_err)?;
        if entry.file_name().to_str() == Some(excluded_name) {
            continue;
        }
        if entry.file_type().map_err(io_err)?.is_symlink() {
            continue;
        }
        copy_or_symlink(&entry.path(), &dst.join(entry.file_name()), false)?;
    }
    Ok(())
}

/// Import an instance from a .mrpack file (Modrinth modpack format).
pub fn import_mrpack(
    mrpack_path: &Path,
    instances_root: &Path,
    _symlink_saves: bool,
) -> LauncherResult<ImportResult> {
    import_mrpack_with_progress(mrpack_path, instances_root, _symlink_saves, None, None)
}

pub fn import_mrpack_with_progress(
    mrpack_path: &Path,
    instances_root: &Path,
    _symlink_saves: bool,
    progress_sink: Option<std::sync::Arc<dyn ProgressSink>>,
    operation_id: Option<OperationId>,
) -> LauncherResult<ImportResult> {
    let file = fs::File::open(mrpack_path).map_err(|e| LauncherError::Generic {
        code: "ERR_IMPORT_OPEN".into(),
        message: format!("Cannot open mrpack: {e}"),
    })?;
    let mut archive = ZipArchive::new(file).map_err(|e| LauncherError::Generic {
        code: "ERR_IMPORT_ZIP".into(),
        message: format!("Invalid zip: {e}"),
    })?;

    let index_entry =
        archive
            .by_name("modrinth.index.json")
            .map_err(|_| LauncherError::Generic {
                code: "ERR_IMPORT_MISSING_INDEX".into(),
                message: "Missing modrinth.index.json".into(),
            })?;
    let index: MrpackIndex =
        serde_json::from_reader(index_entry).map_err(|e| LauncherError::Generic {
            code: "ERR_IMPORT_PARSE_INDEX".into(),
            message: format!("Invalid modrinth.index.json: {e}"),
        })?;

    let name = if index.name.is_empty() {
        mrpack_path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
    } else {
        index.name.clone()
    };

    let (minecraft_version, loader, loader_version) = parse_mrpack_deps(&index.dependencies);
    let target = prepare_import_target(instances_root, &name)?;

    let import_result = (|| -> LauncherResult<usize> {
        let target_dir = &target.staging_dir;
        let total_files = index.files.len() as u32;
        let mut completed_files = 0u32;
        let modrinth_files: BTreeMap<_, _> = index
            .files
            .iter()
            .filter_map(|file| {
                modrinth_file_identity(file).map(|identity| {
                    (
                        manifest_path_key(&relative_archive_path(&file.path)),
                        identity,
                    )
                })
            })
            .collect();
        for file_entry in &index.files {
            if let (Some(sink), Some(operation_id)) = (&progress_sink, &operation_id) {
                sink.report(
                    ProgressEvent::new(
                        operation_id.clone(),
                        ProgressPhase::Downloading,
                        format!("Loading {}", file_entry.path),
                    )
                    .with_plan(operation_id.0.clone(), completed_files, total_files)
                    .with_progress(if total_files == 0 {
                        0.0
                    } else {
                        completed_files as f64 / total_files as f64
                    }),
                );
            }
            let relative_path = relative_archive_path(&file_entry.path);
            assert_safe_path(target_dir, &relative_path)?;
            let dest = target_dir.join(&relative_path);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|e| LauncherError::Generic {
                    code: "ERR_IMPORT_MKDIR".into(),
                    message: format!("Cannot create dir {parent:?}: {e}"),
                })?;
            }
            let mut read_embedded = || -> LauncherResult<Vec<u8>> {
                let idx_path = file_entry.path.replace('\\', "/");
                let mut entry = archive.by_name(&idx_path).map_err(|_| {
                    import_error(
                        "ERR_IMPORT_MISSING_FILE",
                        format!(
                            "Pack entry '{}' could not be downloaded or read locally.",
                            file_entry.path
                        ),
                    )
                })?;
                if entry.size() > MAX_MRPACK_FILE_BYTES as u64 {
                    return Err(import_error(
                        "ERR_IMPORT_FILE_TOO_LARGE",
                        format!(
                            "Pack entry '{}' exceeds the 500MB safety limit.",
                            file_entry.path
                        ),
                    ));
                }
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).map_err(|e| {
                    import_error(
                        "ERR_IMPORT_READ",
                        format!("Cannot read embedded pack entry '{}': {e}", file_entry.path),
                    )
                })?;
                Ok(bytes)
            };

            let bytes = if let Some(url) = file_entry.downloads.first() {
                download_bytes(url)
                    .or_else(|download_error| read_embedded().map_err(|_| download_error))?
            } else {
                read_embedded()?
            };
            verify_mrpack_file_hash(file_entry, &bytes)?;
            fs::write(&dest, &bytes).map_err(|e| LauncherError::Generic {
                code: "ERR_IMPORT_WRITE".into(),
                message: format!("Cannot write {:?}: {e}", dest),
            })?;
            completed_files += 1;
            if let (Some(sink), Some(operation_id)) = (&progress_sink, &operation_id) {
                sink.report(
                    ProgressEvent::new(
                        operation_id.clone(),
                        ProgressPhase::Downloading,
                        format!("Verified {}", file_entry.path),
                    )
                    .with_plan(operation_id.0.clone(), completed_files, total_files)
                    .with_progress(if total_files == 0 {
                        1.0
                    } else {
                        completed_files as f64 / total_files as f64
                    }),
                );
            }
        }

        if !index.overrides.is_empty() {
            let override_prefix = index.overrides.trim_end_matches('/').to_string();
            for i in 0..archive.len() {
                let mut entry = match archive.by_index(i) {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let entry_name = entry.name().replace('\\', "/");
                if entry_name == override_prefix
                    || entry_name.starts_with(&format!("{override_prefix}/"))
                {
                    let relative = entry_name
                        .strip_prefix(&format!("{override_prefix}/"))
                        .unwrap_or(&entry_name);
                    if relative.is_empty() {
                        continue;
                    }
                    let relative_path = relative_archive_path(relative);
                    assert_safe_path(target_dir, &relative_path)?;
                    let dest = target_dir.join(&relative_path);
                    if entry.is_dir() {
                        fs::create_dir_all(&dest).map_err(|e| LauncherError::Generic {
                            code: "ERR_IMPORT_MKDIR".into(),
                            message: format!("Cannot create dir {dest:?}: {e}"),
                        })?;
                    } else {
                        if let Some(parent) = dest.parent() {
                            fs::create_dir_all(parent).map_err(|e| LauncherError::Generic {
                                code: "ERR_IMPORT_MKDIR".into(),
                                message: format!("Cannot create dir {parent:?}: {e}"),
                            })?;
                        }
                        let mut buf = Vec::new();
                        entry
                            .read_to_end(&mut buf)
                            .map_err(|e| LauncherError::Generic {
                                code: "ERR_IMPORT_READ".into(),
                                message: format!("Cannot read {entry_name}: {e}"),
                            })?;
                        fs::write(&dest, &buf).map_err(|e| LauncherError::Generic {
                            code: "ERR_IMPORT_WRITE".into(),
                            message: format!("Cannot write {:?}: {e}", dest),
                        })?;
                    }
                }
            }
        }

        let mods = inventory_pack_content(target_dir, "mods", "mod", &modrinth_files)?;
        let resourcepacks =
            inventory_pack_content(target_dir, "resourcepacks", "resourcepack", &modrinth_files)?;
        let shaders = inventory_pack_content(target_dir, "shaderpacks", "shader", &modrinth_files)?;
        let datapacks =
            inventory_pack_content(target_dir, "datapacks", "datapack", &modrinth_files)?;
        let worlds = inventory_pack_content(target_dir, "saves", "world", &modrinth_files)?;
        let imported_mods = mods.len();
        let manifest = InstanceManifest {
            instance_id: target.instance_id.clone(),
            name: name.clone(),
            minecraft_version: minecraft_version.clone(),
            loader: loader.clone(),
            loader_version: loader_version.clone(),
            is_locked: false,
            created_from_pack: Some(name.clone()),
            mods,
            resourcepacks,
            shaders,
            datapacks,
            worlds,
            user_preferences: serde_json::json!({}),
        };
        let manifest_path = target_dir.join("instance_manifest.json");
        let manifest_json =
            serde_json::to_string_pretty(&manifest).map_err(|e| LauncherError::Generic {
                code: "ERR_IMPORT_SERIALIZE".into(),
                message: format!("Cannot serialize manifest: {e}"),
            })?;
        fs::write(&manifest_path, manifest_json).map_err(|e| LauncherError::Generic {
            code: "ERR_IMPORT_WRITE".into(),
            message: format!("Cannot write manifest: {e}"),
        })?;
        Ok(imported_mods)
    })();

    let imported_mods = match import_result {
        Ok(count) => count,
        Err(error) => {
            cleanup_staging(&target);
            return Err(error);
        }
    };
    if let Err(error) = finalize_import(&target) {
        cleanup_staging(&target);
        return Err(error);
    }

    Ok(ImportResult {
        instance_id: target.instance_id,
        name,
        minecraft_version,
        loader,
        loader_version,
        imported_mods,
        linked_saves: false,
    })
}

fn parse_mrpack_deps(deps: &Option<serde_json::Value>) -> (String, String, String) {
    let default = (String::new(), String::new(), String::new());
    let deps_map = match deps {
        Some(serde_json::Value::Object(m)) => m,
        _ => return default,
    };
    let mc = deps_map
        .get("minecraft")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let loader;
    let loader_version;
    if let Some(v) = deps_map.get("fabric-loader").and_then(|v| v.as_str()) {
        loader = "fabric".to_string();
        loader_version = v.to_string();
    } else if let Some(v) = deps_map.get("quilt-loader").and_then(|v| v.as_str()) {
        loader = "quilt".to_string();
        loader_version = v.to_string();
    } else if let Some(v) = deps_map.get("forge").and_then(|v| v.as_str()) {
        loader = "forge".to_string();
        loader_version = v.to_string();
    } else if let Some(v) = deps_map.get("neoforge").and_then(|v| v.as_str()) {
        loader = "neoforge".to_string();
        loader_version = v.to_string();
    } else {
        loader = String::new();
        loader_version = String::new();
    }
    (mc, loader, loader_version)
}

fn download_bytes(url: &str) -> LauncherResult<Vec<u8>> {
    let _validated = validate_mrpack_download_url(url)?;
    // Create a local HttpClients for the download. Uses the Modrinth/GitHub
    // category allowlists (which cover all MRPACK_DOWNLOAD_ALLOWLIST hosts)
    // with the checked API's redirect re-validation and size enforcement.
    let clients = crate::http_client::HttpClients::new().map_err(|e| LauncherError::Generic {
        code: "ERR_IMPORT_HTTP_CLIENT".into(),
        message: format!("Could not create download client: {e}"),
    })?;
    let category = if url.contains("modrinth.com") {
        crate::http_client::ClientCategory::Modrinth
    } else {
        crate::http_client::ClientCategory::GitHub
    };
    let bytes = crate::http_client::blocking_checked_get_bytes(&clients, category, url)?;
    if bytes.len() > MAX_MRPACK_FILE_BYTES {
        return Err(import_error(
            "ERR_IMPORT_FILE_TOO_LARGE",
            "Imported pack file exceeds the 500MB safety limit.",
        ));
    }
    Ok(bytes)
}

/// Import an instance from a Prism/MMC instance zip.
pub fn import_prism_zip(
    zip_path: &Path,
    instances_root: &Path,
    symlink_saves: bool,
) -> LauncherResult<ImportResult> {
    let file = fs::File::open(zip_path).map_err(|e| LauncherError::Generic {
        code: "ERR_IMPORT_OPEN_ZIP".into(),
        message: format!("Cannot open zip: {e}"),
    })?;
    let mut archive = ZipArchive::new(file).map_err(|e| LauncherError::Generic {
        code: "ERR_IMPORT_ZIP".into(),
        message: format!("Invalid zip: {e}"),
    })?;

    let mut instance_cfg = None;
    let mut pack_json = None;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| LauncherError::Generic {
            code: "ERR_IMPORT_READ_ZIP".into(),
            message: format!("Read error: {e}"),
        })?;
        let name = entry.name().replace('\\', "/");
        if name == "instance.cfg" {
            let mut buf = String::new();
            entry
                .read_to_string(&mut buf)
                .map_err(|e| LauncherError::Generic {
                    code: "ERR_IMPORT_READ_CFG".into(),
                    message: format!("Read instance.cfg: {e}"),
                })?;
            instance_cfg = Some(buf);
        } else if name == "mmc-pack.json" {
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .map_err(|e| LauncherError::Generic {
                    code: "ERR_IMPORT_READ_PACK".into(),
                    message: format!("Read mmc-pack.json: {e}"),
                })?;
            let parsed: PrismPackJson =
                serde_json::from_slice(&buf).map_err(|e| LauncherError::Generic {
                    code: "ERR_IMPORT_PARSE_PACK".into(),
                    message: format!("Parse mmc-pack.json: {e}"),
                })?;
            pack_json = Some(parsed);
        }
    }

    let cfg = instance_cfg.ok_or_else(|| LauncherError::Generic {
        code: "ERR_IMPORT_MISSING_CFG".into(),
        message: "Missing instance.cfg".into(),
    })?;

    let name = parse_prism_cfg(&cfg, "name").unwrap_or_else(|| {
        zip_path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
    });

    let (minecraft_version, loader, loader_version) = parse_prism_components(&pack_json);
    let target = prepare_import_target(instances_root, &name)?;

    let import_result = (|| -> LauncherResult<usize> {
        let target_dir = &target.staging_dir;
        let mut imported_mods = 0;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| LauncherError::Generic {
                code: "ERR_IMPORT_READ_ZIP".into(),
                message: format!("Read error: {e}"),
            })?;
            let entry_name = entry.name().replace('\\', "/");
            if entry_name == "instance.cfg"
                || entry_name == "mmc-pack.json"
                || entry_name.starts_with("minecraft/")
            {
                continue;
            }
            if entry.is_dir() {
                continue;
            }
            let relative_path = relative_archive_path(&entry_name);
            assert_safe_path(target_dir, &relative_path)?;
            let dest = target_dir.join(&relative_path);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|e| LauncherError::Generic {
                    code: "ERR_IMPORT_MKDIR".into(),
                    message: format!("Cannot create dir {parent:?}: {e}"),
                })?;
            }
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .map_err(|e| LauncherError::Generic {
                    code: "ERR_IMPORT_READ".into(),
                    message: format!("Cannot read {entry_name}: {e}"),
                })?;
            fs::write(&dest, &buf).map_err(|e| LauncherError::Generic {
                code: "ERR_IMPORT_WRITE".into(),
                message: format!("Cannot write {:?}: {e}", dest),
            })?;
            if entry_name.starts_with("mods/") {
                imported_mods += 1;
            }
        }

        let manifest = InstanceManifest {
            instance_id: target.instance_id.clone(),
            name: name.clone(),
            minecraft_version: minecraft_version.clone(),
            loader: loader.clone(),
            loader_version: loader_version.clone(),
            is_locked: false,
            created_from_pack: None,
            mods: vec![],
            resourcepacks: vec![],
            shaders: vec![],
            datapacks: vec![],
            worlds: vec![],
            user_preferences: serde_json::json!({}),
        };
        let manifest_path = target_dir.join("instance_manifest.json");
        let manifest_json =
            serde_json::to_string_pretty(&manifest).map_err(|e| LauncherError::Generic {
                code: "ERR_IMPORT_SERIALIZE".into(),
                message: format!("Cannot serialize manifest: {e}"),
            })?;
        fs::write(&manifest_path, manifest_json).map_err(|e| LauncherError::Generic {
            code: "ERR_IMPORT_WRITE".into(),
            message: format!("Cannot write manifest: {e}"),
        })?;
        Ok(imported_mods)
    })();

    let imported_mods = match import_result {
        Ok(count) => count,
        Err(error) => {
            cleanup_staging(&target);
            return Err(error);
        }
    };
    if let Err(error) = finalize_import(&target) {
        cleanup_staging(&target);
        return Err(error);
    }

    Ok(ImportResult {
        instance_id: target.instance_id,
        name,
        minecraft_version,
        loader,
        loader_version,
        imported_mods,
        linked_saves: symlink_saves,
    })
}

fn parse_prism_cfg(cfg: &str, key: &str) -> Option<String> {
    let wanted_key = key.trim().to_lowercase();
    for line in cfg.lines() {
        let line = line.trim();
        if let Some(eq) = line.find('=') {
            let k = line[..eq].trim().to_lowercase();
            if k == wanted_key {
                return Some(line[eq + 1..].trim().to_string());
            }
        }
    }
    None
}

fn parse_prism_components(pack_json: &Option<PrismPackJson>) -> (String, String, String) {
    let default = (String::new(), String::new(), String::new());
    let json = match pack_json {
        Some(j) => j,
        None => return default,
    };
    let mut mc_version = String::new();
    let mut loader = String::new();
    let mut loader_version = String::new();

    for comp in &json.components {
        match comp.uid.as_str() {
            "net.minecraft" => {
                if let Some(v) = &comp.version {
                    mc_version = v.clone();
                }
            }
            "net.fabricmc.fabric-loader" => {
                loader = "fabric".to_string();
                if let Some(v) = &comp.version {
                    loader_version = v.clone();
                }
            }
            "org.quiltmc.quilt-loader" => {
                loader = "quilt".to_string();
                if let Some(v) = &comp.version {
                    loader_version = v.clone();
                }
            }
            "net.minecraftforge" => {
                loader = "forge".to_string();
                if let Some(v) = &comp.version {
                    loader_version = v.clone();
                }
            }
            "net.neoforged" | "net.neoforged.neoforge" => {
                loader = "neoforge".to_string();
                if let Some(v) = &comp.version {
                    loader_version = v.clone();
                }
            }
            _ => {}
        }
    }
    (mc_version, loader, loader_version)
}

/// Import from a raw instance directory (Prism/Modrinth App).
pub fn import_directory(
    source_dir: &Path,
    instances_root: &Path,
    symlink_saves: bool,
) -> LauncherResult<ImportResult> {
    if !source_dir.is_dir() {
        return Err(LauncherError::Generic {
            code: "ERR_IMPORT_NOT_DIR".into(),
            message: format!("Source {:?} is not a directory", source_dir),
        });
    }

    let name = source_dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let target = prepare_import_target(instances_root, &name)?;

    // Avoid recursively copying the staging directory when a user selects the
    // Agora data folder (or the whole instances folder) as an import source.
    let canonical_source = fs::canonicalize(source_dir).map_err(|e| {
        import_error(
            "ERR_IMPORT_PATH",
            format!("Cannot resolve source instance: {e}"),
        )
    })?;
    let canonical_instances_root = fs::canonicalize(instances_root).map_err(|e| {
        import_error(
            "ERR_IMPORT_PATH",
            format!("Cannot resolve instances folder: {e}"),
        )
    })?;
    if canonical_instances_root.starts_with(&canonical_source) {
        cleanup_staging(&target);
        return Err(import_error(
            "ERR_IMPORT_RECURSIVE_SOURCE",
            "Choose a single instance folder to import, not the Agora data or instances folder.",
        ));
    }

    let saves_src = source_dir.join("saves");
    let mut linked_saves = symlink_saves && saves_src.exists();
    let import_result = (|| -> LauncherResult<()> {
        if linked_saves {
            copy_directory_excluding_top_level(source_dir, &target.staging_dir, "saves").map_err(
                |e| import_error("ERR_IMPORT_COPY", format!("Cannot copy instance: {e}")),
            )?;
            if copy_or_symlink(&saves_src, &target.staging_dir.join("saves"), true).is_err() {
                // Symlink creation often requires elevated Windows privileges.
                // Preserve the player's world by falling back to a regular copy
                // and report the actual outcome in `ImportResult`.
                copy_or_symlink(&saves_src, &target.staging_dir.join("saves"), false).map_err(
                    |e| import_error("ERR_IMPORT_COPY", format!("Cannot copy saves: {e}")),
                )?;
                linked_saves = false;
            }
        } else {
            copy_or_symlink(source_dir, &target.staging_dir, false).map_err(|e| {
                import_error("ERR_IMPORT_COPY", format!("Cannot copy instance: {e}"))
            })?;
        }

        let manifest_path = target.staging_dir.join("instance_manifest.json");
        if !manifest_path.exists() {
            let manifest = InstanceManifest {
                instance_id: target.instance_id.clone(),
                name: name.clone(),
                minecraft_version: String::new(),
                loader: String::new(),
                loader_version: String::new(),
                is_locked: false,
                created_from_pack: None,
                mods: vec![],
                resourcepacks: vec![],
                shaders: vec![],
                datapacks: vec![],
                worlds: vec![],
                user_preferences: serde_json::json!({}),
            };
            let manifest_json =
                serde_json::to_string_pretty(&manifest).map_err(|e| LauncherError::Generic {
                    code: "ERR_IMPORT_SERIALIZE".into(),
                    message: format!("Cannot serialize manifest: {e}"),
                })?;
            fs::write(&manifest_path, manifest_json).map_err(|e| LauncherError::Generic {
                code: "ERR_IMPORT_WRITE".into(),
                message: format!("Cannot write manifest: {e}"),
            })?;
        }
        Ok(())
    })();
    if let Err(error) = import_result {
        cleanup_staging(&target);
        return Err(error);
    }
    if let Err(error) = finalize_import(&target) {
        cleanup_staging(&target);
        return Err(error);
    }

    let mc_version = String::new();
    let loader = String::new();
    let loader_version = String::new();

    let mod_count = if target.final_dir.join("mods").exists() {
        fs::read_dir(target.final_dir.join("mods"))
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
                    .count()
            })
            .unwrap_or(0)
    } else {
        0
    };

    Ok(ImportResult {
        instance_id: target.instance_id,
        name,
        minecraft_version: mc_version,
        loader,
        loader_version,
        imported_mods: mod_count,
        linked_saves,
    })
}

/// Auto-detect installed launchers and their instance directories.
pub fn auto_detect_launchers() -> Vec<DetectedLauncher> {
    let mut result = Vec::new();

    let candidates: Vec<(&str, Vec<PathBuf>)> = {
        let mut v = Vec::new();

        #[cfg(target_os = "windows")]
        {
            if let Some(appdata) = dirs::data_dir() {
                v.push((
                    "prism",
                    vec![appdata.join("PrismLauncher").join("instances")],
                ));
                v.push((
                    "modrinth",
                    vec![appdata.join("com.modrinth.app").join("profiles")],
                ));
                v.push((
                    "curseforge",
                    vec![appdata
                        .join("curseforge")
                        .join("minecraft")
                        .join("Instances")],
                ));
                v.push((
                    "atlauncher",
                    vec![appdata.join("ATLauncher").join("instances")],
                ));
                v.push((
                    "gdlauncher",
                    vec![appdata.join("GDLauncher").join("instances")],
                ));
            }
        }

        #[cfg(target_os = "linux")]
        {
            if let Some(home) = dirs::home_dir() {
                let local_share = home.join(".local").join("share");
                v.push((
                    "prism",
                    vec![local_share.join("PrismLauncher").join("instances")],
                ));
                v.push((
                    "modrinth",
                    vec![local_share.join("com.modrinth.app").join("profiles")],
                ));
                v.push((
                    "curseforge",
                    vec![home.join(".curseforge").join("minecraft").join("Instances")],
                ));
                v.push((
                    "atlauncher",
                    vec![local_share.join("ATLauncher").join("instances")],
                ));
                v.push((
                    "gdlauncher",
                    vec![local_share.join("GDLauncher").join("instances")],
                ));
            }
        }

        #[cfg(target_os = "macos")]
        {
            if let Some(home) = dirs::home_dir() {
                let app_support = home.join("Library").join("Application Support");
                v.push((
                    "prism",
                    vec![app_support.join("PrismLauncher").join("instances")],
                ));
                v.push((
                    "modrinth",
                    vec![app_support.join("com.modrinth.app").join("profiles")],
                ));
                v.push((
                    "curseforge",
                    vec![app_support
                        .join("curseforge")
                        .join("minecraft")
                        .join("Instances")],
                ));
                v.push((
                    "atlauncher",
                    vec![app_support.join("ATLauncher").join("instances")],
                ));
                v.push((
                    "gdlauncher",
                    vec![app_support.join("GDLauncher").join("instances")],
                ));
            }
        }

        v
    };

    for (launcher_type, dirs) in candidates {
        for dir in dirs {
            if dir.exists() {
                let count = fs::read_dir(&dir)
                    .map(|entries| {
                        entries
                            .filter_map(|e| e.ok())
                            .filter(|e| e.path().is_dir())
                            .count()
                    })
                    .unwrap_or(0);
                if count > 0 {
                    result.push(DetectedLauncher {
                        launcher_type: launcher_type.to_string(),
                        instances_dir: dir,
                        instance_count: count,
                    });
                }
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_import_mrpack_creates_child_instance_and_preserves_root() {
        let tmp = tempfile::tempdir().unwrap();
        let mrpack_path = tmp.path().join("a-pack.mrpack");
        let file = fs::File::create(&mrpack_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file("modrinth.index.json", zip::write::FileOptions::default())
            .unwrap();
        writer
            .write_all(br#"{"name":"a-pack","dependencies":{},"files":[]}"#)
            .unwrap();
        writer.finish().unwrap();

        let instances_root = tmp.path().join("instances");
        fs::create_dir_all(instances_root.join("keep-me")).unwrap();
        fs::write(instances_root.join("keep-me").join("sentinel"), b"safe").unwrap();

        let result = import_mrpack(&mrpack_path, &instances_root, false).unwrap();

        assert_eq!(result.instance_id, "a-pack");
        let manifest_path = instances_root.join("a-pack").join("instance_manifest.json");
        assert!(manifest_path.exists());
        let manifest: InstanceManifest =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        assert!(!manifest.is_locked);
        assert_eq!(
            fs::read(instances_root.join("keep-me").join("sentinel")).unwrap(),
            b"safe"
        );
    }

    #[test]
    fn test_mrpack_download_url_rejects_untrusted_origins() {
        assert!(validate_mrpack_download_url("https://cdn.modrinth.com/data/mod.jar").is_ok());
        assert!(matches!(
            validate_mrpack_download_url("http://cdn.modrinth.com/data/mod.jar"),
            Err(LauncherError::UntrustedSource)
        ));
        assert!(matches!(
            validate_mrpack_download_url("https://127.0.0.1:39741/private"),
            Err(LauncherError::UntrustedSource)
        ));
    }

    #[test]
    fn test_mrpack_file_requires_matching_sha1() {
        let bytes = b"mod bytes";
        let mut hashes = BTreeMap::new();
        hashes.insert("sha1".to_string(), crate::download::sha1_hex(bytes));
        let file = MrpackFile {
            path: "mods/example.jar".to_string(),
            downloads: Vec::new(),
            hashes,
        };
        assert!(verify_mrpack_file_hash(&file, bytes).is_ok());
        assert!(verify_mrpack_file_hash(&file, b"tampered").is_err());
    }

    #[test]
    fn test_modrinth_project_id_is_extracted_only_from_canonical_cdn_paths() {
        assert_eq!(
            modrinth_project_id_from_download_url(
                "https://cdn.modrinth.com/data/AANobbMI/versions/abc123/sodium.jar"
            ),
            Some("AANobbMI".to_string())
        );
        assert_eq!(
            modrinth_project_id_from_download_url(
                "https://github.com/example/mod/releases/download/v1/mod.jar"
            ),
            None
        );
        assert_eq!(
            modrinth_project_id_from_download_url(
                "http://cdn.modrinth.com/data/AANobbMI/versions/abc123/sodium.jar"
            ),
            None
        );
    }

    #[test]
    fn test_inventory_pack_content_preserves_modrinth_identity() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("mods")).unwrap();
        fs::write(tmp.path().join("mods").join("sodium.jar"), b"mod bytes").unwrap();

        let mut modrinth_files = BTreeMap::new();
        modrinth_files.insert(
            "mods/sodium.jar".to_string(),
            ImportedModrinthFile {
                project_id: "AANobbMI".to_string(),
                download_url: "https://cdn.modrinth.com/data/AANobbMI/versions/abc123/sodium.jar"
                    .to_string(),
            },
        );

        let installed = inventory_pack_content(tmp.path(), "mods", "mod", &modrinth_files).unwrap();
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].modrinth_id.as_deref(), Some("AANobbMI"));
        assert_eq!(installed[0].source, "modrinth-pack");
        assert_eq!(
            installed[0].source_url.as_deref(),
            Some("https://cdn.modrinth.com/data/AANobbMI/versions/abc123/sodium.jar")
        );
    }

    #[test]
    fn test_import_directory_creates_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("my-instance");
        fs::create_dir_all(src.join("mods")).unwrap();
        fs::write(src.join("mods").join("test-mod.jar"), b"fake jar").unwrap();

        let instances_root = tmp.path().join("imported");
        let result = import_directory(&src, &instances_root, false).unwrap();
        assert_eq!(result.name, "my-instance");
        assert_eq!(result.instance_id, "my-instance");
        assert!(instances_root
            .join("my-instance")
            .join("instance_manifest.json")
            .exists());
        assert_eq!(result.imported_mods, 1);
    }

    #[test]
    fn test_import_directory_preserves_existing_instances() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("incoming-instance");
        fs::create_dir_all(src.join("mods")).unwrap();
        fs::write(src.join("mods").join("new.jar"), b"new mod").unwrap();

        let instances_root = tmp.path().join("instances");
        let existing = instances_root.join("keep-me");
        fs::create_dir_all(existing.join("saves")).unwrap();
        fs::write(existing.join("saves").join("level.dat"), b"precious save").unwrap();

        let result = import_directory(&src, &instances_root, false).unwrap();

        assert_eq!(result.instance_id, "incoming-instance");
        assert_eq!(
            fs::read(existing.join("saves").join("level.dat")).unwrap(),
            b"precious save"
        );
        assert!(instances_root
            .join("incoming-instance")
            .join("mods")
            .join("new.jar")
            .exists());
    }

    #[test]
    fn test_import_directory_collision_never_deletes_existing_instance() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("same-name");
        fs::create_dir_all(src.join("mods")).unwrap();
        fs::write(src.join("mods").join("incoming.jar"), b"incoming").unwrap();

        let instances_root = tmp.path().join("instances");
        let existing = instances_root.join("same-name");
        fs::create_dir_all(&existing).unwrap();
        fs::write(existing.join("sentinel.txt"), b"keep this").unwrap();

        let error = import_directory(&src, &instances_root, false).unwrap_err();

        assert_eq!(error.code(), "ERR_INSTANCE_EXISTS");
        assert_eq!(
            fs::read(existing.join("sentinel.txt")).unwrap(),
            b"keep this"
        );
        assert!(!existing.join("mods").join("incoming.jar").exists());
    }

    #[test]
    fn test_assert_safe_path_rejects_nonexistent_parent_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("staging");
        fs::create_dir_all(&base).unwrap();

        let error =
            assert_safe_path(&base, &relative_archive_path("../outside/new.txt")).unwrap_err();

        assert_eq!(error.code(), "ERR_PATH_TRAVERSAL");
        assert!(!tmp.path().join("outside").exists());
    }

    #[test]
    fn test_auto_detect_launchers_does_not_panic() {
        let launchers = auto_detect_launchers();
        for l in &launchers {
            assert!(!l.launcher_type.is_empty());
            assert!(l.instances_dir.exists());
        }
    }

    #[test]
    fn test_import_directory_symlink_saves() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("instance-saves");
        fs::create_dir_all(src.join("saves")).unwrap();
        fs::write(
            src.join("saves").join("world1").to_str().unwrap(),
            b"world data",
        )
        .unwrap();

        let dst = tmp.path().join("linked");
        let _result = import_directory(&src, &dst, true).unwrap();
        assert!(dst
            .join("instance-saves")
            .join("saves")
            .join("world1")
            .exists());
    }

    #[test]
    fn test_parse_mrpack_deps_empty() {
        let (mc, loader, lv) = parse_mrpack_deps(&None);
        assert!(mc.is_empty());
        assert!(loader.is_empty());
        assert!(lv.is_empty());
    }

    #[test]
    fn test_parse_mrpack_deps_fabric() {
        let deps: serde_json::Value = serde_json::json!({
            "minecraft": "1.20.1",
            "fabric-loader": "0.15.0"
        });
        let (mc, loader, lv) = parse_mrpack_deps(&Some(deps));
        assert_eq!(mc, "1.20.1");
        assert_eq!(loader, "fabric");
        assert_eq!(lv, "0.15.0");
    }

    #[test]
    fn test_parse_prism_cfg_basic() {
        let cfg = "name=My Instance\niconKey=default\n";
        assert_eq!(
            parse_prism_cfg(cfg, "name"),
            Some("My Instance".to_string())
        );
        assert_eq!(parse_prism_cfg(cfg, "IconKey"), Some("default".to_string()));
        assert_eq!(parse_prism_cfg(cfg, "missing"), None);
    }

    #[test]
    fn test_parse_prism_components_fabric() {
        let pack = PrismPackJson {
            components: vec![
                PrismComponent {
                    uid: "net.minecraft".to_string(),
                    version: Some("1.20.1".to_string()),
                },
                PrismComponent {
                    uid: "net.fabricmc.fabric-loader".to_string(),
                    version: Some("0.15.0".to_string()),
                },
            ],
        };
        let (mc, loader, lv) = parse_prism_components(&Some(pack));
        assert_eq!(mc, "1.20.1");
        assert_eq!(loader, "fabric");
        assert_eq!(lv, "0.15.0");
    }

    #[test]
    fn test_parse_prism_components_empty() {
        let (mc, loader, lv) = parse_prism_components(&None);
        assert!(mc.is_empty());
        assert!(loader.is_empty());
        assert!(lv.is_empty());
    }

    #[test]
    fn test_import_produces_launchable_instance() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("launchable-instance");
        fs::create_dir_all(src.join("mods")).unwrap();
        fs::write(src.join("mods").join("mod.jar"), b"fake mod").unwrap();

        let instances_root = tmp.path().join("instances");
        let result = import_directory(&src, &instances_root, false).unwrap();
        assert_eq!(result.instance_id, "launchable-instance");

        let manifest_path = instances_root
            .join("launchable-instance")
            .join("instance_manifest.json");
        assert!(manifest_path.exists());

        let manifest: InstanceManifest =
            serde_json::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
        assert_eq!(manifest.instance_id, "launchable-instance");
        assert_eq!(manifest.name, "launchable-instance");
        assert!(manifest.mods.is_empty());
        assert!(manifest.resourcepacks.is_empty());
        assert!(manifest.shaders.is_empty());
        assert!(manifest.datapacks.is_empty());
        assert!(manifest.worlds.is_empty());
        assert!(!manifest.is_locked);
        assert_eq!(manifest.created_from_pack, None);
    }

    #[test]
    fn test_import_manifest_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let mrpack_path = tmp.path().join("rt-pack.mrpack");
        let file = fs::File::create(&mrpack_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file("modrinth.index.json", zip::write::FileOptions::default())
            .unwrap();
        writer
            .write_all(
                br#"{"name":"rt-pack","dependencies":{"minecraft":"1.21","fabric-loader":"0.16.0"},"files":[]}"#,
            )
            .unwrap();
        writer.finish().unwrap();

        let instances_root = tmp.path().join("rt-instances");
        let result = import_mrpack(&mrpack_path, &instances_root, false).unwrap();
        assert_eq!(result.instance_id, "rt-pack");
        assert_eq!(result.minecraft_version, "1.21");
        assert_eq!(result.loader, "fabric");
        assert_eq!(result.loader_version, "0.16.0");

        let manifest_path = instances_root
            .join("rt-pack")
            .join("instance_manifest.json");
        let manifest: InstanceManifest =
            serde_json::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();

        let serialized = serde_json::to_string_pretty(&manifest).unwrap();
        let deserialized: InstanceManifest = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.instance_id, manifest.instance_id);
        assert_eq!(deserialized.name, manifest.name);
        assert_eq!(deserialized.minecraft_version, manifest.minecraft_version);
        assert_eq!(deserialized.loader, manifest.loader);
        assert_eq!(deserialized.loader_version, manifest.loader_version);
        assert_eq!(deserialized.is_locked, manifest.is_locked);
        assert_eq!(deserialized.mods.len(), manifest.mods.len());
        assert_eq!(
            deserialized.resourcepacks.len(),
            manifest.resourcepacks.len()
        );
        assert_eq!(deserialized.shaders.len(), manifest.shaders.len());
        assert_eq!(deserialized.datapacks.len(), manifest.datapacks.len());
        assert_eq!(deserialized.worlds.len(), manifest.worlds.len());
    }
}
