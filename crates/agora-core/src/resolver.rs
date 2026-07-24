//! Core-owned install plan resolver.
//!
//! Transforms an [`InstallIntent`] into a [`PreparedPlan`] with fully-resolved
//! artifacts, dependency dispositions, and conflicts — without requiring Tauri
//! or desktop types.
//!
//! The resolver handles:
//! - Curated (GitHub Release / Modrinth-id) artifact resolution
//! - Raw Modrinth (uncurated) artifact resolution
//! - Manual local file resolution
//! - Dependency BFS traversal for both curated and Modrinth dep graphs
//! - Registry known-conflict checking
//! - Artifact construction with hash specs

use crate::ctx::Ctx;
use crate::dependency_ops::{AliasMap, DepSource, Requirement};
use crate::download;
use crate::error::{LauncherError, LauncherResult};
use crate::github_ratelimit;
use crate::http_client::{self, ClientCategory, HttpClients};
use crate::install_pipeline::{
    ArtifactMetadata, ArtifactSource, ConflictKind, ConflictResolution, DepConflict,
    DepDisposition, HashAlgorithm, HashSpec, HashedValue, InstallAction, InstallIntent,
    PreparedPlan, ResolvedArtifact, ResolvedDep, ResolvedDownload, ResolvedLocal,
    ResolvedOperation, SourceType,
};
use crate::models::{InstalledMod, InstanceManifest, ModVersionCandidate};
use crate::registry::{self, ManifestDeps};
use serde::Deserialize;
use sha2::Digest;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::Path;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A candidate version returned from raw Modrinth API (with dependency info).
#[derive(Debug, Clone)]
pub struct RawModrinthVersionCandidate {
    pub version: String,
    pub version_id: String,
    pub name: String,
    pub filename: String,
    pub download_url: String,
    pub sha1: Option<String>,
    pub sha512: Option<String>,
    pub size: Option<u64>,
    pub mc_versions: Vec<String>,
    pub loaders: Vec<String>,
    pub release_date: Option<String>,
    pub primary: bool,
    pub changelog: Option<String>,
    pub dependencies: Vec<RawModrinthDep>,
}

/// A dependency declared in a raw Modrinth version.
#[derive(Debug, Clone)]
pub struct RawModrinthDep {
    pub project_id: Option<String>,
    pub version_id: Option<String>,
    pub dependency_type: String,
}

// ---------------------------------------------------------------------------
// Private Modrinth API response types (with dependency info)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ModrinthApiVersion {
    id: String,
    name: Option<String>,
    version_number: String,
    date_published: Option<String>,
    game_versions: Option<Vec<String>>,
    loaders: Option<Vec<String>>,
    files: Vec<ModrinthApiFile>,
    #[serde(default)]
    dependencies: Vec<ModrinthApiDep>,
    #[serde(default)]
    changelog: Option<String>,
}

#[derive(Deserialize)]
struct ModrinthApiFile {
    url: String,
    filename: String,
    primary: bool,
    hashes: Option<ModrinthApiHashes>,
    #[serde(default)]
    size: Option<u64>,
}

#[derive(Deserialize)]
struct ModrinthApiHashes {
    sha1: Option<String>,
    sha512: Option<String>,
}

#[derive(Deserialize)]
struct ModrinthApiDep {
    project_id: Option<String>,
    version_id: Option<String>,
    dependency_type: Option<String>,
}

// ---------------------------------------------------------------------------
// Private GitHub Release API types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    published_at: Option<String>,
    assets: Vec<GitHubReleaseAsset>,
}

#[derive(Deserialize)]
struct GitHubReleaseAsset {
    name: String,
    #[allow(dead_code)]
    browser_download_url: String,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    digest: Option<String>,
}

// ---------------------------------------------------------------------------
// Resolver
// ---------------------------------------------------------------------------

/// Core-owned resolver that prepares [`PreparedPlan`] from an [`InstallIntent`].
#[derive(Clone)]
pub struct Resolver {
    ctx: Ctx,
    github_token: Option<String>,
    clear_stored_github_token_on_unauthorized: bool,
}

