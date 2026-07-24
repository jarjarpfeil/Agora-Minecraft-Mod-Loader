//! Core-owned instance lifecycle and CRUD operations.

use crate::clone::ClonePrefs;
use crate::ctx::Ctx;
use crate::error::{LauncherError, LauncherResult};
use crate::event_sink::{CoreEvent, EventStatus, ProgressEvent, ProgressPhase};
use crate::loader_service::LoaderService;
use crate::models::{InstanceManifest, InstanceRow};
use std::path::Path;
use std::sync::Arc;

/// Request used by every frontend to create an instance.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CreateInstanceRequest {
    pub name: String,
    pub instance_id: String,
    pub minecraft_version: String,
    pub loader: String,
    pub loader_version: String,
    #[serde(default)]
    pub jvm_memory_mb: Option<i64>,
    #[serde(default)]
    pub jvm_gc: Option<String>,
    #[serde(default)]
    pub jvm_custom_args: Option<String>,
    #[serde(default)]
    pub jvm_always_pre_touch: Option<bool>,
}

/// Request used to clone an existing instance.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CloneRequest {
    pub source_instance_id: String,
    pub new_name: String,
    #[serde(default)]
    pub prefs: ClonePrefs,
}

/// Combined database and manifest view.
#[derive(Debug, Clone, serde::Serialize)]
pub struct InstanceDetail {
    pub row: InstanceRow,
    pub manifest: Option<InstanceManifest>,
}

/// Core-owned preparation result for an official-launcher handoff.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DelegatedLaunchPreparation {
    pub profile_id: String,
    pub launcher_path: Option<String>,
    pub mod_ids: Vec<String>,
}

/// Narrow adapter type: a synchronous callback that moves a quarantined
/// directory to the OS trash. The desktop adapter supplies the real
/// `trash::delete` implementation; core never depends on the `trash` crate.
pub type TrashFn = Arc<dyn Fn(&Path) -> LauncherResult<()> + Send + Sync>;

/// Core instance lifecycle service.
#[derive(Clone)]
pub struct InstanceService {
    ctx: Ctx,
}

impl InstanceService {
    pub fn new(ctx: Ctx) -> Self {
        Self { ctx }
    }

    pub fn list(&self) -> LauncherResult<Vec<InstanceRow>> {
        let conn = self.connection()?;
        crate::db::list_instances(&conn).map_err(|error| LauncherError::Generic {
            code: "ERR_LOCAL_STATE_FAILED".into(),
            message: error.to_string(),
        })
    }

    pub fn get(&self, instance_id: &str) -> LauncherResult<Option<InstanceDetail>> {
        let instance_id = self.validate_id(instance_id)?;
        let conn = self.connection()?;
        let Some(row) = crate::db::get_instance(&conn, &instance_id).map_err(|error| {
            LauncherError::Generic {
                code: "ERR_LOCAL_STATE_FAILED".into(),
                message: error.to_string(),
            }
        })?
        else {
            return Ok(None);
        };
        let manifest = read_manifest(&self.ctx.paths.instance_manifest(&instance_id)?)?;
        Ok(Some(InstanceDetail { row, manifest }))
    }

    pub fn lock(&self, instance_id: &str) -> LauncherResult<()> {
        self.set_locked(instance_id, true)
    }

    pub fn unlock(&self, instance_id: &str) -> LauncherResult<()> {
        self.set_locked(instance_id, false)
    }

    pub fn update_java(
        &self,
        instance_id: &str,
        java_path: Option<&str>,
        allow_incompatible: bool,
        custom_args: Option<&str>,
    ) -> LauncherResult<()> {
        let instance_id = self.validate_id(instance_id)?;
        let conn = self.connection()?;
        let _row = crate::db::get_instance(&conn, &instance_id)
            .map_err(|error| LauncherError::Generic {
                code: "ERR_LOCAL_STATE_FAILED".into(),
                message: error.to_string(),
            })?
            .ok_or_else(|| LauncherError::Generic {
                code: "ERR_INSTANCE_NOT_FOUND".into(),
                message: format!("Instance '{instance_id}' not found"),
            })?;
        crate::db::update_instance_java(
            &conn,
            &instance_id,
            java_path,
            allow_incompatible,
            custom_args,
        )
        .map_err(|error| LauncherError::Generic {
            code: "ERR_LOCAL_STATE_FAILED".into(),
            message: error.to_string(),
        })
    }

    pub fn update_jvm(
        &self,
        instance_id: &str,
        memory_mb: i64,
        gc: &str,
        always_pre_touch: bool,
        custom_args: &str,
    ) -> LauncherResult<()> {
        let instance_id = self.validate_id(instance_id)?;
        let memory_mb = memory_mb.clamp(2048, 32768);
        let gc = match gc.trim().to_ascii_lowercase().as_str() {
            "auto" | "g1gc" | "zgc" | "shenandoah" | "manual" => gc.trim().to_ascii_lowercase(),
            _ => "auto".to_string(),
        };
        let conn = self.connection()?;
        let _row = crate::db::get_instance(&conn, &instance_id)
            .map_err(|error| LauncherError::Generic {
                code: "ERR_LOCAL_STATE_FAILED".into(),
                message: error.to_string(),
            })?
            .ok_or_else(|| LauncherError::Generic {
                code: "ERR_INSTANCE_NOT_FOUND".into(),
                message: format!("Instance '{instance_id}' not found"),
            })?;
        crate::db::update_instance_jvm(
            &conn,
            &instance_id,
            memory_mb,
            &gc,
            always_pre_touch,
            custom_args,
        )
        .map_err(|error| LauncherError::Generic {
            code: "ERR_LOCAL_STATE_FAILED".into(),
            message: error.to_string(),
        })
    }

