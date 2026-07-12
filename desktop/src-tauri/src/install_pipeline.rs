//! Desktop facade for the canonical install pipeline.
//!
//! This module resolves registry/Modrinth/network facts into the UI-agnostic
//! core types. It performs no live instance mutation; `agora-core` owns plan
//! normalization and execution.

use crate::error::{LauncherError, LauncherResult};
use crate::models::{InstalledMod, InstanceManifest, ModVersionCandidate};
use crate::{db, mod_install, modrinth_raw, paths, registry};
use agora_core::dependency_ops::{AliasMap, DepSource, Requirement};
use agora_core::install_pipeline::{
    ArtifactMetadata, ArtifactSource, ConflictKind, ConflictResolution, DepConflict,
    DepDisposition, HashAlgorithm, HashSpec, HashedValue, InstallAction, InstallIntent,
    PreparedPlan, ResolvedArtifact, ResolvedDep, ResolvedDownload, ResolvedLocal,
    ResolvedOperation, ReverseDepInfo, SourceType,
};
use sha2::Digest;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

pub struct PreparedDesktopPlan {
    pub instance_dir: PathBuf,
    pub prepared: PreparedPlan,
}

pub async fn prepare_plan(
    app: &tauri::AppHandle,
    intent: &InstallIntent,
) -> LauncherResult<PreparedDesktopPlan> {
    let sanitized = paths::sanitize_id(&intent.target_instance);
    if sanitized.is_empty() || sanitized != intent.target_instance {
        return Err(generic(
            "ERR_INVALID_INSTANCE",
            "The target instance ID is empty or invalid.",
        ));
    }
    let instance_dir = paths::instance_dir(app, &sanitized)
        .map_err(|e| generic("ERR_INSTANCE_PATH", e.to_string()))?;
    let manifest = read_manifest(app, &sanitized)?;
    let revision = registry_revision(app)?;

    let prepared = match &intent.action {
        InstallAction::Install {
            source_type,
            item_id,
            candidate_version,
        } => match source_type {
            SourceType::Curated => {
                prepare_curated_install(
                    app,
                    &manifest,
                    item_id,
                    candidate_version.as_deref(),
                    revision,
                    false,
                )
                .await?
            }
            SourceType::Modrinth => {
                prepare_modrinth_install(
                    app,
                    &manifest,
                    item_id,
                    candidate_version.as_deref(),
                    revision,
                    false,
                )
                .await?
            }
            SourceType::Manual => {
                prepare_manual_install(item_id, candidate_version.as_deref(), revision)?
            }
        },
        InstallAction::Update {
            item_id,
            target_version,
        } => {
            let installed = find_installed_by_identity(&manifest, item_id).ok_or_else(|| {
                generic(
                    "ERR_UPDATE_TARGET_MISSING",
                    format!("{item_id} is not installed in this instance."),
                )
            })?;
            if installed.source == "modrinth_raw" {
                let project_id = installed.modrinth_id.as_deref().unwrap_or(item_id);
                prepare_modrinth_install(
                    app,
                    &manifest,
                    project_id,
                    normalize_requested_version(Some(target_version)),
                    revision,
                    true,
                )
                .await?
            } else {
                let registry_id = installed.registry_id.as_deref().unwrap_or(item_id);
                prepare_curated_install(
                    app,
                    &manifest,
                    registry_id,
                    normalize_requested_version(Some(target_version)),
                    revision,
                    true,
                )
                .await?
            }
        }
        InstallAction::Remove { filename } => prepare_remove(&manifest, filename, revision)?,
        InstallAction::BatchUpdate { items } => {
            prepare_batch_update(app, &manifest, items, revision).await?
        }
        InstallAction::BatchInstall { items } => {
            prepare_batch_install(app, &manifest, items, revision).await?
        }
        InstallAction::RepairLockfile { .. } => {
            return Err(generic(
                "ERR_LOCKFILE_COMMAND",
                "Lockfile repair must be prepared by the verified lockfile command.",
            ));
        }
    };

    Ok(PreparedDesktopPlan {
        instance_dir,
        prepared,
    })
}

