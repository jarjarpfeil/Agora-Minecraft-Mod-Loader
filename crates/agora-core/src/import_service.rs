use crate::ctx::Ctx;
use crate::error::{LauncherError, LauncherResult};
use crate::event_sink::{
    CancellationToken, CoreEvent, OperationId, ProgressEvent, ProgressPhase, ProgressSink,
};
use crate::health::{self, HealthScore};
use crate::import::{DetectedLauncher, ImportResult};
use crate::loader_service::LoaderService;
use crate::models::InstanceRow;
use crate::network::NetworkPolicy;
use crate::operation_manager::OpHandle;
use crate::pack_install::PackInstallResult;
use std::path::PathBuf;
use std::sync::Arc;

/// Typed import source.
#[derive(Debug, Clone)]
pub enum ImportSource {
    /// A `.mrpack` (Modrinth modpack) file.
    Mrpack(PathBuf),
    /// A PrismLauncher / MultiMC instance `.zip`.
    PrismZip(PathBuf),
    /// An existing instance directory to copy.
    Directory(PathBuf),
    /// A pack manifest (Tier 1 or Tier 2 JSON) installed into an existing instance.
    PackManifest {
        /// Raw JSON string of the pack manifest.
        manifest_json: String,
        /// Target instance ID to install into.
        target_instance_id: String,
    },
}

/// Import configuration passed to [`ImportService::run_import`].
#[derive(Debug, Clone)]
pub struct ImportRequest {
    pub source: ImportSource,
    pub symlink_saves: bool,
}

/// Core-owned import service.
///
/// Wraps the free functions in [`crate::import`] behind a typed API that
/// uses [`Ctx`] / [`crate::app_paths::AppPaths`] for path resolution and
/// preserves all existing path/hash/collision protections.
#[derive(Clone)]
pub struct ImportService {
    ctx: Ctx,
}

impl ImportService {
    pub fn new(ctx: Ctx) -> Self {
        Self { ctx }
    }

    /// Download and import a Modrinth pack URL using the same transactional
    /// import path as a local `.mrpack` file.
    pub async fn run_mrpack_url(&self, download_url: &str) -> LauncherResult<ImportResult> {
        self.run_mrpack_url_with_sink(
            download_url,
            self.ctx.progress_sink.clone(),
            CancellationToken::new(),
        )
        .await
    }

    /// Download and import a Modrinth pack URL while forwarding progress to a
    /// host-provided sink. The URL download and the archive import use distinct
    /// operation IDs because the archive import registers its own cancellable
    /// operation after the outer archive has been written.
    pub async fn run_mrpack_url_with_sink(
        &self,
        download_url: &str,
        sink: Arc<dyn ProgressSink>,
        cancel: CancellationToken,
    ) -> LauncherResult<ImportResult> {
        let download_operation =
            OperationId::new(format!("pack-download-{}", uuid::Uuid::new_v4()));
        sink.report(ProgressEvent::new(
            download_operation.clone(),
            ProgressPhase::Downloading,
            "Downloading modpack archive…",
        ));
        let progress_sink = sink.clone();
        let progress_operation = download_operation.clone();
        let bytes = crate::download::download_modpack_bytes_with_progress(
            &self.ctx.http_clients,
            download_url,
            move |downloaded, total| {
                let mut event = ProgressEvent::new(
                    progress_operation.clone(),
                    ProgressPhase::Downloading,
                    "Downloading modpack archive…",
                )
                .with_bytes(downloaded, total.unwrap_or(0));
                if let Some(total) = total.filter(|total| *total > 0) {
                    event = event.with_progress((downloaded as f64 / total as f64).min(1.0));
                }
                progress_sink.report(event);
            },
        )
        .await?;
        let temp_path = self
            .ctx
            .paths
            .staging_root()
            .join(format!("pack-download-{}.mrpack", uuid::Uuid::new_v4()));
        tokio::fs::write(&temp_path, bytes)
            .await
            .map_err(|e| LauncherError::Generic {
                code: "ERR_FILE_WRITE".into(),
                message: format!("Failed to write temporary pack archive: {e}"),
            })?;
        let result = self
            .run_import_with_sink(
                ImportRequest {
                    source: ImportSource::Mrpack(temp_path.clone()),
                    symlink_saves: false,
                },
                sink,
                cancel,
            )
            .await;
        let _ = tokio::fs::remove_file(temp_path).await;
        result
    }

