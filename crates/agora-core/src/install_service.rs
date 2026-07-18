//! Core-owned InstallService — resolves and executes install plans.
//!
//! Owns: intent validation, instance/manifest loading, registry revision,
//! removal reverse-dependency planning, source-specific artifact resolution
//! (curated, Modrinth, manual), and InstallPipeline delegation.

use crate::ctx::Ctx;
use crate::dependency_ops::AliasMap;
use crate::error::{LauncherError, LauncherResult};
use crate::install_pipeline::{
    CancellationToken, InstallIntent, InstallOutcome, InstallPipeline, PreparedPlan,
    ProgressReporter, ResolvedInstallPlan, ResolvedOperation, ReverseDepInfo,
};
use crate::models::{InstalledMod, InstanceManifest};
use crate::resolver::Resolver;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// InstanceLoadResult
// ---------------------------------------------------------------------------

/// Result of loading instance state from the filesystem.
pub struct InstanceLoadResult {
    pub instance_dir: PathBuf,
    pub manifest: InstanceManifest,
    pub registry_revision: String,
}

// ---------------------------------------------------------------------------
// InstallService
// ---------------------------------------------------------------------------

/// Core-owned install service.
///
/// Create via [`InstallService::new`] with a [`Ctx`].
#[derive(Clone)]
pub struct InstallService {
    ctx: Ctx,
}

impl InstallService {
    pub fn new(ctx: Ctx) -> Self {
        Self { ctx }
    }

    /// Validate instance ID and load its manifest + registry revision.
    pub fn load_instance(&self, instance_id: &str) -> LauncherResult<InstanceLoadResult> {
        let sanitized = crate::paths::sanitize_id(instance_id);
        crate::app_paths::validate_path_component(&sanitized).map_err(|_| {
            LauncherError::Generic {
                code: "ERR_INVALID_INSTANCE".into(),
                message: "Invalid instance ID.".into(),
            }
        })?;
        let instance_dir = self.ctx.paths.instance_dir(&sanitized)?;
        let manifest_path = self.ctx.paths.instance_manifest(&sanitized)?;
        if !instance_dir.exists() || !manifest_path.exists() {
            return Err(LauncherError::Generic {
                code: "ERR_INSTANCE_NOT_FOUND".into(),
                message: format!("Instance '{instance_id}' not found."),
            });
        }
        let text = std::fs::read_to_string(&manifest_path).map_err(|e| LauncherError::Generic {
            code: "ERR_MANIFEST_READ".into(),
            message: format!("Could not read manifest: {e}"),
        })?;
        let manifest: InstanceManifest =
            serde_json::from_str(&text).map_err(|e| LauncherError::Generic {
                code: "ERR_MANIFEST_PARSE".into(),
                message: format!("Invalid manifest: {e}"),
            })?;
        let registry_revision = self.compute_registry_revision()?;
        Ok(InstanceLoadResult {
            instance_dir,
            manifest,
            registry_revision,
        })
    }

    /// Prepare a removal `PreparedPlan` (pure logic, no network needed).
    ///
    /// Reverse-dependency planning is computed here rather than left to the
    /// caller. This is the core-owned replacement for the desktop adapter's
    /// `prepare_remove` and the CLI's inline removal code.
    pub fn prepare_removal(
        manifest: &InstanceManifest,
        filename: &str,
        registry_revision: String,
    ) -> PreparedPlan {
        let target = all_installed(manifest).find(|item| {
            item.filename == filename
                || effective_filename(item) == filename
                || item
                    .registry_id
                    .as_deref()
                    .map(|id| id.eq_ignore_ascii_case(filename))
                    .unwrap_or(false)
                || item
                    .modrinth_id
                    .as_deref()
                    .map(|id| id.eq_ignore_ascii_case(filename))
                    .unwrap_or(false)
        });

        let (target_filename, reverse_dependents) = match target {
            Some(target) => {
                let aliases = AliasMap::from_pairs(&[]);
                let installed: Vec<InstalledMod> = all_installed(manifest).cloned().collect();
                let removal = crate::dependency_ops::build_removal_plan_with_aliases(
                    &installed, target, &aliases,
                );
                (
                    effective_filename(target),
                    removal
                        .dependents
                        .into_iter()
                        .map(|d| ReverseDepInfo {
                            mod_jar_id: d.mod_id,
                            filename: d.filename,
                            requirement: d.requirement,
                            impact: Some("Would lose a required dependency".into()),
                        })
                        .collect(),
                )
            }
            None => (filename.to_string(), vec![]),
        };

        PreparedPlan {
            operation: ResolvedOperation::Remove {
                target_filename,
                reverse_dependents,
                content_type: None,
            },
            dependencies: vec![],
            conflicts: vec![],
            registry_revision,
        }
    }

