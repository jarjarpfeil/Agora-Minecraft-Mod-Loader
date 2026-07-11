use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use sha2::Digest;
use zip::write::FileOptions;
use zip::CompressionMethod;

const RESTORE_MARKER: &str = ".agora_restore_in_progress";

const TRACKED_ENTRIES: &[&str] = &[
    "mods",
    "config",
    "resourcepacks",
    "shaderpacks",
    "datapacks",
    "saves",
    "options.txt",
    "instance_manifest.json",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub label: Option<String>,
    pub created_at: String,
    pub file_count: usize,
    pub size_estimate: u64,
}

#[derive(Serialize, Deserialize)]
struct SnapshotManifest {
    snapshot: Snapshot,
    files: Vec<SnapshotFileEntry>,
}

#[derive(Serialize, Deserialize)]
struct SnapshotFileEntry {
    relative_path: String,
    size: u64,
    sha256: String,
}

fn snapshots_dir(instance_dir: &Path) -> PathBuf {
    instance_dir.join(".agora_snapshots")
}

fn snapshot_zip_path(instance_dir: &Path, id: &str) -> PathBuf {
    snapshots_dir(instance_dir).join(format!("{id}.zip"))
}

fn pre_restore_dir(instance_dir: &Path) -> PathBuf {
    instance_dir.join(".agora_pre_restore")
}

/// Create a snapshot of an instance directory, stored as a single compressed
/// `.zip` under `<instance_dir>/.agora_snapshots/<id>.zip`.
pub fn create_snapshot(instance_dir: &Path, label: Option<&str>) -> Result<Snapshot, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let zip_path = snapshot_zip_path(instance_dir, &id);

    fs::create_dir_all(snapshots_dir(instance_dir))
        .map_err(|e| format!("failed to create snapshots dir: {e}"))?;

    let file = fs::File::create(&zip_path)
        .map_err(|e| format!("failed to create snapshot zip: {e}"))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = FileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o644);

    let mut files: Vec<SnapshotFileEntry> = Vec::new();
    let mut total_size: u64 = 0;

    for entry_name in TRACKED_ENTRIES {
        let src = instance_dir.join(entry_name);
        if !src.exists() {
            continue;
        }

        if src.is_file() {
            let contents =
                fs::read(&src).map_err(|e| format!("failed to read {entry_name}: {e}"))?;
            zip.start_file(*entry_name, options)
                .map_err(|e| format!("failed to start zip entry {entry_name}: {e}"))?;
            zip.write_all(&contents)
                .map_err(|e| format!("failed to write {entry_name}: {e}"))?;
            let sha256 = {
                let mut hasher = sha2::Sha256::new();
                hasher.update(&contents);
                format!("{:x}", hasher.finalize())
            };
            files.push(SnapshotFileEntry {
                relative_path: entry_name.to_string(),
                size: contents.len() as u64,
                sha256,
            });
            total_size += contents.len() as u64;
        } else if src.is_dir() {
            walk_and_zip(
                &src, entry_name, &mut zip, options.clone(), &mut files, &mut total_size,
            )?;
        }
    }

    let snapshot = Snapshot {
        id: id.clone(),
        label: label.map(String::from),
        created_at: chrono::Utc::now().to_rfc3339(),
        file_count: files.len(),
        size_estimate: total_size,
    };

    let manifest = SnapshotManifest {
        snapshot: snapshot.clone(),
        files,
    };

    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("failed to serialize manifest: {e}"))?;
    zip.start_file("manifest.json", options)
        .map_err(|e| format!("failed to start manifest entry: {e}"))?;
    zip.write_all(manifest_json.as_bytes())
        .map_err(|e| format!("failed to write manifest: {e}"))?;

    zip.finish()
        .map_err(|e| format!("failed to finalize snapshot zip: {e}"))?;

    Ok(snapshot)
}

fn walk_and_zip(
    src: &Path,
    prefix: &str,
    zip: &mut zip::ZipWriter<fs::File>,
    options: FileOptions,
    files: &mut Vec<SnapshotFileEntry>,
    total_size: &mut u64,
) -> Result<(), String> {
    let entries =
        fs::read_dir(src).map_err(|e| format!("failed to read dir {}: {}", src.display(), e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("failed to read entry: {e}"))?;
        let entry_type = entry
            .file_type()
            .map_err(|e| format!("file type error: {e}"))?;
        let entry_name = entry.file_name().to_string_lossy().to_string();
        let src_path = entry.path();
        let relative = format!("{prefix}/{entry_name}");

        if entry_type.is_dir() {
            walk_and_zip(&src_path, &relative, zip, options.clone(), files, total_size)?;
        } else if entry_type.is_file() {
            let contents = fs::read(&src_path)
                .map_err(|e| format!("failed to read {}: {e}", src_path.display()))?;
            zip.start_file(relative.clone(), options)
                .map_err(|e| format!("failed to start zip entry {relative}: {e}"))?;
            zip.write_all(&contents)
                .map_err(|e| format!("failed to write {relative}: {e}"))?;
            let sha256 = {
                let mut hasher = sha2::Sha256::new();
                hasher.update(&contents);
                format!("{:x}", hasher.finalize())
            };
            files.push(SnapshotFileEntry {
                relative_path: relative,
                size: contents.len() as u64,
                sha256,
            });
            *total_size += contents.len() as u64;
        }
    }

    Ok(())
}