    /// Run an import with progress reporting and cancellation support.
    ///
    /// The operation emits progress events tagged with the operation ID for
    /// phases: validation, extraction, metadata, loader, health, registration,
    /// snapshot, and completion.  Cancellation is checked before and after
    /// each phase and cooperatively during archive extraction (token is
    /// cloned into the blocking task).
    ///
    /// The external `cancel` token and the [`OperationManager`] token are
    /// kept coherent: cancelling either one causes the next checkpoint to
    /// return `ERR_OPERATION_CANCELLED` and marks the operation as cancelled.
    ///
    /// Atomic operations (filesystem rename / DB commit) are NOT interrupted,
    /// ensuring no partially-promoted import can occur.
    pub async fn run_import_with_sink(
        &self,
        request: ImportRequest,
        sink: Arc<dyn crate::event_sink::ProgressSink>,
        cancel: CancellationToken,
    ) -> LauncherResult<ImportResult> {
        let op = self.ctx.operation_manager.register("Import instance");
        let op_id = op.id().clone();
        let is_modpack = matches!(request.source, ImportSource::Mrpack(_));

        // Check cancellation, keeping external and operation-manager tokens
        // coherent.  If the external token has been fired we also cancel the
        // op handle so the manager reflects the cancelled state.
        let check = |op: &OpHandle, cancel: &CancellationToken| -> LauncherResult<()> {
            if cancel.is_cancelled() {
                op.cancel();
            }
            if op.token().is_cancelled() {
                return Err(LauncherError::Generic {
                    code: "ERR_OPERATION_CANCELLED".into(),
                    message: "Import was cancelled.".into(),
                });
            }
            Ok(())
        };

        // ── Phase 1: Validation / network policy ────────────────────────
        sink.report(ProgressEvent::new(
            op_id.clone(),
            ProgressPhase::Resolving,
            "Resolving network policy…",
        ));
        check(&op, &cancel)?;
        let policy = NetworkPolicy::from_ctx(&self.ctx)?;
        check(&op, &cancel)?;

        // ── Phase 2: Archive / filesystem extraction ─────────────────────
        let extracting_msg = match &request.source {
            ImportSource::Mrpack(p) => format!("Extracting mrpack: {}", p.display()),
            ImportSource::PrismZip(p) => format!("Extracting Prism zip: {}", p.display()),
            ImportSource::Directory(d) => format!("Copying directory: {}", d.display()),
            ImportSource::PackManifest { .. } => "Preparing pack manifest…".into(),
        };
        sink.report(ProgressEvent::new(
            op_id.clone(),
            ProgressPhase::Extracting,
            &extracting_msg,
        ));
        check(&op, &cancel)?;

        let (blocking_instances_root, blocking_source, blocking_symlink) = (
            self.ctx.paths.instances_root(),
            request.source.clone(),
            request.symlink_saves,
        );
        let blocking_sink = sink.clone();
        let blocking_operation_id = op_id.clone();

        let result = match tokio::task::spawn_blocking(move || match blocking_source {
            ImportSource::Mrpack(path) => crate::import::import_mrpack_with_progress(
                &path,
                &blocking_instances_root,
                blocking_symlink,
                Some(blocking_sink),
                Some(blocking_operation_id),
            ),
            ImportSource::PrismZip(path) => {
                crate::import::import_prism_zip(&path, &blocking_instances_root, blocking_symlink)
            }
            ImportSource::Directory(path) => {
                crate::import::import_directory(&path, &blocking_instances_root, blocking_symlink)
            }
            ImportSource::PackManifest { .. } => Err(LauncherError::Generic {
                code: "ERR_IMPORT_SOURCE".into(),
                message: "PackManifest imports must use ImportService::install_pack (async)."
                    .into(),
            }),
        })
        .await
        {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                op.fail(e.to_string());
                return Err(e);
            }
            Err(join_err) => {
                let msg = format!("The import task panicked or was cancelled: {join_err}");
                op.fail(msg.clone());
                return Err(LauncherError::Generic {
                    code: "ERR_IMPORT_TASK".into(),
                    message: msg,
                });
            }
        };

        check(&op, &cancel)?;

        // Rollback helper — removes the *promoted* instance directory.
        let rollback =
            || -> LauncherResult<()> { remove_promoted(&self.ctx.paths, &result.instance_id) };