async fn prepare_batch_install(
    app: &tauri::AppHandle,
    manifest: &InstanceManifest,
    items: &[agora_core::install_pipeline::BatchInstallItem],
    registry_revision: String,
) -> LauncherResult<PreparedPlan> {
    let mut operations = Vec::new();
    let mut dependencies = BTreeMap::<String, ResolvedDep>::new();
    let mut conflicts = BTreeMap::<String, DepConflict>::new();

    for item in items {
        let prepared = match item.source_type {
            SourceType::Curated => {
                prepare_curated_install(
                    app,
                    manifest,
                    &item.item_id,
                    item.candidate_version.as_deref(),
                    registry_revision.clone(),
                    false,
                )
                .await?
            }
            SourceType::Modrinth => {
                prepare_modrinth_install(
                    app,
                    manifest,
                    &item.item_id,
                    item.candidate_version.as_deref(),
                    registry_revision.clone(),
                    false,
                )
                .await?
            }
            SourceType::Manual => prepare_manual_install(
                &item.item_id,
                item.candidate_version.as_deref(),
                registry_revision.clone(),
            )?,
        };
        operations.push(prepared.operation);
        merge_dependencies(&mut dependencies, prepared.dependencies);
        for conflict in prepared.conflicts {
            conflicts.insert(conflict.conflict_id.clone(), conflict);
        }
    }

    Ok(PreparedPlan {
        operation: ResolvedOperation::BatchInstall { operations },
        dependencies: dependencies.into_values().collect(),
        conflicts: conflicts.into_values().collect(),
        registry_revision,
    })
}

fn merge_dependencies(target: &mut BTreeMap<String, ResolvedDep>, incoming: Vec<ResolvedDep>) {
    for dependency in incoming {
        let key = dependency.mod_jar_id.to_ascii_lowercase();
        target
            .entry(key)
            .and_modify(|existing| {
                if dependency.requirement == Requirement::Required {
                    existing.requirement = Requirement::Required;
                }
            })
            .or_insert(dependency);
    }
}

async fn prepare_batch_update(
    app: &tauri::AppHandle,
    manifest: &InstanceManifest,
    items: &[agora_core::install_pipeline::BatchUpdateItem],
    registry_revision: String,
) -> LauncherResult<PreparedPlan> {
    let mut operations = Vec::new();
    let mut dependencies = BTreeMap::<String, ResolvedDep>::new();
    let mut conflicts = BTreeMap::<String, DepConflict>::new();
    for item in items {
        let installed = find_installed_by_identity(manifest, &item.item_id).ok_or_else(|| {
            generic(
                "ERR_UPDATE_TARGET_MISSING",
                format!("{} is not installed.", item.item_id),
            )
        })?;
        let prepared = if installed.source == "modrinth_raw" {
            let project_id = installed.modrinth_id.as_deref().unwrap_or(&item.item_id);
            prepare_modrinth_install(
                app,
                manifest,
                project_id,
                normalize_requested_version(Some(&item.target_version)),
                registry_revision.clone(),
                true,
            )
            .await?
        } else {
            let registry_id = installed.registry_id.as_deref().unwrap_or(&item.item_id);
            prepare_curated_install(
                app,
                manifest,
                registry_id,
                normalize_requested_version(Some(&item.target_version)),
                registry_revision.clone(),
                true,
            )
            .await?
        };
        operations.push(prepared.operation);
        merge_dependencies(&mut dependencies, prepared.dependencies);
        for conflict in prepared.conflicts {
            conflicts.insert(conflict.conflict_id.clone(), conflict);
        }
    }
    Ok(PreparedPlan {
        operation: ResolvedOperation::BatchUpdate { operations },
        dependencies: dependencies.into_values().collect(),
        conflicts: conflicts.into_values().collect(),
        registry_revision,
    })
}