    pub fn rename(&self, instance_id: &str, name: &str) -> LauncherResult<()> {
        let instance_id = self.validate_id(instance_id)?;
        if name.trim().is_empty() {
            return Err(LauncherError::Generic {
                code: "ERR_INVALID_INSTANCE_NAME".into(),
                message: "Instance name must not be empty.".into(),
            });
        }
        let conn = self.connection()?;
        crate::db::rename_instance(&conn, &instance_id, name.trim()).map_err(|error| {
            LauncherError::Generic {
                code: "ERR_LOCAL_STATE_FAILED".into(),
                message: error.to_string(),
            }
        })
    }

    /// Reconcile the official launcher profile and persistent launch history
    /// before a desktop adapter starts the native Mojang launcher.
    pub fn prepare_delegated_launch(
        &self,
        instance_id: &str,
    ) -> LauncherResult<DelegatedLaunchPreparation> {
        let instance_id = self.validate_id(instance_id)?;
        let conn = self.connection()?;
        let row = crate::db::get_instance(&conn, &instance_id)
            .map_err(|error| LauncherError::Generic {
                code: "ERR_LOCAL_STATE_FAILED".into(),
                message: error.to_string(),
            })?
            .ok_or(LauncherError::LaunchFailed)?;
        let user_override = crate::db::get_setting(&conn, "jvm_always_pre_touch")
            .ok()
            .flatten()
            .and_then(|value| value.as_bool());
        let jvm = crate::models::JvmConfig {
            memory_mb: row.jvm_memory_mb,
            gc: row.jvm_gc.clone(),
            custom_args: row.jvm_custom_args.clone(),
            always_pre_touch: row.jvm_always_pre_touch && user_override.unwrap_or(true),
        };
        let profile_id = format!("agora-{}", row.instance_id);
        let profile = crate::launcher_profiles::LauncherProfileEntry {
            profile_id: profile_id.clone(),
            name: format!("{} (Agora)", row.name),
            last_version_id: loader_version_id(&row),
            game_dir: self.ctx.paths.instance_dir(&instance_id)?,
            java_args: jvm.to_args_for_java(crate::models::recommended_java_version_for_minecraft(
                &row.minecraft_version,
            )),
        };
        if let Some(profiles_path) = &self.ctx.launcher_profiles_path {
            crate::launcher_profiles::upsert_profile(&profile, profiles_path)?;
        }
        crate::db::touch_last_launched(&conn, &instance_id, &chrono::Utc::now().to_rfc3339())
            .map_err(|error| LauncherError::Generic {
                code: "ERR_LOCAL_STATE_FAILED".into(),
                message: error.to_string(),
            })?;
        let mod_ids = read_manifest(&self.ctx.paths.instance_manifest(&instance_id)?)?
            .map(|manifest| {
                manifest
                    .mods
                    .into_iter()
                    .map(|mod_entry| mod_entry.registry_id.unwrap_or(mod_entry.filename))
                    .collect()
            })
            .unwrap_or_default();
        let launcher_path = crate::db::get_setting(&conn, "mojang_launcher_path")
            .ok()
            .flatten()
            .and_then(|value| value.as_str().map(str::to_owned));
        Ok(DelegatedLaunchPreparation {
            profile_id,
            launcher_path,
            mod_ids,
        })
    }

    /// Delete an instance with a filesystem quarantine so a database failure
    /// can restore the original directory before the operation returns.
    ///
    /// When `trash_fn` is provided, the quarantined directory is moved to the
    /// OS trash instead of being hard-removed. This is the narrow adapter
    /// boundary — core never depends on the `trash` crate.
    pub fn delete(&self, instance_id: &str, trash_fn: Option<TrashFn>) -> LauncherResult<()> {
        let instance_id = self.validate_id(instance_id)?;
        let op = self
            .ctx
            .operation_manager
            .register_for_instance("Delete instance", &instance_id);
        let _lock = match self.ctx.lock_manager.acquire(
            crate::lock_manager::LockResource::Instance(instance_id.clone()),
            "delete",
        ) {
            Ok(l) => l,
            Err(e) => {
                op.fail(e.to_string());
                return Err(e);
            }
        };
        let conn = match self.connection() {
            Ok(c) => c,
            Err(e) => {
                op.fail(e.to_string());
                return Err(e);
            }
        };
        let row = match crate::db::get_instance(&conn, &instance_id) {
            Ok(Some(r)) => r,
            Ok(None) => {
                op.fail(format!("Instance '{instance_id}' not found."));
                return Err(LauncherError::Generic {
                    code: "ERR_INSTANCE_NOT_FOUND".into(),
                    message: format!("Instance '{instance_id}' not found."),
                });
            }
            Err(error) => {
                op.fail(error.to_string());
                return Err(LauncherError::Generic {
                    code: "ERR_LOCAL_STATE_FAILED".into(),
                    message: error.to_string(),
                });
            }
        };
        if row.is_locked {
            op.fail(format!("Instance '{instance_id}' is locked."));
            return Err(LauncherError::Generic {
                code: "ERR_INSTANCE_LOCKED".into(),
                message: format!("Instance '{instance_id}' is locked."),
            });
        }
        let dir = self.ctx.paths.instance_dir(&instance_id)?;
        let quarantine = self.ctx.paths.staging_dir(&format!(
            "delete-{}-{}",
            instance_id,
            uuid::Uuid::new_v4()
        ))?;
        let moved = dir.exists();
        if moved {
            if let Some(parent) = quarantine.parent() {
                std::fs::create_dir_all(parent).map_err(|error| LauncherError::Generic {
                    code: "ERR_INSTANCE_DELETE".into(),
                    message: error.to_string(),
                })?;
            }
            std::fs::rename(&dir, &quarantine).map_err(|error| LauncherError::Generic {
                code: "ERR_INSTANCE_DELETE".into(),
                message: error.to_string(),
            })?;
        }
        if let Err(error) = crate::db::delete_instance(&conn, &instance_id) {
            if moved {
                let _ = std::fs::rename(&quarantine, &dir);
            }
            op.fail(error.to_string());
            return Err(LauncherError::Generic {
                code: "ERR_LOCAL_STATE_FAILED".into(),
                message: error.to_string(),
            });
        }
        if moved && quarantine.exists() {
            if let Some(trash) = trash_fn {
                if let Err(error) = trash(&quarantine) {
                    let _ = crate::db::upsert_instance(&conn, &row);
                    let _ = std::fs::rename(&quarantine, &dir);
                    op.fail(error.to_string());
                    return Err(error);
                }
            } else {
                std::fs::remove_dir_all(&quarantine).map_err(|error| LauncherError::Generic {
                    code: "ERR_INSTANCE_DELETE".into(),
                    message: error.to_string(),
                })?;
            }
        }
        if let Some(profiles_path) = &self.ctx.launcher_profiles_path {
            let _ = crate::launcher_profiles::remove_profile(
                &format!("agora-{instance_id}"),
                profiles_path,
            );
        }
        op.complete();
        Ok(())
    }

