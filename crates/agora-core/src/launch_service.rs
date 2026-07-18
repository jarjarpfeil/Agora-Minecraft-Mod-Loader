//! Core-owned launch orchestration for both direct (attached Java) and
//! delegated (external launcher) modes.

use crate::ctx::Ctx;
use crate::error::{LauncherError, LauncherResult};
use crate::java::JavaInstallation;
use crate::launch::LoaderInfo;
use crate::launch_planner::{BuildCommandRequest, LaunchFeatures, LaunchIdentity, ResolveRequest};
use crate::lkg::LaunchOutcome;
use crate::models::InstanceManifest;
use crate::network::NetworkPolicy;
use crate::process_identity::ProcessIdentity;
use crate::process_session_manager::ProcessSession;
use crate::runtime_manager::RuntimeProgress;
use crate::snapshot::{create_snapshot, live_file_index, snapshot_file_index};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

/// Whether the service should directly execute Java or hand off to an
/// external launcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    Direct,
    Delegated,
}

/// Coarse recovery action requested by the frontend before a retry launch.
/// The action is performed in the same backend operation; if it fails the
/// launch is aborted and the error is returned to the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LaunchRecoveryAction {
    /// No recovery — plain launch.
    None,
    /// Provision a managed Java runtime for the given major version, then
    /// retry the launch.
    ProvisionJava { major: u32 },
    /// Force-reinstall the instance's loader (repair), then retry the launch.
    RepairLoader,
}

/// Health behavior selected by the frontend or CLI policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthPolicy {
    BlockOnRed,
    WarnOnly,
}

/// Runtime provisioning policy used when the exact Java major is unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JavaRuntimeMode {
    Automatic,
    Manual,
}

/// UI-neutral launch progress hook. Adapters receive lifecycle callbacks
/// during both direct and delegated launches. The [`handoff`] method is
/// called in Delegated mode so the adapter can invoke the external launcher.
pub trait LaunchProgress: Send + Sync {
    fn phase(&self, _name: &str, _message: &str) {}
    fn started(&self, _started: &LaunchStarted) {}
    fn log(&self, _stream: &str, _line: &str) {}
    fn finished(&self, _result: &LaunchResult) {}
    /// Called when a delegated launch is ready. The adapter must invoke
    /// the external Mojang launcher and return `Ok(())`. The default
    /// returns an error indicating delegated launch is unsupported.
    fn handoff(&self, _identity: &LaunchIdentity) -> LauncherResult<()> {
        Err(LauncherError::Generic {
            code: "ERR_DELEGATED_LAUNCH_ADAPTER_REQUIRED".into(),
            message: "Delegated launch not supported by this adapter.".into(),
        })
    }
}

/// No-op progress implementation for callers that only need the result.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopLaunchProgress;

impl LaunchProgress for NoopLaunchProgress {}

/// Frontend-neutral request for a complete launch operation.
#[derive(Debug, Clone)]
pub struct LaunchRequest {
    pub instance_id: String,
    pub mode: LaunchMode,
    pub health_policy: HealthPolicy,
}

/// State loaded and normalized by [`LaunchService`] before execution.
#[derive(Debug, Clone)]
struct LaunchInputs {
    mode: LaunchMode,
    instance_id: String,
    health_policy: HealthPolicy,
    java_runtime_mode: JavaRuntimeMode,
    manifest: InstanceManifest,
    game_dir: PathBuf,
    minecraft_root: PathBuf,
    assets_dir: PathBuf,
    runtimes_root: PathBuf,
    receipts_root: PathBuf,
    registry_db: Option<PathBuf>,
    network_policy: NetworkPolicy,
    identity: LaunchIdentity,
    java_override: Option<PathBuf>,
    allow_incompatible_java_override: bool,
    java_candidates: Vec<JavaInstallation>,
    jvm_memory_mb: i64,
    jvm_gc_profile: Option<crate::gc::GcProfile>,
    jvm_custom_args: String,
    extra_game_args: Vec<String>,
}

