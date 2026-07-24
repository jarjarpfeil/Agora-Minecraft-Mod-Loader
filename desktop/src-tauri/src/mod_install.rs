use crate::auth;
use crate::error::{LauncherError, LauncherResult};
use crate::instances;
use crate::models::{InstalledMod, InstanceManifest, InstanceRow, ModVersionCandidate};
use crate::paths;
use crate::registry;
use std::path::Path;

/// Resolve instance info via core InstanceService.
pub fn load_instance_info(
    app: &tauri::AppHandle,
    instance_id: &str,
) -> LauncherResult<InstanceRow> {
    let ctx = crate::core_context(app)?;
    let svc = agora_core::instance_service::InstanceService::new(ctx);
    svc.get(instance_id)?
        .map(|detail| detail.row)
        .ok_or_else(|| LauncherError::Generic {
            code: "ERR_INSTANCE_NOT_FOUND".into(),
            message: format!("Instance '{instance_id}' not found."),
        })
}

/// Resolve a registry item via core RegistryService.
pub fn load_registry_item(
    app: &tauri::AppHandle,
    item_id: &str,
) -> LauncherResult<registry::RegistryItem> {
    let ctx = crate::core_context(app)?;
    let svc = agora_core::registry::RegistryService::new(ctx);
    svc.get_item_by_id(item_id)?
        .ok_or_else(|| LauncherError::Generic {
            code: "ERR_ITEM_NOT_FOUND".into(),
            message: format!("Registry item '{item_id}' not found."),
        })
}

/// Check instance is not locked via core InstallService.
pub(crate) fn check_not_locked(app: &tauri::AppHandle, instance_id: &str) -> LauncherResult<()> {
    let ctx = crate::core_context(app)?;
    let svc = agora_core::install_service::InstallService::new(ctx);
    svc.check_not_locked(instance_id)
}

/// List versions for a curated registry item via core Resolver.
pub async fn list_mod_versions(
    app: &tauri::AppHandle,
    instance_id: &str,
    item_id: &str,
) -> LauncherResult<Vec<ModVersionCandidate>> {
    let ctx = crate::core_context(app)?;
    let instance = load_instance_info(app, instance_id)?;
    let item = load_registry_item(app, item_id)?;
    let resolver = agora_core::resolver::Resolver::new(ctx);
    resolver
        .list_curated_versions(&item, &instance.minecraft_version, &instance.loader)
        .await
}

/// Resolve the bounded candidate set used by explicit automatic update checks.
pub async fn list_mod_versions_for_update(
    app: &tauri::AppHandle,
    instance_id: &str,
    item_id: &str,
) -> LauncherResult<Vec<ModVersionCandidate>> {
    let ctx = crate::core_context(app)?;
    let instance = load_instance_info(app, instance_id)?;
    let item = load_registry_item(app, item_id)?;
    make_resolver(ctx, app)
        .list_curated_versions_for_update(&item, &instance.minecraft_version, &instance.loader)
        .await
}

/// Quick compatibility badge via core Resolver.
pub async fn check_mod_compat(
    app: &tauri::AppHandle,
    instance_id: &str,
    item_id: &str,
) -> LauncherResult<String> {
    let ctx = crate::core_context(app)?;
    let instance = load_instance_info(app, instance_id)?;
    let item = load_registry_item(app, item_id)?;
    let resolver = agora_core::resolver::Resolver::new(ctx);
    list_curated_versions_tolerant(
        &resolver,
        &item,
        &instance.minecraft_version,
        &instance.loader,
    )
    .await
}

async fn list_curated_versions_tolerant(
    resolver: &agora_core::resolver::Resolver,
    item: &registry::RegistryItem,
    mc_version: &str,
    loader: &str,
) -> LauncherResult<String> {
    let candidates = resolver
        .list_curated_versions(item, mc_version, loader)
        .await
        .unwrap_or_default();
    Ok(candidates
        .iter()
        .map(|c| c.version_compat.as_str())
        .find(|c| !c.is_empty())
        .unwrap_or("")
        .to_string())
}

fn github_token(app: &tauri::AppHandle) -> Option<String> {
    auth::get_token(app)
}

fn make_resolver(
    ctx: agora_core::ctx::Ctx,
    app: &tauri::AppHandle,
) -> agora_core::resolver::Resolver {
    let base = agora_core::resolver::Resolver::new(ctx);
    match github_token(app) {
        Some(tok) => base.with_stored_github_token(tok),
        None => base,
    }
}

/// Bi-directional initial fetch: page 1 + last 3 pages via core Resolver.
pub async fn resolve_github_releases_initial(
    app: &tauri::AppHandle,
    item: &registry::RegistryItem,
    mc_version: &str,
    loader: &str,
) -> LauncherResult<(Vec<ModVersionCandidate>, u32, Vec<u32>)> {
    let ctx = crate::core_context(app)?;
    let resolver = make_resolver(ctx, app);
    resolver
        .fetch_github_releases_initial(&item.source_identifier, mc_version, loader)
        .await
}

