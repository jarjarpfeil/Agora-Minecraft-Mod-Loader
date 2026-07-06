use crate::error::{LauncherError, LauncherResult};
use crate::paths;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
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
    downloads: Vec<String>,
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

/// Validate that a candidate path stays within the base directory.
/// Rejects path traversal attacks (../ sequences).
fn assert_safe_path(base: &Path, candidate: &Path) -> LauncherResult<()> {
    let resolved = base.join(candidate);
    let canonical_base = std::fs::canonicalize(base).unwrap_or_else(|_| base.to_path_buf());
    let canonical_resolved = std::fs::canonicalize(&resolved).unwrap_or(resolved);
    if !canonical_resolved.starts_with(&canonical_base) {
        Err(LauncherError::Generic {
            code: "ERR_PATH_TRAVERSAL".into(),
            message: format!("Path traversal detected: {}", candidate.display()),
        })
    } else {
        Ok(())
    }
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

/// Import an instance from a .mrpack file (Modrinth modpack format).
pub fn import_mrpack(
    mrpack_path: &Path,
    target_dir: &Path,
    _symlink_saves: bool,
) -> LauncherResult<ImportResult> {
    let file = fs::File::open(mrpack_path).map_err(|e| LauncherError::Generic {
        code: "ERR_IMPORT_OPEN".into(), message: format!("Cannot open mrpack: {e}"),
    })?;
    let mut archive = ZipArchive::new(file).map_err(|e| LauncherError::Generic {
        code: "ERR_IMPORT_ZIP".into(), message: format!("Invalid zip: {e}"),
    })?;

    let index_entry = archive
        .by_name("modrinth.index.json")
        .map_err(|_| LauncherError::Generic {
            code: "ERR_IMPORT_MISSING_INDEX".into(), message: "Missing modrinth.index.json".into(),
        })?;
    let index: MrpackIndex = serde_json::from_reader(index_entry)
        .map_err(|e| LauncherError::Generic {
            code: "ERR_IMPORT_PARSE_INDEX".into(), message: format!("Invalid modrinth.index.json: {e}"),
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

    fs::create_dir_all(target_dir).map_err(|e| LauncherError::Generic {
        code: "ERR_IMPORT_MKDIR".into(), message: format!("Cannot create target dir: {e}"),
    })?;

    let mut imported_mods = 0;
    for file_entry in &index.files {
        assert_safe_path(target_dir, Path::new(&file_entry.path))?;
        let dest = target_dir.join(&file_entry.path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| LauncherError::Generic {
                code: "ERR_IMPORT_MKDIR".into(), message: format!("Cannot create dir {parent:?}: {e}"),
            })?;
        }
        if !file_entry.downloads.is_empty() {
            let url = &file_entry.downloads[0];
            match download_bytes(url) {
                Ok(bytes) => {
                    fs::write(&dest, &bytes).map_err(|e| LauncherError::Generic {
                        code: "ERR_IMPORT_WRITE".into(), message: format!("Cannot write {:?}: {e}", dest),
                    })?;
                    imported_mods += 1;
                }
                Err(_) => {
                    let idx_path = file_entry.path.replace('\\', "/");
                    if let Ok(mut entry) = archive.by_name(&idx_path) {
                        let mut buf = Vec::new();
                        if entry.read_to_end(&mut buf).is_ok() {
                            fs::write(&dest, &buf)
                                .map_err(|e| LauncherError::Generic {
                                    code: "ERR_IMPORT_WRITE".into(), message: format!("Cannot write {:?}: {e}", dest),
                                })?;
                            imported_mods += 1;
                        }
                    }
                }
            }
        } else {
            let idx_path = file_entry.path.replace('\\', "/");
            if let Ok(mut entry) = archive.by_name(&idx_path) {
                let mut buf = Vec::new();
                if entry.read_to_end(&mut buf).is_ok() {
                    fs::write(&dest, &buf)
                        .map_err(|e| LauncherError::Generic {
                            code: "ERR_IMPORT_WRITE".into(), message: format!("Cannot write {:?}: {e}", dest),
                        })?;
                    imported_mods += 1;
                }
            }
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
            if entry_name == override_prefix || entry_name.starts_with(&format!("{override_prefix}/")) {
                let relative = entry_name
                    .strip_prefix(&format!("{override_prefix}/"))
                    .unwrap_or(&entry_name);
                if relative.is_empty() {
                    continue;
                }
                assert_safe_path(target_dir, Path::new(relative))?;
                let dest = target_dir.join(relative);
                if entry.is_dir() {
                    fs::create_dir_all(&dest).map_err(|e| LauncherError::Generic {
                        code: "ERR_IMPORT_MKDIR".into(), message: format!("Cannot create dir {dest:?}: {e}"),
                    })?;
                } else {
                    if let Some(parent) = dest.parent() {
                        fs::create_dir_all(parent)
                            .map_err(|e| LauncherError::Generic {
                                code: "ERR_IMPORT_MKDIR".into(), message: format!("Cannot create dir {parent:?}: {e}"),
                            })?;
                    }
                    let mut buf = Vec::new();
                    entry
                        .read_to_end(&mut buf)
                        .map_err(|e| LauncherError::Generic {
                            code: "ERR_IMPORT_READ".into(), message: format!("Cannot read {entry_name}: {e}"),
                        })?;
                    fs::write(&dest, &buf)
                        .map_err(|e| LauncherError::Generic {
                            code: "ERR_IMPORT_WRITE".into(), message: format!("Cannot write {:?}: {e}", dest),
                        })?;
                }
            }
        }
    }

    let instance_id = sanitize(&name);

    let manifest = serde_json::json!({
        "instance_id": instance_id,
        "name": name,
        "minecraft_version": minecraft_version,
        "loader": loader,
        "loader_version": loader_version,
        "is_modpack": true,
        "is_locked": false,
        "created_at": Utc::now().to_rfc3339(),
    });
    let manifest_path = target_dir.join("instance_manifest.json");
    fs::write(&manifest_path, serde_json::to_string_pretty(&manifest).unwrap())
        .map_err(|e| LauncherError::Generic {
            code: "ERR_IMPORT_WRITE".into(), message: format!("Cannot write manifest: {e}"),
        })?;

    Ok(ImportResult {
        instance_id,
        name,
        minecraft_version,
        loader,
        loader_version,
        imported_mods,
        linked_saves: false,
    })
}

fn parse_mrpack_deps(
    deps: &Option<serde_json::Value>,
) -> (String, String, String) {
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
    if let Some(v) = deps_map
        .get("fabric-loader")
        .and_then(|v| v.as_str())
    {
        loader = "fabric".to_string();
        loader_version = v.to_string();
    } else if let Some(v) = deps_map
        .get("quilt-loader")
        .and_then(|v| v.as_str())
    {
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
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| LauncherError::Generic {
            code: "ERR_IMPORT_RUNTIME".into(), message: format!("Cannot build runtime: {e}"),
        })?;
    rt.block_on(async {
        let client = reqwest::Client::new();
        let resp = client
            .get(url)
            .send()
            .await
            .map_err(|e| LauncherError::Generic {
                code: "ERR_IMPORT_DOWNLOAD".into(), message: format!("Download failed: {e}"),
            })?;
        if !resp.status().is_success() {
            return Err(LauncherError::Generic {
                code: "ERR_IMPORT_HTTP".into(), message: format!("HTTP {}", resp.status()),
            });
        }
        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| LauncherError::Generic {
                code: "ERR_IMPORT_READ_BODY".into(), message: format!("Read body failed: {e}"),
            })
    })
}

/// Import an instance from a Prism/MMC instance zip.
pub fn import_prism_zip(
    zip_path: &Path,
    target_dir: &Path,
    symlink_saves: bool,
) -> LauncherResult<ImportResult> {
    let file = fs::File::open(zip_path).map_err(|e| LauncherError::Generic {
        code: "ERR_IMPORT_OPEN_ZIP".into(), message: format!("Cannot open zip: {e}"),
    })?;
    let mut archive = ZipArchive::new(file).map_err(|e| LauncherError::Generic {
        code: "ERR_IMPORT_ZIP".into(), message: format!("Invalid zip: {e}"),
    })?;

    let mut instance_cfg = None;
    let mut pack_json = None;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| LauncherError::Generic {
            code: "ERR_IMPORT_READ_ZIP".into(), message: format!("Read error: {e}"),
        })?;
        let name = entry.name().replace('\\', "/");
        if name == "instance.cfg" {
            let mut buf = String::new();
            entry
                .read_to_string(&mut buf)
                .map_err(|e| LauncherError::Generic {
                    code: "ERR_IMPORT_READ_CFG".into(), message: format!("Read instance.cfg: {e}"),
                })?;
            instance_cfg = Some(buf);
        } else if name == "mmc-pack.json" {
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .map_err(|e| LauncherError::Generic {
                    code: "ERR_IMPORT_READ_PACK".into(), message: format!("Read mmc-pack.json: {e}"),
                })?;
            let parsed: PrismPackJson =
                serde_json::from_slice(&buf).map_err(|e| LauncherError::Generic {
                    code: "ERR_IMPORT_PARSE_PACK".into(), message: format!("Parse mmc-pack.json: {e}"),
                })?;
            pack_json = Some(parsed);
        }
    }

    let cfg = instance_cfg.ok_or_else(|| LauncherError::Generic {
        code: "ERR_IMPORT_MISSING_CFG".into(), message: "Missing instance.cfg".into(),
    })?;

    let name = parse_prism_cfg(&cfg, "name")
        .unwrap_or_else(|| zip_path.file_stem().unwrap_or_default().to_string_lossy().to_string());

    let (minecraft_version, loader, loader_version) = parse_prism_components(&pack_json);

    fs::create_dir_all(target_dir).map_err(|e| LauncherError::Generic {
        code: "ERR_IMPORT_MKDIR".into(), message: format!("Cannot create target dir: {e}"),
    })?;

    let mut imported_mods = 0;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| LauncherError::Generic {
            code: "ERR_IMPORT_READ_ZIP".into(), message: format!("Read error: {e}"),
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
        assert_safe_path(target_dir, Path::new(&entry_name))?;
        let dest = target_dir.join(&entry_name);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| LauncherError::Generic {
                    code: "ERR_IMPORT_MKDIR".into(), message: format!("Cannot create dir {parent:?}: {e}"),
                })?;
        }
        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|e| LauncherError::Generic {
                code: "ERR_IMPORT_READ".into(), message: format!("Cannot read {entry_name}: {e}"),
            })?;
        fs::write(&dest, &buf).map_err(|e| LauncherError::Generic {
            code: "ERR_IMPORT_WRITE".into(), message: format!("Cannot write {:?}: {e}", dest),
        })?;
        if entry_name.starts_with("mods/") {
            imported_mods += 1;
        }
    }

    let instance_id = sanitize(&name);

    let manifest = serde_json::json!({
        "instance_id": instance_id,
        "name": name,
        "minecraft_version": minecraft_version,
        "loader": loader,
        "loader_version": loader_version,
        "is_modpack": false,
        "is_locked": false,
        "created_at": Utc::now().to_rfc3339(),
    });
    let manifest_path = target_dir.join("instance_manifest.json");
    fs::write(&manifest_path, serde_json::to_string_pretty(&manifest).unwrap())
        .map_err(|e| LauncherError::Generic {
            code: "ERR_IMPORT_WRITE".into(), message: format!("Cannot write manifest: {e}"),
        })?;

    Ok(ImportResult {
        instance_id,
        name,
        minecraft_version,
        loader,
        loader_version,
        imported_mods,
        linked_saves: symlink_saves,
    })
}