/// Restore an instance to a snapshot.  Current files are moved to
/// `.agora_pre_restore/` (safety net), then snapshot files are extracted.
pub fn restore_snapshot(instance_dir: &Path, snapshot_id: &str) -> Result<(), String> {
    let zip_path = snapshot_zip_path(instance_dir, snapshot_id);
    if !zip_path.exists() {
        return Err(format!("snapshot {snapshot_id} not found"));
    }

    let extract_dir = instance_dir.join(".agora_restore_extract");
    if extract_dir.exists() {
        fs::remove_dir_all(&extract_dir)
            .map_err(|e| format!("failed to remove old extract dir: {e}"))?;
    }
    fs::create_dir_all(&extract_dir)
        .map_err(|e| format!("failed to create extract dir: {e}"))?;

    let mut archive = {
        let file = fs::File::open(&zip_path)
            .map_err(|e| format!("failed to open snapshot zip: {e}"))?;
        zip::ZipArchive::new(file)
            .map_err(|e| format!("failed to read snapshot zip: {e}"))?
    };

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("failed to read zip entry {i}: {e}"))?;
        let out_path = extract_dir.join(entry.mangled_name());
        if entry.is_dir() {
            fs::create_dir_all(&out_path)
                .map_err(|e| format!("failed to create dir {}: {e}", out_path.display()))?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("failed to create parent: {e}"))?;
            }
            let mut contents = Vec::new();
            entry
                .read_to_end(&mut contents)
                .map_err(|e| format!("failed to read zip entry: {e}"))?;
            fs::write(&out_path, &contents)
                .map_err(|e| format!("failed to write {}: {e}", out_path.display()))?;
        }
    }

    let manifest_path = extract_dir.join("manifest.json");
    let content = fs::read_to_string(&manifest_path)
        .map_err(|e| format!("failed to read manifest: {e}"))?;
    let manifest: SnapshotManifest =
        serde_json::from_str(&content).map_err(|e| format!("failed to parse manifest: {e}"))?;

    let pre_dir = pre_restore_dir(instance_dir);
    if pre_dir.exists() {
        fs::remove_dir_all(&pre_dir)
            .map_err(|e| format!("failed to remove pre-restore dir: {e}"))?;
    }
    fs::create_dir_all(&pre_dir)
        .map_err(|e| format!("failed to create pre-restore dir: {e}"))?;

    let marker_path = instance_dir.join(RESTORE_MARKER);
    fs::write(&marker_path, b"restore in progress")
        .map_err(|e| format!("failed to write restore marker: {e}"))?;

    for entry_name in TRACKED_ENTRIES {
        let src = instance_dir.join(entry_name);
        if src.exists() {
            let dst = pre_dir.join(entry_name);
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("failed to create parent: {e}"))?;
            }
            fs::rename(&src, &dst)
                .map_err(|e| format!("failed to move {entry_name}: {e}"))?;
        }
    }

    for file_entry in &manifest.files {
        let src = extract_dir.join(&file_entry.relative_path);
        let dst = instance_dir.join(&file_entry.relative_path);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create dir: {e}"))?;
        }
        fs::copy(&src, &dst)
            .map_err(|e| {
                // Rollback on copy failure: move pre-restore files back.
                let _ = rollback_restore(instance_dir, &pre_dir);
                format!("failed to copy {}: {e}", file_entry.relative_path)
            })?;
    }

    if marker_path.exists() {
        fs::remove_file(&marker_path)
            .map_err(|e| format!("failed to remove restore marker: {e}"))?;
    }

    let _ = fs::remove_dir_all(&extract_dir);

    Ok(())
}

/// Reverse a failed restore by moving pre-restore files back into place.
/// Best-effort: returns the first error encountered but continues through all
/// tracked entries so the instance is as restored as possible.
fn rollback_restore(instance_dir: &Path, pre_dir: &Path) -> Result<(), String> {
    let mut first_err: Option<String> = None;
    for entry_name in TRACKED_ENTRIES {
        let src = pre_dir.join(entry_name);
        if src.exists() {
            let dst = instance_dir.join(entry_name);
            if let Err(e) = fs::rename(&src, &dst) {
                if first_err.is_none() {
                    first_err = Some(format!("rollback failed for {entry_name}: {e}"));
                }
            }
        }
    }
    if let Some(e) = first_err {
        Err(e)
    } else {
        Ok(())
    }
}