    /// Clone an existing instance: copy directory contents per prefs, create a
    /// new DB row and launcher profile. Uses safe staging — the target directory
    /// is built in a staging area and atomically renamed on success. The source
    /// is locked for the duration of the operation.
    pub async fn clone(&self, request: CloneRequest) -> LauncherResult<InstanceRow> {
        let source_id = self.validate_id(&request.source_instance_id)?;
        if request.new_name.trim().is_empty() {
            return Err(LauncherError::Generic {
                code: "ERR_INVALID_INSTANCE_NAME".into(),
                message: "Clone name must not be empty.".into(),
            });
        }
        let new_id = crate::paths::sanitize_id(request.new_name.trim());
        crate::app_paths::validate_path_component(&new_id).map_err(|_| LauncherError::Generic {
            code: "ERR_INVALID_INSTANCE_NAME".into(),
            message: format!("'{new_id}' is not a valid instance identifier."),
        })?;
        if source_id == new_id {
            return Err(LauncherError::Generic {
                code: "ERR_CLONE_SAME_ID".into(),
                message: "Source and clone IDs are identical.".into(),
            });
        }

        let op = self
            .ctx
            .operation_manager
            .register_for_instance("Clone instance", &new_id);

        // Validate source
        let conn = match self.connection() {
            Ok(c) => c,
            Err(e) => {
                op.fail(e.to_string());
                return Err(e);
            }
        };
        let source_row = match crate::db::get_instance(&conn, &source_id) {
            Ok(Some(row)) => row,
            Ok(None) => {
                op.fail(format!("Source instance '{source_id}' not found."));
                return Err(LauncherError::Generic {
                    code: "ERR_INSTANCE_NOT_FOUND".into(),
                    message: format!("Source instance '{source_id}' not found."),
                });
            }
            Err(error) => {
                op.fail(error.to_string());
                return Err(LauncherError::Generic {
                    code: "ERR_LOCAL_STATE_FAILED".into(),
                    message: error.to_string(),
                });
            }
        };
        if source_row.is_locked {
            op.fail(format!("Source instance '{source_id}' is locked."));
            return Err(LauncherError::Generic {
                code: "ERR_INSTANCE_LOCKED".into(),
                message: format!("Source instance '{source_id}' is locked."),
            });
        }
        drop(conn);

        // Check destination does not exist before acquiring the ordered locks.
        let dest_dir = self.ctx.paths.instance_dir(&new_id)?;
        if dest_dir.exists() {
            op.fail(format!("An instance named '{new_id}' already exists."));
            return Err(LauncherError::Generic {
                code: "ERR_INSTANCE_EXISTS".into(),
                message: format!("An instance named '{new_id}' already exists."),
            });
        }

        // Acquire source and destination locks in a stable order so reverse
        // clone requests cannot deadlock each other.
        let (first_id, second_id) = if source_id < new_id {
            (source_id.as_str(), new_id.as_str())
        } else {
            (new_id.as_str(), source_id.as_str())
        };
        let _first_lock = match self.ctx.lock_manager.acquire(
            crate::lock_manager::LockResource::Instance(first_id.to_owned()),
            "clone",
        ) {
            Ok(l) => l,
            Err(e) => {
                op.fail(e.to_string());
                return Err(e);
            }
        };
        let _second_lock = match self.ctx.lock_manager.acquire(
            crate::lock_manager::LockResource::Instance(second_id.to_owned()),
            "clone",
        ) {
            Ok(l) => l,
            Err(e) => {
                op.fail(e.to_string());
                return Err(e);
            }
        };

        let conn = match self.connection() {
            Ok(c) => c,
            Err(e) => {
                op.fail(e.to_string());
                return Err(e);
            }
        };
        if crate::db::get_instance(&conn, &new_id)
            .map_err(|error| LauncherError::Generic {
                code: "ERR_LOCAL_STATE_FAILED".into(),
                message: error.to_string(),
            })?
            .is_some()
        {
            op.fail(format!("An instance named '{new_id}' already exists."));
            return Err(LauncherError::Generic {
                code: "ERR_INSTANCE_EXISTS".into(),
                message: format!("An instance named '{new_id}' already exists."),
            });
        }
        drop(conn);

        // Read source manifest
        let manifest_path = self.ctx.paths.instance_manifest(&source_id)?;
        let source_manifest: InstanceManifest = if manifest_path.exists() {
            let text =
                std::fs::read_to_string(&manifest_path).map_err(|e| LauncherError::Generic {
                    code: "ERR_CLONE".into(),
                    message: format!("Cannot read source manifest: {e}"),
                })?;
            serde_json::from_str(&text).map_err(|e| LauncherError::Generic {
                code: "ERR_CLONE".into(),
                message: format!("Cannot parse source manifest: {e}"),
            })?
        } else {
            // Synthesise a minimal manifest from the DB row.
            InstanceManifest {
                instance_id: new_id.clone(),
                name: request.new_name.trim().to_owned(),
                created_from_pack: None,
                minecraft_version: source_row.minecraft_version.clone(),
                loader: source_row.loader.clone(),
                loader_version: source_row.loader_version.clone(),
                is_locked: source_row.is_locked,
                mods: Vec::new(),
                resourcepacks: Vec::new(),
                shaders: Vec::new(),
                datapacks: Vec::new(),
                worlds: Vec::new(),
                user_preferences: serde_json::json!({}),
            }
        };

        // Build new row and manifest
        let now = chrono::Utc::now().to_rfc3339();
        let new_row = InstanceRow {
            instance_id: new_id.clone(),
            name: request.new_name.trim().to_owned(),
            minecraft_version: source_row.minecraft_version.clone(),
            loader: source_row.loader.clone(),
            loader_version: source_row.loader_version.clone(),
            is_modpack: source_row.is_modpack,
            is_locked: false,
            last_launched_at: None,
            jvm_memory_mb: source_row.jvm_memory_mb,
            jvm_gc: source_row.jvm_gc.clone(),
            jvm_custom_args: source_row.jvm_custom_args.clone(),
            jvm_always_pre_touch: source_row.jvm_always_pre_touch,
            created_at: now,
            java_path: source_row.java_path.clone(),
            java_incompatible_override: source_row.java_incompatible_override,
        };
        let new_manifest = InstanceManifest {
            instance_id: new_id.clone(),
            name: request.new_name.trim().to_owned(),
            ..source_manifest
        };

        let operation_id = op.id().clone();
        self.ctx.progress_sink.report(ProgressEvent::new(
            operation_id.clone(),
            ProgressPhase::Staging,
            "Cloning instance files",
        ));

        // Build in staging then atomically rename
        let staging = self
            .ctx
            .paths
            .staging_dir(&format!("clone-{}", uuid::Uuid::new_v4()))?;
        let src_dir = self.ctx.paths.instance_dir(&source_id)?;

        if let Err(error) = crate::clone::clone_instance(&src_dir, &staging, &request.prefs) {
            let _ = std::fs::remove_dir_all(&staging);
            op.fail(error.clone());
            return Err(LauncherError::Generic {
                code: "ERR_CLONE".into(),
                message: error,
            });
        }

        // Write updated manifest
        let manifest_bytes = serde_json::to_vec_pretty(&new_manifest)
            .map_err(|_| LauncherError::InstanceCreateFailed)?;
        if let Err(error) = std::fs::write(staging.join("instance_manifest.json"), manifest_bytes) {
            let _ = std::fs::remove_dir_all(&staging);
            op.fail(format!("Cannot write clone manifest: {error}"));
            return Err(LauncherError::Generic {
                code: "ERR_CLONE".into(),
                message: format!("Cannot write clone manifest: {error}"),
            });
        }

        // Mark staging complete
        if let Err(error) = std::fs::write(staging.join("staging-complete"), b"complete") {
            let _ = std::fs::remove_dir_all(&staging);
            op.fail(error.to_string());
            return Err(LauncherError::Generic {
                code: "ERR_CLONE".into(),
                message: error.to_string(),
            });
        }

        // Promote staging to destination
        if let Err(error) = std::fs::remove_file(staging.join("staging-complete")) {
            let _ = std::fs::remove_dir_all(&staging);
            op.fail(format!("Cannot finalize clone staging: {error}"));
            return Err(LauncherError::Generic {
                code: "ERR_CLONE".into(),
                message: format!("Cannot finalize clone staging: {error}"),
            });
        }
        if let Err(error) = std::fs::rename(&staging, &dest_dir) {
            let _ = std::fs::remove_dir_all(&staging);
            op.fail(format!("Cannot promote clone: {error}"));
            return Err(LauncherError::Generic {
                code: "ERR_CLONE".into(),
                message: format!("Cannot promote clone: {error}"),
            });
        }

        // Persist DB row
        let conn = match self.connection() {
            Ok(conn) => conn,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&dest_dir);
                op.fail(error.to_string());
                return Err(error);
            }
        };
        if let Err(error) = crate::db::upsert_instance(&conn, &new_row) {
            let _ = std::fs::remove_dir_all(&dest_dir);
            op.fail(error.to_string());
            return Err(LauncherError::Generic {
                code: "ERR_LOCAL_STATE_FAILED".into(),
                message: error.to_string(),
            });
        }

        // Launcher profile
        if let Some(profiles_path) = &self.ctx.launcher_profiles_path {
            let user_override = crate::db::get_setting(&conn, "jvm_always_pre_touch")
                .ok()
                .flatten()
                .and_then(|value| value.as_bool());
            let jvm = crate::models::JvmConfig {
                memory_mb: new_row.jvm_memory_mb,
                gc: new_row.jvm_gc.clone(),
                custom_args: new_row.jvm_custom_args.clone(),
                always_pre_touch: new_row.jvm_always_pre_touch && user_override.unwrap_or(true),
            };
            let profile = crate::launcher_profiles::LauncherProfileEntry {
                profile_id: format!("agora-{}", new_row.instance_id),
                name: format!("{} (Agora)", new_row.name),
                last_version_id: loader_version_id(&new_row),
                game_dir: dest_dir,
                java_args: jvm.to_args_for_java(
                    crate::models::recommended_java_version_for_minecraft(
                        &new_row.minecraft_version,
                    ),
                ),
            };
            if let Err(error) = crate::launcher_profiles::upsert_profile(&profile, profiles_path) {
                let _ = crate::db::delete_instance(&conn, &new_row.instance_id);
                let _ = std::fs::remove_dir_all(&self.ctx.paths.instance_dir(&new_id)?);
                op.fail(error.to_string());
                return Err(error);
            }
        }

        op.complete();
        self.ctx.event_sink.emit(CoreEvent::Instance {
            operation_id,
            instance_id: new_id,
            status: EventStatus::Completed,
            message: "Instance cloned".into(),
        });
        Ok(new_row)
    }

    /// Create the instance directory, bootstrap shared metadata, install its
    /// loader, and persist the row. All partial instance state is removed on
    /// failure; shared loader artifacts are retained.
    pub async fn create(&self, request: CreateInstanceRequest) -> LauncherResult<InstanceRow> {
        let instance_id = self.validate_id(&request.instance_id)?;
        if request.name.trim().is_empty() || request.minecraft_version.trim().is_empty() {
            return Err(LauncherError::InstanceCreateFailed);
        }
        let op = self
            .ctx
            .operation_manager
            .register_for_instance("Create instance", &instance_id);
        let _lock = self.ctx.lock_manager.acquire(
            crate::lock_manager::LockResource::Instance(instance_id.clone()),
            "create",
        )?;
        let dir = self.ctx.paths.instance_dir(&instance_id)?;
        if dir.exists() {
            op.fail(format!("An instance named '{instance_id}' already exists."));
            return Err(LauncherError::Generic {
                code: "ERR_INSTANCE_EXISTS".into(),
                message: format!("An instance named '{instance_id}' already exists."),
            });
        }
        let staging_dir = self
            .ctx
            .paths
            .staging_dir(&format!("instance-{}", uuid::Uuid::new_v4()))?;
        let operation_id = op.id().clone();
        self.ctx.progress_sink.report(ProgressEvent::new(
            operation_id.clone(),
            ProgressPhase::Staging,
            "Preparing instance directory",
        ));
        let row = prepare_row(&instance_id, &request);
        let manifest = manifest_from_request(&instance_id, &request);
        if let Err(error) = self.prepare_files(&staging_dir, &manifest) {
            let _ = std::fs::remove_dir_all(&staging_dir);
            op.fail(error.to_string());
            return Err(error);
        }
        if let Err(error) = std::fs::write(staging_dir.join("staging-complete"), b"complete") {
            let _ = std::fs::remove_dir_all(&staging_dir);
            op.fail(error.to_string());
            return Err(LauncherError::Generic {
                code: "ERR_INSTANCE_STAGING".into(),
                message: error.to_string(),
            });
        }

        let minecraft_root = self.ctx.paths.minecraft_runtime_root();
        if let Err(error) = crate::minecraft_runtime::ensure_runtime_layout(&minecraft_root) {
            let _ = std::fs::remove_dir_all(&staging_dir);
            op.fail(error.to_string());
            return Err(error);
        }
        let conn = match self.connection() {
            Ok(conn) => conn,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&staging_dir);
                op.fail(error.to_string());
                return Err(error);
            }
        };
        let policy = crate::network::NetworkPolicy::from_db(&conn);
        drop(conn);
        if let Err(error) = crate::minecraft_metadata::ensure_base_version_metadata(
            &minecraft_root,
            &request.minecraft_version,
            &policy,
        )
        .await
        {
            let _ = std::fs::remove_dir_all(&staging_dir);
            op.fail(error.to_string());
            return Err(error);
        }

        if !matches!(request.loader.as_str(), "" | "vanilla") {
            if let Err(error) = LoaderService::new(self.ctx.clone())
                .ensure_installed(
                    &request.loader,
                    &request.minecraft_version,
                    &request.loader_version,
                    false,
                )
                .await
            {
                let _ = std::fs::remove_dir_all(&staging_dir);
                op.fail(error.to_string());
                return Err(error);
            }
        }

        if let Err(error) = std::fs::remove_file(staging_dir.join("staging-complete")) {
            let _ = std::fs::remove_dir_all(&staging_dir);
            op.fail(error.to_string());
            return Err(LauncherError::Generic {
                code: "ERR_INSTANCE_STAGING".into(),
                message: error.to_string(),
            });
        }
        if let Err(error) = std::fs::rename(&staging_dir, &dir) {
            let _ = std::fs::remove_dir_all(&staging_dir);
            op.fail(error.to_string());
            return Err(LauncherError::Generic {
                code: "ERR_INSTANCE_PROMOTE".into(),
                message: error.to_string(),
            });
        }

        let conn = match self.connection() {
            Ok(conn) => conn,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&dir);
                op.fail(error.to_string());
                return Err(error);
            }
        };
        if let Err(error) = crate::db::upsert_instance(&conn, &row) {
            let _ = std::fs::remove_dir_all(&dir);
            op.fail(error.to_string());
            return Err(LauncherError::Generic {
                code: "ERR_LOCAL_STATE_FAILED".into(),
                message: error.to_string(),
            });
        }
        if let Some(profiles_path) = &self.ctx.launcher_profiles_path {
            let user_override = crate::db::get_setting(&conn, "jvm_always_pre_touch")
                .ok()
                .flatten()
                .and_then(|value| value.as_bool());
            let jvm = crate::models::JvmConfig {
                memory_mb: row.jvm_memory_mb,
                gc: row.jvm_gc.clone(),
                custom_args: row.jvm_custom_args.clone(),
                always_pre_touch: row.jvm_always_pre_touch && user_override.unwrap_or(true),
            };
            let profile = crate::launcher_profiles::LauncherProfileEntry {
                profile_id: format!("agora-{}", row.instance_id),
                name: format!("{} (Agora)", row.name),
                last_version_id: loader_version_id(&row),
                game_dir: dir.clone(),
                java_args: jvm.to_args(),
            };
            if let Err(error) = crate::launcher_profiles::upsert_profile(&profile, profiles_path) {
                let _ = crate::db::delete_instance(&conn, &row.instance_id);
                let _ = std::fs::remove_dir_all(&dir);
                op.fail(error.to_string());
                return Err(error);
            }
        }
        op.complete();
        self.ctx
            .event_sink
            .emit(crate::event_sink::CoreEvent::Instance {
                operation_id,
                instance_id,
                status: EventStatus::Completed,
                message: "Instance created".into(),
            });
        Ok(row)
    }

    fn connection(&self) -> LauncherResult<rusqlite::Connection> {
        crate::db::local_state_connection(&self.ctx.paths.local_state_db()).map_err(|error| {
            LauncherError::Generic {
                code: "ERR_LOCAL_STATE_FAILED".into(),
                message: error.to_string(),
            }
        })
    }

    fn validate_id(&self, instance_id: &str) -> LauncherResult<String> {
        let sanitized = crate::paths::sanitize_id(instance_id);
        crate::app_paths::validate_path_component(&sanitized)?;
        Ok(sanitized)
    }

    fn set_locked(&self, instance_id: &str, locked: bool) -> LauncherResult<()> {
        let instance_id = self.validate_id(instance_id)?;
        let conn = self.connection()?;
        let manifest_path = self.ctx.paths.instance_manifest(&instance_id)?;
        let mut manifest = read_manifest(&manifest_path)?;
        if let Some(manifest) = manifest.as_mut() {
            manifest.is_locked = locked;
        }

        crate::db::set_locked(&conn, &instance_id, locked).map_err(|error| {
            LauncherError::Generic {
                code: "ERR_LOCAL_STATE_FAILED".into(),
                message: error.to_string(),
            }
        })?;

        if let Some(manifest) = manifest {
            if let Err(error) = crate::helpers::atomic_write_manifest(&manifest_path, &manifest) {
                // Keep the database and manifest aligned if the filesystem write fails.
                let _ = crate::db::set_locked(&conn, &instance_id, !locked);
                return Err(error);
            }
        }
        Ok(())
    }

    fn prepare_files(&self, dir: &Path, manifest: &InstanceManifest) -> LauncherResult<()> {
        for child in [
            "mods",
            "resourcepacks",
            "shaderpacks",
            "datapacks",
            "config",
            "crash-reports",
            "logs",
            "saves",
            "screenshots",
        ] {
            std::fs::create_dir_all(dir.join(child))
                .map_err(|_| LauncherError::InstanceCreateFailed)?;
        }
        let path = dir.join("instance_manifest.json");
        let bytes =
            serde_json::to_vec_pretty(manifest).map_err(|_| LauncherError::InstanceCreateFailed)?;
        std::fs::write(path, bytes).map_err(|_| LauncherError::InstanceCreateFailed)
    }
}