fn parse_prism_cfg(cfg: &str, key: &str) -> Option<String> {
    for line in cfg.lines() {
        let line = line.trim();
        if let Some(eq) = line.find('=') {
            let k = line[..eq].trim().to_lowercase();
            if k == key {
                return Some(line[eq + 1..].trim().to_string());
            }
        }
    }
    None
}

fn parse_prism_components(
    pack_json: &Option<PrismPackJson>,
) -> (String, String, String) {
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
    target_dir: &Path,
    symlink_saves: bool,
) -> LauncherResult<ImportResult> {
    if !source_dir.is_dir() {
        return Err(LauncherError::Generic {
            code: "ERR_IMPORT_NOT_DIR".into(), message: format!("Source {:?} is not a directory", source_dir),
        });
    }

    let name = source_dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let instance_id = sanitize(&name);

    if target_dir.exists() {
        fs::remove_dir_all(target_dir)
            .map_err(|e| LauncherError::Generic {
                code: "ERR_IMPORT_RMDIR".into(), message: format!("Cannot remove existing target {target_dir:?}: {e}"),
            })?;
    }

    if symlink_saves {
        let saves_src = source_dir.join("saves");
        if saves_src.exists() {
            let saves_dst = target_dir.join("saves");
            fs::create_dir_all(target_dir)
                .map_err(|e| LauncherError::Generic {
                    code: "ERR_IMPORT_MKDIR".into(), message: format!("Cannot create target dir: {e}"),
                })?;
            copy_or_symlink(&saves_src, &saves_dst, true)
                .map_err(|e| LauncherError::Generic {
                    code: "ERR_IMPORT_SYMLINK".into(), message: format!("Cannot symlink saves: {e}"),
                })?;
        }
        let rest_dst = target_dir.join("_instance");
        copy_or_symlink(source_dir, &rest_dst, false)
            .map_err(|e| LauncherError::Generic {
                code: "ERR_IMPORT_COPY".into(), message: format!("Cannot copy instance: {e}"),
            })?;
    } else {
        copy_or_symlink(source_dir, target_dir, false)
            .map_err(|e| LauncherError::Generic {
                code: "ERR_IMPORT_COPY".into(), message: format!("Cannot copy instance: {e}"),
            })?;
    }

    let mc_version = String::new();
    let loader = String::new();
    let loader_version = String::new();

    let manifest_path = target_dir.join("instance_manifest.json");
    if !manifest_path.exists() {
        let manifest = serde_json::json!({
            "instance_id": instance_id,
            "name": name,
            "minecraft_version": mc_version,
            "loader": loader,
            "loader_version": loader_version,
            "is_modpack": false,
            "is_locked": false,
            "created_at": Utc::now().to_rfc3339(),
        });
        fs::write(&manifest_path, serde_json::to_string_pretty(&manifest).unwrap())
            .map_err(|e| LauncherError::Generic {
                code: "ERR_IMPORT_WRITE".into(), message: format!("Cannot write manifest: {e}"),
            })?;
    }

    let mod_count = if target_dir.join("mods").exists() {
        fs::read_dir(target_dir.join("mods"))
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
        instance_id,
        name,
        minecraft_version: mc_version,
        loader,
        loader_version,
        imported_mods: mod_count,
        linked_saves: symlink_saves,
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
                v.push(("prism", vec![appdata.join("PrismLauncher").join("instances")]));
                v.push(("modrinth", vec![appdata.join("com.modrinth.app").join("profiles")]));
                v.push(("curseforge", vec![appdata.join("curseforge").join("minecraft").join("Instances")]));
                v.push(("atlauncher", vec![appdata.join("ATLauncher").join("instances")]));
                v.push(("gdlauncher", vec![appdata.join("GDLauncher").join("instances")]));
            }
        }

        #[cfg(target_os = "linux")]
        {
            if let Some(home) = dirs::home_dir() {
                let local_share = home.join(".local").join("share");
                v.push(("prism", vec![local_share.join("PrismLauncher").join("instances")]));
                v.push(("modrinth", vec![local_share.join("com.modrinth.app").join("profiles")]));
                v.push(("curseforge", vec![home.join(".curseforge").join("minecraft").join("Instances")]));
                v.push(("atlauncher", vec![local_share.join("ATLauncher").join("instances")]));
                v.push(("gdlauncher", vec![local_share.join("GDLauncher").join("instances")]));
            }
        }

        #[cfg(target_os = "macos")]
        {
            if let Some(home) = dirs::home_dir() {
                let app_support = home.join("Library").join("Application Support");
                v.push(("prism", vec![app_support.join("PrismLauncher").join("instances")]));
                v.push(("modrinth", vec![app_support.join("com.modrinth.app").join("profiles")]));
                v.push(("curseforge", vec![app_support.join("curseforge").join("minecraft").join("Instances")]));
                v.push(("atlauncher", vec![app_support.join("ATLauncher").join("instances")]));
                v.push(("gdlauncher", vec![app_support.join("GDLauncher").join("instances")]));
            }
        }

        v
    };

    for (launcher_type, dirs) in candidates {
        for dir in dirs {
            if dir.exists() {
                let count = fs::read_dir(&dir)
                    .map(|entries| entries.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()).count())
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

    #[test]
    fn test_import_directory_creates_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("my-instance");
        fs::create_dir_all(src.join("mods")).unwrap();
        fs::write(src.join("mods").join("test-mod.jar"), b"fake jar").unwrap();

        let dst = tmp.path().join("imported");
        let result = import_directory(&src, &dst, false).unwrap();
        assert_eq!(result.name, "my-instance");
        assert!(dst.join("instance_manifest.json").exists());
        assert_eq!(result.imported_mods, 1);
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
        fs::write(src.join("saves").join("world1").to_str().unwrap(), b"world data").unwrap();

        let dst = tmp.path().join("linked");
        let result = import_directory(&src, &dst, true).unwrap();
        assert!(result.linked_saves);
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
        assert_eq!(parse_prism_cfg(cfg, "name"), Some("My Instance".to_string()));
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
}