/// List all snapshots for an instance.
pub fn list_snapshots(instance_dir: &Path) -> Result<Vec<Snapshot>, String> {
    let marker = instance_dir.join(RESTORE_MARKER);
    if marker.exists() {
        return Err(
            "Previous restore was interrupted. Check .agora_pre_restore/ for backed-up files."
                .into(),
        );
    }

    let dir = snapshots_dir(instance_dir);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut snapshots = Vec::new();

    let entries =
        fs::read_dir(&dir).map_err(|e| format!("failed to read snapshots dir: {e}"))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("failed to read entry: {e}"))?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("zip") {
            continue;
        }

        let file = match fs::File::open(&path) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let mut archive = match zip::ZipArchive::new(file) {
            Ok(a) => a,
            Err(_) => continue,
        };

        let mut manifest_entry = match archive.by_name("manifest.json") {
            Ok(e) => e,
            Err(_) => continue,
        };

        let mut content = String::new();
        if manifest_entry.read_to_string(&mut content).is_err() {
            continue;
        }

        if let Ok(manifest) = serde_json::from_str::<SnapshotManifest>(&content) {
            snapshots.push(manifest.snapshot);
        }
    }

    snapshots.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(snapshots)
}

/// Delete a snapshot.
pub fn delete_snapshot(instance_dir: &Path, snapshot_id: &str) -> Result<(), String> {
    let zip_path = snapshot_zip_path(instance_dir, snapshot_id);
    if !zip_path.exists() {
        return Err(format!("snapshot {snapshot_id} not found"));
    }

    fs::remove_file(&zip_path)
        .map_err(|e| format!("failed to delete snapshot zip: {e}"))?;

    let dir = snapshots_dir(instance_dir);
    if dir.exists()
        && dir
            .read_dir()
            .map(|mut d| d.next().is_none())
            .unwrap_or(false)
    {
        let _ = fs::remove_dir(&dir);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_instance(tmp: &TempDir) -> PathBuf {
        let dir = tmp.path().join("instance");
        fs::create_dir_all(dir.join("mods")).unwrap();
        fs::create_dir_all(dir.join("config")).unwrap();
        fs::create_dir_all(dir.join("resourcepacks")).unwrap();
        fs::create_dir_all(dir.join("shaderpacks")).unwrap();
        fs::write(dir.join("mods").join("test.jar"), b"mod content").unwrap();
        fs::write(dir.join("config").join("settings.toml"), b"key=value").unwrap();
        fs::write(dir.join("options.txt"), b"render_distance=12").unwrap();
        dir
    }

    #[test]
    fn create_and_list_snapshot() {
        let tmp = TempDir::new().unwrap();
        let inst = make_instance(&tmp);

        let snap = create_snapshot(&inst, Some("before-update")).unwrap();
        assert_eq!(snap.label.as_deref(), Some("before-update"));
        assert!(snap.file_count > 0);
        assert!(snap.size_estimate > 0);

        let snaps = list_snapshots(&inst).unwrap();
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].id, snap.id);
    }

    #[test]
    fn restore_snapshot_preserves_content() {
        let tmp = TempDir::new().unwrap();
        let inst = make_instance(&tmp);

        let snap = create_snapshot(&inst, None).unwrap();

        fs::write(inst.join("mods").join("test.jar"), b"modified").unwrap();
        fs::write(inst.join("options.txt"), b"modified").unwrap();

        restore_snapshot(&inst, &snap.id).unwrap();

        assert_eq!(
            fs::read(inst.join("mods").join("test.jar")).unwrap(),
            b"mod content"
        );
        assert_eq!(
            fs::read(inst.join("options.txt")).unwrap(),
            b"render_distance=12"
        );

        assert!(inst.join(".agora_pre_restore").exists());
    }

    #[test]
    fn snapshot_is_immutable() {
        let tmp = TempDir::new().unwrap();
        let inst = make_instance(&tmp);

        let snap = create_snapshot(&inst, None).unwrap();

        fs::write(inst.join("mods").join("test.jar"), b"changed").unwrap();

        let zip_path = snapshot_zip_path(&inst, &snap.id);
        let file = fs::File::open(&zip_path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut entry = archive.by_name("mods/test.jar").unwrap();
        let mut content = Vec::new();
        entry.read_to_end(&mut content).unwrap();
        assert_eq!(content, b"mod content");
    }

    #[test]
    fn delete_snapshot_removes_zip() {
        let tmp = TempDir::new().unwrap();
        let inst = make_instance(&tmp);

        let snap = create_snapshot(&inst, None).unwrap();
        let zip_path = snapshot_zip_path(&inst, &snap.id);
        assert!(zip_path.exists());

        delete_snapshot(&inst, &snap.id).unwrap();
        assert!(!zip_path.exists());
    }

    #[test]
    fn list_snapshots_empty_when_none() {
        let tmp = TempDir::new().unwrap();
        let snaps = list_snapshots(tmp.path()).unwrap();
        assert!(snaps.is_empty());
    }
}