fn prepare_row(instance_id: &str, request: &CreateInstanceRequest) -> InstanceRow {
    InstanceRow {
        instance_id: instance_id.into(),
        name: request.name.clone(),
        minecraft_version: request.minecraft_version.clone(),
        loader: request.loader.clone(),
        loader_version: request.loader_version.clone(),
        is_modpack: false,
        is_locked: false,
        last_launched_at: None,
        jvm_memory_mb: request.jvm_memory_mb.unwrap_or(4096),
        jvm_gc: request.jvm_gc.clone().unwrap_or_else(|| "auto".into()),
        jvm_custom_args: request.jvm_custom_args.clone().unwrap_or_default(),
        jvm_always_pre_touch: request.jvm_always_pre_touch.unwrap_or_else(|| {
            crate::models::recommended_java_version_for_minecraft(&request.minecraft_version) < 21
        }),
        created_at: chrono::Utc::now().to_rfc3339(),
        java_path: None,
        java_incompatible_override: false,
    }
}

fn manifest_from_request(instance_id: &str, request: &CreateInstanceRequest) -> InstanceManifest {
    InstanceManifest {
        instance_id: instance_id.into(),
        name: request.name.clone(),
        created_from_pack: None,
        minecraft_version: request.minecraft_version.clone(),
        loader: request.loader.clone(),
        loader_version: request.loader_version.clone(),
        is_locked: false,
        mods: Vec::new(),
        resourcepacks: Vec::new(),
        shaders: Vec::new(),
        datapacks: Vec::new(),
        worlds: Vec::new(),
        user_preferences: serde_json::json!({}),
    }
}