impl Resolver {
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            github_token: None,
            clear_stored_github_token_on_unauthorized: false,
        }
    }

    pub fn with_github_token(mut self, token: String) -> Self {
        self.github_token = Some(token);
        self
    }

    /// Attach a token read from Agora's secure credential store. If GitHub
    /// rejects it, clear the stored credential before retrying anonymously.
    pub fn with_stored_github_token(mut self, token: String) -> Self {
        self.github_token = Some(token);
        self.clear_stored_github_token_on_unauthorized = true;
        self
    }

    // ------------------------------------------------------------------
    // Top-level dispatch
    // ------------------------------------------------------------------

    /// Resolve an intent into a [`PreparedPlan`].
    pub async fn resolve(
        &self,
        intent: &InstallIntent,
        manifest: &InstanceManifest,
    ) -> LauncherResult<PreparedPlan> {
        let revision = self.compute_registry_revision()?;

        match &intent.action {
            InstallAction::Install {
                source_type,
                item_id,
                candidate_version,
            } => match source_type {
                SourceType::Curated => {
                    self.resolve_curated_install(
                        manifest,
                        item_id,
                        candidate_version.as_deref(),
                        revision,
                        false,
                    )
                    .await
                }
                SourceType::Modrinth => {
                    self.resolve_raw_modrinth_install(
                        manifest,
                        item_id,
                        candidate_version.as_deref(),
                        revision,
                        false,
                    )
                    .await
                }
                SourceType::Manual => {
                    resolve_manual_install(item_id, candidate_version.as_deref(), revision)
                }
            },
            InstallAction::Update {
                item_id,
                target_version,
            } => {
                let installed = find_installed_by_identity(manifest, item_id).ok_or_else(|| {
                    LauncherError::Generic {
                        code: "ERR_UPDATE_TARGET_MISSING".into(),
                        message: format!("{item_id} is not installed in this instance."),
                    }
                })?;
                if installed.source == "modrinth_raw" {
                    let project_id = installed.modrinth_id.as_deref().unwrap_or(item_id);
                    self.resolve_raw_modrinth_install(
                        manifest,
                        project_id,
                        normalize_requested_version(Some(target_version)),
                        revision,
                        true,
                    )
                    .await
                } else {
                    let registry_id = installed.registry_id.as_deref().unwrap_or(item_id);
                    self.resolve_curated_install(
                        manifest,
                        registry_id,
                        normalize_requested_version(Some(target_version)),
                        revision,
                        true,
                    )
                    .await
                }
            }
            InstallAction::Remove { filename } => {
                Ok(crate::install_service::InstallService::prepare_removal(
                    manifest, filename, revision,
                ))
            }
            InstallAction::BatchUpdate { items } => {
                self.resolve_batch_update(manifest, items, revision).await
            }
            InstallAction::BatchInstall { items } => {
                self.resolve_batch_install(manifest, items, revision).await
            }
            InstallAction::RepairLockfile { .. } => Err(LauncherError::Generic {
                code: "ERR_LOCKFILE_COMMAND".into(),
                message: "Lockfile repair must be prepared by the verified lockfile command."
                    .into(),
            }),
        }
    }

    // ------------------------------------------------------------------
    // Registry revision
    // ------------------------------------------------------------------

    fn compute_registry_revision(&self) -> LauncherResult<String> {
        let path = self.ctx.paths.registry_db();
        if !path.is_file() {
            return Ok("registry-unavailable".into());
        }
        let bytes = std::fs::read(&path).map_err(|e| LauncherError::Generic {
            code: "ERR_REGISTRY_READ".into(),
            message: format!("Could not read registry: {e}"),
        })?;
        let mut hasher = sha2::Sha256::new();
        hasher.update(bytes);
        Ok(format!("{:x}", hasher.finalize()))
    }

    // ------------------------------------------------------------------
    // Curated install resolution
    // ------------------------------------------------------------------

    async fn resolve_curated_install(
        &self,
        manifest: &InstanceManifest,
        item_id: &str,
        requested_version: Option<&str>,
        registry_revision: String,
        update: bool,
    ) -> LauncherResult<PreparedPlan> {
        let item = {
            let conn = open_registry_db(&self.ctx.paths.registry_db())?;
            registry::get_item_by_id(&conn, item_id)?.ok_or_else(|| LauncherError::Generic {
                code: "ERR_ITEM_NOT_FOUND".into(),
                message: format!("Registry item '{item_id}' not found."),
            })?
        };

        let mc_version = &manifest.minecraft_version;
        let loader = &manifest.loader;
        let candidates = self
            .list_curated_versions(&item, mc_version, loader)
            .await?;
        let candidate = select_curated_candidate(&candidates, requested_version)?;
        let artifact = curated_artifact(&item, candidate)?;
        let (dependencies, conflicts) =
            self.resolve_curated_dependencies(manifest, item_id).await?;

        let operation = if update {
            let installed = find_installed_by_identity(manifest, item_id).ok_or_else(|| {
                LauncherError::Generic {
                    code: "ERR_UPDATE_TARGET_MISSING".into(),
                    message: format!("{item_id} is not installed."),
                }
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

    async fn resolve_curated_dependencies(
        &self,
        manifest: &InstanceManifest,
        root_item_id: &str,
    ) -> LauncherResult<(Vec<ResolvedDep>, Vec<DepConflict>)> {
        let (dependency_map, aliases, known_conflicts) = {
            let conn = open_registry_db(&self.ctx.paths.registry_db())?;
            let dependency_map = registry::get_all_manifest_dependencies(&conn)?;
            let alias_pairs = registry::get_all_mod_aliases(&conn)?;
            let known_conflicts = registry::get_known_conflicts(&conn)?;
            (
                dependency_map,
                AliasMap::from_pairs(&alias_pairs),
                known_conflicts,
            )
        };

        let installed: Vec<&InstalledMod> = all_installed(manifest).collect();
        let installed_ids: BTreeMap<String, &&InstalledMod> = installed
            .iter()
            .flat_map(|item| {
                let ids: [Option<&str>; 3] = [
                    item.registry_id.as_deref(),
                    item.modrinth_id.as_deref(),
                    item.mod_jar_id.as_deref(),
                ];
                ids.into_iter()
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
                            installed_filename: effective_installed_filename(installed),
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

            let disposition = self.load_curated_dep(&canonical, manifest).await;
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
        let conflicts =
            build_known_conflicts(&known_conflicts, &aliases, &incoming, &installed_set);

        Ok((resolved.into_values().collect(), conflicts))
    }

    async fn load_curated_dep(&self, item_id: &str, manifest: &InstanceManifest) -> DepDisposition {
        let item = {
            let conn = match open_registry_db(&self.ctx.paths.registry_db()) {
                Ok(c) => c,
                Err(e) => {
                    return DepDisposition::Unresolved {
                        reason: e.to_string(),
                    }
                }
            };
            match registry::get_item_by_id(&conn, item_id) {
                Ok(Some(item)) => item,
                Ok(None) => {
                    return DepDisposition::Unresolved {
                        reason: format!("Registry item '{item_id}' not found."),
                    }
                }
                Err(e) => {
                    return DepDisposition::Unresolved {
                        reason: e.to_string(),
                    }
                }
            }
        };

        let candidates = match self
            .list_curated_versions(&item, &manifest.minecraft_version, &manifest.loader)
            .await
        {
            Ok(c) => c,
            Err(e) => {
                return DepDisposition::Unresolved {
                    reason: e.to_string(),
                }
            }
        };

        match select_curated_candidate(&candidates, None) {
            Ok(candidate) => match curated_artifact(&item, candidate) {
                Ok(artifact) => DepDisposition::InstallCandidate { artifact },
                Err(e) => DepDisposition::Unresolved {
                    reason: e.to_string(),
                },
            },
            Err(e) => DepDisposition::Unresolved {
                reason: e.to_string(),
            },
        }
    }

    // ------------------------------------------------------------------
    // GitHub Releases version list
    // ------------------------------------------------------------------

    /// Compute which tail pages to fetch after page 1 for the bi-directional
    /// initial fetch heuristic.
    ///
    /// When page 1 has no compatible candidates and there are multiple pages,
    /// returns up to 3 oldest pages (highest page numbers) that are most
    /// likely to contain versions matching an older MC version.
    pub fn compute_tail_pages(total_pages: u32, page1_has_compatible: bool) -> Vec<u32> {
        if total_pages <= 1 || page1_has_compatible {
            return vec![];
        }
        let mut pages: Vec<u32> = (2..=total_pages).rev().collect();
        pages.truncate(3);
        pages
    }

    /// Bi-directional initial fetch: page 1 + tail pages via core Resolver.
    ///
    /// Fetches the first page (newest releases). If no compatible candidate
    /// is found and more pages exist, also fetches the last few pages
    /// (oldest releases) which are most likely to match older MC versions.
    /// Results are sorted by compatibility then release date.
    pub async fn fetch_github_releases_initial(
        &self,
        source: &str,
        mc_version: &str,
        loader: &str,
    ) -> LauncherResult<(Vec<ModVersionCandidate>, u32, Vec<u32>)> {
        let (page1, total_pages) = self
            .fetch_github_releases_page(source, mc_version, loader, 1)
            .await?;
        let mut all = page1;
        let mut pages_fetched = vec![1u32];
        let page1_has_compatible = all.iter().any(|c| c.is_compatible);
        let tail = Self::compute_tail_pages(total_pages, page1_has_compatible);
        for &p in &tail {
            if let Ok((cands, _)) = self
                .fetch_github_releases_page(source, mc_version, loader, p)
                .await
            {
                pages_fetched.push(p);
                all.extend(cands);
            }
        }
        sort_versions_by_compatibility(&mut all);
        Ok((all, total_pages, pages_fetched))
    }

    /// Batch-fetch specific GitHub pages concurrently.
    ///
    /// Results preserve page order from the input slice. Individual page
    /// failures are tolerated and skipped (only success responses are
    /// returned).
    pub async fn fetch_github_versions_batch(
        &self,
        source: &str,
        mc_version: &str,
        loader: &str,
        pages: &[u32],
    ) -> LauncherResult<Vec<(u32, Vec<ModVersionCandidate>)>> {
        let mut handles = Vec::new();
        for &p in pages {
            let mc = mc_version.to_owned();
            let ld = loader.to_owned();
            let src = source.to_owned();
            let resolver = self.clone();
            handles.push(tokio::spawn(async move {
                resolver
                    .fetch_github_releases_page(&src, &mc, &ld, p)
                    .await
                    .map(|(c, _)| (p, c))
                    .map_err(|e| e.to_string())
            }));
        }
        let mut results = Vec::new();
        for handle in handles {
            match handle.await {
                Ok(Ok((p, cands))) => results.push((p, cands)),
                Ok(Err(e)) => {
                    eprintln!("fetch_github_versions_batch: page failed: {e}");
                }
                Err(e) => {
                    eprintln!("fetch_github_versions_batch: task join failed: {e}");
                }
            }
        }
        Ok(results)
    }

    /// List curated versions for a registry item, filtered by MC version and loader.
    pub async fn list_curated_versions(
        &self,
        item: &crate::registry::RegistryItem,
        mc_version: &str,
        loader: &str,
    ) -> LauncherResult<Vec<ModVersionCandidate>> {
        let has_modrinth = item.modrinth_id.as_deref().is_some_and(|id| !id.is_empty());

        match item.download_strategy.as_str() {
            "github_release" => {
                let primary = self
                    .fetch_all_github_releases(&item.source_identifier, mc_version, loader)
                    .await;
                let candidates = match primary {
                    Ok(c) if !c.is_empty() => return Ok(c),
                    Ok(_) => Vec::new(),
                    Err(_) => Vec::new(),
                };
                if has_modrinth {
                    let alt = fetch_modrinth_versions_for_item(
                        &self.ctx.http_clients,
                        &item.source_identifier,
                        item.modrinth_id.as_deref(),
                        mc_version,
                        loader,
                    )
                    .await?;
                    if !alt.is_empty() {
                        return Ok(alt);
                    }
                }
                Ok(candidates)
            }
            "modrinth_id" => {
                fetch_modrinth_versions_for_item(
                    &self.ctx.http_clients,
                    &item.source_identifier,
                    None,
                    mc_version,
                    loader,
                )
                .await
            }
            _ => Err(LauncherError::Generic {
                code: "ERR_UNSUPPORTED_STRATEGY".into(),
                message: format!(
                    "Download strategy '{}' is not supported for version resolution.",
                    item.download_strategy
                ),
            }),
        }
    }

    /// Resolve candidates for an automatic update check without walking the
    /// complete GitHub release history. The interactive Versions tab can load
    /// older pages on demand; background checks use page one plus up to three
    /// tail pages instead.
    pub async fn list_curated_versions_for_update(
        &self,
        item: &crate::registry::RegistryItem,
        mc_version: &str,
        loader: &str,
    ) -> LauncherResult<Vec<ModVersionCandidate>> {
        if item.download_strategy != "github_release" {
            return self.list_curated_versions(item, mc_version, loader).await;
        }

        let has_modrinth = item.modrinth_id.as_deref().is_some_and(|id| !id.is_empty());
        let primary = match self
            .fetch_github_releases_initial(&item.source_identifier, mc_version, loader)
            .await
        {
            Ok((candidates, _, _)) => candidates,
            Err(_) => Vec::new(),
        };
        if !primary.is_empty() {
            return Ok(primary);
        }
        if has_modrinth {
            return fetch_modrinth_versions_for_item(
                &self.ctx.http_clients,
                &item.source_identifier,
                item.modrinth_id.as_deref(),
                mc_version,
                loader,
            )
            .await;
        }
        Ok(primary)
    }

    /// Fetch all GitHub release pages for a source, returning candidates filtered
    /// and sorted by compatibility.
    pub async fn fetch_all_github_releases(
        &self,
        source: &str,
        mc_version: &str,
        loader: &str,
    ) -> LauncherResult<Vec<ModVersionCandidate>> {
        let mut all = Vec::new();
        let mut page: u32 = 1;
        let max_pages = 50;
        let mut total_pages = 1;

        loop {
            if page > max_pages || page > total_pages {
                break;
            }
            match self
                .fetch_github_releases_page(source, mc_version, loader, page)
                .await
            {
                Ok((candidates, reported_total_pages)) => {
                    all.extend(candidates);
                    total_pages = reported_total_pages.max(page);
                    if page >= total_pages {
                        break;
                    }
                }
                Err(_) => {
                    break;
                }
            }
            page += 1;
        }

        sort_versions_by_compatibility(&mut all);
        Ok(all)
    }

    /// Fetch a single page of GitHub releases.
    pub async fn fetch_github_releases_page(
        &self,
        source: &str,
        mc_version: &str,
        loader: &str,
        page: u32,
    ) -> LauncherResult<(Vec<ModVersionCandidate>, u32)> {
        let url =
            format!("https://api.github.com/repos/{source}/releases?per_page=100&page={page}");

        let headers = github_auth_headers(self.github_token.as_deref());
        let mut response = self.send_github_releases_request(&url, &headers).await?;

        // Release listings are public. A stale or malformed stored token must
        // not turn a public request into a hard failure, and must not be
        // retried with the same invalid Authorization header.
        if response.status() == reqwest::StatusCode::UNAUTHORIZED && self.github_token.is_some() {
            if self.clear_stored_github_token_on_unauthorized {
                let _ = crate::auth::clear_token();
            }
            response = self.send_github_releases_request(&url, &[]).await?;
        }

        if github_ratelimit::is_rate_limit_response(&response) {
            let retry = github_ratelimit::parse_retry_after(&response);
            github_ratelimit::report_rate_limit(retry).await;
            return Err(LauncherError::Generic {
                code: "ERR_RATE_LIMITED".into(),
                message: format!("GitHub rate limit hit while fetching releases for {source}."),
            });
        }

        let link_value = response
            .headers()
            .get("link")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let releases: Vec<GitHubRelease> = response
            .error_for_status()
            .map_err(|e| LauncherError::Generic {
                code: "ERR_NETWORK".into(),
                message: format!("GitHub API request failed: {e}"),
            })?
            .json()
            .await
            .map_err(|_| LauncherError::Generic {
                code: "ERR_NETWORK".into(),
                message: "Failed to parse GitHub releases response.".into(),
            })?;

        let total_pages = parse_link_total_pages(link_value.as_deref());
        let mut candidates: Vec<ModVersionCandidate> = Vec::new();

        for release in &releases {
            for asset in &release.assets {
                if !is_installable_github_asset(&asset.name) {
                    continue;
                }
                let (found_mc, loader_str, compat) = parse_version_from_github_asset(
                    &asset.name,
                    &release.tag_name,
                    mc_version,
                    loader,
                );

                let download_url = format!(
                    "https://github.com/{source}/releases/download/{tag}/{asset_name}",
                    tag = urlencoding::encode(&release.tag_name),
                    asset_name = asset.name,
                );

                candidates.push(ModVersionCandidate {
                    version: release.tag_name.clone(),
                    filename: asset.name.clone(),
                    download_url,
                    mc_version: found_mc,
                    loader: loader_str,
                    release_date: release.published_at.clone(),
                    is_compatible: compat == "compatible",
                    version_compat: compat.to_string(),
                    sha1: None,
                    sha256: asset
                        .digest
                        .as_deref()
                        .and_then(|digest| digest.strip_prefix("sha256:"))
                        .map(str::to_string),
                    sha512: None,
                    size: asset.size,
                });
            }
        }

        Ok((candidates, total_pages))
    }

    async fn send_github_releases_request(
        &self,
        url: &str,
        headers: &[(String, String)],
    ) -> LauncherResult<reqwest::Response> {
        let _permit = crate::github_ratelimit::acquire_github_permit().await;
        crate::http_client::checked_send(
            &self.ctx.http_clients,
            ClientCategory::GitHub,
            reqwest::Method::GET,
            url,
            headers,
            None,
            None,
        )
        .await
    }

    // ------------------------------------------------------------------
    // Raw Modrinth install resolution
    // ------------------------------------------------------------------

    async fn resolve_raw_modrinth_install(
        &self,
        manifest: &InstanceManifest,
        project_id: &str,
        requested_version: Option<&str>,
        registry_revision: String,
        update: bool,
    ) -> LauncherResult<PreparedPlan> {
        let candidates = self
            .list_raw_modrinth_versions(manifest, project_id)
            .await?;
        let candidate = select_raw_modrinth_candidate(&candidates, requested_version)?;
        let artifact = raw_modrinth_artifact(project_id, candidate)?;
        // A multi-loader Modrinth version may advertise compatibility-route
        // dependencies (for example Connector for NeoForge) at the version
        // level. Once the verified JAR is available, its active-loader-native
        // metadata is the authoritative dependency source.
        let native_metadata = self.native_loader_metadata(manifest, candidate).await;
        let dependencies = self
            .resolve_raw_modrinth_deps(manifest, candidate, native_metadata.as_ref())
            .await;

        let operation = if update {
            let installed = find_installed_by_identity(manifest, project_id).ok_or_else(|| {
                LauncherError::Generic {
                    code: "ERR_UPDATE_TARGET_MISSING".into(),
                    message: format!("{project_id} is not installed."),
                }
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

    /// List raw Modrinth versions for a project, filtered by MC version and loader.
    pub async fn list_raw_modrinth_versions(
        &self,
        manifest: &InstanceManifest,
        project_id: &str,
    ) -> LauncherResult<Vec<RawModrinthVersionCandidate>> {
        let url = format!(
            "https://api.modrinth.com/v2/project/{pid}/version?game_versions=[\"{gv}\"]&loaders=[\"{ld}\"]",
            pid = urlencoding::encode(project_id),
            gv = urlencoding::encode(&manifest.minecraft_version),
            ld = urlencoding::encode(&manifest.loader),
        );

        let versions: Vec<ModrinthApiVersion> =
            http_client::checked_get_json(&self.ctx.http_clients, ClientCategory::Modrinth, &url)
                .await?;

        Ok(versions
            .into_iter()
            .map(|v| {
                let primary_file = v
                    .files
                    .iter()
                    .find(|f| f.primary)
                    .or_else(|| v.files.first());
                let (filename, download_url, sha1, file_size) = match primary_file {
                    Some(f) => (
                        f.filename.clone(),
                        f.url.clone(),
                        f.hashes.as_ref().and_then(|h| h.sha1.clone()),
                        f.size,
                    ),
                    None => (String::new(), String::new(), None, None),
                };
                RawModrinthVersionCandidate {
                    version: v.version_number,
                    version_id: v.id,
                    name: v.name.unwrap_or_default(),
                    filename,
                    download_url,
                    sha1,
                    sha512: primary_file
                        .and_then(|f| f.hashes.as_ref())
                        .and_then(|h| h.sha512.clone()),
                    size: file_size,
                    mc_versions: v.game_versions.unwrap_or_default(),
                    loaders: v
                        .loaders
                        .unwrap_or_default()
                        .into_iter()
                        .map(|l| l.to_lowercase())
                        .collect(),
                    release_date: v.date_published,
                    primary: primary_file.map(|f| f.primary).unwrap_or(false),
                    changelog: v.changelog,
                    dependencies: v
                        .dependencies
                        .into_iter()
                        .filter_map(|d| {
                            d.dependency_type.map(|dt| RawModrinthDep {
                                project_id: d.project_id,
                                version_id: d.version_id,
                                dependency_type: dt,
                            })
                        })
                        .collect(),
                }
            })
            .filter(|c| !c.download_url.is_empty())
            .collect())
    }

    /// Download and verify a selected Modrinth artifact solely to inspect its
    /// active-loader metadata during dependency resolution. A failed download
    /// or missing/mismatched API hash deliberately falls back to the Modrinth
    /// version dependency list rather than inventing a dependency result.
    async fn native_loader_metadata(
        &self,
        manifest: &InstanceManifest,
        candidate: &RawModrinthVersionCandidate,
    ) -> Option<crate::dependency_ops::JarDeps> {
        let expected_sha1 = candidate.sha1.as_deref()?.trim();
        if expected_sha1.is_empty() {
            return None;
        }
        let bytes =
            crate::download::download_mod_bytes(&self.ctx.http_clients, &candidate.download_url)
                .await
                .ok()?;
        if !crate::download::sha1_hex(&bytes).eq_ignore_ascii_case(expected_sha1) {
            return None;
        }

        let parsed =
            crate::jar_metadata::parse_jar_metadata_bytes_for_loader(&bytes, &manifest.loader);
        parsed.has_native_metadata.then_some(parsed.metadata)
    }

    async fn resolve_raw_modrinth_deps(
        &self,
        manifest: &InstanceManifest,
        root: &RawModrinthVersionCandidate,
        root_native_metadata: Option<&crate::dependency_ops::JarDeps>,
    ) -> Vec<ResolvedDep> {
        let installed_ids: HashSet<String> = all_installed(manifest)
            .filter_map(|item| item.modrinth_id.as_ref())
            .map(|id| id.to_ascii_lowercase())
            .collect();

        let mut queue = VecDeque::new();
        for dep in effective_raw_modrinth_dependencies(root, root_native_metadata) {
            let requirement = match dep.dependency_type.as_str() {
                "required" => Requirement::Required,
                "optional" => Requirement::Optional,
                _ => continue,
            };
            queue.push_back((dep.project_id.clone(), dep.version_id.clone(), requirement));
        }

        let mut expanded = BTreeMap::<String, Requirement>::new();
        let mut resolved = BTreeMap::<String, ResolvedDep>::new();

        while let Some((project_id, version_id, requirement)) = queue.pop_front() {
            let Some(pid) = project_id else {
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
            let key = pid.to_ascii_lowercase();
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
            expanded.insert(key.clone(), requirement);

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
                        mod_jar_id: pid.clone(),
                        requirement,
                        source: DepSource::Manifest,
                        disposition: DepDisposition::ReuseExisting {
                            mod_jar_id: pid,
                            installed_filename: installed
                                .map(effective_installed_filename)
                                .unwrap_or_else(|| "installed".into()),
                        },
                    },
                );
                continue;
            }

            let candidates = self.list_raw_modrinth_versions(manifest, &pid).await.ok();
            let (disposition, child_deps) = match candidates {
                Some(candidates) => {
                    match select_raw_modrinth_candidate(&candidates, version_id.as_deref()) {
                        Ok(candidate) => {
                            let native_metadata =
                                self.native_loader_metadata(manifest, candidate).await;
                            let children = effective_raw_modrinth_dependencies(
                                candidate,
                                native_metadata.as_ref(),
                            );
                            match raw_modrinth_artifact(&pid, candidate) {
                                Ok(artifact) => {
                                    (DepDisposition::InstallCandidate { artifact }, children)
                                }
                                Err(e) => (
                                    DepDisposition::Unresolved {
                                        reason: e.to_string(),
                                    },
                                    Vec::new(),
                                ),
                            }
                        }
                        Err(e) => (
                            DepDisposition::Unresolved {
                                reason: e.to_string(),
                            },
                            Vec::new(),
                        ),
                    }
                }
                None => (
                    DepDisposition::Unresolved {
                        reason: "Failed to list Modrinth versions".into(),
                    },
                    Vec::new(),
                ),
            };
            resolved.insert(
                key,
                ResolvedDep {
                    mod_jar_id: pid.clone(),
                    requirement,
                    source: DepSource::Manifest,
                    disposition,
                },
            );
            for child in child_deps {
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

    // ------------------------------------------------------------------
    // Batch resolution
    // ------------------------------------------------------------------

    async fn resolve_batch_install(
        &self,
        manifest: &InstanceManifest,
        items: &[crate::install_pipeline::BatchInstallItem],
        registry_revision: String,
    ) -> LauncherResult<PreparedPlan> {
        let mut operations = Vec::new();
        let mut deps_map = BTreeMap::<String, ResolvedDep>::new();
        let mut conflicts_map = BTreeMap::<String, DepConflict>::new();

        for item in items {
            let prepared = match item.source_type {
                SourceType::Curated => {
                    self.resolve_curated_install(
                        manifest,
                        &item.item_id,
                        item.candidate_version.as_deref(),
                        registry_revision.clone(),
                        false,
                    )
                    .await?
                }
                SourceType::Modrinth => {
                    self.resolve_raw_modrinth_install(
                        manifest,
                        &item.item_id,
                        item.candidate_version.as_deref(),
                        registry_revision.clone(),
                        false,
                    )
                    .await?
                }
                SourceType::Manual => resolve_manual_install(
                    &item.item_id,
                    item.candidate_version.as_deref(),
                    registry_revision.clone(),
                )?,
            };
            operations.push(prepared.operation);
            merge_deps(&mut deps_map, prepared.dependencies);
            for conflict in prepared.conflicts {
                conflicts_map.insert(conflict.conflict_id.clone(), conflict);
            }
        }

        Ok(PreparedPlan {
            operation: ResolvedOperation::BatchInstall { operations },
            dependencies: deps_map.into_values().collect(),
            conflicts: conflicts_map.into_values().collect(),
            registry_revision,
        })
    }

    async fn resolve_batch_update(
        &self,
        manifest: &InstanceManifest,
        items: &[crate::install_pipeline::BatchUpdateItem],
        registry_revision: String,
    ) -> LauncherResult<PreparedPlan> {
        let mut operations = Vec::new();
        let mut deps_map = BTreeMap::<String, ResolvedDep>::new();
        let mut conflicts_map = BTreeMap::<String, DepConflict>::new();

        for item in items {
            let installed =
                find_installed_by_identity(manifest, &item.item_id).ok_or_else(|| {
                    LauncherError::Generic {
                        code: "ERR_UPDATE_TARGET_MISSING".into(),
                        message: format!("{} is not installed.", item.item_id),
                    }
                })?;
            let prepared = if installed.source == "modrinth_raw" {
                let project_id = installed.modrinth_id.as_deref().unwrap_or(&item.item_id);
                self.resolve_raw_modrinth_install(
                    manifest,
                    project_id,
                    normalize_requested_version(Some(&item.target_version)),
                    registry_revision.clone(),
                    true,
                )
                .await?
            } else {
                let registry_id = installed.registry_id.as_deref().unwrap_or(&item.item_id);
                self.resolve_curated_install(
                    manifest,
                    registry_id,
                    normalize_requested_version(Some(&item.target_version)),
                    registry_revision.clone(),
                    true,
                )
                .await?
            };
            operations.push(prepared.operation);
            merge_deps(&mut deps_map, prepared.dependencies);
            for conflict in prepared.conflicts {
                conflicts_map.insert(conflict.conflict_id.clone(), conflict);
            }
        }

        Ok(PreparedPlan {
            operation: ResolvedOperation::BatchUpdate { operations },
            dependencies: deps_map.into_values().collect(),
            conflicts: conflicts_map.into_values().collect(),
            registry_revision,
        })
    }
}

/// Prefer active-loader-native dependencies over Modrinth's version-level
/// dependency list. The latter has no loader condition, so it can include a
/// compatibility bridge that is irrelevant to a native Fabric/Quilt build.
fn effective_raw_modrinth_dependencies(
    candidate: &RawModrinthVersionCandidate,
    native_metadata: Option<&crate::dependency_ops::JarDeps>,
) -> Vec<RawModrinthDep> {
    let Some(metadata) = native_metadata else {
        return candidate.dependencies.clone();
    };

    metadata
        .depends_on
        .iter()
        .map(|project_id| RawModrinthDep {
            project_id: Some(project_id.clone()),
            version_id: None,
            dependency_type: "required".into(),
        })
        .chain(
            metadata
                .optional_deps
                .iter()
                .map(|project_id| RawModrinthDep {
                    project_id: Some(project_id.clone()),
                    version_id: None,
                    dependency_type: "optional".into(),
                }),
        )
        .collect()
}

// ---------------------------------------------------------------------------
// Standalone functions: GitHub release helpers
// ---------------------------------------------------------------------------

fn github_auth_headers(token: Option<&str>) -> Vec<(String, String)> {
    token
        .map(|token| vec![("Authorization".into(), format!("Bearer {token}"))])
        .unwrap_or_default()
}

/// Parse the GitHub API `Link` response header to discover the total number of pages.
pub fn parse_link_total_pages(header_value: Option<&str>) -> u32 {
    let value = match header_value {
        Some(v) => v,
        None => return 1,
    };
    for part in value.split(',') {
        let trimmed = part.trim();
        if trimmed.contains("rel=\"last\"") {
            if let Some(close) = trimmed.rfind('>') {
                let substr = &trimmed[..close];
                if let Some(open) = substr.rfind('<') {
                    let url = &substr[open + 1..];
                    for segment in url.split(&['?', '&'][..]) {
                        if let Some(num) = segment.strip_prefix("page=") {
                            return num.parse::<u32>().unwrap_or(1);
                        }
                    }
                }
            }
        }
    }
    1
}

/// Sort version candidates by compatibility tier (compatible → major_match → other),
/// then by release date descending within each tier.
pub fn sort_versions_by_compatibility(versions: &mut [ModVersionCandidate]) {
    versions.sort_by(|a, b| {
        let tier = |c: &ModVersionCandidate| -> u8 {
            match c.version_compat.as_str() {
                "compatible" => 0,
                "major_match" => 1,
                _ => 2,
            }
        };
        let ta = tier(a);
        let tb = tier(b);
        ta.cmp(&tb).then_with(|| {
            b.release_date
                .as_deref()
                .unwrap_or("")
                .cmp(a.release_date.as_deref().unwrap_or(""))
        })
    });
}

/// Determine MC version and loader compatibility for a GitHub release asset.
pub fn parse_version_from_github_asset(
    filename: &str,
    tag_name: &str,
    mc_version: &str,
    loader: &str,
) -> (Option<String>, Option<String>, &'static str) {
    let mc = extract_mc_version(filename).or_else(|| extract_mc_version(tag_name));
    let lo = extract_loader(filename).or_else(|| extract_loader(tag_name));

    let mc_match = mc.as_deref().map(|v| {
        let target = mc_version.to_lowercase();
        let stripped_target = target.strip_prefix("1.").unwrap_or(&target);
        let stripped_found = v.strip_prefix("1.").unwrap_or(v);
        stripped_found == stripped_target
    });

    let lo_match = lo.as_deref().map(|l| l.eq_ignore_ascii_case(loader));
    let loader_ok = lo_match == Some(true);
    let loader_mismatch = lo.is_some() && !loader_ok;

    let major_matches = mc.as_deref().is_some_and(|found| {
        let target = mc_version.to_lowercase();
        let stripped_target = target.strip_prefix("1.").unwrap_or(&target);
        let stripped_found = found.strip_prefix("1.").unwrap_or(found);
        let target_major = stripped_target.split('.').next().unwrap_or("");
        let found_major = stripped_found.split('.').next().unwrap_or("");
        if target_major.is_empty() || found_major.is_empty() {
            return false;
        }
        target_major == found_major
            && (stripped_found.starts_with(stripped_target)
                || stripped_target.starts_with(stripped_found))
    });

    let compat = if mc_match == Some(true) && !loader_mismatch {
        "compatible"
    } else if major_matches && !loader_mismatch {
        "major_match"
    } else {
        ""
    };

    let matched_mc = if mc_match == Some(true) {
        Some(mc_version.to_string())
    } else {
        mc
    };

    (matched_mc, lo, compat)
}

/// Extract a Minecraft version hint from a string.
pub fn extract_mc_version(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let bytes_lower = lower.as_bytes();

    let mut pos = 0;
    while pos < bytes_lower.len() {
        if pos + 1 < bytes_lower.len() && bytes_lower[pos] == b'm' && bytes_lower[pos + 1] == b'c' {
            let before_ok = pos == 0 || !bytes_lower[pos - 1].is_ascii_alphanumeric();
            let after_pos = pos + 2;
            if before_ok && after_pos < bytes_lower.len() && bytes_lower[after_pos].is_ascii_digit()
            {
                let rest = &text[after_pos..];
                let end = rest
                    .find(|c: char| !c.is_ascii_digit() && c != '.')
                    .unwrap_or(rest.len());
                let ver = &rest[..end];
                let ver = ver.strip_suffix('.').unwrap_or(ver);
                if !ver.is_empty() {
                    return Some(ver.to_string());
                }
            }
            pos += 2;
        } else {
            pos += 1;
        }
    }

    let mut i = 0;
    while i < bytes_lower.len() {
        if bytes_lower[i].is_ascii_digit() {
            if i > 0 && bytes_lower[i - 1].is_ascii_alphanumeric() {
                i += 1;
                continue;
            }
            let mut end = i + 1;
            while end < bytes_lower.len()
                && (bytes_lower[end].is_ascii_digit() || bytes_lower[end] == b'.')
            {
                end += 1;
            }
            let mut ver_end = end;
            while ver_end > i + 1 && bytes_lower[ver_end - 1] == b'.' {
                ver_end -= 1;
            }
            let candidate = &lower[i..ver_end];
            if candidate.contains('.') {
                if let Some(major_str) = candidate.split('.').next() {
                    if let Ok(major) = major_str.parse::<u32>() {
                        if major == 1 || major > 25 {
                            return Some(candidate.to_string());
                        }
                    }
                }
            }
            i = end;
        } else {
            i += 1;
        }
    }

    None
}

/// Extract a loader hint from a string.
pub fn extract_loader(text: &str) -> Option<String> {
    const KNOWN_LOADERS: &[&str] = &["fabric", "forge", "neoforge", "quilt"];
    let lower = text.to_lowercase();
    for loader in KNOWN_LOADERS {
        let mut idx = 0;
        while let Some(pos) = lower[idx..].find(loader) {
            let abs = idx + pos;
            let before_ok = abs == 0 || !lower.as_bytes()[abs - 1].is_ascii_alphanumeric();
            let after_pos = abs + loader.len();
            let after_ok =
                after_pos >= lower.len() || !lower.as_bytes()[after_pos].is_ascii_alphanumeric();
            if before_ok && after_ok {
                return Some(loader.to_string());
            }
            idx = abs + 1;
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Standalone functions: Modrinth version listing for curated items
// ---------------------------------------------------------------------------

async fn fetch_modrinth_versions_for_item(
    clients: &HttpClients,
    source_identifier: &str,
    modrinth_id: Option<&str>,
    mc_version: &str,
    loader: &str,
) -> LauncherResult<Vec<ModVersionCandidate>> {
    let project_id = modrinth_id.unwrap_or(source_identifier);

    let url = format!(
        "https://api.modrinth.com/v2/project/{pid}/version?game_versions=[\"{gv}\"]&loaders=[\"{ld}\"]",
        pid = urlencoding::encode(project_id),
        gv = urlencoding::encode(mc_version),
        ld = urlencoding::encode(loader),
    );

    #[derive(Deserialize)]
    struct MRFileHashes {
        sha1: Option<String>,
        sha256: Option<String>,
        sha512: Option<String>,
    }
    #[derive(Deserialize)]
    struct MRFile {
        url: String,
        filename: String,
        primary: bool,
        hashes: Option<MRFileHashes>,
        #[serde(default)]
        size: Option<u64>,
    }
    #[derive(Deserialize)]
    struct MRVersion {
        version_number: String,
        files: Vec<MRFile>,
    }

    let versions: Vec<MRVersion> =
        http_client::checked_get_json(clients, ClientCategory::Modrinth, &url).await?;

    let mut candidates: Vec<ModVersionCandidate> = Vec::new();
    for version in &versions {
        let primary_file = version
            .files
            .iter()
            .find(|f| f.primary)
            .or_else(|| version.files.first());
        let file = match primary_file {
            Some(f) => f,
            None => continue,
        };
        let (mc_ver, lo, compat) = parse_version_from_github_asset(
            &file.filename,
            &version.version_number,
            mc_version,
            loader,
        );
        candidates.push(ModVersionCandidate {
            version: version.version_number.clone(),
            filename: file.filename.clone(),
            download_url: file.url.clone(),
            mc_version: mc_ver,
            loader: lo,
            release_date: None,
            is_compatible: compat == "compatible",
            version_compat: compat.to_string(),
            sha1: file.hashes.as_ref().and_then(|h| h.sha1.clone()),
            sha256: file.hashes.as_ref().and_then(|h| h.sha256.clone()),
            sha512: file.hashes.as_ref().and_then(|h| h.sha512.clone()),
            size: file.size,
        });
    }
    Ok(candidates)
}

// ---------------------------------------------------------------------------
// Candidate selection
// ---------------------------------------------------------------------------

/// Select the best candidate from a list of curated versions.
/// When `requested` is set, finds that exact version; otherwise finds
/// the first compatible candidate.
pub fn select_curated_candidate<'a>(
    candidates: &'a [ModVersionCandidate],
    requested: Option<&str>,
) -> LauncherResult<&'a ModVersionCandidate> {
    let requested = normalize_requested_version(requested);
    if let Some(requested) = requested {
        return candidates
            .iter()
            .find(|c| c.version == requested || c.filename == requested)
            .ok_or(LauncherError::VersionNotFound);
    }
    candidates
        .iter()
        .find(|c| c.is_compatible)
        .ok_or(LauncherError::VersionNotFound)
}

fn is_installable_github_asset(filename: &str) -> bool {
    let lower = filename.to_ascii_lowercase();
    let Some(stem) = lower.strip_suffix(".jar") else {
        return false;
    };
    ![
        "-api", "-dev", "-sources", "-source", "-javadoc", "-tests", "-test", "-slim",
    ]
    .iter()
    .any(|suffix| stem.ends_with(suffix))
}

/// Select the best candidate from a list of raw Modrinth versions.
pub fn select_raw_modrinth_candidate<'a>(
    candidates: &'a [RawModrinthVersionCandidate],
    requested: Option<&str>,
) -> LauncherResult<&'a RawModrinthVersionCandidate> {
    let requested = normalize_requested_version(requested);
    if let Some(requested) = requested {
        return candidates
            .iter()
            .find(|c| {
                c.version_id == requested || c.version == requested || c.filename == requested
            })
            .ok_or(LauncherError::VersionNotFound);
    }
    candidates.first().ok_or(LauncherError::VersionNotFound)
}

/// Normalize a requested version string: trim whitespace, reject empty/"available"/"latest".
pub fn normalize_requested_version(requested: Option<&str>) -> Option<&str> {
    requested
        .map(str::trim)
        .filter(|v| !v.is_empty() && *v != "available" && *v != "latest")
}

// ---------------------------------------------------------------------------
// Artifact builders
// ---------------------------------------------------------------------------

fn curated_artifact(
    item: &crate::registry::RegistryItem,
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
        .ok_or_else(|| LauncherError::Generic {
            code: "ERR_HASH_UNAVAILABLE".into(),
            message: format!(
                "No trusted SHA-256 is available for {} {}.",
                item.id, candidate.version
            ),
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
    candidate: &RawModrinthVersionCandidate,
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
        return Err(LauncherError::Generic {
            code: "ERR_HASH_UNAVAILABLE".into(),
            message: format!(
                "Modrinth did not publish a usable hash for {}.",
                candidate.filename
            ),
        });
    }

    Ok(ResolvedArtifact::Download(ResolvedDownload {
        item_id: project_id.to_string(),
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
            modrinth_id: Some(project_id.to_string()),
            content_type: "mod".into(),
        },
    }))
}

fn resolve_manual_install(
    item_id: &str,
    source_path: Option<&str>,
    registry_revision: String,
) -> LauncherResult<PreparedPlan> {
    let source_path = source_path
        .filter(|p| !p.trim().is_empty())
        .ok_or_else(|| LauncherError::Generic {
            code: "ERR_MANUAL_PATH".into(),
            message: "Manual install requires a local file path.".into(),
        })?;
    let path = Path::new(source_path);
    if !path.is_file() {
        return Err(LauncherError::Generic {
            code: "ERR_MANUAL_PATH".into(),
            message: format!("Manual artifact does not exist: {}", path.display()),
        });
    }
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| name.to_ascii_lowercase().ends_with(".jar"))
        .ok_or_else(|| LauncherError::Generic {
            code: "ERR_MANUAL_FILE".into(),
            message: "Manual mods must be .jar files.".into(),
        })?;
    let bytes = std::fs::read(path).map_err(|e| LauncherError::Generic {
        code: "ERR_MANUAL_READ".into(),
        message: format!("Could not read manual artifact: {e}"),
    })?;
    let sha256 = download::sha256_hex(&bytes);

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

// ---------------------------------------------------------------------------
// Conflict builder
// ---------------------------------------------------------------------------

fn build_known_conflicts(
    known_conflicts: &[crate::registry::KnownConflict],
    aliases: &AliasMap,
    incoming: &HashSet<String>,
    installed_set: &HashSet<String>,
) -> Vec<DepConflict> {
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
                message: conflict.notes.clone().unwrap_or_else(|| {
                    format!("The curated registry reports a conflict between {a} and {b}.")
                }),
                blocking: conflict.severity != "info",
                resolution_options: vec![ConflictResolution::Abort, ConflictResolution::Skip],
                chosen: None,
            });
        }
    }
    conflicts
}

// ---------------------------------------------------------------------------
// General helpers
// ---------------------------------------------------------------------------

fn enqueue_manifest_deps(queue: &mut VecDeque<(String, Requirement)>, deps: &ManifestDeps) {
    queue.extend(
        deps.required
            .iter()
            .cloned()
            .map(|id| (id, Requirement::Required)),
    );
    queue.extend(
        deps.optional
            .iter()
            .cloned()
            .map(|id| (id, Requirement::Optional)),
    );
}

/// Validate that a hash hex string has the expected length.
pub fn valid_hash(value: Option<&str>, length: usize) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| v.len() == length && v.bytes().all(|byte| byte.is_ascii_hexdigit()))
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

fn effective_installed_filename(item: &InstalledMod) -> String {
    if item.enabled || item.filename.ends_with(".disabled") {
        item.filename.clone()
    } else {
        format!("{}.disabled", item.filename)
    }
}

fn merge_deps(target: &mut BTreeMap<String, ResolvedDep>, incoming: Vec<ResolvedDep>) {
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

fn open_registry_db(path: &std::path::Path) -> LauncherResult<rusqlite::Connection> {
    if !path.is_file() {
        return Err(LauncherError::RegistryMissing);
    }
    let conn = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| LauncherError::RegistryMissing)?;
    conn.pragma_update(None, "query_only", "ON")
        .map_err(|_| LauncherError::RegistryMissing)?;
    Ok(conn)
}

// ---------------------------------------------------------------------------
// Required API access for fetch_modrinth_versions_for_item's HttpClients::new
// ---------------------------------------------------------------------------

// The fetch_modrinth_versions_for_item function receives HttpClients from the
// Resolver's Ctx, so no standalone HttpClients::new() is needed in this module.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_requested_version() {
        assert_eq!(normalize_requested_version(None), None);
        assert_eq!(normalize_requested_version(Some("")), None);
        assert_eq!(normalize_requested_version(Some("  ")), None);
        assert_eq!(normalize_requested_version(Some("available")), None);
        assert_eq!(normalize_requested_version(Some("latest")), None);
        assert_eq!(normalize_requested_version(Some("1.0.0")), Some("1.0.0"));
        assert_eq!(
            normalize_requested_version(Some(" 1.20.1 ")),
            Some("1.20.1")
        );
    }

    #[test]
    fn test_valid_hash() {
        assert_eq!(valid_hash(Some("xyz"), 3), None);
        assert_eq!(valid_hash(Some("abc"), 2), None);
        assert_eq!(
            valid_hash(Some("abcdef0123456789abcdef0123456789"), 32),
            Some("abcdef0123456789abcdef0123456789".into())
        );
        assert_eq!(valid_hash(Some("ABCdef"), 6), Some("abcdef".into()));
        assert_eq!(valid_hash(None, 64), None);
        assert_eq!(valid_hash(Some(&"a".repeat(64)), 64), Some("a".repeat(64)));
    }

    #[test]
    fn test_extract_mc_version() {
        assert_eq!(
            extract_mc_version("fabric-api-0.91.0+1.20.1"),
            Some("1.20.1".into())
        );
        assert_eq!(extract_mc_version("mod-1.19.2.jar"), Some("1.19.2".into()));
        assert_eq!(extract_mc_version("nocontainer"), None);
        assert_eq!(extract_mc_version("lib-0.1.0"), None);
    }

    #[test]
    fn test_extract_loader() {
        assert_eq!(extract_loader("fabric-api-0.91.0"), Some("fabric".into()));
        assert_eq!(extract_loader("My-Forge-Mod"), Some("forge".into()));
        assert_eq!(extract_loader("neoforge-mod"), Some("neoforge".into()));
        assert_eq!(extract_loader("justamod"), None);
    }

    #[test]
    fn test_select_curated_candidate_no_requested_prefers_compatible() {
        let candidates = vec![
            ModVersionCandidate {
                version: "v1.0".into(),
                filename: "a.jar".into(),
                download_url: "https://example.com/a".into(),
                mc_version: None,
                loader: None,
                release_date: Some("2024-01-01".into()),
                is_compatible: false,
                sha1: None,
                sha256: None,
                sha512: None,
                size: None,
                version_compat: "".into(),
            },
            ModVersionCandidate {
                version: "v2.0".into(),
                filename: "b.jar".into(),
                download_url: "https://example.com/b".into(),
                mc_version: None,
                loader: None,
                release_date: Some("2024-02-01".into()),
                is_compatible: true,
                sha1: None,
                sha256: None,
                sha512: None,
                size: None,
                version_compat: "compatible".into(),
            },
        ];
        let selected = select_curated_candidate(&candidates, None).unwrap();
        assert_eq!(selected.version, "v2.0");
    }

    #[test]
    fn test_select_curated_candidate_without_compatible_version_fails() {
        let candidates = vec![ModVersionCandidate {
            version: "v1.0".into(),
            filename: "mod-1.17.jar".into(),
            download_url: "https://example.com/mod-1.17.jar".into(),
            mc_version: Some("1.17".into()),
            loader: Some("fabric".into()),
            release_date: None,
            is_compatible: false,
            sha1: None,
            sha256: None,
            sha512: None,
            size: None,
            version_compat: "incompatible".into(),
        }];

        assert!(matches!(
            select_curated_candidate(&candidates, None),
            Err(LauncherError::VersionNotFound)
        ));
    }

    #[test]
    fn test_github_asset_filter_rejects_non_runtime_jars() {
        assert!(is_installable_github_asset(
            "fabric-api-0.116.14+1.21.1.jar"
        ));
        assert!(is_installable_github_asset(
            "lithium-fabric-0.15.4+mc1.21.1.jar"
        ));
        assert!(!is_installable_github_asset(
            "lithium-fabric-0.15.4+mc1.21.1-api.jar"
        ));
        assert!(!is_installable_github_asset("example-sources.jar"));
        assert!(!is_installable_github_asset("example-javadoc.jar"));
        assert!(!is_installable_github_asset("README.txt"));
    }

    #[test]
    fn test_select_curated_candidate_requested_version() {
        let candidates = vec![
            ModVersionCandidate {
                version: "v1.0".into(),
                filename: "a.jar".into(),
                download_url: "https://example.com/a".into(),
                mc_version: None,
                loader: None,
                release_date: None,
                is_compatible: false,
                sha1: None,
                sha256: None,
                sha512: None,
                size: None,
                version_compat: "".into(),
            },
            ModVersionCandidate {
                version: "v2.0".into(),
                filename: "b.jar".into(),
                download_url: "https://example.com/b".into(),
                mc_version: None,
                loader: None,
                release_date: None,
                is_compatible: true,
                sha1: None,
                sha256: None,
                sha512: None,
                size: None,
                version_compat: "compatible".into(),
            },
        ];
        let selected = select_curated_candidate(&candidates, Some("v1.0")).unwrap();
        assert_eq!(selected.version, "v1.0");
    }

    #[test]
    fn test_select_curated_candidate_requested_by_filename() {
        let candidates = vec![ModVersionCandidate {
            version: "v1.0".into(),
            filename: "specific.jar".into(),
            download_url: "https://example.com/specific".into(),
            mc_version: None,
            loader: None,
            release_date: None,
            is_compatible: false,
            sha1: None,
            sha256: None,
            sha512: None,
            size: None,
            version_compat: "".into(),
        }];
        let selected = select_curated_candidate(&candidates, Some("specific.jar")).unwrap();
        assert_eq!(selected.filename, "specific.jar");
    }

    #[test]
    fn test_is_platform_dependency() {
        assert!(is_platform_dependency("minecraft", "fabric"));
        assert!(is_platform_dependency("fabricloader", "fabric"));
        assert!(is_platform_dependency("quilt_loader", "quilt"));
        assert!(is_platform_dependency("neoforge", "neoforge"));
        assert!(!is_platform_dependency("some-mod", "fabric"));
    }

    #[test]
    fn native_loader_dependencies_replace_unscoped_modrinth_route_dependencies() {
        let candidate = RawModrinthVersionCandidate {
            version: "1.0".into(),
            version_id: "version".into(),
            name: "SwingThrough".into(),
            filename: "swingthrough.jar".into(),
            download_url: "https://cdn.modrinth.com/swingthrough.jar".into(),
            sha1: Some("a".repeat(40)),
            sha512: None,
            size: None,
            mc_versions: vec!["1.21.1".into()],
            loaders: vec!["fabric".into(), "neoforge".into()],
            release_date: None,
            primary: true,
            changelog: None,
            dependencies: vec![RawModrinthDep {
                project_id: Some("connector".into()),
                version_id: None,
                dependency_type: "required".into(),
            }],
        };
        let native_fabric = crate::dependency_ops::JarDeps {
            mod_jar_id: Some("swingthrough".into()),
            ..Default::default()
        };

        assert!(effective_raw_modrinth_dependencies(&candidate, Some(&native_fabric)).is_empty());
        assert_eq!(
            effective_raw_modrinth_dependencies(&candidate, None)[0]
                .project_id
                .as_deref(),
            Some("connector")
        );
    }

    #[test]
    fn test_parse_link_total_pages() {
        assert_eq!(parse_link_total_pages(None), 1);
        assert_eq!(
            parse_link_total_pages(Some("<https://api.github.com/repos/owner/repo/releases?page=2>; rel=\"next\", <https://api.github.com/repos/owner/repo/releases?page=5>; rel=\"last\"")),
            5
        );
    }

    #[test]
    fn test_sort_versions_by_compatibility() {
        let mut versions = vec![
            ModVersionCandidate {
                version: "v1".into(),
                filename: "a.jar".into(),
                download_url: "".into(),
                mc_version: None,
                loader: None,
                release_date: Some("2024-01-01".into()),
                is_compatible: false,
                sha1: None,
                sha256: None,
                sha512: None,
                size: None,
                version_compat: "".into(),
            },
            ModVersionCandidate {
                version: "v2".into(),
                filename: "b.jar".into(),
                download_url: "".into(),
                mc_version: None,
                loader: None,
                release_date: Some("2024-02-01".into()),
                is_compatible: true,
                sha1: None,
                sha256: None,
                sha512: None,
                size: None,
                version_compat: "compatible".into(),
            },
            ModVersionCandidate {
                version: "v3".into(),
                filename: "c.jar".into(),
                download_url: "".into(),
                mc_version: None,
                loader: None,
                release_date: Some("2024-03-01".into()),
                is_compatible: false,
                sha1: None,
                sha256: None,
                sha512: None,
                size: None,
                version_compat: "major_match".into(),
            },
        ];
        sort_versions_by_compatibility(&mut versions);
        assert_eq!(versions[0].version_compat, "compatible");
        assert_eq!(versions[1].version_compat, "major_match");
        // Within non-compatible tier, newer dates first
        assert!(versions[2].release_date.as_deref().unwrap_or("") <= "2024-01-01");
    }

    #[test]
    fn test_parse_version_from_github_asset() {
        let (mc, lo, compat) = parse_version_from_github_asset(
            "fabric-api-0.91.0+1.20.1.jar",
            "v0.91.0",
            "1.20.1",
            "fabric",
        );
        assert_eq!(mc, Some("1.20.1".into()));
        assert_eq!(lo, Some("fabric".into()));
        assert_eq!(compat, "compatible");

        let (mc2, _lo2, compat2) =
            parse_version_from_github_asset("my-mod-1.20.jar", "v1.0", "1.20.1", "fabric");
        assert_eq!(mc2, Some("1.20".into()));
        // "1.20" is a prefix of "1.20.1" → major_match (no loader mismatch)
        assert_eq!(compat2, "major_match");
    }

    #[test]
    fn test_find_installed_by_identity_empty_manifest() {
        let manifest = InstanceManifest {
            instance_id: "test".into(),
            name: "Test".into(),
            minecraft_version: "1.20.1".into(),
            loader: "fabric".into(),
            loader_version: "0.15.0".into(),
            is_locked: false,
            mods: vec![],
            resourcepacks: vec![],
            shaders: vec![],
            datapacks: vec![],
            worlds: vec![],
            created_from_pack: None,
            user_preferences: serde_json::json!({}),
        };
        assert!(find_installed_by_identity(&manifest, "anything").is_none());
    }

    #[test]
    fn test_all_installed() {
        let manifest = InstanceManifest {
            instance_id: "test".into(),
            name: "Test".into(),
            minecraft_version: "1.20.1".into(),
            loader: "fabric".into(),
            loader_version: "0.15.0".into(),
            is_locked: false,
            mods: vec![InstalledMod {
                filename: "a.jar".into(),
                source: "registry".into(),
                sha256: "aa".into(),
                installed_at: "now".into(),
                enabled: true,
                content_type: "mod".into(),
                registry_id: None,
                modrinth_id: None,
                source_url: None,
                version: None,
                java_packages: vec![],
                mod_jar_id: None,
                provided_mod_ids: vec![],
                depends_on: vec![],
                optional_deps: vec![],
                incompatible_deps: vec![],
            }],
            resourcepacks: vec![InstalledMod {
                filename: "b.zip".into(),
                source: "registry".into(),
                sha256: "bb".into(),
                installed_at: "now".into(),
                enabled: true,
                content_type: "resourcepack".into(),
                registry_id: None,
                modrinth_id: None,
                source_url: None,
                version: None,
                java_packages: vec![],
                mod_jar_id: None,
                provided_mod_ids: vec![],
                depends_on: vec![],
                optional_deps: vec![],
                incompatible_deps: vec![],
            }],
            shaders: vec![],
            datapacks: vec![],
            worlds: vec![],
            created_from_pack: None,
            user_preferences: serde_json::json!({}),
        };
        let items: Vec<&InstalledMod> = all_installed(&manifest).collect();
        assert_eq!(items.len(), 2);
    }

    // ------------------------------------------------------------------
    // fetch_github_releases_initial / compute_tail_pages / batch
    // ------------------------------------------------------------------

    #[test]
    fn github_release_auth_header_contains_one_bearer_prefix() {
        assert_eq!(
            github_auth_headers(Some("gho_test_token")),
            vec![(
                "Authorization".to_string(),
                "Bearer gho_test_token".to_string()
            )]
        );
        assert!(github_auth_headers(None).is_empty());
    }

    #[test]
    fn test_compute_tail_pages_single_page() {
        assert!(
            Resolver::compute_tail_pages(1, true).is_empty(),
            "single page with compatibles should yield no tail"
        );
        assert!(
            Resolver::compute_tail_pages(1, false).is_empty(),
            "single page without compatibles should yield no tail"
        );
    }

    #[test]
    fn test_compute_tail_pages_compatible_on_page1() {
        assert!(
            Resolver::compute_tail_pages(10, true).is_empty(),
            "compatible found on page 1: no tail needed"
        );
    }

    #[test]
    fn test_compute_tail_pages_few_pages() {
        assert_eq!(
            Resolver::compute_tail_pages(2, false),
            vec![2],
            "2 pages without compatibles: one tail page"
        );
        assert_eq!(
            Resolver::compute_tail_pages(3, false),
            vec![3, 2],
            "3 pages without compatibles: two tail pages"
        );
    }

    #[test]
    fn test_compute_tail_pages_many_pages() {
        assert_eq!(
            Resolver::compute_tail_pages(50, false),
            vec![50, 49, 48],
            "50 pages without compatibles: three oldest pages"
        );
        assert_eq!(
            Resolver::compute_tail_pages(100, false),
            vec![100, 99, 98],
            "100 pages without compatibles: three oldest pages"
        );
    }

    #[test]
    fn test_compute_tail_pages_exactly_four_pages() {
        assert_eq!(
            Resolver::compute_tail_pages(4, false),
            vec![4, 3, 2],
            "4 pages without compatibles: three oldest pages"
        );
    }

    #[tokio::test]
    async fn test_fetch_github_versions_batch_empty_pages() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = crate::ctx::Ctx::for_testing(tmp.path().to_path_buf());
        let resolver = Resolver::new(ctx);
        let result = resolver
            .fetch_github_versions_batch("owner/repo", "1.20.1", "fabric", &[])
            .await;
        assert!(result.is_ok(), "empty pages must not fail");
        assert!(
            result.unwrap().is_empty(),
            "empty pages must return empty results"
        );
    }

    #[tokio::test]
    async fn test_fetch_github_versions_batch_single_page() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = crate::ctx::Ctx::for_testing(tmp.path().to_path_buf());
        let resolver = Resolver::new(ctx);
        // A single page for a non-existent repo — will error but must not panic
        let result = resolver
            .fetch_github_versions_batch(
                "nonexistent/repo-does-not-exist",
                "1.20.1",
                "fabric",
                &[1],
            )
            .await;
        // The function tolerates page failures, so the outer Result is Ok
        // with whatever pages succeeded (could be empty).
        assert!(
            result.is_ok(),
            "batch must tolerate individual page failures"
        );
    }

    #[test]
    fn test_sort_merge_dedup() {
        // Verify that sorting places compatible candidates first,
        // major_match second, and nothing else third, with newer
        // dates first within each tier.
        let mut candidates = vec![
            ModVersionCandidate {
                version: "v1".into(),
                filename: "a.jar".into(),
                download_url: "".into(),
                mc_version: None,
                loader: None,
                release_date: Some("2024-01-01".into()),
                is_compatible: false,
                sha1: None,
                sha256: None,
                sha512: None,
                size: None,
                version_compat: "".into(),
            },
            ModVersionCandidate {
                version: "v2".into(),
                filename: "b.jar".into(),
                download_url: "".into(),
                mc_version: None,
                loader: None,
                release_date: Some("2024-02-01".into()),
                is_compatible: true,
                sha1: None,
                sha256: None,
                sha512: None,
                size: None,
                version_compat: "compatible".into(),
            },
            ModVersionCandidate {
                version: "v3".into(),
                filename: "c.jar".into(),
                download_url: "".into(),
                mc_version: None,
                loader: Some("fabric".into()),
                release_date: Some("2024-03-01".into()),
                is_compatible: false,
                sha1: None,
                sha256: None,
                sha512: None,
                size: None,
                version_compat: "major_match".into(),
            },
            ModVersionCandidate {
                version: "v2dup".into(),
                filename: "d.jar".into(),
                download_url: "".into(),
                mc_version: None,
                loader: None,
                release_date: Some("2024-02-15".into()),
                is_compatible: true,
                sha1: None,
                sha256: None,
                sha512: None,
                size: None,
                version_compat: "compatible".into(),
            },
        ];
        sort_versions_by_compatibility(&mut candidates);
        // First two are compatible tier (v2 and v2dup) with newer first
        assert_eq!(candidates[0].version_compat, "compatible");
        assert_eq!(candidates[1].version_compat, "compatible");
        // Compatible tier should be sorted by date descending
        assert!(
            candidates[0].release_date.as_deref().unwrap_or("")
                >= candidates[1].release_date.as_deref().unwrap_or("")
        );
        // Then major_match
        assert_eq!(candidates[2].version_compat, "major_match");
        // Then other
        assert_eq!(candidates[3].version_compat, "");
    }

    #[test]
    fn test_initial_tail_merge_correct_page_order() {
        // Simulate the merge that fetch_github_releases_initial does:
        // page 1 candidates + tail page candidates, then sorted.
        let page1 = vec![
            ModVersionCandidate {
                version: "v1".into(),
                filename: "page1_a.jar".into(),
                download_url: "".into(),
                mc_version: None,
                loader: None,
                release_date: Some("2024-01-01".into()),
                is_compatible: false,
                sha1: None,
                sha256: None,
                sha512: None,
                size: None,
                version_compat: "".into(),
            },
            ModVersionCandidate {
                version: "v2".into(),
                filename: "page1_b.jar".into(),
                download_url: "".into(),
                mc_version: None,
                loader: Some("fabric".into()),
                release_date: Some("2024-02-01".into()),
                is_compatible: false,
                sha1: None,
                sha256: None,
                sha512: None,
                size: None,
                version_compat: "major_match".into(),
            },
        ];
        let tail = vec![
            ModVersionCandidate {
                version: "v10".into(),
                filename: "tail_c.jar".into(),
                download_url: "".into(),
                mc_version: None,
                loader: None,
                release_date: Some("2023-01-01".into()),
                is_compatible: true,
                sha1: None,
                sha256: None,
                sha512: None,
                size: None,
                version_compat: "compatible".into(),
            },
            ModVersionCandidate {
                version: "v11".into(),
                filename: "tail_d.jar".into(),
                download_url: "".into(),
                mc_version: None,
                loader: None,
                release_date: Some("2023-02-01".into()),
                is_compatible: false,
                sha1: None,
                sha256: None,
                sha512: None,
                size: None,
                version_compat: "".into(),
            },
        ];
        let mut merged = page1;
        merged.extend(tail);
        sort_versions_by_compatibility(&mut merged);

        // After merge+sort: compatible first (v10), then major_match (v2), then others (v1, v11)
        assert_eq!(merged.len(), 4, "all candidates preserved after merge");
        // First: compatible
        assert_eq!(
            merged[0].filename, "tail_c.jar",
            "compatible should sort first"
        );
        assert_eq!(merged[0].version_compat, "compatible");
        // Second: major_match
        assert_eq!(
            merged[1].filename, "page1_b.jar",
            "major_match should sort second"
        );
        assert_eq!(merged[1].version_compat, "major_match");
        // Third and fourth: empty compat, sorted by date descending
        assert_eq!(merged[2].version_compat, "");
        assert_eq!(merged[3].version_compat, "");
        assert!(
            merged[2].release_date.as_deref().unwrap_or("")
                >= merged[3].release_date.as_deref().unwrap_or(""),
            "same-tier candidates sorted by date descending"
        );
    }

    #[test]
    fn test_page1_has_compatible_skips_tail_heuristic() {
        // When page 1 contains a compatible candidate, compute_tail_pages
        // should return empty. This is the decision function used by
        // fetch_github_releases_initial.
        let page1_has_compat = true;
        assert!(
            Resolver::compute_tail_pages(10, page1_has_compat).is_empty(),
            "tail pages not needed when page 1 has compatible"
        );

        let page1_has_compat = false;
        assert_eq!(
            Resolver::compute_tail_pages(10, page1_has_compat),
            vec![10, 9, 8],
            "tail pages needed when page 1 has no compatible"
        );
    }
}