    /// Resolve an install intent into a read-only plan.
    ///
    /// Uses the core-owned [`Resolver`] to prepare curated, Modrinth, or
    /// manual artifacts and dependency dispositions, then normalizes through
    /// [`InstallPipeline`] to produce the final plan.
    pub async fn resolve(
        &self,
        intent: InstallIntent,
        reporter: &dyn ProgressReporter,
    ) -> LauncherResult<ResolvedInstallPlan> {
        let load = self.load_instance(&intent.target_instance)?;
        let mut resolver = Resolver::new(self.ctx.clone());
        if let Some(token) = std::env::var("GITHUB_TOKEN")
            .ok()
            .filter(|value| !value.is_empty())
            .or_else(crate::auth::get_token)
        {
            resolver = resolver.with_github_token(token);
        }
        let prepared = resolver.resolve(&intent, &load.manifest).await?;
        InstallPipeline
            .resolve_plan(intent, &load.instance_dir, prepared, reporter)
            .await
            .map_err(|e| LauncherError::Generic {
                code: "ERR_RESOLVE".into(),
                message: e,
            })
    }

    /// Execute a fully-resolved plan with freshness checks.
    ///
    /// Re-validates instance state and registry revision before delegating to
    /// `InstallPipeline::execute_plan`.
    pub async fn execute(
        &self,
        plan: &ResolvedInstallPlan,
        reporter: &dyn ProgressReporter,
        cancel: &CancellationToken,
    ) -> InstallOutcome {
        let load = match self.load_instance(&plan.intent.target_instance) {
            Ok(load) => load,
            Err(error) => {
                return InstallOutcome::Failed {
                    error: format!("Instance not accessible before execution: {error}"),
                    rollback_performed: false,
                    snapshot_id: None,
                };
            }
        };
        InstallPipeline
            .execute_plan(
                plan,
                &load.instance_dir,
                &load.registry_revision,
                reporter,
                cancel,
            )
            .await
    }

    /// Resolve a caller-prepared reconciliation plan through the pipeline.
    ///
    /// Used by trusted adapters (e.g., desktop lockfile repair/import) that
    /// have already computed the backend-resolved operations. The service
    /// handles instance loading, registry revision, and pipeline normalization.
    pub async fn resolve_prepared(
        &self,
        intent: InstallIntent,
        mut prepared: PreparedPlan,
        reporter: &dyn ProgressReporter,
    ) -> LauncherResult<ResolvedInstallPlan> {
        let load = self.load_instance(&intent.target_instance)?;
        prepared.registry_revision = load.registry_revision;
        InstallPipeline
            .resolve_plan(intent, &load.instance_dir, prepared, reporter)
            .await
            .map_err(|e| LauncherError::Generic {
                code: "ERR_RESOLVE".into(),
                message: e,
            })
    }

    /// Check that the instance is not locked.
    pub fn check_not_locked(&self, instance_id: &str) -> LauncherResult<()> {
        let manifest_path = self.ctx.paths.instance_manifest(instance_id)?;
        if !manifest_path.exists() {
            return Ok(());
        }
        let text = std::fs::read_to_string(&manifest_path)
            .map_err(|_| LauncherError::InstanceCreateFailed)?;
        let manifest: InstanceManifest =
            serde_json::from_str(&text).map_err(|_| LauncherError::InstanceCreateFailed)?;
        if manifest.is_locked {
            return Err(LauncherError::InstanceLocked);
        }
        Ok(())
    }