pub fn registry_revision(app: &tauri::AppHandle) -> LauncherResult<String> {
    let path = paths::registry_db_path(app).map_err(|_| LauncherError::RegistryMissing)?;
    if !path.is_file() {
        return Ok("registry-unavailable".into());
    }
    let bytes = std::fs::read(&path)
        .map_err(|e| generic("ERR_REGISTRY_READ", format!("Could not read registry: {e}")))?;
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

fn read_manifest(app: &tauri::AppHandle, instance_id: &str) -> LauncherResult<InstanceManifest> {
    let path = paths::instance_manifest_path(app, instance_id)
        .map_err(|e| generic("ERR_INSTANCE_PATH", e.to_string()))?;
    let text = std::fs::read_to_string(&path).map_err(|e| {
        generic(
            "ERR_MANIFEST_READ",
            format!("Could not read {}: {e}", path.display()),
        )
    })?;
    serde_json::from_str(&text).map_err(|e| {
        generic(
            "ERR_MANIFEST_PARSE",
            format!("Invalid instance manifest: {e}"),
        )
    })
}

async fn prepare_curated_install(
    app: &tauri::AppHandle,
    manifest: &InstanceManifest,
    item_id: &str,
    requested_version: Option<&str>,
    registry_revision: String,
    update: bool,
) -> LauncherResult<PreparedPlan> {
    let item = mod_install::load_registry_item(app, item_id)?;
    let candidates = mod_install::list_mod_versions(app, &manifest.instance_id, item_id).await?;
    let candidate = select_curated_candidate(&candidates, requested_version)?;
    let artifact = curated_artifact(&item, candidate)?;
    let (dependencies, conflicts) = resolve_curated_dependencies(app, manifest, item_id).await?;

    let operation = if update {
        let installed = find_installed_by_identity(manifest, item_id).ok_or_else(|| {
            generic(
                "ERR_UPDATE_TARGET_MISSING",
                format!("{item_id} is not installed."),
            )
        })?;
        ResolvedOperation::Update {
            old_version_id: installed
                .version
                .clone()
                .unwrap_or_else(|| "unknown".into()),
            new_artifact: artifact,
        }
    } else {
        ResolvedOperation::Install { artifact }
    };

    Ok(PreparedPlan {
        operation,
        dependencies,
        conflicts,
        registry_revision,
    })
}

async fn prepare_modrinth_install(
    app: &tauri::AppHandle,
    manifest: &InstanceManifest,
    project_id: &str,
    requested_version: Option<&str>,
    registry_revision: String,
    update: bool,
) -> LauncherResult<PreparedPlan> {
    let candidates = modrinth_raw::list_raw_modrinth_versions(
        app,
        Some(&manifest.instance_id),
        project_id,
        Some("mod"),
    )
    .await?;
    let candidate = select_modrinth_candidate(&candidates, requested_version)?;
    let artifact = raw_modrinth_artifact(project_id, candidate)?;
    let dependencies = resolve_modrinth_dependencies(app, manifest, candidate).await;

    let operation = if update {
        let installed = find_installed_by_identity(manifest, project_id).ok_or_else(|| {
            generic(
                "ERR_UPDATE_TARGET_MISSING",
                format!("{project_id} is not installed."),
            )
        })?;
        ResolvedOperation::Update {
            old_version_id: installed
                .version
                .clone()
                .unwrap_or_else(|| "unknown".into()),
            new_artifact: artifact,
        }
    } else {
        ResolvedOperation::Install { artifact }
    };
    Ok(PreparedPlan {
        operation,
        dependencies,
        conflicts: Vec::new(),
        registry_revision,
    })
}

fn prepare_manual_install(
    item_id: &str,
    source_path: Option<&str>,
    registry_revision: String,
) -> LauncherResult<PreparedPlan> {
    let source_path = source_path
        .filter(|path| !path.trim().is_empty())
        .ok_or_else(|| {
            generic(
                "ERR_MANUAL_PATH",
                "Manual install requires a local file path.",
            )
        })?;
    let path = Path::new(source_path);
    if !path.is_file() {
        return Err(generic(
            "ERR_MANUAL_PATH",
            format!("Manual artifact does not exist: {}", path.display()),
        ));
    }
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| name.to_ascii_lowercase().ends_with(".jar"))
        .ok_or_else(|| generic("ERR_MANUAL_FILE", "Manual mods must be .jar files."))?;
    let bytes = std::fs::read(path).map_err(|e| {
        generic(
            "ERR_MANUAL_READ",
            format!("Could not read manual artifact: {e}"),
        )
    })?;
    let sha256 = agora_core::download::sha256_hex(&bytes);
    Ok(PreparedPlan {
        operation: ResolvedOperation::Install {
            artifact: ResolvedArtifact::LocalFile(ResolvedLocal {
                item_id: item_id.to_string(),
                source_path: source_path.to_string(),
                hashes: HashSpec {
                    values: vec![HashedValue {
                        algorithm: HashAlgorithm::Sha256,
                        value: sha256,
                    }],
                },
                size: bytes.len() as u64,
                filename: filename.to_string(),
                metadata: ArtifactMetadata {
                    source_type: SourceType::Manual,
                    registry_id: None,
                    modrinth_id: None,
                    content_type: "mod".into(),
                },
            }),
        },
        dependencies: Vec::new(),
        conflicts: Vec::new(),
        registry_revision,
    })
}