/// Batch-fetch specific GitHub pages via core Resolver.
pub async fn fetch_github_versions_batch(
    app: &tauri::AppHandle,
    source: &str,
    mc_version: &str,
    loader: &str,
    pages: &[u32],
) -> LauncherResult<Vec<(u32, Vec<ModVersionCandidate>)>> {
    let ctx = crate::core_context(app)?;
    let resolver = make_resolver(ctx, app);
    resolver
        .fetch_github_versions_batch(source, mc_version, loader, pages)
        .await
}

/// Install a mod version via core InstallService.
pub async fn install_mod_version(
    app: &tauri::AppHandle,
    instance_id: &str,
    item_id: &str,
    candidate: &ModVersionCandidate,
) -> LauncherResult<InstalledMod> {
    let ctx = crate::core_context(app)?;
    let item = load_registry_item(app, item_id)?;
    let content_type = if item.content_type.is_empty() {
        "mod"
    } else {
        &item.content_type
    };
    let pinned = item.sha256.trim();
    let exp_sha256 = if item.download_strategy == "github_release" {
        None
    } else if !pinned.is_empty() {
        Some(pinned)
    } else {
        candidate.sha256.as_deref()
    };
    let svc = agora_core::install_service::InstallService::new(ctx);
    svc.install_artifact(
        instance_id,
        &candidate.filename,
        content_type,
        &candidate.download_url,
        Some(item_id),
        item.modrinth_id.as_deref(),
        "registry",
        Some(&candidate.version),
        candidate.sha1.as_deref(),
        exp_sha256,
    )
    .await
}

/// Remove artifact via core InstallService.
pub async fn remove_mod_from_instance(
    app: &tauri::AppHandle,
    instance_id: &str,
    filename: &str,
) -> LauncherResult<()> {
    let ctx = crate::core_context(app)?;
    let (iid, fn_own) = (instance_id.to_string(), filename.to_string());
    let removed = tokio::task::spawn_blocking(move || {
        let svc = agora_core::install_service::InstallService::new(ctx);
        svc.remove_artifact(&iid, &fn_own)
    })
    .await
    .map_err(|_| LauncherError::Generic {
        code: "ERR_REMOVE_FAILED".into(),
        message: "Remove file task failed.".into(),
    })??;
    auth::log_line(&format!(
        "remove_mod_from_instance: file '{filename}' removed={removed}"
    ));
    Ok(())
}

/// Disable artifact via core CrashService.
pub fn disable_instance_mod(
    app: &tauri::AppHandle,
    instance_id: &str,
    filename: &str,
) -> LauncherResult<()> {
    let ctx = crate::core_context(app)?;
    let svc = agora_core::crash_service::CrashService::new(ctx);
    svc.disable_artifact(instance_id, filename)
}

/// Enable artifact via core CrashService.
pub fn enable_instance_mod(
    app: &tauri::AppHandle,
    instance_id: &str,
    filename: &str,
) -> LauncherResult<()> {
    let ctx = crate::core_context(app)?;
    let svc = agora_core::crash_service::CrashService::new(ctx);
    svc.enable_artifact(instance_id, filename)
}

/// Add manual .jar via core InstallService.
pub async fn add_manual_mod(
    app: &tauri::AppHandle,
    instance_id: &str,
    source_path: &str,
) -> LauncherResult<InstalledMod> {
    let ctx = crate::core_context(app)?;
    let (iid, sp) = (instance_id.to_string(), source_path.to_string());
    tokio::task::spawn_blocking(move || {
        let svc = agora_core::install_service::InstallService::new(ctx);
        svc.add_manual_artifact(&iid, &sp)
    })
    .await
    .map_err(|_| LauncherError::Generic {
        code: "ERR_MANIFEST_WRITE".into(),
        message: "Manual mod add task failed.".into(),
    })?
}

/// Export pack via core ExportService.
pub async fn export_instance_pack(
    app: &tauri::AppHandle,
    instance_id: &str,
    format: &str,
) -> LauncherResult<String> {
    let ctx = crate::core_context(app)?;
    let manifest_path = paths::instance_manifest_path(app, instance_id)
        .map_err(|_| LauncherError::InstanceCreateFailed)?;
    if !manifest_path.exists() {
        return Err(LauncherError::Generic {
            code: "ERR_MANIFEST_MISSING".into(),
            message: "Instance manifest not found.".into(),
        });
    }
    let manifest: InstanceManifest = {
        let text = std::fs::read_to_string(&manifest_path)
            .map_err(|_| LauncherError::InstanceCreateFailed)?;
        serde_json::from_str(&text).map_err(|_| LauncherError::InstanceCreateFailed)?
    };
    let instance_dir = ctx.paths.instance_dir(instance_id)?;
    let exports_dir = ctx.paths.root().join("exports");
    agora_core::export_service::export_instance_pack(&instance_dir, &manifest, &exports_dir, format)
        .await
}