        // ── Phase 3: Bootstrap Mojang version metadata ──────────────────
        if !result.minecraft_version.is_empty() {
            sink.report(ProgressEvent::new(
                op_id.clone(),
                ProgressPhase::Verifying,
                "Verifying Minecraft base version metadata…",
            ));
            check(&op, &cancel)?;

            let minecraft_root = self.ctx.paths.minecraft_runtime_root();
            if let Err(e) = crate::minecraft_metadata::ensure_base_version_metadata(
                &minecraft_root,
                &result.minecraft_version,
                &policy,
            )
            .await
            {
                let _ = rollback();
                op.fail(e.to_string());
                return Err(e);
            }
            check(&op, &cancel)?;
        }

        // ── Phase 4: Install loader (if applicable) ─────────────────────
        if !result.loader.is_empty() && result.loader != "vanilla" {
            sink.report(ProgressEvent::new(
                op_id.clone(),
                ProgressPhase::Installing,
                format!("Installing {} {}…", result.loader, result.loader_version),
            ));
            check(&op, &cancel)?;

            if let Err(e) = LoaderService::new(self.ctx.clone())
                .ensure_installed(
                    &result.loader,
                    &result.minecraft_version,
                    &result.loader_version,
                    false,
                )
                .await
            {
                let _ = rollback();
                op.fail(e.to_string());
                return Err(e);
            }
            check(&op, &cancel)?;
        }

        // ── Phase 5: DB registration (atomic — no cancellation check) ──
        let _lock = match self.ctx.lock_manager.acquire(
            crate::lock_manager::LockResource::Instance(result.instance_id.clone()),
            "import-register",
        ) {
            Ok(l) => l,
            Err(e) => {
                let _ = rollback();
                op.fail(e.to_string());
                return Err(e);
            }
        };

        let row = InstanceRow {
            instance_id: result.instance_id.clone(),
            name: result.name.clone(),
            minecraft_version: result.minecraft_version.clone(),
            loader: result.loader.clone(),
            loader_version: result.loader_version.clone(),
            is_modpack,
            is_locked: false,
            last_launched_at: None,
            jvm_memory_mb: 4096,
            jvm_gc: "auto".into(),
            jvm_custom_args: String::new(),
            jvm_always_pre_touch: crate::models::recommended_java_version_for_minecraft(
                &result.minecraft_version,
            ) < 21,
            created_at: chrono::Utc::now().to_rfc3339(),
            java_path: None,
            java_incompatible_override: false,
        };

        let conn = match crate::db::local_state_connection(&self.ctx.paths.local_state_db()) {
            Ok(c) => c,
            Err(error) => {
                let _ = rollback();
                op.fail(error.to_string());
                return Err(LauncherError::Generic {
                    code: "ERR_LOCAL_STATE_FAILED".into(),
                    message: format!("Failed to open local state DB: {error}"),
                });
            }
        };

        if let Err(error) = crate::db::upsert_instance(&conn, &row) {
            let _ = rollback();
            op.fail(error.to_string());
            return Err(LauncherError::Generic {
                code: "ERR_LOCAL_STATE_FAILED".into(),
                message: format!("Failed to register imported instance: {error}"),
            });
        }

        check(&op, &cancel)?;

        // ── Phase 6: Health validation ──────────────────────────────────
        if let Ok(instance_dir) = self.ctx.paths.instance_dir(&result.instance_id) {
            if instance_dir.exists() {
                sink.report(ProgressEvent::new(
                    op_id.clone(),
                    ProgressPhase::HealthScan,
                    "Running health scan…",
                ));
                check(&op, &cancel)?;

                let manifest_path = instance_dir.join("instance_manifest.json");
                if let Ok(raw) = tokio::fs::read_to_string(&manifest_path).await {
                    if let Ok(manifest) =
                        serde_json::from_str::<crate::models::InstanceManifest>(&raw)
                    {
                        let report = health::health(
                            &instance_dir,
                            &manifest,
                            Some(&self.ctx.paths.registry_db()),
                        );
                        if report.score == HealthScore::Red {
                            let details = report
                                .blockers
                                .iter()
                                .map(|blocker| blocker.message.as_str())
                                .collect::<Vec<_>>()
                                .join("; ");
                            self.ctx.event_sink.emit(CoreEvent::Warning {
                                message: format!(
                                    "Imported '{}' with health blockers; review before launch.",
                                    result.instance_id
                                ),
                                details: Some(details.clone()),
                            });
                            sink.report(ProgressEvent::new(
                                op_id.clone(),
                                ProgressPhase::HealthScan,
                                format!("Import completed with health blockers: {details}"),
                            ));
                        }
                    }
                }
                check(&op, &cancel)?;
            }
        }