fn prepare_remove(
    manifest: &InstanceManifest,
    filename: &str,
    registry_revision: String,
) -> LauncherResult<PreparedPlan> {
    let target = all_installed(manifest)
        .find(|item| item.filename == filename || effective_filename(item) == filename)
        .ok_or_else(|| {
            generic(
                "ERR_NOT_INSTALLED",
                format!("{filename} is not present in the instance manifest."),
            )
        })?;
    let aliases = AliasMap::from_pairs(&[]);
    let installed: Vec<InstalledMod> = all_installed(manifest).cloned().collect();
    let removal =
        agora_core::dependency_ops::build_removal_plan_with_aliases(&installed, target, &aliases);
    Ok(PreparedPlan {
        operation: ResolvedOperation::Remove {
            target_filename: effective_filename(target),
            reverse_dependents: removal
                .dependents
                .into_iter()
                .map(|dependent| ReverseDepInfo {
                    mod_jar_id: dependent.mod_id,
                    filename: dependent.filename,
                    requirement: dependent.requirement,
                    impact: Some("Would lose a required dependency".into()),
                })
                .collect(),
        },
        dependencies: Vec::new(),
        conflicts: Vec::new(),
        registry_revision,
    })
}

async fn resolve_curated_dependencies(
    app: &tauri::AppHandle,
    manifest: &InstanceManifest,
    root_item_id: &str,
) -> LauncherResult<(Vec<ResolvedDep>, Vec<DepConflict>)> {
    let (dependency_map, aliases, known_conflicts) = {
        let connection =
            db::registry_connection(app).map_err(|e| generic("ERR_REGISTRY_DB", e.to_string()))?;
        let dependency_map = agora_core::registry::get_all_manifest_dependencies(&connection)?;
        let alias_pairs = registry::get_all_mod_aliases(&connection)?;
        let known_conflicts = registry::get_known_conflicts(&connection)?;
        (
            dependency_map,
            AliasMap::from_pairs(&alias_pairs),
            known_conflicts,
        )
    };
    let installed: Vec<InstalledMod> = all_installed(manifest).cloned().collect();
    let installed_ids: BTreeMap<String, &InstalledMod> = installed
        .iter()
        .flat_map(|item| {
            [
                item.registry_id.as_deref(),
                item.modrinth_id.as_deref(),
                item.mod_jar_id.as_deref(),
            ]
            .into_iter()
            .flatten()
            .map(|id| (aliases.resolve_or_self(id).to_ascii_lowercase(), item))
            .collect::<Vec<_>>()
        })
        .collect();

    let mut queue = VecDeque::new();
    if let Some(root) = dependency_map.get(root_item_id) {
        enqueue_manifest_deps(&mut queue, root);
    }
    let mut resolved = BTreeMap::<String, ResolvedDep>::new();
    let mut expanded = HashSet::new();
    while let Some((raw_id, requirement)) = queue.pop_front() {
        let canonical = aliases.resolve_or_self(&raw_id);
        let key = canonical.to_ascii_lowercase();
        if let Some(existing) = resolved.get_mut(&key) {
            if requirement == Requirement::Required {
                existing.requirement = Requirement::Required;
            }
            continue;
        }
        if let Some(installed) = installed_ids.get(&key) {
            resolved.insert(
                key,
                ResolvedDep {
                    mod_jar_id: canonical,
                    requirement,
                    source: DepSource::Manifest,
                    disposition: DepDisposition::ReuseExisting {
                        mod_jar_id: installed
                            .mod_jar_id
                            .clone()
                            .unwrap_or_else(|| raw_id.clone()),
                        installed_filename: effective_filename(installed),
                    },
                },
            );
            continue;
        }
        if is_platform_dependency(&key, &manifest.loader) {
            resolved.insert(
                key,
                ResolvedDep {
                    mod_jar_id: canonical,
                    requirement,
                    source: DepSource::Manifest,
                    disposition: DepDisposition::ReuseExisting {
                        mod_jar_id: raw_id,
                        installed_filename: format!("provided by {} loader", manifest.loader),
                    },
                },
            );
            continue;
        }

        let registry_item = mod_install::load_registry_item(app, &canonical);
        let disposition = match registry_item {
            Ok(item) => {
                match mod_install::list_mod_versions(app, &manifest.instance_id, &canonical).await {
                    Ok(candidates) => match select_curated_candidate(&candidates, None) {
                        Ok(candidate) => match curated_artifact(&item, candidate) {
                            Ok(artifact) => DepDisposition::InstallCandidate { artifact },
                            Err(error) => DepDisposition::Unresolved {
                                reason: error.to_string(),
                            },
                        },
                        Err(error) => DepDisposition::Unresolved {
                            reason: error.to_string(),
                        },
                    },
                    Err(error) => DepDisposition::Unresolved {
                        reason: error.to_string(),
                    },
                }
            }
            Err(error) => DepDisposition::Unresolved {
                reason: error.to_string(),
            },
        };
        resolved.insert(
            key.clone(),
            ResolvedDep {
                mod_jar_id: canonical.clone(),
                requirement,
                source: DepSource::Manifest,
                disposition,
            },
        );
        if expanded.insert(key) {
            if let Some(child) = dependency_map.get(&canonical) {
                enqueue_manifest_deps(&mut queue, child);
            }
        }
    }

    let incoming: HashSet<String> = std::iter::once(root_item_id.to_ascii_lowercase())
        .chain(resolved.keys().cloned())
        .collect();
    let installed_set: HashSet<String> = installed_ids.keys().cloned().collect();
    let mut conflicts = Vec::new();
    for conflict in known_conflicts {
        let a = aliases
            .resolve_or_self(&conflict.mod_a_id)
            .to_ascii_lowercase();
        let b = aliases
            .resolve_or_self(&conflict.mod_b_id)
            .to_ascii_lowercase();
        if (incoming.contains(&a) && (installed_set.contains(&b) || incoming.contains(&b)))
            || (incoming.contains(&b) && (installed_set.contains(&a) || incoming.contains(&a)))
        {
            conflicts.push(DepConflict {
                conflict_id: format!("known:{a}:{b}"),
                kind: ConflictKind::IncompatibleMod,
                existing_mod_jar_id: if installed_set.contains(&a) {
                    a.clone()
                } else {
                    b.clone()
                },
                incoming_mod_jar_id: if incoming.contains(&a) {
                    a.clone()
                } else {
                    b.clone()
                },
                message: conflict.notes.unwrap_or_else(|| {
                    format!("The curated registry reports a conflict between {a} and {b}.")
                }),
                blocking: conflict.severity != "info",
                resolution_options: vec![ConflictResolution::Abort, ConflictResolution::Skip],
                chosen: None,
            });
        }
    }
    Ok((resolved.into_values().collect(), conflicts))
}