/// Import a pack file.  .mrpack → core ImportService.  .agora-pack.json → local orchestrator.
pub async fn import_instance_pack(
    app: &tauri::AppHandle,
    source_path: &str,
) -> LauncherResult<String> {
    let lower = source_path.to_ascii_lowercase();
    if lower.ends_with(".mrpack") {
        import_mrpack(app, source_path).await
    } else if lower.ends_with(".json") || lower.ends_with(".agora-pack.json") {
        import_agora_json(app, source_path).await
    } else {
        Err(LauncherError::Generic {
            code: "ERR_INVALID_FORMAT".into(),
            message: "Unsupported pack file extension. Use .mrpack or .agora-pack.json.".into(),
        })
    }
}

async fn import_mrpack(app: &tauri::AppHandle, source_path: &str) -> LauncherResult<String> {
    let ctx = crate::core_context(app)?;
    let svc = agora_core::import_service::ImportService::new(ctx);
    let request = agora_core::import_service::ImportRequest {
        source: agora_core::import_service::ImportSource::Mrpack(
            Path::new(source_path).to_path_buf(),
        ),
        symlink_saves: false,
    };
    let result = svc.run_import(request).await?;
    Ok(result.instance_id)
}

/// Import an Agora plain-JSON pack (.agora-pack.json).
async fn import_agora_json(app: &tauri::AppHandle, source_path: &str) -> LauncherResult<String> {
    let ctx = crate::core_context(app)?;
    let text = std::fs::read_to_string(source_path).map_err(|_| LauncherError::Generic {
        code: "ERR_PACK_READ".into(),
        message: format!("Cannot read pack file: {source_path}"),
    })?;
    let pack: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| LauncherError::Generic {
            code: "ERR_PACK_PARSE".into(),
            message: "Failed to parse agora-pack JSON.".into(),
        })?;
    let inst = pack.get("instance").ok_or_else(|| LauncherError::Generic {
        code: "ERR_PACK_PARSE".into(),
        message: "agora-pack missing 'instance' object.".into(),
    })?;
    let mc_version = inst
        .get("minecraft_version")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| LauncherError::Generic {
            code: "ERR_PACK_PARSE".into(),
            message: "agora-pack missing minecraft_version.".into(),
        })?;
    let loader = inst
        .get("loader")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| LauncherError::Generic {
            code: "ERR_PACK_PARSE".into(),
            message: "agora-pack missing loader.".into(),
        })?;
    let loader_version = inst
        .get("loader_version")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| LauncherError::Generic {
            code: "ERR_PACK_PARSE".into(),
            message: "agora-pack missing loader_version.".into(),
        })?;
    let name = inst
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| {
            inst.get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "imported-pack".to_string());
    let instance_id = paths::sanitize_id(inst.get("id").and_then(|v| v.as_str()).unwrap_or(&name));
    let instance_id = if instance_id.is_empty() {
        "imported-pack".into()
    } else {
        instance_id
    };
    let req = instances::CreateInstanceRequest {
        name: name.clone(),
        instance_id: instance_id.clone(),
        minecraft_version: mc_version.to_string(),
        loader: loader.to_string(),
        loader_version: loader_version.to_string(),
        jvm_memory_mb: Some(4096),
        jvm_gc: None,
        jvm_custom_args: None,
        jvm_always_pre_touch: None,
    };
    instances::create_instance(app.clone(), req).await?;
    if let Some(mods_arr) = pack.get("mods").and_then(|m| m.as_array()) {
        for entry in mods_arr {
            if let Some(rid) = entry
                .get("registry_id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                let filename = entry.get("filename").and_then(|v| v.as_str()).unwrap_or("");
                let candidates = list_mod_versions(app, &instance_id, rid)
                    .await
                    .unwrap_or_default();
                let candidate = candidates
                    .iter()
                    .find(|c| c.filename == filename)
                    .or_else(|| {
                        entry
                            .get("version")
                            .and_then(|v| v.as_str())
                            .and_then(|v| candidates.iter().find(|c| c.version == v))
                    });
                if let Some(c) = candidate {
                    let _ = install_mod_version(app, &instance_id, rid, c).await;
                }
            } else if let Some(mid) = entry
                .get("modrinth_id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
            {
                let candidates = crate::modrinth_raw::list_raw_modrinth_versions(
                    &ctx.http_clients,
                    app,
                    Some(&instance_id),
                    mid,
                    Some("mod"),
                )
                .await
                .unwrap_or_default();
                let candidate = candidates
                    .iter()
                    .find(|c| c.primary)
                    .or_else(|| candidates.first());
                if let Some(c) = candidate {
                    let _ =
                        crate::modrinth_raw::install_raw_modrinth(app, &instance_id, mid, c, "mod")
                            .await;
                }
            }
        }
    }
    Ok(instance_id)
}