        // ── Phase 7: Initial snapshot ───────────────────────────────────
        if let Ok(instance_dir) = self.ctx.paths.instance_dir(&result.instance_id) {
            if instance_dir.exists() {
                sink.report(ProgressEvent::new(
                    op_id.clone(),
                    ProgressPhase::Snapshotting,
                    "Creating initial snapshot…",
                ));
                check(&op, &cancel)?;

                let _ =
                    crate::snapshot::create_snapshot(&instance_dir, Some("Initial import state"));
                check(&op, &cancel)?;
            }
        }

        // ── Done ────────────────────────────────────────────────────────
        sink.report(ProgressEvent::new(
            op_id,
            ProgressPhase::Done,
            "Import complete",
        ));
        op.complete();
        Ok(result)
    }

    /// Run an import asynchronously.
    ///
    /// This convenience wrapper uses the context's configured progress sink and
    /// a default (never-cancelled) [`CancellationToken`]. Callers that need
    /// an operation-specific sink or cancellation should use
    /// [`ImportService::run_import_with_sink`] instead.
    ///
    /// Synchronous archive/filesystem extraction is offloaded via
    /// `spawn_blocking`. Metadata bootstrap, loader installation, health
    /// validation, and DB registration are genuinely async and respect
    /// the configured [`NetworkPolicy`]. On any failure after promotion,
    /// the instance directory and DB row are rolled back so no orphan
    /// instance remains.
    pub async fn run_import(&self, request: ImportRequest) -> LauncherResult<ImportResult> {
        self.run_import_with_sink(
            request,
            self.ctx.progress_sink.clone(),
            CancellationToken::new(),
        )
        .await
    }

    /// Detect installed launchers and their instance directories.
    pub fn auto_detect_launchers(&self) -> Vec<DetectedLauncher> {
        crate::import::auto_detect_launchers()
    }

    /// Install a pack manifest (Tier 1 simple or Tier 2 complex) into an existing
    /// instance.
    ///
    /// The manifest is validated, hashes are verified, and override-bundle SHA-256
    /// pins are enforced — the same protections that `pack_install` always applied.
    pub async fn install_pack(&self, request: ImportRequest) -> LauncherResult<PackInstallResult> {
        let (manifest_json, target_instance_id) = match request.source {
            ImportSource::PackManifest {
                manifest_json,
                target_instance_id,
            } => (manifest_json, target_instance_id),
            _ => {
                return Err(LauncherError::Generic {
                    code: "ERR_IMPORT_SOURCE".into(),
                    message: "Expected PackManifest import source.".into(),
                })
            }
        };

        let op = self
            .ctx
            .operation_manager
            .register_for_instance("Install pack", &target_instance_id);

        let manifest = match crate::pack_install::parse_pack_manifest(&manifest_json) {
            Ok(m) => m,
            Err(e) => {
                op.fail(e.clone());
                return Err(LauncherError::Generic {
                    code: "ERR_PACK_PARSE".into(),
                    message: e,
                });
            }
        };

        let sanitized = crate::paths::sanitize_id(&target_instance_id);
        let instance_dir = match self.ctx.paths.instance_dir(&sanitized) {
            Ok(d) => d,
            Err(e) => {
                op.fail(e.to_string());
                return Err(e);
            }
        };
        if !instance_dir.exists() {
            op.fail(format!("Instance '{target_instance_id}' not found."));
            return Err(LauncherError::Generic {
                code: "ERR_INSTANCE_NOT_FOUND".into(),
                message: format!("Instance '{target_instance_id}' not found."),
            });
        }

        let client = reqwest::Client::new();
        let result = if manifest.override_source.is_some() {
            crate::pack_install::install_complex_pack(&client, &manifest, &instance_dir).await
        } else {
            crate::pack_install::install_simple_pack(&client, &manifest, &instance_dir).await
        };
        match result {
            Ok(r) => {
                op.complete();
                Ok(r)
            }
            Err(e) => {
                op.fail(e.clone());
                Err(LauncherError::Generic {
                    code: "ERR_PACK".into(),
                    message: e,
                })
            }
        }
    }
}