async fn resolve_modrinth_dependencies(
    app: &tauri::AppHandle,
    manifest: &InstanceManifest,
    root: &modrinth_raw::RawModrinthVersionCandidate,
) -> Vec<ResolvedDep> {
    let installed_ids: HashSet<String> = all_installed(manifest)
        .filter_map(|item| item.modrinth_id.as_ref())
        .map(|id| id.to_ascii_lowercase())
        .collect();
    let mut queue = VecDeque::new();
    for dependency in &root.dependencies {
        let requirement = match dependency.dependency_type.as_str() {
            "required" => Requirement::Required,
            "optional" => Requirement::Optional,
            _ => continue,
        };
        queue.push_back((
            dependency.project_id.clone(),
            dependency.version_id.clone(),
            requirement,
        ));
    }

    let mut expanded = BTreeMap::<String, Requirement>::new();
    let mut resolved = BTreeMap::<String, ResolvedDep>::new();
    while let Some((project_id, version_id, requirement)) = queue.pop_front() {
        let Some(project_id) = project_id else {
            let identity = version_id.unwrap_or_else(|| "unknown-version".into());
            resolved.insert(
                identity.clone(),
                ResolvedDep {
                    mod_jar_id: identity,
                    requirement,
                    source: DepSource::Manifest,
                    disposition: DepDisposition::Unresolved {
                        reason: "Modrinth dependency omitted its project ID.".into(),
                    },
                },
            );
            continue;
        };
        let key = project_id.to_ascii_lowercase();
        let should_expand = match expanded.get(&key) {
            Some(Requirement::Required) => false,
            Some(Requirement::Optional) if requirement == Requirement::Optional => false,
            _ => true,
        };
        if !should_expand {
            if requirement == Requirement::Required {
                if let Some(existing) = resolved.get_mut(&key) {
                    existing.requirement = Requirement::Required;
                }
            }
            continue;
        }
        expanded.insert(key.clone(), requirement.clone());

        if installed_ids.contains(&key) {
            let installed = all_installed(manifest).find(|item| {
                item.modrinth_id
                    .as_deref()
                    .map(str::to_ascii_lowercase)
                    .as_deref()
                    == Some(key.as_str())
            });
            resolved.insert(
                key,
                ResolvedDep {
                    mod_jar_id: project_id.clone(),
                    requirement,
                    source: DepSource::Manifest,
                    disposition: DepDisposition::ReuseExisting {
                        mod_jar_id: project_id,
                        installed_filename: installed
                            .map(effective_filename)
                            .unwrap_or_else(|| "installed".into()),
                    },
                },
            );
            continue;
        }

        let candidates = modrinth_raw::list_raw_modrinth_versions(
            app,
            Some(&manifest.instance_id),
            &project_id,
            Some("mod"),
        )
        .await;
        let (disposition, child_dependencies) = match candidates {
            Ok(candidates) => match select_modrinth_candidate(&candidates, version_id.as_deref()) {
                Ok(candidate) => {
                    let children = candidate.dependencies.clone();
                    match raw_modrinth_artifact(&project_id, candidate) {
                        Ok(artifact) => (DepDisposition::InstallCandidate { artifact }, children),
                        Err(error) => (
                            DepDisposition::Unresolved {
                                reason: error.to_string(),
                            },
                            Vec::new(),
                        ),
                    }
                }
                Err(error) => (
                    DepDisposition::Unresolved {
                        reason: error.to_string(),
                    },
                    Vec::new(),
                ),
            },
            Err(error) => (
                DepDisposition::Unresolved {
                    reason: error.to_string(),
                },
                Vec::new(),
            ),
        };
        resolved.insert(
            key,
            ResolvedDep {
                mod_jar_id: project_id,
                requirement: requirement.clone(),
                source: DepSource::Manifest,
                disposition,
            },
        );
        for child in child_dependencies {
            let child_requirement = match child.dependency_type.as_str() {
                "required" if requirement == Requirement::Required => Requirement::Required,
                "required" | "optional" => Requirement::Optional,
                _ => continue,
            };
            queue.push_back((child.project_id, child.version_id, child_requirement));
        }
    }

    resolved.into_values().collect()
}