/// Process-start result delivered before the attached launch completes.
#[derive(Debug, Clone)]
pub struct LaunchStarted {
    pub pid: u32,
    pub session_id: u64,
    pub snapshot_id: String,
    pub java_path: PathBuf,
    pub process_identity: ProcessIdentity,
}

/// Result of a complete attached direct launch.
#[derive(Debug, Clone)]
pub struct LaunchResult {
    pub pid: u32,
    pub session_id: u64,
    pub outcome: LaunchOutcome,
    pub snapshot_id: String,
    pub java_path: PathBuf,
    pub process_identity: ProcessIdentity,
}

/// Core-owned launch lifecycle service.
#[derive(Clone)]
pub struct LaunchService {
    ctx: Ctx,
}

impl LaunchService {
    pub fn new(ctx: Ctx) -> Self {
        Self { ctx }
    }

    /// Execute the complete launch lifecycle once. Both Direct and Delegated
    /// modes go through the same preparation (health checks, resolution,
    /// materialization, snapshot). Direct mode spawns Java and waits for exit;
    /// Delegated mode calls [`LaunchProgress::handoff`] and returns promptly.
    /// Callers of Delegated mode should spawn a background task calling
    /// [`Self::wait_delegated`] for monitoring.
    pub async fn launch(
        &self,
        request: LaunchRequest,
        progress: &dyn LaunchProgress,
    ) -> LauncherResult<LaunchResult> {
        validate_instance_id(&request.instance_id)?;
        let _lock = self.ctx.lock_manager.acquire(
            crate::lock_manager::LockResource::Instance(request.instance_id.clone()),
            "launch",
        )?;
        let inputs = self.load_inputs(request).await?;
        self.launch_inputs(inputs, progress).await
    }

    /// Execute a launch with an optional recovery step performed before
    /// the actual launch. If the recovery action fails the launch is aborted
    /// and the error is returned. [`LaunchRecoveryAction::None`] behaves
    /// identically to [`Self::launch`].
    pub async fn launch_with_recovery(
        &self,
        request: LaunchRequest,
        action: LaunchRecoveryAction,
        progress: &dyn LaunchProgress,
    ) -> LauncherResult<LaunchResult> {
        validate_instance_id(&request.instance_id)?;
        match action {
            LaunchRecoveryAction::None => {}
            LaunchRecoveryAction::ProvisionJava { major } => {
                progress.phase("recovery", "Provisioning the required Java runtime");
                let policy = crate::network::NetworkPolicy::from_ctx(&self.ctx)?;
                policy.check(crate::network::NetworkCategory::JavaRuntime)?;
                let runtimes_root = self.ctx.paths.java_runtimes_root();
                let catalog = self.ctx.runtime_catalog.snapshot();
                let lock_manager = self.ctx.lock_manager.clone();
                tokio::task::spawn_blocking(move || {
                    crate::runtime_manager::ensure_runtime(
                        &runtimes_root,
                        major,
                        &catalog,
                        &policy,
                        None::<&dyn crate::runtime_manager::RuntimeProgress>,
                        Some(&lock_manager),
                    )
                })
                .await
                .map_err(|error| LauncherError::Generic {
                    code: "ERR_JAVA_PROVISION".into(),
                    message: format!("Java provisioning task failed: {error}"),
                })??;
            }
            LaunchRecoveryAction::RepairLoader => {
                progress.phase("recovery", "Repairing loader installation");
                let loader_svc = crate::loader_service::LoaderService::new(self.ctx.clone());
                loader_svc.repair(&request.instance_id).await?;
            }
        }
        self.launch(request, progress).await
    }