    /// Download, verify, and install a single artifact into an instance.
    ///
    /// This is a convenience method for direct installs that bypass the full
    /// pipeline (snapshot, staging, health scan). For production use, prefer
    /// `resolve_and_execute` through the pipeline.
    #[allow(clippy::too_many_arguments)]
    pub async fn install_artifact(
        &self,
        instance_id: &str,
        filename: &str,
        content_type: &str,
        download_url: &str,
        registry_id: Option<&str>,
        modrinth_id: Option<&str>,
        source: &str,
        version: Option<&str>,
        expected_sha1: Option<&str>,
        expected_sha256: Option<&str>,
    ) -> LauncherResult<InstalledMod> {
        self.check_not_locked(instance_id)?;

        let dir = self.ctx.paths.instance_dir(instance_id)?;
        crate::helpers::check_disk_space(&dir)?;

        let bytes =
            crate::download::download_mod_bytes(&self.ctx.http_clients, download_url).await?;

        let candidate_sha1 = expected_sha1.unwrap_or("").trim().to_lowercase();
        if !candidate_sha1.is_empty() {
            let actual_sha1 = crate::download::sha1_hex(&bytes);
            if actual_sha1 != candidate_sha1 {
                return Err(LauncherError::HashMismatch);
            }
        } else if let Some(pinned) = expected_sha256 {
            let trimmed = pinned.trim();
            if !trimmed.is_empty() {
                let actual_sha = crate::download::sha256_hex(&bytes);
                if actual_sha != trimmed {
                    return Err(LauncherError::HashMismatch);
                }
            }
        }

        let installed_sha256 = crate::download::sha256_hex(&bytes);
        let target_dir = dir.join(crate::helpers::content_subdir(content_type));
        std::fs::create_dir_all(&target_dir).map_err(|_| LauncherError::InstanceCreateFailed)?;
        let item_path = target_dir.join(filename);
        std::fs::write(&item_path, &bytes).map_err(|_| LauncherError::InstanceCreateFailed)?;

        let metadata = crate::jar_metadata::parse_jar_metadata(&item_path);
        let manifest_path = self.ctx.paths.instance_manifest(instance_id)?;
        let mut manifest = crate::helpers::read_manifest(&manifest_path)?;

        let installed_mod = InstalledMod {
            filename: filename.to_string(),
            registry_id: registry_id.map(|s| s.to_string()),
            modrinth_id: modrinth_id.map(|s| s.to_string()),
            source: source.to_string(),
            source_url: Some(download_url.to_string()),
            version: version.map(|s| s.to_string()),
            sha256: installed_sha256,
            installed_at: chrono::Utc::now().to_rfc3339(),
            java_packages: metadata.java_packages,
            mod_jar_id: metadata.mod_jar_id,
            depends_on: metadata.depends_on,
            optional_deps: metadata.optional_deps,
            incompatible_deps: metadata.incompatible_deps,
            provided_mod_ids: metadata
                .provided_mods
                .into_iter()
                .map(|provided| provided.mod_id)
                .collect(),
            enabled: true,
            content_type: if content_type.is_empty() {
                "mod".to_string()
            } else {
                content_type.to_string()
            },
        };

        crate::helpers::push_to_content_array(&mut manifest, &installed_mod);
        crate::helpers::atomic_write_manifest(&manifest_path, &manifest)?;

        Ok(installed_mod)
    }

    /// Remove an artifact from an instance by filename.
    ///
    /// Deletes the file from whichever content subdirectory it resides in and
    /// updates the manifest atomically.
    pub fn remove_artifact(&self, instance_id: &str, filename: &str) -> LauncherResult<bool> {
        self.check_not_locked(instance_id)?;

        if filename.contains("..") || filename.contains('/') || filename.contains('\\') {
            return Err(LauncherError::Generic {
                code: "ERR_INVALID_FILENAME".to_string(),
                message: "Filename contains invalid characters.".to_string(),
            });
        }

        let dir = self.ctx.paths.instance_dir(instance_id)?;
        let removed = crate::helpers::find_and_delete_file(&dir, filename);

        let manifest_path = self.ctx.paths.instance_manifest(instance_id)?;
        if manifest_path.exists() {
            let text = std::fs::read_to_string(&manifest_path)
                .map_err(|_| LauncherError::InstanceCreateFailed)?;
            let mut manifest: InstanceManifest =
                serde_json::from_str(&text).map_err(|_| LauncherError::InstanceCreateFailed)?;

            if crate::helpers::remove_from_content_array(&mut manifest, filename) {
                crate::helpers::atomic_write_manifest(&manifest_path, &manifest)?;
            }
        }

        Ok(removed)
    }