fn enqueue_manifest_deps(
    queue: &mut VecDeque<(String, Requirement)>,
    dependencies: &registry::ManifestDeps,
) {
    queue.extend(
        dependencies
            .required
            .iter()
            .cloned()
            .map(|id| (id, Requirement::Required)),
    );
    queue.extend(
        dependencies
            .optional
            .iter()
            .cloned()
            .map(|id| (id, Requirement::Optional)),
    );
}

fn curated_artifact(
    item: &registry::RegistryItem,
    candidate: &ModVersionCandidate,
) -> LauncherResult<ResolvedArtifact> {
    let mut hashes = Vec::new();
    if let Some(sha512) = valid_hash(candidate.sha512.as_deref(), 128) {
        hashes.push(HashedValue {
            algorithm: HashAlgorithm::Sha512,
            value: sha512,
        });
    }
    let sha256 = valid_hash(candidate.sha256.as_deref(), 64)
        .or_else(|| valid_hash(Some(&item.sha256), 64))
        .ok_or_else(|| {
            generic(
                "ERR_HASH_UNAVAILABLE",
                format!(
                    "No trusted SHA-256 is available for {} {}.",
                    item.id, candidate.version
                ),
            )
        })?;
    hashes.push(HashedValue {
        algorithm: HashAlgorithm::Sha256,
        value: sha256,
    });
    if let Some(sha1) = valid_hash(candidate.sha1.as_deref(), 40) {
        hashes.push(HashedValue {
            algorithm: HashAlgorithm::Sha1,
            value: sha1,
        });
    }
    Ok(ResolvedArtifact::Download(ResolvedDownload {
        item_id: item.id.clone(),
        version_id: candidate.version.clone(),
        source: ArtifactSource::Download {
            url: candidate.download_url.clone(),
        },
        hashes: HashSpec { values: hashes },
        size: candidate.size.unwrap_or(0),
        filename: candidate.filename.clone(),
        metadata: ArtifactMetadata {
            source_type: SourceType::Curated,
            registry_id: Some(item.id.clone()),
            modrinth_id: item.modrinth_id.clone(),
            content_type: item.content_type.clone(),
        },
    }))
}