    async fn load_inputs(&self, request: LaunchRequest) -> LauncherResult<LaunchInputs> {
        let conn = crate::db::local_state_connection(&self.ctx.paths.local_state_db()).map_err(
            |error| LauncherError::Generic {
                code: "ERR_LOCAL_STATE_FAILED".into(),
                message: error.to_string(),
            },
        )?;
        let row = crate::db::get_instance(&conn, &request.instance_id)
            .map_err(|error| LauncherError::Generic {
                code: "ERR_LOCAL_STATE_FAILED".into(),
                message: error.to_string(),
            })?
            .ok_or_else(|| LauncherError::Generic {
                code: "ERR_INSTANCE_NOT_FOUND".into(),
                message: format!("Instance '{}' not found.", request.instance_id),
            })?;
        let manifest_path = self.ctx.paths.instance_manifest(&request.instance_id)?;
        let manifest_text =
            std::fs::read_to_string(&manifest_path).map_err(|error| LauncherError::Generic {
                code: "ERR_INSTANCE_MANIFEST".into(),
                message: error.to_string(),
            })?;
        let manifest: InstanceManifest =
            serde_json::from_str(&manifest_text).map_err(|error| LauncherError::Generic {
                code: "ERR_INSTANCE_MANIFEST".into(),
                message: error.to_string(),
            })?;
        let network_policy = NetworkPolicy::from_db(&conn);
        network_policy.check(crate::network::NetworkCategory::MicrosoftAuthentication)?;
        let mut credentials =
            crate::msa::load_credentials()?.ok_or(LauncherError::MsaAuthRequired)?;
        if credentials.needs_refresh() {
            credentials = crate::msa::refresh_credentials(
                self.ctx
                    .http_clients
                    .get(crate::http_client::ClientCategory::Microsoft),
                &credentials,
                &self.ctx.paths.local_state_db(),
            )
            .await
            .map_err(|error| LauncherError::Generic {
                code: "ERR_AUTH_REFRESH_FAILED".into(),
                message: error.to_string(),
            })?;
        }
        if credentials.is_expired() {
            return Err(LauncherError::AuthExpired);
        }
        let java_runtime_mode = match crate::db::get_setting(&conn, "java_runtime_mode")
            .ok()
            .flatten()
            .and_then(|value| value.as_str().map(str::to_owned))
            .as_deref()
        {
            Some("manual") => JavaRuntimeMode::Manual,
            _ => JavaRuntimeMode::Automatic,
        };
        let java_override = row
            .java_path
            .as_deref()
            .filter(|path| !path.trim().is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                crate::db::get_setting(&conn, "java_path")
                    .ok()
                    .flatten()
                    .and_then(|value| value.as_str().map(PathBuf::from))
            });
        let minecraft_root = self.ctx.paths.minecraft_runtime_root();
        let layout = crate::minecraft_runtime::ensure_runtime_layout(&minecraft_root)?;
        let jvm_gc_profile = match row.jvm_gc.to_ascii_lowercase().as_str() {
            "zgc" | "low_latency" => Some(crate::gc::GcProfile::LowLatency),
            "g1gc" | "high_efficiency" => Some(crate::gc::GcProfile::HighEfficiency),
            "manual" => Some(crate::gc::GcProfile::Manual),
            _ => None,
        };