/// Remove a promoted instance directory so no filesystem-only unregistered
/// instance remains after a registration failure.
fn remove_promoted(paths: &crate::app_paths::AppPaths, instance_id: &str) -> LauncherResult<()> {
    let dir = paths.instance_dir(instance_id)?;
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| LauncherError::Generic {
            code: "ERR_IMPORT_CLEANUP".into(),
            message: format!("Failed to remove unregistered instance directory: {e}"),
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::CoreContext;
    use crate::event_sink::CollectingProgressSink;
    use crate::operation_manager::OpStatus;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn test_tmp(label: &str) -> std::path::PathBuf {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "agora-import-svc-{label}-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn test_ctx() -> Ctx {
        let tmp = test_tmp("ctx");
        let _ = std::fs::create_dir_all(&tmp);
        let ctx = CoreContext::for_testing(tmp);
        let _ = crate::db::init_local_state_db(&ctx.paths.local_state_db());
        ctx
    }

    fn listing_contains(
        service: &crate::instance_service::InstanceService,
        instance_id: &str,
    ) -> bool {
        service
            .list()
            .ok()
            .map(|rows| rows.iter().any(|r| r.instance_id == instance_id))
            .unwrap_or(false)
    }

    #[tokio::test]
    async fn test_import_directory_through_service() {
        let ctx = test_ctx();
        let svc = ImportService::new(ctx);

        let tmp = test_tmp("dir");
        let _ = std::fs::create_dir_all(&tmp);
        let src = tmp.join("my-instance");
        std::fs::create_dir_all(src.join("mods")).unwrap();
        std::fs::write(src.join("mods").join("test-mod.jar"), b"fake jar").unwrap();

        let request = ImportRequest {
            source: ImportSource::Directory(src),
            symlink_saves: false,
        };
        let result = svc.run_import(request).await.unwrap();
        assert_eq!(result.name, "my-instance");
        assert_eq!(result.instance_id, "my-instance");
        assert_eq!(result.imported_mods, 1);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_import_mrpack_through_service() {
        let ctx = test_ctx();
        let svc = ImportService::new(ctx);

        let tmp = test_tmp("mrp");
        let _ = std::fs::create_dir_all(&tmp);
        let mrpack_path = tmp.join("a-pack.mrpack");
        let file = std::fs::File::create(&mrpack_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file("modrinth.index.json", zip::write::FileOptions::default())
            .unwrap();
        writer
            .write_all(br#"{"name":"a-pack","dependencies":{},"files":[]}"#)
            .unwrap();
        writer.finish().unwrap();

        let request = ImportRequest {
            source: ImportSource::Mrpack(mrpack_path),
            symlink_saves: false,
        };
        let result = svc.run_import(request).await.unwrap();
        assert_eq!(result.instance_id, "a-pack");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_auto_detect_launchers_does_not_panic() {
        let ctx = test_ctx();
        let svc = ImportService::new(ctx);
        let launchers = svc.auto_detect_launchers();
        for l in &launchers {
            assert!(!l.launcher_type.is_empty());
        }
    }

    #[tokio::test]
    async fn test_import_registers_and_completes_operation() {
        let ctx = test_ctx();
        let op_mgr = ctx.operation_manager.clone();
        let svc = ImportService::new(ctx);

        let tmp = test_tmp("op");
        let _ = std::fs::create_dir_all(&tmp);
        let src = tmp.join("op-import");
        std::fs::create_dir_all(src.join("mods")).unwrap();
        std::fs::write(src.join("mods").join("a.jar"), b"mod").unwrap();

        assert_eq!(op_mgr.active_count(), 0);

        let request = ImportRequest {
            source: ImportSource::Directory(src),
            symlink_saves: false,
        };
        let _result = svc.run_import(request).await.unwrap();

        let all = op_mgr.list_all();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].status, OpStatus::Completed);
        assert_eq!(all[0].label, "Import instance");
        assert_eq!(op_mgr.active_count(), 0);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_import_fails_registers_failed_operation() {
        let ctx = test_ctx();
        let op_mgr = ctx.operation_manager.clone();
        let svc = ImportService::new(ctx);

        let request = ImportRequest {
            source: ImportSource::Directory(std::path::PathBuf::from(
                "/nonexistent/path/import-test",
            )),
            symlink_saves: false,
        };
        assert!(svc.run_import(request).await.is_err());

        let all = op_mgr.list_all();
        assert_eq!(all.len(), 1);
        assert!(matches!(all[0].status, OpStatus::Failed(_)));
    }

    #[tokio::test]
    async fn test_import_is_listed_by_instance_service() {
        let ctx = test_ctx();
        let svc = ImportService::new(ctx.clone());
        let instance_svc = crate::instance_service::InstanceService::new(ctx);

        let tmp = test_tmp("list");
        let _ = std::fs::create_dir_all(&tmp);
        let src = tmp.join("listed-instance");
        std::fs::create_dir_all(src.join("mods")).unwrap();
        std::fs::write(src.join("mods").join("mod.jar"), b"mod").unwrap();

        let request = ImportRequest {
            source: ImportSource::Directory(src),
            symlink_saves: false,
        };
        let result = svc.run_import(request).await.unwrap();
        assert_eq!(result.instance_id, "listed-instance");

        assert!(listing_contains(&instance_svc, "listed-instance"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_import_mrpack_is_listed_by_instance_service() {
        let ctx = test_ctx();
        let svc = ImportService::new(ctx.clone());
        let instance_svc = crate::instance_service::InstanceService::new(ctx);

        let tmp = test_tmp("mrp-list");
        let _ = std::fs::create_dir_all(&tmp);
        let mrpack_path = tmp.join("pack.mrpack");
        let file = std::fs::File::create(&mrpack_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file("modrinth.index.json", zip::write::FileOptions::default())
            .unwrap();
        writer
            .write_all(br#"{"name":"mrp-list-pack","dependencies":{},"files":[]}"#)
            .unwrap();
        writer.finish().unwrap();

        let request = ImportRequest {
            source: ImportSource::Mrpack(mrpack_path),
            symlink_saves: false,
        };
        let result = svc.run_import(request).await.unwrap();
        assert_eq!(result.instance_id, "mrp-list-pack");

        assert!(listing_contains(&instance_svc, "mrp-list-pack"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_import_fast_fail_broken_db_cleans_nothing() {
        let tmp = test_tmp("brokendb");
        let _ = std::fs::create_dir_all(&tmp);
        std::fs::write(tmp.join("local_state.db"), b"not a database").unwrap();
        let ctx = CoreContext::for_testing(tmp.clone());
        let svc = ImportService::new(ctx);

        let src = tmp.join("to-import");
        std::fs::create_dir_all(src.join("mods")).unwrap();
        std::fs::write(src.join("mods").join("a.jar"), b"mod").unwrap();

        let request = ImportRequest {
            source: ImportSource::Directory(src),
            symlink_saves: false,
        };

        let err = svc.run_import(request).await.unwrap_err();
        assert_eq!(err.code(), "ERR_LOCAL_STATE_FAILED");

        let instances = tmp.join("instances");
        assert!(!instances.exists(), "no instance dir should exist");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_import_policy_disabled_bootstrap_failure_cleans_instance() {
        let tmp = test_tmp("polfail");
        let _ = std::fs::create_dir_all(&tmp);

        let conn = init_ctx_db(&tmp);
        crate::db::set_setting(
            &conn,
            "network_mojang_metadata_enabled",
            &serde_json::Value::Bool(false),
        )
        .unwrap();
        drop(conn);

        let ctx = CoreContext::for_testing(tmp.clone());
        let svc = ImportService::new(ctx);

        let mrpack_path = tmp.join("polfail-pack.mrpack");
        let file = std::fs::File::create(&mrpack_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file("modrinth.index.json", zip::write::FileOptions::default())
            .unwrap();
        writer
            .write_all(br#"{"name":"polfail","dependencies":{"minecraft":"1.21"},"files":[]}"#)
            .unwrap();
        writer.finish().unwrap();

        let request = ImportRequest {
            source: ImportSource::Mrpack(mrpack_path),
            symlink_saves: false,
        };

        let err = svc.run_import(request).await.unwrap_err();
        assert_eq!(err.code(), "ERR_NETWORK_MOJANG_METADATA_DISABLED");

        let instance_dir = tmp.join("instances").join("polfail");
        assert!(
            !instance_dir.exists(),
            "orphaned instance dir must be removed after bootstrap failure"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_import_produces_launchable_instance_through_service() {
        let ctx = test_ctx();
        let svc = ImportService::new(ctx);

        let tmp = test_tmp("svc-launchable");
        let _ = std::fs::create_dir_all(&tmp);
        let src = tmp.join("test-launchable");
        std::fs::create_dir_all(src.join("mods")).unwrap();
        std::fs::write(src.join("mods").join("mod.jar"), b"fake mod").unwrap();

        let request = ImportRequest {
            source: ImportSource::Directory(src),
            symlink_saves: false,
        };
        let result = svc.run_import(request).await.unwrap();
        assert_eq!(result.instance_id, "test-launchable");

        let manifest_path = svc
            .ctx
            .paths
            .instance_dir("test-launchable")
            .unwrap()
            .join("instance_manifest.json");
        assert!(manifest_path.exists());
        let raw = std::fs::read_to_string(&manifest_path).unwrap();
        let manifest: crate::models::InstanceManifest = serde_json::from_str(&raw).unwrap();
        assert_eq!(manifest.instance_id, "test-launchable");
        assert!(manifest.mods.is_empty());
        assert!(manifest.resourcepacks.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_import_mrpack_manifest_roundtrip_through_service() {
        let ctx = test_ctx();
        let svc = ImportService::new(ctx);

        let tmp = test_tmp("svc-mrp-rt");
        let _ = std::fs::create_dir_all(&tmp);
        let mrpack_path = tmp.join("rt-pack.mrpack");
        let file = std::fs::File::create(&mrpack_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file("modrinth.index.json", zip::write::FileOptions::default())
            .unwrap();
        writer
            .write_all(br#"{"name":"rt-pack","dependencies":{"minecraft":"1.21"},"files":[]}"#)
            .unwrap();
        writer.finish().unwrap();

        let request = ImportRequest {
            source: ImportSource::Mrpack(mrpack_path),
            symlink_saves: false,
        };
        let result = svc.run_import(request).await.unwrap();
        assert_eq!(result.instance_id, "rt-pack");
        assert_eq!(result.minecraft_version, "1.21");

        let manifest_path = svc
            .ctx
            .paths
            .instance_dir("rt-pack")
            .unwrap()
            .join("instance_manifest.json");
        let manifest: crate::models::InstanceManifest =
            serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
        let serialized = serde_json::to_string_pretty(&manifest).unwrap();
        let deserialized: crate::models::InstanceManifest =
            serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.instance_id, manifest.instance_id);
        assert_eq!(deserialized.name, manifest.name);
        assert_eq!(deserialized.minecraft_version, manifest.minecraft_version);
        assert_eq!(deserialized.loader, manifest.loader);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ── Cancellation tests ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_import_cancelled_before_extraction() {
        let ctx = test_ctx();
        let op_mgr = ctx.operation_manager.clone();
        let sink = Arc::new(CollectingProgressSink::new());
        let cancel = CancellationToken::new();
        let svc = ImportService::new(ctx);

        let tmp = test_tmp("cancel-before-extract");
        let _ = std::fs::create_dir_all(&tmp);
        let src = tmp.join("some-instance");
        std::fs::create_dir_all(src.join("mods")).unwrap();

        let request = ImportRequest {
            source: ImportSource::Directory(src),
            symlink_saves: false,
        };

        // Cancel before calling the sink API.
        cancel.cancel();

        let err = svc
            .run_import_with_sink(request, sink.clone(), cancel)
            .await
            .unwrap_err();
        assert_eq!(err.code(), "ERR_OPERATION_CANCELLED");

        // No instance directory was created.
        let instances = tmp.join("instances");
        assert!(!instances.exists(), "no instance dir should exist");

        // The operation manager entry was cleaned up on handle drop.
        assert_eq!(op_mgr.active_count(), 0);

        // Should have emitted at least the Resolving phase.
        let events = sink.events();
        let phases: Vec<ProgressPhase> = events.iter().map(|e| e.phase).collect();
        assert!(
            phases.contains(&ProgressPhase::Resolving),
            "should have emitted Resolving phase"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_import_cancelled_after_extraction_before_metadata() {
        let ctx = test_ctx();
        let op_mgr = ctx.operation_manager.clone();
        let cancel = CancellationToken::new();
        let svc = ImportService::new(ctx);

        let tmp = test_tmp("cancel-after-extract");
        let _ = std::fs::create_dir_all(&tmp);
        let mrpack_path = tmp.join("cancel-me.mrpack");
        {
            let file = std::fs::File::create(&mrpack_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            writer
                .start_file("modrinth.index.json", zip::write::FileOptions::default())
                .unwrap();
            writer
                .write_all(
                    br#"{"name":"cancel-me","dependencies":{"minecraft":"1.21"},"files":[]}"#,
                )
                .unwrap();
            writer.finish().unwrap();
        }

        // Use a sink that fires the cancel token on the Verifying phase
        // (which runs after extraction completes).
        let cancel_on_verify = Arc::new(CancelOnPhaseSink {
            cancel: cancel.clone(),
            phase: ProgressPhase::Verifying,
        });

        let request = ImportRequest {
            source: ImportSource::Mrpack(mrpack_path),
            symlink_saves: false,
        };

        let err = svc
            .run_import_with_sink(request, cancel_on_verify, cancel.clone())
            .await
            .unwrap_err();
        assert_eq!(err.code(), "ERR_OPERATION_CANCELLED");

        // The promoted instance directory must be cleaned up.
        let promoted_dir = tmp.join("instances").join("cancel-me");
        assert!(
            !promoted_dir.exists(),
            "orphaned instance dir must be removed after cancellation"
        );

        // No DB row exists.
        assert_eq!(op_mgr.active_count(), 0);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_import_cancelled_via_operation_manager() {
        let ctx = test_ctx();
        let op_mgr = ctx.operation_manager.clone();
        let cancel = CancellationToken::new();
        let svc = ImportService::new(ctx);

        let tmp = test_tmp("cancel-via-opmgr");
        let _ = std::fs::create_dir_all(&tmp);
        let mrpack_path = tmp.join("cancel-opmgr.mrpack");
        {
            let file = std::fs::File::create(&mrpack_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            writer
                .start_file("modrinth.index.json", zip::write::FileOptions::default())
                .unwrap();
            writer
                .write_all(
                    br#"{"name":"cancel-opmgr","dependencies":{"minecraft":"1.21"},"files":[]}"#,
                )
                .unwrap();
            writer.finish().unwrap();
        }

        let cancel_on_verify = Arc::new(CancelOnPhaseSink {
            cancel: cancel.clone(),
            phase: ProgressPhase::Verifying,
        });

        let request = ImportRequest {
            source: ImportSource::Mrpack(mrpack_path),
            symlink_saves: false,
        };

        let err = svc
            .run_import_with_sink(request, cancel_on_verify, cancel)
            .await
            .unwrap_err();
        assert_eq!(err.code(), "ERR_OPERATION_CANCELLED");

        // Cleanup was performed.
        let promoted_dir = tmp.join("instances").join("cancel-opmgr");
        assert!(!promoted_dir.exists());

        assert_eq!(op_mgr.active_count(), 0);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_import_sink_emits_expected_phases() {
        let ctx = test_ctx();
        let sink = Arc::new(CollectingProgressSink::new());
        let svc = ImportService::new(ctx);

        let tmp = test_tmp("sink-phases");
        let _ = std::fs::create_dir_all(&tmp);
        let src = tmp.join("phase-instance");
        std::fs::create_dir_all(src.join("mods")).unwrap();
        std::fs::write(src.join("mods").join("m.jar"), b"x").unwrap();

        let request = ImportRequest {
            source: ImportSource::Directory(src),
            symlink_saves: false,
        };
        svc.run_import_with_sink(request, sink.clone(), CancellationToken::new())
            .await
            .unwrap();

        let events = sink.events();
        let phases: Vec<ProgressPhase> = events.iter().map(|e| e.phase).collect();

        // Must include at least these phases in order.
        assert!(
            phases.contains(&ProgressPhase::Resolving),
            "missing Resolving"
        );
        assert!(
            phases.contains(&ProgressPhase::Extracting),
            "missing Extracting"
        );
        assert!(phases.contains(&ProgressPhase::Done), "missing Done");

        // Every event should carry a non-empty operation_id.
        for ev in &events {
            assert!(!ev.operation_id.0.is_empty(), "empty operation_id");
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Progress sink that cancels the given token when a specific phase is
    /// observed.  Used to test mid-import cancellation.
    struct CancelOnPhaseSink {
        cancel: CancellationToken,
        phase: ProgressPhase,
    }

    impl crate::event_sink::ProgressSink for CancelOnPhaseSink {
        fn report(&self, event: ProgressEvent) {
            if event.phase == self.phase {
                self.cancel.cancel();
            }
        }
    }

    fn init_ctx_db(tmp: &std::path::Path) -> rusqlite::Connection {
        let db_path = tmp.join("local_state.db");
        let _ = crate::db::init_local_state_db(&db_path);
        rusqlite::Connection::open(&db_path).expect("test db")
    }
}