fn raw_modrinth_artifact(
    project_id: &str,
    candidate: &modrinth_raw::RawModrinthVersionCandidate,
) -> LauncherResult<ResolvedArtifact> {
    let mut hashes = Vec::new();
    if let Some(sha512) = valid_hash(candidate.sha512.as_deref(), 128) {
        hashes.push(HashedValue {
            algorithm: HashAlgorithm::Sha512,
            value: sha512,
        });
    }
    if let Some(sha1) = valid_hash(candidate.sha1.as_deref(), 40) {
        hashes.push(HashedValue {
            algorithm: HashAlgorithm::Sha1,
            value: sha1,
        });
    }
    if hashes.is_empty() {
        return Err(generic(
            "ERR_HASH_UNAVAILABLE",
            format!(
                "Modrinth did not publish a usable hash for {}.",
                candidate.filename
            ),
        ));
    }
    Ok(ResolvedArtifact::Download(ResolvedDownload {
        item_id: project_id.into(),
        version_id: candidate.version_id.clone(),
        source: ArtifactSource::Download {
            url: candidate.download_url.clone(),
        },
        hashes: HashSpec { values: hashes },
        size: candidate.size.unwrap_or(0),
        filename: candidate.filename.clone(),
        metadata: ArtifactMetadata {
            source_type: SourceType::Modrinth,
            registry_id: None,
            modrinth_id: Some(project_id.into()),
            content_type: "mod".into(),
        },
    }))
}

fn select_curated_candidate<'a>(
    candidates: &'a [ModVersionCandidate],
    requested: Option<&str>,
) -> LauncherResult<&'a ModVersionCandidate> {
    let requested = normalize_requested_version(requested);
    if let Some(requested) = requested {
        return candidates
            .iter()
            .find(|candidate| candidate.version == requested || candidate.filename == requested)
            .ok_or(LauncherError::VersionNotFound);
    }
    candidates
        .iter()
        .find(|candidate| candidate.is_compatible)
        .or_else(|| candidates.first())
        .ok_or(LauncherError::VersionNotFound)
}

fn select_modrinth_candidate<'a>(
    candidates: &'a [modrinth_raw::RawModrinthVersionCandidate],
    requested: Option<&str>,
) -> LauncherResult<&'a modrinth_raw::RawModrinthVersionCandidate> {
    let requested = normalize_requested_version(requested);
    if let Some(requested) = requested {
        return candidates
            .iter()
            .find(|candidate| {
                candidate.version_id == requested
                    || candidate.version == requested
                    || candidate.filename == requested
            })
            .ok_or(LauncherError::VersionNotFound);
    }
    candidates.first().ok_or(LauncherError::VersionNotFound)
}

fn normalize_requested_version(requested: Option<&str>) -> Option<&str> {
    requested
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "available" && *value != "latest")
}

fn valid_hash(value: Option<&str>, length: usize) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
}

fn is_platform_dependency(dependency: &str, loader: &str) -> bool {
    matches!(
        dependency,
        "minecraft" | "java" | "fabricloader" | "fabric_loader" | "quilt_loader" | "quilt-loader"
    ) || dependency.eq_ignore_ascii_case(loader)
        || (loader == "neoforge" && dependency == "forge")
}

fn find_installed_by_identity<'a>(
    manifest: &'a InstanceManifest,
    identity: &str,
) -> Option<&'a InstalledMod> {
    all_installed(manifest).find(|item| {
        item.registry_id
            .as_deref()
            .map(|id| id.eq_ignore_ascii_case(identity))
            .unwrap_or(false)
            || item
                .modrinth_id
                .as_deref()
                .map(|id| id.eq_ignore_ascii_case(identity))
                .unwrap_or(false)
            || item
                .mod_jar_id
                .as_deref()
                .map(|id| id.eq_ignore_ascii_case(identity))
                .unwrap_or(false)
    })
}

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

fn generic(code: impl Into<String>, message: impl Into<String>) -> LauncherError {
    LauncherError::Generic {
        code: code.into(),
        message: message.into(),
    }
}