        Ok(LaunchInputs {
            mode: request.mode,
            instance_id: request.instance_id,
            health_policy: request.health_policy,
            java_runtime_mode,
            manifest,
            game_dir: self.ctx.paths.instance_dir(&row.instance_id)?,
            minecraft_root,
            assets_dir: layout.assets,
            runtimes_root: self.ctx.paths.java_runtimes_root(),
            receipts_root: self.ctx.paths.loader_receipts(),
            registry_db: self
                .ctx
                .paths
                .registry_db()
                .exists()
                .then(|| self.ctx.paths.registry_db()),
            network_policy,
            identity: LaunchIdentity {
                username: credentials.username,
                access_token: credentials.access_token,
                uuid: credentials.uuid,
                user_type: "msa".into(),
                client_id: String::new(),
                xuid: String::new(),
                user_properties: "{}".into(),
            },
            java_override,
            allow_incompatible_java_override: row.java_incompatible_override,
            java_candidates: Vec::new(),
            jvm_memory_mb: row.jvm_memory_mb,
            jvm_gc_profile,
            jvm_custom_args: row.jvm_custom_args,
            extra_game_args: Vec::new(),
        })
    }

    async fn launch_inputs(
        &self,
        request: LaunchInputs,
        progress: &dyn LaunchProgress,
    ) -> LauncherResult<LaunchResult> {
        let session_id = NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed);
        let _op_handle = self.ctx.operation_manager.register_for_instance(
            &format!("launch '{}'", request.instance_id),
            &request.instance_id,
        );
        progress.phase("checking-health", "Checking instance health");

        if request.health_policy == HealthPolicy::BlockOnRed {
            let report = crate::health::health(
                &request.game_dir,
                &request.manifest,
                request.registry_db.as_deref(),
            );
            if report.score == crate::health::HealthScore::Red {
                return Err(LauncherError::Generic {
                    code: "ERR_HEALTH_BLOCKED".into(),
                    message: "Health checks found blockers that prevent launch.".into(),
                });
            }
        }

        progress.phase("resolving", "Resolving Minecraft metadata and Java");
        let java_candidates = if request.java_candidates.is_empty() {
            let runtimes_root = request.runtimes_root.clone();
            tokio::task::spawn_blocking(move || {
                crate::java::detect_java_candidates(
                    Some(&runtimes_root),
                    crate::paths::minecraft_dir().as_deref(),
                )
            })
            .await
            .map_err(|error| LauncherError::Generic {
                code: "ERR_JAVA_DISCOVERY".into(),
                message: format!("Java discovery task failed: {error}"),
            })?
        } else {
            request.java_candidates.clone()
        };

        let loader = loader_info(&request.manifest);
        let resolve_request = || ResolveRequest {
            instance_id: request.instance_id.clone(),
            base_version_id: request.manifest.minecraft_version.clone(),
            loader: loader.clone(),
            game_dir: request.game_dir.clone(),
            assets_dir: request.assets_dir.clone(),
            cache_dir: request.minecraft_root.clone(),
            java_override: request.java_override.clone(),
            java_candidates: java_candidates.clone(),
            network_policy: request.network_policy.clone(),
            allow_incompatible_java_override: request.allow_incompatible_java_override,
            minecraft_dir: Some(request.minecraft_root.clone()),
            receipts_root: Some(request.receipts_root.clone()),
        };

        let resolved = match crate::launch_planner::resolve(resolve_request()).await {
            Ok(plan) => plan,
            Err(LauncherError::JavaRuntimeMissing { major, .. })
                if request.java_runtime_mode == JavaRuntimeMode::Automatic =>
            {
                progress.phase(
                    "provisioning-java",
                    "Provisioning the required Java runtime",
                );
                request
                    .network_policy
                    .check(crate::network::NetworkCategory::JavaRuntime)?;
                let runtime_root = request.runtimes_root.clone();
                let network_policy = request.network_policy.clone();
                let catalog = self.ctx.runtime_catalog.snapshot();
                let lock_manager = self.ctx.lock_manager.clone();
                let ensured = tokio::task::spawn_blocking(move || {
                    crate::runtime_manager::ensure_runtime(
                        &runtime_root,
                        major,
                        &catalog,
                        &network_policy,
                        None::<&dyn RuntimeProgress>,
                        Some(&lock_manager),
                    )
                })
                .await
                .map_err(|error| LauncherError::Generic {
                    code: "ERR_JAVA_PROVISION".into(),
                    message: format!("Java provisioning task failed: {error}"),
                })??;
                let mut refreshed = java_candidates.clone();
                refreshed.push(JavaInstallation {
                    path: ensured.path,
                    version: ensured.version,
                    version_string: ensured.version_string,
                    source: crate::java::JavaSource::Managed,
                    arch: ensured.arch,
                });
                crate::launch_planner::resolve(ResolveRequest {
                    java_candidates: refreshed,
                    ..resolve_request()
                })
                .await?
            }
            Err(error) => return Err(error),
        };

        // Record this session as the latest for its instance (used by
        // delegated monitoring to detect same-instance replacement without
        // cross-instance interference).
        self.ctx
            .process_session_manager
            .note_latest(&request.instance_id, session_id);

        progress.phase("materializing", "Materializing verified launch artifacts");
        let _materialization_lock = self.ctx.lock_manager.acquire(
            crate::lock_manager::LockResource::Materialization,
            "launch-materialize",
        )?;
        let materialized = crate::launch_planner::materialize(resolved).await?;
        let java_path = materialized.resolved.java.path.clone();
        let gc_args = crate::gc::compute_gc(
            materialized.resolved.java.major_version,
            request.jvm_memory_mb,
            &request.jvm_custom_args,
            request.jvm_gc_profile,
        )
        .jvm_args;
        let user_jvm_args = crate::launch_planner::parse_argument_string(&gc_args)?;
        let prepared = crate::launch_planner::build_command(BuildCommandRequest {
            plan: &materialized,
            identity: &request.identity,
            features: &LaunchFeatures::default(),
            user_jvm_args: &user_jvm_args,
            extra_game_args: &request.extra_game_args,
        })?;

        progress.phase("snapshot", "Creating the pre-launch snapshot");
        let snapshot_id = create_or_reuse_snapshot(&request.game_dir)?;
        let operation_id = _op_handle.id().clone();

        if request.mode == LaunchMode::Delegated {
            progress.phase("handoff", "Handing off to external launcher");
            progress.handoff(&request.identity)?;

            let pid = 0;
            let process_identity = ProcessIdentity {
                pid: 0,
                start_time: 0,
                expected_exe: None,
            };

            let started = LaunchStarted {
                pid,
                session_id,
                snapshot_id: snapshot_id.clone(),
                java_path: java_path.clone(),
                process_identity: process_identity.clone(),
            };
            progress.started(&started);
            self.ctx
                .event_sink
                .emit(crate::event_sink::CoreEvent::Launch {
                    operation_id: operation_id.clone(),
                    instance_id: request.instance_id.clone(),
                    status: crate::event_sink::EventStatus::Started,
                    pid: None,
                });

            let result = LaunchResult {
                pid,
                session_id,
                outcome: LaunchOutcome::Unknown,
                snapshot_id: snapshot_id.clone(),
                java_path: java_path.clone(),
                process_identity,
            };
            progress.finished(&result);
            _op_handle.complete();
            return Ok(result);
        }

        // -- Direct mode: spawn Java and attach --
        progress.phase("launching", "Starting Minecraft");
        let child = crate::launch_planner::spawn(&prepared)?;
        let pid = child.id().ok_or_else(|| LauncherError::Generic {
            code: "ERR_NO_PID".into(),
            message: "Spawned process has no PID.".into(),
        })?;
        let process_identity = crate::process_identity::capture(pid)?;

        // Register the session with the core-owned process session manager.
        // Non-fatal: a duplicate registration should never happen in practice
        // and we continue regardless.
        let _ = self.ctx.process_session_manager.register(ProcessSession {
            instance_id: request.instance_id.clone(),
            session_id,
            pid,
            process_identity: process_identity.clone(),
            snapshot_id: snapshot_id.clone(),
            start_time: std::time::SystemTime::now(),
            attached: true,
            user_cancelled: false,
        });

        let started = LaunchStarted {
            pid,
            session_id,
            snapshot_id: snapshot_id.clone(),
            java_path: java_path.clone(),
            process_identity: process_identity.clone(),
        };
        progress.started(&started);
        self.ctx
            .event_sink
            .emit(crate::event_sink::CoreEvent::Launch {
                operation_id: operation_id.clone(),
                instance_id: request.instance_id.clone(),
                status: crate::event_sink::EventStatus::Started,
                pid: Some(pid),
            });

        progress.phase("running", "Waiting for Minecraft to exit");
        let secret = request.identity.access_token.as_str();
        let output_progress = |stream: &str, line: &str| progress.log(stream, line);
        let outcome = crate::launch_planner::wait_and_classify_with_progress(
            child,
            &request.game_dir,
            &[secret],
            Some(&output_progress),
        )
        .await
        .inspect_err(|_| {
            self.ctx.process_session_manager.remove(session_id);
        })?;

        let local_state = crate::db::local_state_connection(&self.ctx.paths.local_state_db())
            .map_err(|error| LauncherError::Generic {
                code: "ERR_LOCAL_STATE_FAILED".into(),
                message: error.to_string(),
            })?;
        crate::db::touch_last_launched(
            &local_state,
            &request.instance_id,
            &chrono::Utc::now().to_rfc3339(),
        )
        .map_err(|error| {
            self.ctx.process_session_manager.remove(session_id);
            LauncherError::Generic {
                code: "ERR_INSTANCE_UPDATE".into(),
                message: error.to_string(),
            }
        })?;
        crate::lkg::record_launch_outcome(
            &request.game_dir,
            Some(&snapshot_id),
            &format!("session-{session_id}"),
            outcome.clone(),
        )
        .map_err(|error| {
            self.ctx.process_session_manager.remove(session_id);
            LauncherError::Generic {
                code: "ERR_LKG_UPDATE".into(),
                message: error.to_string(),
            }
        })?;
        if outcome == LaunchOutcome::Success {
            let _ = crate::runtime_manager::mark_successful_use(&request.runtimes_root, &java_path);
        }

        // Session completed successfully — remove from manager.
        self.ctx.process_session_manager.remove(session_id);

        self.ctx
            .event_sink
            .emit(crate::event_sink::CoreEvent::Launch {
                operation_id,
                instance_id: request.instance_id,
                status: crate::event_sink::EventStatus::Completed,
                pid: Some(pid),
            });
        _op_handle.complete();

        let result = LaunchResult {
            pid,
            session_id,
            outcome,
            snapshot_id,
            java_path,
            process_identity,
        };
        progress.finished(&result);
        Ok(result)
    }

    /// Return all currently running process sessions.
    pub fn running_processes(&self) -> Vec<ProcessSession> {
        self.ctx.process_session_manager.list()
    }

    /// Monitor a delegated launch by polling for crash reports, log markers,
    /// and staleness (per-instance, never global). After the loop completes,
    /// records the LKG outcome and runs snapshot retention. The desktop
    /// adapter only needs to emit the Tauri event from the return value.
    ///
    /// Staleness is checked against the instance's latest session in
    /// [`ProcessSessionManager`]: a newer launch for a *different* instance
    /// does NOT end monitoring; only a same-instance replacement does.
    pub async fn wait_delegated(
        ctx: &Ctx,
        instance_id: &str,
        game_dir: &Path,
        snapshot_id: &str,
        session_id: u64,
        launched_at: SystemTime,
    ) -> LaunchOutcome {
        const MAX_CAPTURED_LAUNCH_LOG_BYTES: usize = 1_048_576;
        let started = std::time::Instant::now();

        let outcome = loop {
            // Per-instance staleness: only a newer session for the SAME
            // instance ends this monitor. Different instances are independent.
            if !ctx
                .process_session_manager
                .is_latest_for_instance(instance_id, session_id)
            {
                break LaunchOutcome::Unknown;
            }

            // Crash report check
            let crash_dir = game_dir.join("crash-reports");
            let has_crash = std::fs::read_dir(&crash_dir)
                .ok()
                .map(|entries| {
                    entries.flatten().any(|entry| {
                        entry
                            .metadata()
                            .ok()
                            .filter(|m| m.is_file())
                            .and_then(|m| m.modified().ok())
                            .map(|modified| modified >= launched_at)
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false);
            if has_crash {
                break LaunchOutcome::Crash;
            }

            // Log tail triage
            let log_path = game_dir.join("logs").join("latest.log");
            let log_tail = std::fs::metadata(&log_path)
                .ok()
                .filter(|m| m.modified().ok().is_some_and(|t| t >= launched_at))
                .and_then(|metadata| {
                    let mut file = std::fs::File::open(&log_path).ok()?;
                    let keep = metadata.len().min(MAX_CAPTURED_LAUNCH_LOG_BYTES as u64);
                    file.seek(SeekFrom::End(-(keep as i64))).ok()?;
                    let mut bytes = Vec::with_capacity(keep as usize);
                    file.read_to_end(&mut bytes).ok()?;
                    Some(String::from_utf8_lossy(&bytes).into_owned())
                });

            if let Some(ref log) = log_tail {
                if crate::crash_diagnostics::triage(log).matched {
                    break LaunchOutcome::Crash;
                }
                // The delegated launcher does not expose the game PID or exit code.
                // A clean-shutdown log marker is the only safe success signal.
                if log.lines().any(|line| line.contains("Stopping!")) {
                    break crate::lkg::classify_launch(&crate::lkg::LaunchEvents {
                        exit_code: Some(0),
                        runtime_ms: started.elapsed().as_millis() as u64,
                        was_user_cancelled: false,
                        crash_report_found: false,
                        log_crash_signature_matched: false,
                    });
                }
            }

            if started.elapsed() >= std::time::Duration::from_secs(12 * 60 * 60) {
                break LaunchOutcome::Unknown;
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        };

        // Record LKG outcome and run retention — core-owned, not desktop.
        let _ = crate::lkg::record_launch_outcome(
            game_dir,
            Some(snapshot_id),
            &format!("delegated-{session_id}"),
            outcome.clone(),
        );
        let _ = crate::lkg::run_retention(game_dir);

        outcome
    }
}

fn validate_instance_id(instance_id: &str) -> LauncherResult<()> {
    crate::app_paths::validate_path_component(instance_id)
}

fn loader_info(manifest: &InstanceManifest) -> Option<LoaderInfo> {
    if matches!(manifest.loader.as_str(), "" | "vanilla") {
        None
    } else {
        Some(LoaderInfo {
            loader_type: manifest.loader.clone(),
            version: manifest.loader_version.clone(),
            version_url: String::new(),
        })
    }
}

fn create_or_reuse_snapshot(instance_dir: &Path) -> LauncherResult<String> {
    let lkg = crate::lkg::read_lkg_state(instance_dir).map_err(|error| LauncherError::Generic {
        code: "ERR_LKG_READ".into(),
        message: error.to_string(),
    })?;
    if let Some(snapshot_id) = lkg.current_lkg_snapshot_id {
        let reference = snapshot_file_index(instance_dir, &snapshot_id);
        let current = live_file_index(instance_dir);
        if let (Ok(reference), Ok(current)) = (reference, current) {
            if reference == current {
                return Ok(snapshot_id);
            }
        }
    }
    create_snapshot(instance_dir, Some("pre-launch"))
        .map(|snapshot| snapshot.id)
        .map_err(|error| LauncherError::Generic {
            code: "ERR_SNAPSHOT_CREATE".into(),
            message: error.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_traversal_instance_ids() {
        assert!(validate_instance_id("../outside").is_err());
        assert!(validate_instance_id("safe-instance").is_ok());
    }

    #[test]
    fn session_ids_are_monotonic() {
        let first = NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed);
        let second = NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed);
        assert!(second > first);
    }

    #[test]
    fn validate_instance_id_rejects_empty() {
        assert!(validate_instance_id("").is_err());
    }

    #[test]
    fn launch_recovery_action_serde_roundtrip() {
        for action in &[
            LaunchRecoveryAction::None,
            LaunchRecoveryAction::ProvisionJava { major: 21 },
            LaunchRecoveryAction::RepairLoader,
        ] {
            let json = serde_json::to_string(action).unwrap();
            let back: LaunchRecoveryAction = serde_json::from_str(&json).unwrap();
            assert_eq!(*action, back);
        }
    }

    #[test]
    fn launch_recovery_action_none_is_noop() {
        // None should serialize/deserialize without error
        let json = serde_json::to_string(&LaunchRecoveryAction::None).unwrap();
        let back: LaunchRecoveryAction = serde_json::from_str(&json).unwrap();
        assert_eq!(back, LaunchRecoveryAction::None);
    }

    #[test]
    fn launch_recovery_action_provision_java_carries_major() {
        let action = LaunchRecoveryAction::ProvisionJava { major: 17 };
        let json = serde_json::to_string(&action).unwrap();
        assert!(json.contains("17"));
        let back: LaunchRecoveryAction = serde_json::from_str(&json).unwrap();
        match back {
            LaunchRecoveryAction::ProvisionJava { major } => assert_eq!(major, 17),
            _ => panic!("expected ProvisionJava"),
        }
    }

    #[test]
    fn launch_recovery_action_repair_loader_roundtrips() {
        let json = serde_json::to_string(&LaunchRecoveryAction::RepairLoader).unwrap();
        let back: LaunchRecoveryAction = serde_json::from_str(&json).unwrap();
        assert_eq!(back, LaunchRecoveryAction::RepairLoader);
    }
}