fn loader_version_id(row: &InstanceRow) -> String {
    match row.loader.as_str() {
        "fabric" => format!(
            "fabric-loader-{}-{}",
            row.loader_version, row.minecraft_version
        ),
        "quilt" => format!(
            "quilt-loader-{}-{}",
            row.loader_version, row.minecraft_version
        ),
        "forge" => format!("forge-{}-{}", row.minecraft_version, row.loader_version),
        "neoforge" => format!("neoforge-{}", row.loader_version),
        _ => row.minecraft_version.clone(),
    }
}

fn read_manifest(path: &std::path::Path) -> LauncherResult<Option<InstanceManifest>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path).map_err(|_| LauncherError::InstanceCreateFailed)?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|_| LauncherError::InstanceCreateFailed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation_manager::OpStatus;
    use std::path::PathBuf;

    fn context() -> (Ctx, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "agora-instance-service-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let ctx = Ctx::for_testing(root.clone());
        crate::db::init_local_state_db(&ctx.paths.local_state_db()).unwrap();
        (ctx, root)
    }

    #[test]
    fn crud_uses_core_paths_and_database() {
        let (ctx, root) = context();
        let request = CreateInstanceRequest {
            name: "Test".into(),
            instance_id: "test".into(),
            minecraft_version: "1.21".into(),
            loader: "vanilla".into(),
            loader_version: "".into(),
            jvm_memory_mb: None,
            jvm_gc: None,
            jvm_custom_args: None,
            jvm_always_pre_touch: None,
        };
        let row = prepare_row("test", &request);
        let conn = crate::db::local_state_connection(&ctx.paths.local_state_db()).unwrap();
        crate::db::upsert_instance(&conn, &row).unwrap();
        let manifest = manifest_from_request("test", &request);
        let dir = ctx.paths.instance_dir("test").unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            ctx.paths.instance_manifest("test").unwrap(),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();

        let service = InstanceService::new(ctx.clone());
        assert_eq!(service.list().unwrap().len(), 1);
        assert!(service.get("test").unwrap().is_some());
        service.lock("test").unwrap();
        let locked_detail = service.get("test").unwrap().unwrap();
        assert!(locked_detail.row.is_locked);
        assert!(locked_detail.manifest.unwrap().is_locked);
        service.rename("test", "Renamed").unwrap();
        assert_eq!(service.list().unwrap()[0].name, "Renamed");
        service.unlock("test").unwrap();
        let unlocked_detail = service.get("test").unwrap().unwrap();
        assert!(!unlocked_detail.row.is_locked);
        assert!(!unlocked_detail.manifest.unwrap().is_locked);
        assert!(crate::install_service::InstallService::new(ctx)
            .check_not_locked("test")
            .is_ok());
        service.delete("test", None).unwrap();
        assert!(service.list().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn clone_creates_new_instance_with_copied_files() {
        let (ctx, root) = context();
        let request = CreateInstanceRequest {
            name: "Original".into(),
            instance_id: "original".into(),
            minecraft_version: "1.21".into(),
            loader: "vanilla".into(),
            loader_version: "".into(),
            jvm_memory_mb: None,
            jvm_gc: None,
            jvm_custom_args: None,
            jvm_always_pre_touch: None,
        };
        let row = prepare_row("original", &request);
        let conn = crate::db::local_state_connection(&ctx.paths.local_state_db()).unwrap();
        crate::db::upsert_instance(&conn, &row).unwrap();
        drop(conn);
        // Create source directory and manifest
        let src_dir = ctx.paths.instance_dir("original").unwrap();
        std::fs::create_dir_all(src_dir.join("mods")).unwrap();
        std::fs::write(src_dir.join("mods").join("test.jar"), b"mod data").unwrap();
        std::fs::create_dir_all(src_dir.join("saves")).unwrap();
        std::fs::write(src_dir.join("saves").join("world.dat"), b"world").unwrap();
        let manifest = manifest_from_request("original", &request);
        std::fs::write(
            ctx.paths.instance_manifest("original").unwrap(),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();

        let service = InstanceService::new(ctx.clone());
        let rt = tokio::runtime::Runtime::new().unwrap();
        let clone_row = rt
            .block_on(service.clone(CloneRequest {
                source_instance_id: "original".into(),
                new_name: "Clone".into(),
                prefs: ClonePrefs::default(),
            }))
            .unwrap();

        assert_eq!(clone_row.name, "Clone");
        assert_eq!(clone_row.minecraft_version, "1.21");
        assert_eq!(clone_row.loader, "vanilla");

        let cloned_dir = ctx.paths.instance_dir("Clone").unwrap();
        assert!(cloned_dir.join("mods").join("test.jar").exists());
        assert!(cloned_dir.join("saves").join("world.dat").exists());

        let cloned_conn = crate::db::local_state_connection(&ctx.paths.local_state_db()).unwrap();
        let cloned_detail = crate::db::get_instance(&cloned_conn, "Clone").unwrap();
        assert!(cloned_detail.is_some());
        assert_eq!(cloned_detail.unwrap().name, "Clone");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn clone_fails_when_source_is_locked() {
        let (ctx, root) = context();
        let request = CreateInstanceRequest {
            name: "Locked".into(),
            instance_id: "locked".into(),
            minecraft_version: "1.21".into(),
            loader: "vanilla".into(),
            loader_version: "".into(),
            jvm_memory_mb: None,
            jvm_gc: None,
            jvm_custom_args: None,
            jvm_always_pre_touch: None,
        };
        let row = prepare_row("locked", &request);
        let conn = crate::db::local_state_connection(&ctx.paths.local_state_db()).unwrap();
        crate::db::upsert_instance(&conn, &row).unwrap();
        crate::db::set_locked(&conn, "locked", true).unwrap();
        drop(conn);
        let src_dir = ctx.paths.instance_dir("locked").unwrap();
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::write(
            ctx.paths.instance_manifest("locked").unwrap(),
            serde_json::to_vec(&manifest_from_request("locked", &request)).unwrap(),
        )
        .unwrap();

        let service = InstanceService::new(ctx);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt
            .block_on(service.clone(CloneRequest {
                source_instance_id: "locked".into(),
                new_name: "Clone".into(),
                prefs: ClonePrefs::default(),
            }))
            .unwrap_err();
        assert_eq!(err.code(), "ERR_INSTANCE_LOCKED");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn delete_registers_and_completes_operation() {
        let (ctx, root) = context();
        let op_mgr = ctx.operation_manager.clone();
        let request = CreateInstanceRequest {
            name: "DelOp".into(),
            instance_id: "delop".into(),
            minecraft_version: "1.21".into(),
            loader: "vanilla".into(),
            loader_version: "".into(),
            jvm_memory_mb: None,
            jvm_gc: None,
            jvm_custom_args: None,
            jvm_always_pre_touch: None,
        };
        let row = prepare_row("delop", &request);
        let conn = crate::db::local_state_connection(&ctx.paths.local_state_db()).unwrap();
        crate::db::upsert_instance(&conn, &row).unwrap();
        let manifest = manifest_from_request("delop", &request);
        let dir = ctx.paths.instance_dir("delop").unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            ctx.paths.instance_manifest("delop").unwrap(),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        drop(conn);

        let service = InstanceService::new(ctx);
        assert_eq!(op_mgr.active_count(), 0);

        service.delete("delop", None).unwrap();

        let all = op_mgr.list_all();
        let del_ops: Vec<_> = all
            .iter()
            .filter(|o| o.label == "Delete instance")
            .collect();
        assert_eq!(del_ops.len(), 1);
        assert_eq!(del_ops[0].status, OpStatus::Completed);

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(test)]
    mod clone_tracking {
        use super::*;

        #[test]
        fn clone_registers_and_completes_operation() {
            let (ctx, root) = context();
            let profiles_path = ctx.launcher_profiles_path.clone().unwrap();
            assert!(profiles_path.starts_with(&root));
            let op_mgr = ctx.operation_manager.clone();
            let request = CreateInstanceRequest {
                name: "Src".into(),
                instance_id: "src".into(),
                minecraft_version: "1.21".into(),
                loader: "vanilla".into(),
                loader_version: "".into(),
                jvm_memory_mb: None,
                jvm_gc: None,
                jvm_custom_args: None,
                jvm_always_pre_touch: None,
            };
            let row = prepare_row("src", &request);
            let conn = crate::db::local_state_connection(&ctx.paths.local_state_db()).unwrap();
            crate::db::upsert_instance(&conn, &row).unwrap();
            drop(conn);
            let src_dir = ctx.paths.instance_dir("src").unwrap();
            std::fs::create_dir_all(&src_dir).unwrap();
            std::fs::write(
                ctx.paths.instance_manifest("src").unwrap(),
                serde_json::to_vec(&manifest_from_request("src", &request)).unwrap(),
            )
            .unwrap();

            let service = InstanceService::new(ctx);
            let rt = tokio::runtime::Runtime::new().unwrap();
            let _clone_row = rt
                .block_on(service.clone(CloneRequest {
                    source_instance_id: "src".into(),
                    new_name: "CloneOp".into(),
                    prefs: ClonePrefs::default(),
                }))
                .unwrap();

            let all = op_mgr.list_all();
            let clone_ops: Vec<_> = all.iter().filter(|o| o.label == "Clone instance").collect();
            assert_eq!(clone_ops.len(), 1);
            assert_eq!(clone_ops[0].status, OpStatus::Completed);
            let profiles: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(&profiles_path).expect("isolated launcher profiles"),
            )
            .unwrap();
            assert!(profiles["profiles"].get("agora-CloneOp").is_some());

            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn clone_failure_registers_failed_operation() {
        let (ctx, root) = context();
        let op_mgr = ctx.operation_manager.clone();
        let service = InstanceService::new(ctx);
        let rt = tokio::runtime::Runtime::new().unwrap();
        // Source does not exist in DB — should fail.
        let err = rt
            .block_on(service.clone(CloneRequest {
                source_instance_id: "nonexistent".into(),
                new_name: "FailClone".into(),
                prefs: ClonePrefs::default(),
            }))
            .unwrap_err();
        assert_eq!(err.code(), "ERR_INSTANCE_NOT_FOUND");

        let all = op_mgr.list_all();
        let clone_ops: Vec<_> = all.iter().filter(|o| o.label == "Clone instance").collect();
        assert_eq!(clone_ops.len(), 1);
        assert!(matches!(clone_ops[0].status, OpStatus::Failed(_)));

        let _ = std::fs::remove_dir_all(root);
    }
}