    /// Add a manually-dropped .jar file into an instance's `mods/` folder.
    ///
    /// Security: the source path must resolve to one of the user's allowlisted
    /// drop directories (Downloads, Desktop, Documents, or system temp).
    pub fn add_manual_artifact(
        &self,
        instance_id: &str,
        source_path: &str,
    ) -> LauncherResult<InstalledMod> {
        self.check_not_locked(instance_id)?;

        let src = std::path::Path::new(source_path);
        let ext = src
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase());
        if ext.as_deref() != Some("jar") {
            return Err(LauncherError::Generic {
                code: "ERR_INVALID_FILENAME".to_string(),
                message: "Only .jar files can be added manually.".to_string(),
            });
        }
        let file_name =
            src.file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| LauncherError::Generic {
                    code: "ERR_INVALID_FILENAME".to_string(),
                    message: "Could not determine a valid file name.".to_string(),
                })?;
        if file_name.contains("..") || file_name.contains('/') || file_name.contains('\\') {
            return Err(LauncherError::Generic {
                code: "ERR_INVALID_FILENAME".to_string(),
                message: "Filename contains invalid characters.".to_string(),
            });
        }

        let dir = self.ctx.paths.instance_dir(instance_id)?;
        let mods_dir = dir.join("mods");
        let dest = mods_dir.join(file_name);
        let manifest_path = self.ctx.paths.instance_manifest(instance_id)?;

        let canonical = std::fs::canonicalize(source_path).map_err(|_| LauncherError::Generic {
            code: "ERR_INVALID_SOURCE".to_string(),
            message: "Source file does not exist or cannot be resolved.".to_string(),
        })?;

        let mut roots: Vec<std::path::PathBuf> = Vec::new();
        for r in [
            dirs::download_dir(),
            dirs::desktop_dir(),
            dirs::document_dir(),
            Some(std::env::temp_dir()),
        ]
        .into_iter()
        .flatten()
        {
            if let Ok(c) = std::fs::canonicalize(&r) {
                roots.push(c);
            }
        }
        let allowed = roots.iter().any(|root| canonical.starts_with(root));
        if !allowed {
            return Err(LauncherError::Generic {
                code: "ERR_SOURCE_NOT_ALLOWED".to_string(),
                message: "Source file is outside the allowed drop directories \
                          (Downloads, Desktop, Documents, or system temp)."
                    .to_string(),
            });
        }

        let bytes = std::fs::read(&canonical).map_err(|_| LauncherError::Generic {
            code: "ERR_READ_FAILED".to_string(),
            message: "Failed to read the dropped file.".to_string(),
        })?;

        std::fs::create_dir_all(&mods_dir).map_err(|_| LauncherError::InstanceCreateFailed)?;
        std::fs::write(&dest, &bytes).map_err(|_| LauncherError::InstanceCreateFailed)?;
        let sha256 = crate::download::sha256_hex(&bytes);

        if !manifest_path.exists() {
            return Err(LauncherError::Generic {
                code: "ERR_MANIFEST_MISSING".to_string(),
                message: "Instance manifest not found. Create the instance first.".to_string(),
            });
        }
        let text = std::fs::read_to_string(&manifest_path)
            .map_err(|_| LauncherError::InstanceCreateFailed)?;
        let mut manifest: InstanceManifest =
            serde_json::from_str(&text).map_err(|_| LauncherError::InstanceCreateFailed)?;

        let metadata = crate::jar_metadata::parse_jar_metadata(&dest);
        let installed_mod = InstalledMod {
            filename: file_name.to_string(),
            registry_id: None,
            modrinth_id: None,
            source: "manual_drag_drop".to_string(),
            source_url: None,
            version: None,
            sha256,
            installed_at: chrono::Utc::now().to_rfc3339(),
            java_packages: metadata.java_packages,
            mod_jar_id: metadata.mod_jar_id,
            depends_on: metadata.depends_on,
            optional_deps: metadata.optional_deps,
            incompatible_deps: metadata.incompatible_deps,
            provided_mod_ids: metadata
                .provided_mods
                .into_iter()
                .map(|provided| provided.mod_id)
                .collect(),
            enabled: true,
            content_type: "mod".to_string(),
        };
        crate::helpers::push_to_content_array(&mut manifest, &installed_mod);
        crate::helpers::atomic_write_manifest(&manifest_path, &manifest)?;

        Ok(installed_mod)
    }

    /// Convenience: resolve and immediately execute (one-shot).
    ///
    /// Useful for the CLI and non-interactive callers.
    pub async fn resolve_and_execute(
        &self,
        intent: InstallIntent,
        reporter: &dyn ProgressReporter,
        cancel: &CancellationToken,
    ) -> LauncherResult<InstallOutcome> {
        let plan = self.resolve(intent, reporter).await?;
        Ok(self.execute(&plan, reporter, cancel).await)
    }

    fn compute_registry_revision(&self) -> LauncherResult<String> {
        let path = self.ctx.paths.registry_db();
        if !path.is_file() {
            return Ok("registry-unavailable".into());
        }
        let bytes = std::fs::read(&path).map_err(|e| LauncherError::Generic {
            code: "ERR_REGISTRY_READ".into(),
            message: format!("Could not read registry: {e}"),
        })?;
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(bytes);
        Ok(format!("{:x}", hasher.finalize()))
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn all_installed(manifest: &InstanceManifest) -> impl Iterator<Item = &InstalledMod> {
    manifest
        .mods
        .iter()
        .chain(manifest.resourcepacks.iter())
        .chain(manifest.shaders.iter())
        .chain(manifest.datapacks.iter())
        .chain(manifest.worlds.iter())
}

fn effective_filename(item: &InstalledMod) -> String {
    if item.enabled || item.filename.ends_with(".disabled") {
        item.filename.clone()
    } else {
        format!("{}.disabled", item.filename)
    }
}
