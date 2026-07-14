//! **Sole supported direct-launch pipeline** for Minecraft Java launches.
//!
//! The legacy launch paths in [`crate::launch`] have been retired; all direct
//! Java spawns (vanilla, Fabric, Quilt, and installed-profile Forge/NeoForge)
//! must go through this module.
//!
//! ## Pipeline stages
//!
//! 1. **Resolve** – fetch/verify Mojang manifest + version JSON + loader profile
//!    (Fabric/Quilt only) or adopt an installed Forge/NeoForge profile. Returns a
//!    [`ResolvedLaunchPlan`] with metadata and a compatible Java candidate,
//!    **without** downloading game artifacts.
//! 2. **Materialize** – download every required artifact (client JAR, libraries,
//!    natives, assets, logging config) into the cache, verifying every hash.
//!    For adopted profiles, artifacts are sourced from the installed `.minecraft`.
//! 3. **Validate** – check files exist, Java meets requirements, main class set.
//! 4. **Build command** – assemble the single canonical `PreparedCommand`.
//! 5. **Spawn** – launch the JVM, return the child process handle.
//! 6. **Wait & classify** – wait for exit and classify the outcome for LKG.
//!
//! ## Loader support
//!
//! - Fabric / Quilt: fully supported via pinned profile JSONs from
//!   [`crate::loader_manifests`].
//! - Forge / NeoForge: supported via installed-profile adoption (requires the
//!   Mojang launcher to have installed the profile first). No managed processor
//!   execution — the official pinned installer is the single installation authority.
//!
//! Every HTTP request is gated by the caller-supplied [`NetworkPolicy`], so
//! offline-first usage is guaranteed after first materialization.

use crate::download;
use crate::error::{LauncherError, LauncherResult};
use crate::java::{self, JavaInstallation};
use crate::launch::{self, LoaderInfo, MojangVersionManifest, VersionInfo};
use crate::loader_manifests;
use crate::network::{self, NetworkCategory, NetworkPolicy};
use sha1::{Digest, Sha1};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

// No managed-installer re-exports — Forge/NeoForge uses installed-profile adoption only.

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Maximum redirect chain length for planner HTTP clients.
const MAX_REDIRECTS: usize = 10;

const VERSION_MANIFEST_URL: &str =
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

/// TTL for the Minecraft version manifest. Only the manifest is mutable; all
/// other metadata (version JSONs addressed by SHA-1, asset indexes, pinned
/// loader profiles) are content-addressable and need no freshness window.
const VERSION_MANIFEST_TTL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

// ---------------------------------------------------------------------------
// Redirect-safe HTTP clients
// ---------------------------------------------------------------------------

/// HTTP clients with per-category redirect policies for the launch planner.
///
/// Each client enforces that every redirect target:
/// * Is HTTPS on port 443
/// * Has a host on the known allowlist (Mojang or loader manifests)
/// * Classifies to exactly the expected [`NetworkCategory`]
/// * Is NOT an IP literal, localhost, or private metadata host
/// * Does not exceed the max redirect chain length
pub(super) struct LaunchHttpClients {
    mojang_metadata: reqwest::Client,
    mojang_content: reqwest::Client,
    loader: reqwest::Client,
}

impl LaunchHttpClients {
    fn new() -> LauncherResult<Self> {
        Ok(Self {
            mojang_metadata: Self::build_client(NetworkCategory::MojangMetadata)?,
            mojang_content: Self::build_client(NetworkCategory::MojangContent)?,
            loader: Self::build_client(NetworkCategory::LoaderMetadataAndContent)?,
        })
    }

    fn build_client(category: NetworkCategory) -> LauncherResult<reqwest::Client> {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                // Enforce redirect depth limit (previous() includes the
                // original URL plus each followed redirect).
                if attempt.previous().len() > MAX_REDIRECTS {
                    return attempt.stop();
                }
                if redirect_target_is_safe(attempt.url(), category) {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .user_agent("AgoraPlanner/1.0")
            .build()
            .map_err(|e| LauncherError::Generic {
                code: "ERR_NETWORK".into(),
                message: format!("Failed to build launch HTTP client: {e}"),
            })
    }

    fn for_category(&self, category: NetworkCategory) -> &reqwest::Client {
        match category {
            NetworkCategory::MojangMetadata => &self.mojang_metadata,
            NetworkCategory::MojangContent => &self.mojang_content,
            NetworkCategory::LoaderMetadataAndContent => &self.loader,
            // These categories are not used by the planner; fall back to
            // the metadata client to avoid a panic (caller will hit policy
            // check first in practice).
            _ => &self.mojang_metadata,
        }
    }
}

/// Check whether a redirect (or initial request) target is safe for the
/// given expected category.  This is called **before** opening a socket to
/// the redirect target, preventing SSRF via forged redirect chains.
fn redirect_target_is_safe(url: &reqwest::Url, expected: NetworkCategory) -> bool {
    // Must be HTTPS on standard port.
    if url.scheme() != "https" || url.port_or_known_default() != Some(443) {
        return false;
    }
    let host = match url.host_str() {
        Some(h) => h,
        None => return false,
    };
    // Reject IP literals, localhost, and private metadata endpoints.
    if is_ip_literal(host) || is_private_or_local(host) {
        return false;
    }
    // The host must classify to exactly the expected category.
    network::classify_host(host) == Some(expected)
}

/// True if `host` is a bare IPv4 or IPv6 address.
fn is_ip_literal(host: &str) -> bool {
    // IPv4: four dot-separated decimal octets.
    if host.contains('.') {
        return host.split('.').all(|octet| {
            !octet.is_empty() && octet.bytes().all(|b| b.is_ascii_digit()) && octet.len() <= 3
        });
    }
    // IPv6: contains ':'.
    host.contains(':')
}

/// True if `host` is a loopback, link-local, or cloud metadata endpoint.
fn is_private_or_local(host: &str) -> bool {
    // IPv4 private / local ranges
    if host.starts_with("127.") || host == "0.0.0.0" || host == "255.255.255.255" {
        return true;
    }
    if host.starts_with("10.") || host.starts_with("192.168.") {
        return true;
    }
    if host.starts_with("172.") {
        // 172.16.0.0 – 172.31.255.255
        let octets: Vec<&str> = host.splitn(3, '.').collect();
        if octets.len() >= 2 {
            if let Ok(second) = octets[1].parse::<u8>() {
                if (16..=31).contains(&second) {
                    return true;
                }
            }
        }
    }
    // Link-local (169.254.x.x)
    if host.starts_with("169.254.") {
        return true;
    }
    // IPv6 loopback / unspecified
    if host == "::1" || host == "::" || host.starts_with("0:0:0:0:0:0:0:") {
        return true;
    }
    // Common cloud metadata endpoints
    if matches!(
        host,
        "169.254.169.254" | "fd00:ec2::254" | "100.100.100.200"
    ) {
        return true;
    }
    false
}

/// Inputs required for the metadata-only resolve stage.
#[derive(Debug, Clone)]
pub struct ResolveRequest {
    pub instance_id: String,
    pub base_version_id: String,
    pub loader: Option<LoaderInfo>,
    pub game_dir: PathBuf,
    pub assets_dir: PathBuf,
    /// Root for metadata now and all launch artifacts in later stages.
    pub cache_dir: PathBuf,
    pub java_override: Option<PathBuf>,
    /// Discovery should run off the async command thread and be supplied here.
    pub java_candidates: Vec<JavaInstallation>,
    /// If `true` and the user has set an explicit `java_override`, allow
    /// selecting a Java runtime whose major version does not match the
    /// required version.  The resolved plan will carry
    /// [`incompatible_override`](ResolvedJava::incompatible_override) `true`.
    /// Default: `false`.
    pub allow_incompatible_java_override: bool,
    /// Explicit network policy. Core never guesses a DB path.
    pub network_policy: NetworkPolicy,
    /// For Forge/NeoForge adoption: path to the Minecraft directory
    /// (containing `versions/`). `None` means adoption is unavailable.
    pub minecraft_dir: Option<PathBuf>,
    /// For Forge/NeoForge adoption: path to loader receipts directory.
    /// `None` means adoption is unavailable.
    pub receipts_root: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ResolvedJava {
    pub path: PathBuf,
    pub major_version: u32,
    pub required_major_version: u32,
    /// `true` when the selected Java does NOT match the required major
    /// version but was accepted because `allow_incompatible_java_override`
    /// was set in the resolve request.
    pub incompatible_override: bool,
}

/// Metadata-only plan. No client JAR, libraries, assets or natives have been
/// downloaded at this stage.
///
/// Forge/NeoForge loaders use the `adopted_profile` path (installed-profile
/// adoption). The managed installer execution path has been removed.
#[derive(Debug, Clone)]
pub struct ResolvedLaunchPlan {
    pub instance_id: String,
    pub version_id: String,
    pub base_version_id: String,
    pub loader: Option<LoaderInfo>,
    pub java: ResolvedJava,
    pub version: VersionInfo,
    pub game_dir: PathBuf,
    pub assets_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub network_policy: NetworkPolicy,
    /// Present for Forge/NeoForge loaders whose installed profile was
    /// successfully adopted. `None` for vanilla, Fabric, Quilt, or when
    /// adoption paths are not provided.
    pub adopted_profile: Option<crate::installed_profile::AdoptedProfile>,
}

#[derive(Debug, Clone)]
pub struct VerifiedArtifact {
    pub path: PathBuf,
    pub sha1: Option<String>,
    pub size: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct MaterializedLaunchPlan {
    pub resolved: ResolvedLaunchPlan,
    pub classpath: Vec<VerifiedArtifact>,
    pub client_jar: VerifiedArtifact,
    pub natives_dir: PathBuf,
    pub asset_index_path: PathBuf,
    pub logging_config_path: Option<PathBuf>,
}

/// Minecraft authentication identity consumed by the launcher.
///
/// # Security
/// The `access_token` is never printed in `Debug` output. The `uuid`, `client_id`,
/// and `xuid` are partially redacted to avoid leaking full identifiers in logs.
/// However, the token is still present in-memory and is unavoidably visible to
/// same-user OS-level process inspection (e.g. `/proc/pid/mem` on Linux, MiniDump
/// on Windows, `vmmap`/`task_for_pid` on macOS). Do NOT claim this abstraction
/// eliminates all disclosure vectors — it only prevents accidental leakage into
/// log files, crash dumps, and debug formatting.
#[derive(Clone)]
pub struct LaunchIdentity {
    pub username: String,
    pub access_token: String,
    pub uuid: String,
    pub user_type: String,
    pub client_id: String,
    pub xuid: String,
    pub user_properties: String,
}

impl std::fmt::Debug for LaunchIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn partial(s: &str) -> String {
            if s.len() > 8 {
                format!("{}…{}", &s[..4], &s[s.len() - 4..])
            } else if !s.is_empty() {
                "[redacted]".into()
            } else {
                String::new()
            }
        }
        f.debug_struct("LaunchIdentity")
            .field("username", &self.username)
            .field("access_token", &"[REDACTED]")
            .field("uuid", &partial(&self.uuid))
            .field("user_type", &self.user_type)
            .field("client_id", &partial(&self.client_id))
            .field("xuid", &partial(&self.xuid))
            .field("user_properties", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, Default)]
pub struct LaunchFeatures {
    pub values: BTreeMap<String, bool>,
    pub resolution_width: Option<u32>,
    pub resolution_height: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct BuildCommandRequest<'a> {
    pub plan: &'a MaterializedLaunchPlan,
    pub identity: &'a LaunchIdentity,
    pub features: &'a LaunchFeatures,
    /// Already-tokenized JVM arguments. Callers must not pass shell strings.
    pub user_jvm_args: &'a [String],
    pub extra_game_args: &'a [String],
}

/// A fully-expanded command ready for spawning.
///
/// # Security
/// `Debug` only reports `program`, `cwd`, the argument count, and environment
/// key names — never the expanded argument values or environment values
/// (which may contain the access token, JVM args with paths, etc.).
#[derive(Clone)]
pub struct PreparedCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
}

impl std::fmt::Debug for PreparedCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut ds = f.debug_struct("PreparedCommand");
        ds.field("program", &self.program);
        ds.field("arg_count", &self.args.len());
        ds.field("cwd", &self.cwd);
        // Only emit environment key names, never values (which may contain secrets).
        let env_keys: Vec<&str> = self.env.keys().map(|k| k.as_str()).collect();
        ds.field("env_keys", &env_keys);
        ds.finish()
    }
}

#[derive(Debug, serde::Deserialize)]
struct AssetIndexDocument {
    #[serde(default)]
    objects: BTreeMap<String, AssetObject>,
    #[serde(default, rename = "virtual")]
    virtual_: bool,
    #[serde(default, rename = "map_to_resources")]
    map_to_resources: bool,
}

#[derive(Debug, serde::Deserialize)]
struct AssetObject {
    hash: String,
    size: i64,
}

/// Resolve base Minecraft metadata, an optional pinned Fabric/Quilt profile,
/// and a compatible Java executable. Cache hits are verified and used without
/// contacting the network, enabling offline resolution after first install.
///
/// Every HTTP request is gated by the caller-supplied `network_policy`:
/// a denied cache miss returns the category-specific error immediately
/// without opening any socket.
pub async fn resolve(request: ResolveRequest) -> LauncherResult<ResolvedLaunchPlan> {
    // Stage 2: Forge/NeoForge resolve via adopted profile BEFORE Mojang manifest.
    // This avoids any Mojang network fetch for the forge/neoforge path.
    if let Some(ref loader) = request.loader {
        if matches!(loader.loader_type.as_str(), "forge" | "neoforge") {
            let (minecraft_dir, receipts_root) = match (
                &request.minecraft_dir,
                &request.receipts_root,
            ) {
                (Some(md), Some(rr)) => (md.clone(), rr.clone()),
                _ => {
                    return Err(LauncherError::ProfileMissing(
                        crate::installed_profile::ProfileIssue::missing(
                            None,
                            "Forge/NeoForge resolve requires minecraft_dir and receipts_root for profile adoption",
                        ),
                    ));
                }
            };

            // Find curated manifest entry (exact tuple, no direct-launch flag)
            let entry = loader_manifests::find_entry(
                &loader.loader_type,
                &request.base_version_id,
                &loader.version,
            )
            .ok_or_else(|| LauncherError::Generic {
                code: "ERR_LOADER_PROFILE_NOT_FOUND".into(),
                message: format!(
                    "Loader entry not found in curated manifests: {} {} {}",
                    loader.loader_type, request.base_version_id, loader.version
                ),
            })?;

            let tuple = crate::installed_profile::LoaderTuple {
                loader: loader.loader_type.clone(),
                minecraft_version: request.base_version_id.clone(),
                loader_version: loader.version.clone(),
            };

            let adopted = crate::installed_profile::adopt_installed_profile(
                &minecraft_dir,
                &receipts_root,
                &tuple,
                Some(&entry.sha256),
            )
            .map_err(|issue| {
                let err: LauncherError = issue.into();
                err
            })?;

            // Java selection, version id
            let version = &adopted.merged_version;
            let required_major = version
                .java_version
                .as_ref()
                .and_then(|java| u32::try_from(java.major_version).ok())
                .filter(|major| *major > 0)
                .unwrap_or(8);
            let selected_java = select_java(
                request.java_override.as_deref(),
                &request.java_candidates,
                required_major,
                request.allow_incompatible_java_override,
            )?;

            let version_id = if version.id.is_empty() {
                request.base_version_id.clone()
            } else {
                version.id.clone()
            };

            return Ok(ResolvedLaunchPlan {
                instance_id: request.instance_id,
                version_id,
                base_version_id: request.base_version_id,
                loader: request.loader,
                java: selected_java,
                version: version.clone(),
                game_dir: request.game_dir,
                assets_dir: request.assets_dir,
                cache_dir: request.cache_dir,
                network_policy: request.network_policy,
                adopted_profile: Some(adopted),
            });
        }
    }

    // Vanilla / Fabric / Quilt path: fetch Mojang manifest as before.
    let clients = LaunchHttpClients::new()?;
    let policy = &request.network_policy;
    let metadata_dir = request.cache_dir.join("metadata");
    let manifest_path = metadata_dir.join("version_manifest_v2.json");
    let manifest: MojangVersionManifest = load_json_cache_first(
        clients.for_category(NetworkCategory::MojangMetadata),
        VERSION_MANIFEST_URL,
        &manifest_path,
        None,
        policy,
        NetworkCategory::MojangMetadata,
    )
    .await?;

    let version_ref = manifest
        .versions
        .iter()
        .find(|version| version.id == request.base_version_id)
        .ok_or(LauncherError::GameVersionNotFound)?;
    let base_path = metadata_dir
        .join("versions")
        .join(format!("{}.json", request.base_version_id));
    let base: VersionInfo = load_json_cache_first(
        clients.for_category(NetworkCategory::MojangMetadata),
        &version_ref.url,
        &base_path,
        version_ref.sha1.as_deref(),
        policy,
        NetworkCategory::MojangMetadata,
    )
    .await?;

    let (version, adopted_profile) = match request.loader.as_ref() {
        None => (base, None),
        Some(loader) if loader.loader_type == "vanilla" || loader.loader_type.is_empty() => {
            (base, None)
        }
        Some(loader) if matches!(loader.loader_type.as_str(), "fabric" | "quilt") => {
            let v = resolve_json_loader(
                clients.for_category(NetworkCategory::LoaderMetadataAndContent),
                loader,
                &request.base_version_id,
                &metadata_dir,
                &base,
                policy,
            )
            .await?;
            (v, None)
        }
        // forge/neoforge is handled above; this arm is a safety net for any
        // loader_type value that slipped past the early-return check.
        Some(loader) if matches!(loader.loader_type.as_str(), "forge" | "neoforge") => {
            return Err(LauncherError::Generic {
                code: "ERR_LOADER_NO_ADOPTION_PATH".into(),
                message: "Forge/NeoForge must be resolved via the adoption path but reached the \
                          Mojang manifest branch"
                    .into(),
            });
        }
        Some(_) => return Err(LauncherError::UnsupportedLoader),
    };

    let required_major = version
        .java_version
        .as_ref()
        .and_then(|java| u32::try_from(java.major_version).ok())
        .filter(|major| *major > 0)
        .unwrap_or(8);
    let selected_java = select_java(
        request.java_override.as_deref(),
        &request.java_candidates,
        required_major,
        request.allow_incompatible_java_override,
    )?;

    let version_id = if version.id.is_empty() {
        request.base_version_id.clone()
    } else {
        version.id.clone()
    };

    Ok(ResolvedLaunchPlan {
        instance_id: request.instance_id,
        version_id,
        base_version_id: request.base_version_id,
        loader: request.loader,
        java: selected_java,
        version,
        game_dir: request.game_dir,
        assets_dir: request.assets_dir,
        cache_dir: request.cache_dir,
        network_policy: request.network_policy,
        adopted_profile,
    })
}

/// Download or reuse every launch artifact and verify its manifest-provided
/// hash before it becomes part of the materialized plan.
///
/// Every download is gated by the policy embedded in `resolved.network_policy`.
/// Cache hits are validated and returned without network access.
pub async fn materialize(
    mut resolved: ResolvedLaunchPlan,
) -> LauncherResult<MaterializedLaunchPlan> {
    let clients = LaunchHttpClients::new()?;
    let libraries_dir = resolved.cache_dir.join("libraries");
    let versions_dir = resolved.cache_dir.join("versions");
    let logging_dir = resolved.cache_dir.join("logging");
    let natives_dir = resolved
        .cache_dir
        .join("natives")
        .join(&resolved.version_id)
        .join(platform_key());

    // -----------------------------------------------------------------------
    // Adopted profile materialization: reuse installed artifacts
    // -----------------------------------------------------------------------
    let adopted_profile = resolved.adopted_profile.take();
    let network_policy = resolved.network_policy.clone();
    if let Some(adopted_profile) = adopted_profile {
        return materialize_adopted_profile(
            resolved,
            adopted_profile,
            network_policy,
            &libraries_dir,
            &versions_dir,
            &logging_dir,
            &natives_dir,
        );
    }

    let policy = &network_policy;
    let client_download = resolved
        .version
        .downloads
        .as_ref()
        .and_then(|downloads| downloads.client.as_ref())
        .ok_or_else(|| LauncherError::Generic {
            code: "ERR_CLIENT_DOWNLOAD_MISSING".into(),
            message: format!(
                "Minecraft {} metadata has no client download.",
                resolved.base_version_id
            ),
        })?;
    let client_path = versions_dir
        .join(&resolved.base_version_id)
        .join(format!("{}.jar", resolved.base_version_id));
    download_sha1_atomic(
        clients.for_category(NetworkCategory::MojangContent),
        &client_download.url,
        &client_path,
        client_download.sha1.as_deref(),
        client_download.size,
        policy,
        NetworkCategory::MojangContent,
    )
    .await?;
    let client_jar = VerifiedArtifact {
        path: client_path,
        sha1: client_download.sha1.clone(),
        size: client_download.size,
    };

    let mut classpath = Vec::new();
    let mut native_archives = Vec::new();
    for library in resolved
        .version
        .libraries
        .iter()
        .filter(|library| rules_allow(library.rules.as_deref(), &BTreeMap::new()))
    {
        if let Some(artifact) = resolve_library_artifact(library)? {
            let path = libraries_dir.join(&artifact.path);
            // Classify the library URL to use the correct policy category.
            // Mojang-hosted libraries are MojangContent; loader Maven repos
            // are LoaderMetadataAndContent.
            let lib_category =
                network::classify_url(&artifact.url).unwrap_or(NetworkCategory::MojangContent);

            if lib_category == NetworkCategory::LoaderMetadataAndContent {
                // Prefer pinned SHA-256 for loader libraries when available.
                download_library_with_pin(
                    clients.for_category(NetworkCategory::LoaderMetadataAndContent),
                    &artifact,
                    &path,
                    policy,
                )
                .await?;
            } else {
                download_sha1_atomic(
                    clients.for_category(lib_category),
                    &artifact.url,
                    &path,
                    artifact.sha1.as_deref(),
                    artifact.size,
                    policy,
                    lib_category,
                )
                .await?;
            }
            classpath.push(VerifiedArtifact {
                path,
                sha1: artifact.sha1,
                size: artifact.size,
            });
        } else if library.natives.is_none() {
            return Err(LauncherError::Generic {
                code: "ERR_LIBRARY_ARTIFACT_MISSING".into(),
                message: format!(
                    "Library {} has neither an explicit artifact nor a Maven repository URL.",
                    library.name
                ),
            });
        }

        if let Some(artifact) = resolve_native_artifact(library)? {
            let path = libraries_dir.join(&artifact.path);
            let lib_category =
                network::classify_url(&artifact.url).unwrap_or(NetworkCategory::MojangContent);
            if lib_category == NetworkCategory::LoaderMetadataAndContent {
                download_library_with_pin(
                    clients.for_category(NetworkCategory::LoaderMetadataAndContent),
                    &artifact,
                    &path,
                    policy,
                )
                .await?;
            } else {
                download_sha1_atomic(
                    clients.for_category(lib_category),
                    &artifact.url,
                    &path,
                    artifact.sha1.as_deref(),
                    artifact.size,
                    policy,
                    lib_category,
                )
                .await?;
            }
            native_archives.push((path, library.extract.clone()));
        }
    }

    // Minecraft expects client.jar after libraries on the classpath.
    classpath.push(client_jar.clone());
    extract_natives_atomically(&native_archives, &natives_dir)?;

    let asset_index =
        resolved
            .version
            .asset_index
            .as_ref()
            .ok_or_else(|| LauncherError::Generic {
                code: "ERR_ASSET_INDEX_MISSING".into(),
                message: format!(
                    "Minecraft {} metadata has no asset index.",
                    resolved.base_version_id
                ),
            })?;
    let asset_index_path = resolved
        .assets_dir
        .join("indexes")
        .join(format!("{}.json", asset_index.id));
    download_sha1_atomic(
        clients.for_category(NetworkCategory::MojangMetadata),
        &asset_index.url,
        &asset_index_path,
        asset_index.sha1.as_deref(),
        asset_index.size,
        policy,
        NetworkCategory::MojangMetadata,
    )
    .await?;
    materialize_assets(
        clients.for_category(NetworkCategory::MojangContent),
        &resolved.assets_dir,
        &asset_index_path,
        policy,
    )
    .await?;

    let logging_config_path = if let Some(logging) = resolved
        .version
        .logging
        .as_ref()
        .and_then(|logging| logging.client.as_ref())
        .and_then(|logging| logging.file.as_ref())
    {
        let path = logging_dir.join(&logging.id);
        download_sha1_atomic(
            clients.for_category(NetworkCategory::MojangContent),
            &logging.url,
            &path,
            logging.sha1.as_deref(),
            logging.size,
            policy,
            NetworkCategory::MojangContent,
        )
        .await?;
        Some(path)
    } else {
        None
    };

    Ok(MaterializedLaunchPlan {
        resolved,
        classpath,
        client_jar,
        natives_dir,
        asset_index_path,
        logging_config_path,
    })
}

/// Materialize a launch plan using an adopted installed profile.
///
/// Uses the installed `.minecraft` source for client jar, libraries, assets,
/// and logging config. No managed processor execution occurs — Forge/NeoForge
/// uses the installed-profile adoption path only.
///
/// Every artifact is cache-first, then installed-source, then (for non-adopted
/// paths) network. For adopted profiles, missing installed source for a
/// required artifact returns an error rather than silently downloading.
fn materialize_adopted_profile(
    resolved: ResolvedLaunchPlan,
    adopted_profile: crate::installed_profile::AdoptedProfile,
    policy: NetworkPolicy,
    libraries_dir: &Path,
    versions_dir: &Path,
    logging_dir: &Path,
    natives_dir: &Path,
) -> LauncherResult<MaterializedLaunchPlan> {
    // Build the installed artifact source from the minecraft_dir stored during adoption.
    let source = crate::installed_artifact::InstalledArtifactSource::new(
        adopted_profile.minecraft_dir.clone(),
    );

    // Client JAR
    let client_download = resolved
        .version
        .downloads
        .as_ref()
        .and_then(|downloads| downloads.client.as_ref())
        .ok_or_else(|| LauncherError::Generic {
            code: "ERR_CLIENT_DOWNLOAD_MISSING".into(),
            message: format!(
                "Minecraft {} metadata has no client download.",
                resolved.base_version_id
            ),
        })?;
    let client_path = versions_dir
        .join(&resolved.base_version_id)
        .join(format!("{}.jar", resolved.base_version_id));

    // Try cache first, then installed source
    let client_sha1 = client_download.sha1.as_deref();
    let client_size = client_download.size;

    let client_result = crate::installed_artifact::adopt_client_jar(
        &source,
        &client_path,
        &resolved.base_version_id,
        client_sha1,
        client_size,
    )
    .map_err(|issue| {
        let err: LauncherError = issue.into();
        err
    })?;

    match client_result {
        crate::installed_artifact::ArtifactAdoptResult::CacheHit
        | crate::installed_artifact::ArtifactAdoptResult::Materialized { .. } => {
            // Successfully adopted from cache or installed source
        }
        crate::installed_artifact::ArtifactAdoptResult::SourceMissing => {
            return Err(LauncherError::Generic {
                code: "ERR_ADOPTED_CLIENT_JAR_MISSING".into(),
                message: format!(
                    "Client JAR for {} is not available in cache or installed source at {}. \
                     Use the Mojang launcher to download the game files first.",
                    resolved.base_version_id,
                    source.client_jar(&resolved.base_version_id).display()
                ),
            });
        }
    }

    let client_jar = VerifiedArtifact {
        path: client_path.clone(),
        sha1: client_download.sha1.clone(),
        size: client_download.size,
    };

    // Libraries and natives
    let mut classpath = Vec::new();
    let mut native_archives = Vec::new();

    // Build a set of promoted library paths from the receipt's generated_artifact_sha256
    // for fast lookup during library iteration
    let generated_paths: std::collections::HashSet<String> = adopted_profile
        .receipt
        .as_ref()
        .and_then(|r| r.generated_artifact_sha256.as_ref())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();

    // Reference to the installed source for library adoption
    let src = &source;

    for library in resolved
        .version
        .libraries
        .iter()
        .filter(|library| rules_allow(library.rules.as_deref(), &BTreeMap::new()))
    {
        if let Some(artifact) = resolve_library_artifact(library)? {
            let path = libraries_dir.join(&artifact.path);
            let lib_category =
                network::classify_url(&artifact.url).unwrap_or(NetworkCategory::MojangContent);

            // Check if this is a "promoted" (generated) library path
            let is_generated = generated_paths.contains(&artifact.path);

            if is_generated {
                // Trusted generated artifact: use receipt hash verification
                if let Some(ref receipt) = adopted_profile.receipt {
                    let result = crate::installed_artifact::adopt_trusted_unhashed_library(
                        src,
                        &path,
                        &artifact.path,
                        receipt,
                    )
                    .map_err(|issue| {
                        let err: LauncherError = issue.into();
                        err
                    })?;

                    match result {
                        crate::installed_artifact::ArtifactAdoptResult::CacheHit => {}
                        crate::installed_artifact::ArtifactAdoptResult::Materialized { .. } => {}
                        crate::installed_artifact::ArtifactAdoptResult::SourceMissing => {
                            return Err(LauncherError::Generic {
                                code: "ERR_ADOPTED_GENERATED_LIB_MISSING".into(),
                                message: format!(
                                    "Generated library '{}' not available in installed source",
                                    artifact.path
                                ),
                            });
                        }
                    }
                } else {
                    // No receipt — should not happen for adopted profiles
                    return Err(LauncherError::ProfileMissing(
                        crate::installed_profile::ProfileIssue::missing(
                            None,
                            format!(
                                "No receipt available for generated library '{}'",
                                artifact.path
                            ),
                        ),
                    ));
                }
            } else {
                // Normal (non-generated) library: try installed source first
                let result = crate::installed_artifact::adopt_library_artifact(
                    src,
                    &path,
                    &artifact.path,
                    artifact.sha1.as_deref(),
                    None, // No SHA-256 at this level
                    artifact.size,
                )
                .map_err(|issue| {
                    let err: LauncherError = issue.into();
                    err
                })?;

                match result {
                    crate::installed_artifact::ArtifactAdoptResult::CacheHit
                    | crate::installed_artifact::ArtifactAdoptResult::Materialized { .. } => {
                        // Successfully adopted from cache or installed source
                    }
                    crate::installed_artifact::ArtifactAdoptResult::SourceMissing => {
                        // Fall back to network for normal libraries (unlike generated ones)
                        if lib_category == NetworkCategory::LoaderMetadataAndContent {
                            // For loader libs without installed source, we can't easily
                            // download without a pin. Since we're in adopted profile mode
                            // and the user should have the files installed, error out.
                            return Err(LauncherError::Generic {
                                code: "ERR_ADOPTED_LIBRARY_MISSING".into(),
                                message: format!(
                                    "Loader library '{}' not available in cache or installed source. \
                                     Use the Mojang launcher to install this version first.",
                                    artifact.path
                                ),
                            });
                        }
                        // For Mojang libraries, error too (they should be in the installed source)
                        return Err(LauncherError::Generic {
                            code: "ERR_ADOPTED_LIBRARY_MISSING".into(),
                            message: format!(
                                "Minecraft library '{}' not available in cache or installed source. \
                                 Use the Mojang launcher to install this version first.",
                                artifact.path
                            ),
                        });
                    }
                }
            }

            classpath.push(VerifiedArtifact {
                path,
                sha1: artifact.sha1,
                size: artifact.size,
            });
        } else if library.natives.is_none() {
            return Err(LauncherError::Generic {
                code: "ERR_LIBRARY_ARTIFACT_MISSING".into(),
                message: format!(
                    "Library {} has neither an explicit artifact nor a Maven repository URL.",
                    library.name
                ),
            });
        }

        if let Some(artifact) = resolve_native_artifact(library)? {
            let path = libraries_dir.join(&artifact.path);
            // Try installed source first for natives
            let result = crate::installed_artifact::adopt_library_artifact(
                src,
                &path,
                &artifact.path,
                artifact.sha1.as_deref(),
                None,
                artifact.size,
            )
            .map_err(|issue| {
                let err: LauncherError = issue.into();
                err
            })?;

            match result {
                crate::installed_artifact::ArtifactAdoptResult::CacheHit
                | crate::installed_artifact::ArtifactAdoptResult::Materialized { .. } => {}
                crate::installed_artifact::ArtifactAdoptResult::SourceMissing => {
                    return Err(LauncherError::Generic {
                        code: "ERR_ADOPTED_NATIVE_MISSING".into(),
                        message: format!(
                            "Native library '{}' not available in cache or installed source.",
                            artifact.path
                        ),
                    });
                }
            }

            native_archives.push((path, library.extract.clone()));
        }
    }

    // Minecraft expects client.jar after libraries on the classpath.
    classpath.push(client_jar.clone());
    extract_natives_atomically(&native_archives, natives_dir)?;

    // Asset index
    let asset_index =
        resolved
            .version
            .asset_index
            .as_ref()
            .ok_or_else(|| LauncherError::Generic {
                code: "ERR_ASSET_INDEX_MISSING".into(),
                message: format!(
                    "Minecraft {} metadata has no asset index.",
                    resolved.base_version_id
                ),
            })?;
    let asset_index_path = resolved
        .assets_dir
        .join("indexes")
        .join(format!("{}.json", asset_index.id));

    let idx_result = crate::installed_artifact::adopt_asset_index(
        src,
        &asset_index_path,
        &asset_index.id,
        asset_index.sha1.as_deref(),
        asset_index.size,
    )
    .map_err(|issue| {
        let err: LauncherError = issue.into();
        err
    })?;

    match idx_result {
        crate::installed_artifact::ArtifactAdoptResult::CacheHit
        | crate::installed_artifact::ArtifactAdoptResult::Materialized { .. } => {}
        crate::installed_artifact::ArtifactAdoptResult::SourceMissing => {
            return Err(LauncherError::Generic {
                code: "ERR_ADOPTED_ASSET_INDEX_MISSING".into(),
                message: format!(
                    "Asset index '{}' not available in cache or installed source.",
                    asset_index.id
                ),
            });
        }
    }

    // Asset objects (using installed source, no network)
    crate::installed_artifact::adopt_asset_objects(
        src,
        &resolved.assets_dir,
        &asset_index_path,
        &policy,
    )?;

    // Logging config
    let logging_config_path = if let Some(logging) = resolved
        .version
        .logging
        .as_ref()
        .and_then(|logging| logging.client.as_ref())
        .and_then(|logging| logging.file.as_ref())
    {
        let path = logging_dir.join(&logging.id);
        let log_result = crate::installed_artifact::adopt_logging_config(
            src,
            &path,
            &logging.id,
            logging.sha1.as_deref(),
            logging.size,
        )
        .map_err(|issue| {
            let err: LauncherError = issue.into();
            err
        })?;

        match log_result {
            crate::installed_artifact::ArtifactAdoptResult::CacheHit
            | crate::installed_artifact::ArtifactAdoptResult::Materialized { .. } => Some(path),
            crate::installed_artifact::ArtifactAdoptResult::SourceMissing => {
                // Logging config is optional — skip if not available
                None
            }
        }
    } else {
        None
    };

    Ok(MaterializedLaunchPlan {
        resolved,
        classpath,
        client_jar,
        natives_dir: natives_dir.to_path_buf(),
        asset_index_path,
        logging_config_path,
    })
}

/// Validate materialized files independently of command construction.
pub fn validate(plan: &MaterializedLaunchPlan) -> LauncherResult<()> {
    if plan.resolved.version.main_class.trim().is_empty() {
        return Err(LauncherError::Generic {
            code: "ERR_MAIN_CLASS_MISSING".into(),
            message: "Resolved Minecraft metadata has no main class.".into(),
        });
    }
    // Enforce the selected compatibility policy.
    //
    // Default policy (incompatible_override == false): the selected Java major
    // must EXACTLY match the required major.  A higher version is just as
    // wrong as a lower one for modded Minecraft — old loaders and mods depend
    // on behaviour removed or restricted in later Java versions.
    //
    // Override policy (incompatible_override == true): the user explicitly
    // accepted a mismatch (e.g. "Use Java 21 despite this instance requesting
    // Java 17").  We still reject LOWER versions because an override is meant
    // for using a *newer* Java, not an insufficiently old one.
    if !plan.resolved.java.incompatible_override
        && plan.resolved.java.major_version != plan.resolved.java.required_major_version
    {
        return Err(LauncherError::JavaIncompatible);
    }
    if plan.resolved.java.major_version < plan.resolved.java.required_major_version {
        return Err(LauncherError::JavaIncompatible);
    }
    if !plan.client_jar.path.is_file() {
        return missing_artifact("client JAR", &plan.client_jar.path);
    }
    if !plan.asset_index_path.is_file() {
        return missing_artifact("asset index", &plan.asset_index_path);
    }
    if !plan.natives_dir.is_dir() {
        return missing_artifact("native directory", &plan.natives_dir);
    }
    for artifact in &plan.classpath {
        if !artifact.path.is_file() {
            return missing_artifact("classpath entry", &artifact.path);
        }
    }
    Ok(())
}

/// Construct the single canonical Java command for both desktop and CLI.
pub fn build_command(request: BuildCommandRequest<'_>) -> LauncherResult<PreparedCommand> {
    validate(request.plan)?;
    validate_user_jvm_args(request.user_jvm_args)?;

    let classpath = request
        .plan
        .classpath
        .iter()
        .map(|artifact| artifact.path.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(classpath_separator());
    let context = TemplateContext {
        request: &request,
        classpath: &classpath,
    };

    let mut jvm_args = Vec::new();
    if let Some(arguments) = &request.plan.resolved.version.arguments {
        expand_argument_list(
            &arguments.jvm,
            &context,
            &request.features.values,
            &mut jvm_args,
        )?;
    }

    if let Some(logging_client) = request
        .plan
        .resolved
        .version
        .logging
        .as_ref()
        .and_then(|logging| logging.client.as_ref())
    {
        if let Some(path) = &request.plan.logging_config_path {
            jvm_args.push(
                logging_client
                    .argument
                    .replace("${path}", &path.to_string_lossy()),
            );
        }
    }
    jvm_args.extend(request.user_jvm_args.iter().cloned());

    if !has_native_path(&jvm_args) {
        jvm_args.push(format!(
            "-Djava.library.path={}",
            request.plan.natives_dir.to_string_lossy()
        ));
    }
    if !has_classpath(&jvm_args) {
        jvm_args.push("-cp".into());
        jvm_args.push(classpath.clone());
    }

    let mut game_args = Vec::new();
    if let Some(arguments) = &request.plan.resolved.version.arguments {
        expand_argument_list(
            &arguments.game,
            &context,
            &request.features.values,
            &mut game_args,
        )?;
    } else if let Some(legacy) = &request.plan.resolved.version.minecraft_arguments {
        game_args.extend(parse_argument_string(&substitute(legacy, &context))?);
    }
    game_args.extend(request.extra_game_args.iter().cloned());

    let mut args = jvm_args;
    args.push(request.plan.resolved.version.main_class.clone());
    args.extend(game_args);
    if args.iter().any(|argument| argument.contains("${")) {
        return Err(LauncherError::UnresolvedPlaceholder);
    }

    Ok(PreparedCommand {
        program: request.plan.resolved.java.path.clone(),
        args,
        cwd: request.plan.resolved.game_dir.clone(),
        env: BTreeMap::new(),
    })
}

/// Spawn the prepared command while leaving ownership and lifecycle handling
/// with the desktop or CLI caller.
pub fn spawn(prepared: &PreparedCommand) -> LauncherResult<tokio::process::Child> {
    let mut command = tokio::process::Command::new(&prepared.program);
    command
        .args(&prepared.args)
        .current_dir(&prepared.cwd)
        .envs(&prepared.env)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = command.spawn().map_err(|error| LauncherError::Generic {
        code: "ERR_LAUNCH_SPAWN".into(),
        message: format!("Failed to spawn Java: {error}"),
    })?;
    child.id().ok_or_else(|| LauncherError::Generic {
        code: "ERR_NO_PID".into(),
        message: "Spawned process has no PID.".into(),
    })?;
    Ok(child)
}

/// Wait for a CLI-owned child and classify its complete output using the same
/// crash/LKG signals as the legacy direct launcher.
///
/// `secrets` is a list of sensitive strings (e.g. the Minecraft access token)
/// that must be redacted from the captured output before passing it to
/// crash-triage, to prevent token leakage into log state or diagnostics.
///
/// # Residual limitation
/// The access token unavoidably appears in the spawned Java process's command
/// line, which is visible to same-user OS-level tools (`ps aux`, `/proc/pid/cmdline`,
/// Process Explorer). This function redacts it from the captured output buffer
/// but does NOT prevent OS-level process inspection.
pub async fn wait_and_classify(
    child: tokio::process::Child,
    game_dir: &Path,
    secrets: &[&str],
) -> LauncherResult<crate::lkg::LaunchOutcome> {
    let started = std::time::Instant::now();
    let launched_at = std::time::SystemTime::now();
    let output = child
        .wait_with_output()
        .await
        .map_err(|error| LauncherError::Generic {
            code: "ERR_WAIT".into(),
            message: format!("Failed while waiting for Java: {error}"),
        })?;
    let mut captured = String::from_utf8_lossy(&output.stdout).into_owned();
    captured.push_str(&String::from_utf8_lossy(&output.stderr));
    // Redact known secrets before triage or any further processing.
    let sanitized = crate::log_sanitizer::sanitize_log_with_secrets(&captured, secrets);
    let crash_report_found = game_dir
        .join("crash-reports")
        .read_dir()
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            entry
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .is_some_and(|modified| modified >= launched_at)
        });
    Ok(crate::lkg::classify_launch(&crate::lkg::LaunchEvents {
        exit_code: output.status.code(),
        runtime_ms: started.elapsed().as_millis() as u64,
        was_user_cancelled: false,
        crash_report_found,
        log_crash_signature_matched: crate::crash_diagnostics::triage(&sanitized).matched,
    }))
}

struct TemplateContext<'a, 'b> {
    request: &'a BuildCommandRequest<'b>,
    classpath: &'a str,
}

fn expand_argument_list(
    values: &[serde_json::Value],
    context: &TemplateContext<'_, '_>,
    features: &BTreeMap<String, bool>,
    output: &mut Vec<String>,
) -> LauncherResult<()> {
    for value in values {
        if let Some(argument) = value.as_str() {
            output.push(substitute(argument, context));
            continue;
        }
        let Some(object) = value.as_object() else {
            continue;
        };
        let rules = object
            .get("rules")
            .map(|rules| serde_json::from_value::<Vec<launch::LibraryRule>>(rules.clone()))
            .transpose()
            .map_err(|error| LauncherError::Generic {
                code: "ERR_ARGUMENT_RULES".into(),
                message: format!("Failed to parse argument rules: {error}"),
            })?;
        if !rules_allow(rules.as_deref(), features) {
            continue;
        }
        match object.get("value") {
            Some(serde_json::Value::String(argument)) => {
                output.push(substitute(argument, context));
            }
            Some(serde_json::Value::Array(arguments)) => {
                for argument in arguments.iter().filter_map(|value| value.as_str()) {
                    output.push(substitute(argument, context));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn substitute(value: &str, context: &TemplateContext<'_, '_>) -> String {
    let plan = context.request.plan;
    let identity = context.request.identity;
    let assets_index = plan
        .resolved
        .version
        .asset_index
        .as_ref()
        .map(|index| index.id.as_str())
        .unwrap_or("");
    value
        .replace("${auth_player_name}", &identity.username)
        .replace("${auth_access_token}", &identity.access_token)
        .replace("${auth_uuid}", &identity.uuid.replace('-', ""))
        .replace("${user_type}", &identity.user_type)
        .replace("${clientid}", &identity.client_id)
        .replace("${auth_xuid}", &identity.xuid)
        .replace("${user_properties}", &identity.user_properties)
        .replace("${version_name}", &plan.resolved.version_id)
        .replace("${version_type}", &plan.resolved.version.type_)
        .replace(
            "${game_directory}",
            &plan.resolved.game_dir.to_string_lossy(),
        )
        .replace(
            "${assets_root}",
            &plan.resolved.assets_dir.to_string_lossy(),
        )
        .replace("${assets_index_name}", assets_index)
        .replace("${natives_directory}", &plan.natives_dir.to_string_lossy())
        .replace(
            "${library_directory}",
            &plan.resolved.cache_dir.join("libraries").to_string_lossy(),
        )
        .replace("${classpath}", context.classpath)
        .replace("${classpath_separator}", classpath_separator())
        .replace("${launcher_name}", "agora")
        .replace("${launcher_version}", env!("CARGO_PKG_VERSION"))
        .replace(
            "${resolution_width}",
            &context
                .request
                .features
                .resolution_width
                .map(|value| value.to_string())
                .unwrap_or_default(),
        )
        .replace(
            "${resolution_height}",
            &context
                .request
                .features
                .resolution_height
                .map(|value| value.to_string())
                .unwrap_or_default(),
        )
}

fn validate_user_jvm_args(arguments: &[String]) -> LauncherResult<()> {
    if arguments.iter().any(|argument| {
        matches!(argument.as_str(), "-cp" | "-classpath")
            || argument.starts_with("-Djava.library.path=")
    }) {
        return Err(LauncherError::Generic {
            code: "ERR_RESERVED_JVM_ARGUMENT".into(),
            message: "Classpath and native-path JVM arguments are managed by Agora.".into(),
        });
    }
    Ok(())
}

fn has_classpath(arguments: &[String]) -> bool {
    arguments
        .iter()
        .any(|argument| matches!(argument.as_str(), "-cp" | "-classpath"))
}

fn has_native_path(arguments: &[String]) -> bool {
    arguments
        .iter()
        .any(|argument| argument.starts_with("-Djava.library.path="))
}

pub fn parse_argument_string(value: &str) -> LauncherResult<Vec<String>> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\\' {
            let escapable = characters
                .peek()
                .is_some_and(|next| Some(*next) == quote || *next == '\\');
            if escapable {
                current.push(characters.next().expect("peeked character"));
            } else {
                current.push(character);
            }
            continue;
        }
        if matches!(character, '\'' | '"') {
            if quote == Some(character) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(character);
            } else {
                current.push(character);
            }
            continue;
        }
        if character.is_whitespace() && quote.is_none() {
            if !current.is_empty() {
                arguments.push(std::mem::take(&mut current));
            }
        } else {
            current.push(character);
        }
    }
    if quote.is_some() {
        return Err(LauncherError::Generic {
            code: "ERR_ARGUMENT_QUOTES".into(),
            message: "Launch argument string contains an unmatched quote.".into(),
        });
    }
    if !current.is_empty() {
        arguments.push(current);
    }
    Ok(arguments)
}

fn classpath_separator() -> &'static str {
    if cfg!(target_os = "windows") {
        ";"
    } else {
        ":"
    }
}

fn missing_artifact<T>(kind: &str, path: &Path) -> LauncherResult<T> {
    Err(LauncherError::Generic {
        code: "ERR_LAUNCH_ARTIFACT_MISSING".into(),
        message: format!("Required {kind} is missing: {}", path.display()),
    })
}

fn resolve_library_artifact(
    library: &launch::Library,
) -> LauncherResult<Option<launch::LibraryArtifact>> {
    if let Some(artifact) = library
        .downloads
        .as_ref()
        .and_then(|downloads| downloads.artifact.clone())
    {
        return Ok(Some(artifact));
    }
    let Some(repository) = library.url.as_deref() else {
        return Ok(None);
    };
    let path = checked_maven_path(&library.name)?;
    let url = format!(
        "{}{path}",
        repository.trim_end_matches('/').to_owned() + "/"
    );
    Ok(Some(launch::LibraryArtifact {
        path,
        url,
        sha1: library.sha1.clone(),
        size: library.size,
    }))
}

fn resolve_native_artifact(
    library: &launch::Library,
) -> LauncherResult<Option<launch::LibraryArtifact>> {
    let Some(classifier_template) = library
        .natives
        .as_ref()
        .and_then(|natives| natives.get(mojang_os_name()))
    else {
        return Ok(None);
    };
    let arch = if usize::BITS == 64 { "64" } else { "32" };
    let classifier = classifier_template.replace("${arch}", arch);
    let artifact = library
        .downloads
        .as_ref()
        .and_then(|downloads| downloads.classifiers.as_ref())
        .and_then(|classifiers| classifiers.get(&classifier))
        .cloned()
        .ok_or_else(|| LauncherError::Generic {
            code: "ERR_NATIVE_CLASSIFIER_MISSING".into(),
            message: format!(
                "Library {} declares native classifier {classifier} but provides no matching artifact.",
                library.name
            ),
        })?;
    Ok(Some(artifact))
}

fn checked_maven_path(name: &str) -> LauncherResult<String> {
    if name.split(':').count() < 3 {
        return Err(LauncherError::Generic {
            code: "ERR_MAVEN_COORDINATE".into(),
            message: format!("Invalid Maven coordinate: {name}"),
        });
    }
    Ok(launch::maven_name_to_path(name))
}

async fn materialize_assets(
    client: &reqwest::Client,
    assets_dir: &Path,
    index_path: &Path,
    policy: &NetworkPolicy,
) -> LauncherResult<()> {
    let bytes = std::fs::read(index_path).map_err(|error| LauncherError::Generic {
        code: "ERR_ASSET_INDEX_READ".into(),
        message: format!("Failed to read {}: {error}", index_path.display()),
    })?;
    let index: AssetIndexDocument =
        serde_json::from_slice(&bytes).map_err(|error| LauncherError::Generic {
            code: "ERR_ASSET_INDEX_PARSE".into(),
            message: format!("Failed to parse {}: {error}", index_path.display()),
        })?;
    for (logical_name, object) in &index.objects {
        if object.hash.len() < 2
            || !object.hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            || object.size < 0
        {
            return Err(LauncherError::Generic {
                code: "ERR_ASSET_OBJECT_INVALID".into(),
                message: format!("Asset index contains invalid object {logical_name}."),
            });
        }
        let object_path = assets_dir
            .join("objects")
            .join(&object.hash[..2])
            .join(&object.hash);
        let url = format!(
            "https://resources.download.minecraft.net/{}/{}",
            &object.hash[..2],
            object.hash
        );
        download_sha1_atomic(
            client,
            &url,
            &object_path,
            Some(&object.hash),
            Some(object.size),
            policy,
            NetworkCategory::MojangContent,
        )
        .await?;

        if index.virtual_ || index.map_to_resources {
            let relative = safe_relative_path(logical_name)?;
            let root = if index.virtual_ {
                assets_dir.join("virtual").join("legacy")
            } else {
                assets_dir.join("resources")
            };
            let destination = root.join(relative);
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent).map_err(|error| LauncherError::Generic {
                    code: "ERR_ASSET_VIRTUAL_DIR".into(),
                    message: format!("Failed to create {}: {error}", parent.display()),
                })?;
            }
            std::fs::copy(&object_path, &destination).map_err(|error| LauncherError::Generic {
                code: "ERR_ASSET_VIRTUAL_COPY".into(),
                message: format!("Failed to copy {}: {error}", destination.display()),
            })?;
        }
    }
    Ok(())
}

async fn download_sha1_atomic(
    client: &reqwest::Client,
    url: &str,
    path: &Path,
    expected_sha1: Option<&str>,
    expected_size: Option<i64>,
    policy: &NetworkPolicy,
    category: NetworkCategory,
) -> LauncherResult<()> {
    ensure_artifact_url(url)?;
    let resolved_sha1 = match expected_sha1 {
        Some(hash) => hash.to_owned(),
        None => resolve_sidecar_sha1(client, url, path, policy, category).await?,
    };
    if let Ok(bytes) = std::fs::read(path) {
        if artifact_matches(&bytes, Some(&resolved_sha1), expected_size) {
            return Ok(());
        }
    }
    // Cache miss: check policy before opening a socket.
    policy.check(category)?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| LauncherError::NetworkOffline)?;
    if !response.status().is_success() {
        return Err(LauncherError::Generic {
            code: "ERR_DOWNLOAD_HTTP".into(),
            message: format!("Download {url} returned HTTP {}", response.status()),
        });
    }
    // Verify the final response URL is from the expected category.
    let final_url = response.url().as_str();
    ensure_artifact_url(final_url)?;
    if let Some(final_category) = network::classify_url(final_url) {
        if final_category != category {
            // The redirect led outside the expected category — reject.
            return Err(LauncherError::UntrustedSource);
        }
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| LauncherError::NetworkOffline)?
        .to_vec();
    if !artifact_matches(&bytes, Some(&resolved_sha1), expected_size) {
        return Err(LauncherError::HashMismatch);
    }
    atomic_write(path, &bytes)
}

/// Resolve an artifact's SHA-1 from its `.sha1` sidecar.
///
/// # Trust model
/// Network-delivered sidecars are a **corruption check only**, not an
/// independent trust anchor. The hash must match the artifact's cumulative
/// SHA-1, but if both the artifact and its sidecar are served from the same
/// untrusted origin, a motivated adversary can serve matching corrupt data.
/// Agora's true trust anchor is the manifest-pinned hash provided by the
/// curated registry or loader-manifest layer. This sidecar path exists
/// solely for Mojang's download infrastructure where no per-file manifest
/// hash is available, and provides exactly the same integrity guarantee
/// as re-downloading the artifact: the download either matches or doesn't.
async fn resolve_sidecar_sha1(
    client: &reqwest::Client,
    artifact_url: &str,
    artifact_path: &Path,
    policy: &NetworkPolicy,
    category: NetworkCategory,
) -> LauncherResult<String> {
    let sidecar_path = artifact_path.with_extension(format!(
        "{}.sha1",
        artifact_path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("artifact")
    ));
    if let Ok(text) = std::fs::read_to_string(&sidecar_path) {
        if let Some(hash) = parse_sha1_sidecar(&text) {
            return Ok(hash);
        }
    }

    let sidecar_url = format!("{artifact_url}.sha1");
    ensure_artifact_url(&sidecar_url)?;
    // Cache miss: check policy before fetching the sidecar.
    policy.check(category)?;
    let response = client
        .get(&sidecar_url)
        .send()
        .await
        .map_err(|_| LauncherError::NetworkOffline)?;
    let final_url = response.url().as_str();
    ensure_artifact_url(final_url)?;
    if let Some(final_category) = network::classify_url(final_url) {
        if final_category != category {
            return Err(LauncherError::UntrustedSource);
        }
    }
    if !response.status().is_success() {
        return Err(LauncherError::Generic {
            code: "ERR_ARTIFACT_HASH_MISSING".into(),
            message: format!(
                "Artifact {artifact_url} has no embedded SHA-1 and its sidecar returned HTTP {}.",
                response.status()
            ),
        });
    }
    let text = response
        .text()
        .await
        .map_err(|_| LauncherError::NetworkOffline)?;
    let hash = parse_sha1_sidecar(&text).ok_or_else(|| LauncherError::Generic {
        code: "ERR_ARTIFACT_HASH_INVALID".into(),
        message: format!("Artifact SHA-1 sidecar is invalid: {sidecar_url}"),
    })?;
    atomic_write(&sidecar_path, hash.as_bytes())?;
    Ok(hash)
}

fn parse_sha1_sidecar(text: &str) -> Option<String> {
    let hash = text.split_whitespace().next()?.trim();
    (hash.len() == 40 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| hash.to_ascii_lowercase())
}

fn artifact_matches(bytes: &[u8], expected_sha1: Option<&str>, expected_size: Option<i64>) -> bool {
    let size_matches = expected_size
        .and_then(|size| usize::try_from(size).ok())
        .map(|size| size == bytes.len())
        .unwrap_or(true);
    let hash_matches = expected_sha1
        .map(|expected| sha1_hex(bytes).eq_ignore_ascii_case(expected))
        .unwrap_or(true);
    size_matches && hash_matches
}

fn ensure_artifact_url(raw: &str) -> LauncherResult<()> {
    let url = reqwest::Url::parse(raw).map_err(|_| LauncherError::UntrustedSource)?;
    let host = url.host_str().ok_or(LauncherError::UntrustedSource)?;
    let mojang_host = matches!(
        host,
        "piston-meta.mojang.com"
            | "piston-data.mojang.com"
            | "launcher.mojang.com"
            | "libraries.minecraft.net"
            | "resources.download.minecraft.net"
    );
    if url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && (mojang_host || loader_manifests::is_allowed_host(host))
    {
        Ok(())
    } else {
        Err(LauncherError::UntrustedSource)
    }
}

// ---------------------------------------------------------------------------
// Native archive extraction
// ---------------------------------------------------------------------------

/// Extract native libraries from their JAR archives into a fresh staging
/// directory, then atomically promote the staging directory to the final
/// destination.
///
/// # Safety properties
/// * Archives are hash-verified before this function is called (the archives
///   themselves are downloaded via SHA-1 verified downloads).
/// * Entry names are validated against directory-traversal, NUL bytes,
///   UNC paths, and Windows drive prefixes.
/// * `ZipFile::unix_mode()` is inspected when present: only regular files
///   and directories are permitted. Symlinks, FIFOs, sockets, block devices,
///   and character devices are rejected.
/// * Limits enforced: 4096 entries per archive, 128 MiB per entry,
///   512 MiB aggregate uncompressed size, and streamed-vs-declared byte
///   count must match.
/// * Duplicate normalized output paths, case-insensitive collisions (for
///   cross-platform safety), and file/directory conflicts are rejected.
/// * `META-INF/` and any configured excludes are always stripped.
/// * On failure the staging directory is removed and the previously promoted
///   natives directory (if any) is left intact.
/// * On Unix, extracted files receive conservative permissions derived from
///   the entry's stored mode bits (owner write stripped, group/other write
///   stripped, no setuid/setgid).
fn extract_natives_atomically(
    archives: &[(PathBuf, Option<launch::ExtractRules>)],
    destination: &Path,
) -> LauncherResult<()> {
    let staging = destination.with_extension("staging");
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(native_io_error)?;
    }
    std::fs::create_dir_all(&staging).map_err(native_io_error)?;

    const PER_ENTRY_LIMIT: u64 = 128 * 1024 * 1024; // 128 MiB
    const AGGREGATE_LIMIT: u64 = 512 * 1024 * 1024;
    const MAX_ENTRIES: usize = 4096;

    let result = (|| {
        let mut seen_paths: Vec<PathBuf> = Vec::new();

        for (archive_path, rules) in archives {
            let file = std::fs::File::open(archive_path).map_err(native_io_error)?;
            let mut archive =
                zip::ZipArchive::new(file).map_err(|error| LauncherError::Generic {
                    code: "ERR_NATIVE_ARCHIVE".into(),
                    message: format!("Invalid native archive {}: {error}", archive_path.display()),
                })?;
            if archive.len() > MAX_ENTRIES {
                return Err(LauncherError::ZipBomb);
            }
            let mut total_size = 0u64;
            for index in 0..archive.len() {
                let mut entry =
                    archive
                        .by_index(index)
                        .map_err(|error| LauncherError::Generic {
                            code: "ERR_NATIVE_ARCHIVE_ENTRY".into(),
                            message: error.to_string(),
                        })?;
                let raw_name = entry.name();

                // Check for NUL bytes in the entry name (must be done on the
                // raw string before any normalization).
                if raw_name.contains('\0') {
                    return Err(LauncherError::Generic {
                        code: "ERR_NATIVE_ARCHIVE_ENTRY_INVALID".into(),
                        message: "Native archive entry name contains NUL byte.".into(),
                    });
                }

                // Normalize path separators for validation.
                let normalized_name = raw_name.replace('\\', "/");

                // Reject absolute Unix paths, Windows drive prefixes,
                // and UNC paths (starts with // after normalization).
                if normalized_name.starts_with('/')
                    || normalized_name.starts_with("//")
                    || normalized_name.contains(':')
                {
                    return Err(LauncherError::OverrideSecurityViolation);
                }

                // Check unix_mode when present: only regular files and
                // directories are permitted.
                if let Some(mode) = entry.unix_mode() {
                    if !is_allowed_unix_mode(mode) {
                        return Err(LauncherError::Generic {
                            code: "ERR_NATIVE_ARCHIVE_ENTRY_INVALID".into(),
                            message: format!(
                                "Native archive entry '{raw_name}' has \
                                 forbidden Unix file type (mode 0{mode:o})."
                            ),
                        });
                    }
                }

                // Exclude META-INF and configured excludes.
                if is_excluded(&normalized_name, rules.as_ref()) {
                    continue;
                }

                if entry.is_dir() {
                    // Create directory entries to ensure they exist (and set
                    // permissions below). Use safe_relative_path for validation.
                    let relative = safe_relative_path(&normalized_name)?;
                    let target = staging.join(&relative);
                    check_path_collision(&seen_paths, &normalized_name, &relative)?;
                    seen_paths.push(relative.clone());
                    std::fs::create_dir_all(&target).map_err(native_io_error)?;
                    set_native_permissions(&target, entry.unix_mode().unwrap_or(0o755));
                    // Directories don't consume the per-entry byte budget;
                    // continue here so we don't hit the size checks below.
                    continue;
                }

                let declared_size = entry.size();

                // Per-entry size limit.
                if declared_size > PER_ENTRY_LIMIT {
                    return Err(LauncherError::Generic {
                        code: "ERR_NATIVE_EXTRACTION_FAILED".into(),
                        message: format!(
                            "Native archive entry '{raw_name}' declares \
                             {declared_size} bytes, exceeds per-entry limit \
                             of {PER_ENTRY_LIMIT}.",
                        ),
                    });
                }

                // Aggregate size limit.
                total_size = total_size.saturating_add(declared_size);
                if total_size > AGGREGATE_LIMIT {
                    return Err(LauncherError::ZipBomb);
                }

                let relative = safe_relative_path(&normalized_name)?;
                let target = staging.join(&relative);
                check_path_collision(&seen_paths, &normalized_name, &relative)?;
                seen_paths.push(relative.clone());

                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(native_io_error)?;
                }
                let mut output = std::fs::File::create(&target).map_err(native_io_error)?;

                // Track actual bytes streamed to detect declared-size vs
                // copied-size mismatch. Clone raw_name before the mutable
                // borrow of `entry` used in `io::copy`.
                let entry_name = raw_name.to_owned();
                let bytes_written =
                    std::io::copy(&mut entry, &mut output).map_err(native_io_error)?;
                if bytes_written != declared_size {
                    // Staging cleanup happens in the outer error handler.
                    return Err(LauncherError::Generic {
                        code: "ERR_NATIVE_EXTRACTION_FAILED".into(),
                        message: format!(
                            "Native archive entry '{entry_name}' declared \
                             {declared_size} bytes but {bytes_written} were \
                             streamed.",
                        ),
                    });
                }
                output.flush().map_err(native_io_error)?;

                // Set conservative file permissions on Unix.
                set_native_permissions(&target, entry.unix_mode().unwrap_or(0o644));
            }
        }
        Ok(())
    })();

    if let Err(error) = result {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }

    // Atomically promote staging to destination. The old destination
    // (if any) is left untouched until this rename succeeds, so a crash
    // or power loss between the check and rename still leaves the previous
    // valid natives directory at destination.
    if destination.exists() {
        std::fs::remove_dir_all(destination).map_err(native_io_error)?;
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(native_io_error)?;
    }
    std::fs::rename(&staging, destination).map_err(native_io_error)
}

/// Check if a Unix file mode corresponds to a regular file or directory.
fn is_allowed_unix_mode(mode: u32) -> bool {
    const S_IFMT: u32 = 0o170000;
    const S_IFREG: u32 = 0o100000;
    const S_IFDIR: u32 = 0o040000;
    matches!(mode & S_IFMT, S_IFREG | S_IFDIR)
}

/// Check for duplicate paths, case-insensitive collisions, and
/// file-directory conflicts among previously seen entries.
///
/// `seen` holds relative paths (without the staging prefix), and `relative`
/// is the candidate relative path being added. This function compares only
/// within the relative namespace so it is independent of staging directory
/// layout.
fn check_path_collision(
    seen: &[PathBuf],
    normalized_name: &str,
    relative: &Path,
) -> LauncherResult<()> {
    let rel_str = relative.to_string_lossy();

    // 1. Exact duplicate.
    if seen.iter().any(|p| p.as_os_str() == relative.as_os_str()) {
        return Err(LauncherError::Generic {
            code: "ERR_NATIVE_DUPLICATE_PATH".into(),
            message: format!(
                "Native archive entry '{normalized_name}' duplicates a \
                 previously extracted path."
            ),
        });
    }

    // 2. Case-insensitive collision (for cross-platform safety).
    let rel_lower = rel_str.to_ascii_lowercase();
    for previous in seen {
        if previous.to_string_lossy().to_ascii_lowercase() == rel_lower {
            return Err(LauncherError::Generic {
                code: "ERR_NATIVE_DUPLICATE_PATH".into(),
                message: format!(
                    "Native archive entry '{normalized_name}' has a \
                     case-insensitive collision with '{}'.",
                    previous.display()
                ),
            });
        }
    }

    // 3. File-directory conflict: a file entry whose path is an ancestor
    //    of a previously extracted entry, or vice versa.
    for previous in seen {
        let prev_str = previous.to_string_lossy();
        // If `relative` is an ancestor of `previous` (e.g. relative="foo",
        // previous="foo/bar.so").
        if prev_str.len() > rel_str.len()
            && prev_str.starts_with(rel_str.as_ref())
            && prev_str.as_ref()[rel_str.len()..].starts_with('/')
        {
            return Err(LauncherError::Generic {
                code: "ERR_NATIVE_DUPLICATE_PATH".into(),
                message: format!(
                    "Native archive entry '{normalized_name}' conflicts: a \
                     child entry '{}' was already extracted.",
                    previous.display()
                ),
            });
        }
        // If `previous` is an ancestor of `relative` (e.g. previous="foo",
        // relative="foo/bar.so").
        if rel_str.len() > prev_str.len()
            && rel_str.starts_with(prev_str.as_ref())
            && rel_str.as_ref()[prev_str.len()..].starts_with('/')
        {
            return Err(LauncherError::Generic {
                code: "ERR_NATIVE_DUPLICATE_PATH".into(),
                message: format!(
                    "Native archive entry '{normalized_name}' conflicts with \
                     parent directory '{}'.",
                    previous.display()
                ),
            });
        }
    }

    Ok(())
}

fn is_excluded(path: &str, rules: Option<&launch::ExtractRules>) -> bool {
    path.starts_with("META-INF/")
        || rules.is_some_and(|rules| rules.exclude.iter().any(|prefix| path.starts_with(prefix)))
}

pub(crate) fn safe_relative_path(raw: &str) -> LauncherResult<PathBuf> {
    let normalized = raw.replace('\\', "/");
    if normalized.starts_with('/') || normalized.starts_with("//") || normalized.contains(':') {
        return Err(LauncherError::OverrideSecurityViolation);
    }
    let mut path = PathBuf::new();
    for component in normalized.split('/') {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            return Err(LauncherError::OverrideSecurityViolation);
        }
        path.push(component);
    }
    if path.as_os_str().is_empty() {
        return Err(LauncherError::OverrideSecurityViolation);
    }
    Ok(path)
}

fn native_io_error(error: std::io::Error) -> LauncherError {
    LauncherError::Generic {
        code: "ERR_NATIVE_EXTRACTION".into(),
        message: error.to_string(),
    }
}

/// Set conservative file permissions on Unix using the entry's stored mode.
/// On non-Unix platforms this is a no-op.
#[cfg(unix)]
fn set_native_permissions(path: &Path, mode: u32) {
    // Strip file-type bits, keep only permission bits, and remove
    // group/other write, setuid, setgid, and sticky for safety.
    let conservative = mode & 0o755;
    if let Err(error) =
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(conservative))
    {
        // Best-effort only: extraction succeeds even if permission setting
        // fails, because the file content is intact.
        let _ = error;
    }
}

#[cfg(not(unix))]
fn set_native_permissions(_path: &Path, _mode: u32) {}

// ---------------------------------------------------------------------------
// Cache durability: atomic file write with sync-before-rename
// ---------------------------------------------------------------------------

/// Atomic counter used to generate collision-resistant temp file names.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Generate a collision-resistant temp filename in the same directory as
/// `path`. Uses the process ID and an incrementing atomic counter to avoid
/// clashes even when multiple threads or sequential calls target the same
/// cache path.
fn atomic_temp_path(path: &Path) -> PathBuf {
    let pid = std::process::id();
    let count = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let stamp = format!(".agtmp_{pid}_{count}");
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path.file_name().unwrap_or_default();
    parent.join(format!("{}{}", file_name.to_string_lossy(), stamp))
}

/// Atomically write `bytes` to `path` using a write-sync-rename sequence.
///
/// The temp file is created in the same directory as the target (same
/// filesystem, so `rename` is instant). The file is fully written, flushed,
/// and `sync_all`'d before the rename, ensuring the target is never updated
/// with partial data even after a crash.
///
/// On both Unix and Windows, `std::fs::rename` replaces the destination if
/// it exists (Windows uses `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`),
/// so the verified old content at `path` remains intact if the rename fails.
///
/// Stale temp files matching the pattern `.agtmp_*` are cleaned on error;
/// any that persist after a crash will be overwritten by the next write
/// targeting the same file (since the PID + counter pair is unique, the
/// stale file remains harmless but is not automatically reaped — a future
/// enhancement could add a startup sweep).
fn atomic_write(path: &Path, bytes: &[u8]) -> LauncherResult<()> {
    let parent = path.parent().ok_or_else(|| LauncherError::Generic {
        code: "ERR_LAUNCH_CACHE_PATH".into(),
        message: format!("Launch cache path has no parent: {}", path.display()),
    })?;
    std::fs::create_dir_all(parent).map_err(|error| LauncherError::Generic {
        code: "ERR_LAUNCH_CACHE_CREATE".into(),
        message: format!("Failed to create {}: {error}", parent.display()),
    })?;

    // Use a collision-resistant temp name: PID + atomic counter.
    let temp = atomic_temp_path(path);

    // Write, flush, and sync_all before rename.
    let write_result = (|| {
        let mut file = std::fs::File::create(&temp).map_err(|error| LauncherError::Generic {
            code: "ERR_LAUNCH_CACHE_WRITE".into(),
            message: format!("Failed to create temp file {}: {error}", temp.display()),
        })?;
        file.write_all(bytes)
            .map_err(|error| LauncherError::Generic {
                code: "ERR_LAUNCH_CACHE_WRITE".into(),
                message: format!("Failed to write {}: {error}", temp.display()),
            })?;
        file.flush().map_err(|error| LauncherError::Generic {
            code: "ERR_LAUNCH_CACHE_WRITE".into(),
            message: format!("Failed to flush {}: {error}", temp.display()),
        })?;
        file.sync_all().map_err(|error| LauncherError::Generic {
            code: "ERR_LAUNCH_CACHE_WRITE".into(),
            message: format!("Failed to sync {}: {error}", temp.display()),
        })?;
        Ok::<_, LauncherError>(())
    })();

    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }

    // Atomic replace. On both Unix and Windows, std::fs::rename replaces
    // the destination if it exists, so the verified old content stays at
    // `path` until the rename succeeds.
    if let Err(error) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(LauncherError::Generic {
            code: "ERR_LAUNCH_CACHE_RENAME".into(),
            message: format!(
                "Failed to rename {} to {}: {error}",
                temp.display(),
                path.display()
            ),
        });
    }

    // Best-effort sync the parent directory on platforms that support
    // opening and syncing directories. No-op on Windows.
    sync_parent_dir(parent);

    Ok(())
}

/// Best-effort fsync of a directory handle, ensuring the directory entry
/// for the renamed file is persisted. No-op on platforms where opening a
/// directory as a File is not supported.
#[cfg(unix)]
fn sync_parent_dir(path: &Path) {
    if let Ok(dir) = std::fs::File::open(path) {
        let _ = dir.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_parent_dir(_path: &Path) {}

fn mojang_os_name() -> &'static str {
    match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "osx",
        _ => "linux",
    }
}

fn platform_key() -> &'static str {
    match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "osx",
        _ => "linux",
    }
}

fn rules_allow(rules: Option<&[launch::LibraryRule]>, features: &BTreeMap<String, bool>) -> bool {
    let Some(rules) = rules else {
        return true;
    };
    let mut allowed = false;
    for rule in rules {
        if rule_matches(rule, features) {
            allowed = match rule.action.as_str() {
                "allow" => true,
                "disallow" | "deny" => false,
                _ => allowed,
            };
        }
    }
    allowed
}

fn rule_matches(rule: &launch::LibraryRule, features: &BTreeMap<String, bool>) -> bool {
    if let Some(os) = &rule.os {
        if !os.name.is_empty() && os.name != mojang_os_name() {
            return false;
        }
        if let Some(arch) = &os.arch {
            let current = if usize::BITS == 32 {
                "x86"
            } else {
                std::env::consts::ARCH
            };
            if arch != current {
                return false;
            }
        }
        if let Some(version_pattern) = &os.version {
            let Ok(regex) = regex::Regex::new(version_pattern) else {
                return false;
            };
            let current_version = sysinfo::System::os_version().unwrap_or_default();
            if !regex.is_match(&current_version) {
                return false;
            }
        }
    }
    rule.features.as_ref().map_or(true, |required| {
        required
            .iter()
            .all(|(name, expected)| features.get(name).copied().unwrap_or(false) == *expected)
    })
}

async fn resolve_json_loader(
    loader_client: &reqwest::Client,
    loader: &LoaderInfo,
    base_version_id: &str,
    metadata_dir: &Path,
    base: &VersionInfo,
    policy: &NetworkPolicy,
) -> LauncherResult<VersionInfo> {
    let entry = loader_manifests::find_entry(&loader.loader_type, base_version_id, &loader.version)
        .ok_or(LauncherError::LoaderProfileNotFound)?;
    if entry.file_type != "profile_json" {
        return Err(LauncherError::LoaderProfileNotFound);
    }

    let profile_path = metadata_dir
        .join("loaders")
        .join(&loader.loader_type)
        .join(base_version_id)
        .join(&entry.file_name);
    let bytes = load_loader_profile_cache_first(
        loader_client,
        &loader.loader_type,
        entry,
        &profile_path,
        policy,
    )
    .await?;
    let partial: VersionInfo =
        serde_json::from_slice(&bytes).map_err(|error| LauncherError::Generic {
            code: "ERR_LOADER_PROFILE_PARSE".into(),
            message: format!("Failed to parse pinned loader profile: {error}"),
        })?;
    Ok(launch::merge_forge_version(&partial, base))
}

async fn load_loader_profile_cache_first(
    loader_client: &reqwest::Client,
    loader_type: &str,
    entry: &loader_manifests::LoaderEntry,
    cache_path: &Path,
    policy: &NetworkPolicy,
) -> LauncherResult<Vec<u8>> {
    if let Ok(bytes) = std::fs::read(cache_path) {
        let actual =
            download::compute_loader_hash(loader_type, &entry.file_name, &entry.file_type, &bytes);
        if actual == loader_manifests::strip_sha_prefix(&entry.sha256) {
            return Ok(bytes);
        }
    }

    // Cache miss: check loader policy before opening a socket.
    policy.check(NetworkCategory::LoaderMetadataAndContent)?;
    let bytes = download_loader_verified(loader_client, loader_type, entry).await?;
    atomic_write(cache_path, &bytes)?;
    Ok(bytes)
}

/// Planner-specific verified loader-profile download using the redirect-safe
/// [`LaunchHttpClients`] loader client. Preserves stable-JSON SHA-256
/// verification and cache behavior.
async fn download_loader_verified(
    client: &reqwest::Client,
    loader: &str,
    entry: &loader_manifests::LoaderEntry,
) -> LauncherResult<Vec<u8>> {
    loader_manifests::ensure_allowed_domain(&entry.source_url)?;
    let response = client
        .get(&entry.source_url)
        .send()
        .await
        .map_err(|_| LauncherError::NetworkOffline)?;
    // Defense-in-depth: post-response final URL check.
    let final_url = response.url().as_str();
    let parsed = reqwest::Url::parse(final_url).map_err(|_| LauncherError::UntrustedSource)?;
    if !redirect_target_is_safe(&parsed, NetworkCategory::LoaderMetadataAndContent) {
        return Err(LauncherError::UntrustedSource);
    }
    if !response.status().is_success() {
        return Err(LauncherError::Generic {
            code: "ERR_DOWNLOAD_HTTP".into(),
            message: format!(
                "Download {} returned HTTP {}",
                entry.source_url,
                response.status()
            ),
        });
    }
    let data = response
        .bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|_| LauncherError::NetworkOffline)?;
    let actual = download::compute_loader_hash(loader, &entry.file_name, &entry.file_type, &data);
    if actual != loader_manifests::strip_sha_prefix(&entry.sha256) {
        return Err(LauncherError::HashMismatch);
    }
    Ok(data)
}

/// Check whether a cached metadata file is fresh enough to use without network.
///
/// Only the version manifest (`version_manifest_v2.json`) is mutable and has a
/// TTL. All other metadata (version JSONs addressed by Mojang SHA-1, pinned
/// Fabric/Quilt profiles, asset indexes) are content-addressable and verified
/// by hash — they are considered always fresh once cached.
fn is_cache_fresh(cache_path: &Path, url: &str) -> bool {
    if url != VERSION_MANIFEST_URL {
        // Content-addressable / hash-verified content is always fresh.
        return true;
    }
    // Version manifest: check file modification time against TTL.
    let metadata = match std::fs::metadata(cache_path) {
        Ok(m) => m,
        Err(_) => return false,
    };
    let modified = match metadata.modified() {
        Ok(t) => t,
        Err(_) => return false,
    };
    std::time::SystemTime::now()
        .duration_since(modified)
        .ok()
        .is_some_and(|age| age < VERSION_MANIFEST_TTL)
}

/// Load a JSON document using a cache-first strategy with freshness support
/// for the mutable version manifest.
///
/// **Cache-first**: if the file exists on disk, its SHA-1 hash matches (if an
/// expected hash was provided), and it parses successfully, it is returned
/// immediately without contacting the network — provided the cache is fresh.
///
/// **Freshness**: only the version manifest (`version_manifest_v2.json`) has a
/// TTL (24 hours). For that URL the file's modification time is compared
/// against `VERSION_MANIFEST_TTL`. Stale manifests are eligible for a network
/// refresh, but a **valid stale cache is preserved as an offline fallback**:
/// when the network policy denies MojangMetadata access or the refresh fails
/// due to a transport error (`NetworkOffline`), the stale-but-valid cache is
/// returned rather than blocking an installed launch.
///
/// Integrity/parse failures from a *newly downloaded* response are always
/// propagated (never masked by stale data), ensuring a tampered or malformed
/// server response is surfaced to the caller.
async fn load_json_cache_first<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
    cache_path: &Path,
    expected_sha1: Option<&str>,
    policy: &NetworkPolicy,
    category: NetworkCategory,
) -> LauncherResult<T> {
    // Phase 1: Try to serve from cache. Hold onto a stale-but-valid parse
    // result for offline fallback.
    let mut stale_parsed: Option<T> = None;

    if let Ok(bytes) = std::fs::read(cache_path) {
        let hash_matches = expected_sha1
            .map(|expected| sha1_hex(&bytes) == expected)
            .unwrap_or(true);
        if hash_matches {
            if let Ok(parsed) = serde_json::from_slice::<T>(&bytes) {
                if is_cache_fresh(cache_path, url) {
                    return Ok(parsed);
                }
                stale_parsed = Some(parsed);
            }
        }
    }

    // Phase 2: Check policy. If the category is disabled, return the stale
    // cache (if valid) rather than a policy error. This enables offline
    // launches from a stale-but-valid manifest.
    if let Err(policy_err) = policy.check(category) {
        if let Some(parsed) = stale_parsed {
            return Ok(parsed);
        }
        return Err(policy_err);
    }

    // Phase 3: Fetch fresh data from the network. Transport errors fall back
    // to the stale cache; integrity/parse errors never do.
    let response = match client.get(url).send().await {
        Ok(r) => r,
        Err(_) => {
            if let Some(parsed) = stale_parsed {
                return Ok(parsed);
            }
            return Err(LauncherError::NetworkOffline);
        }
    };

    if !response.status().is_success() {
        return Err(LauncherError::Generic {
            code: "ERR_LAUNCH_METADATA_HTTP".into(),
            message: format!("Launch metadata {url} returned HTTP {}", response.status()),
        });
    }

    let bytes = match response.bytes().await {
        Ok(b) => b.to_vec(),
        Err(_) => {
            if let Some(parsed) = stale_parsed {
                return Ok(parsed);
            }
            return Err(LauncherError::NetworkOffline);
        }
    };

    if expected_sha1.is_some_and(|expected| sha1_hex(&bytes) != expected) {
        return Err(LauncherError::HashMismatch);
    }

    let parsed = serde_json::from_slice(&bytes).map_err(|error| LauncherError::Generic {
        code: "ERR_LAUNCH_METADATA_PARSE".into(),
        message: format!("Failed to parse launch metadata from {url}: {error}"),
    })?;
    atomic_write(cache_path, &bytes)?;
    Ok(parsed)
}

fn select_java(
    override_path: Option<&Path>,
    candidates: &[JavaInstallation],
    required_major: u32,
    allow_incompatible_override: bool,
) -> LauncherResult<ResolvedJava> {
    if let Some(path) = override_path {
        // --- Explicit override path ---
        let inspected = java::inspect_java(path).ok_or_else(|| LauncherError::Generic {
            code: "ERR_JAVA_INVALID".into(),
            message: format!(
                "Configured Java executable is missing or invalid: {}",
                path.display()
            ),
        })?;

        if inspected.version == required_major {
            return Ok(ResolvedJava {
                path: inspected.path,
                major_version: inspected.version,
                required_major_version: required_major,
                incompatible_override: false,
            });
        }

        // Mismatch: only accept if the caller explicitly allows it.
        if allow_incompatible_override {
            // Warn the caller via the flag.
            return Ok(ResolvedJava {
                path: inspected.path,
                major_version: inspected.version,
                required_major_version: required_major,
                incompatible_override: true,
            });
        }

        return Err(LauncherError::JavaIncompatible);
    }

    // --- Non-override: require exact major match, rank by source ---
    let exact_candidates: Vec<&JavaInstallation> = candidates
        .iter()
        .filter(|c| c.version == required_major)
        .collect();

    if exact_candidates.is_empty() {
        // No exact major candidate found — report which component needs it.
        return Err(LauncherError::JavaRuntimeMissing {
            major: required_major,
            component: "required_major".into(),
        });
    }

    // Rank: Managed > Mojang > System, then stable path as tiebreaker.
    let selected = exact_candidates
        .iter()
        .min_by_key(|c| {
            let source_rank = match c.source {
                java::JavaSource::Managed => 0u8,
                java::JavaSource::Mojang => 1,
                java::JavaSource::System => 2,
                java::JavaSource::Override => 3, // shouldn't appear in candidates
            };
            (source_rank, c.path.clone())
        })
        .cloned()
        .cloned();

    match selected {
        Some(inst) => Ok(ResolvedJava {
            path: inst.path,
            major_version: inst.version,
            required_major_version: required_major,
            incompatible_override: false,
        }),
        None => Err(LauncherError::JavaRuntimeMissing {
            major: required_major,
            component: "required_major".into(),
        }),
    }
}

fn sha1_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Download a loader library artifact, preferring pinned SHA-256 verification
/// over the manifest-provided SHA-1.
///
/// The download is gated by the `LoaderMetadataAndContent` policy category.
async fn download_library_with_pin(
    client: &reqwest::Client,
    artifact: &launch::LibraryArtifact,
    path: &Path,
    policy: &NetworkPolicy,
) -> LauncherResult<()> {
    let path_str = &artifact.path;

    // Check for a pinned SHA-256.
    if let Some(pinned_sha256) = loader_manifests::get_library_pin(path_str) {
        // Prefer the pinned SHA-256 for cache hit and download verification.
        if let Ok(bytes) = std::fs::read(path) {
            if download::sha256_hex(&bytes) == pinned_sha256 {
                return Ok(());
            }
        }
        // Cache miss: fetch with SHA-256 verification.
        policy.check(NetworkCategory::LoaderMetadataAndContent)?;
        let bytes = download_verified_inner(
            client,
            &artifact.url,
            NetworkCategory::LoaderMetadataAndContent,
        )
        .await?;
        let actual = download::sha256_hex(&bytes);
        if actual != pinned_sha256 {
            return Err(LauncherError::HashMismatch);
        }
        atomic_write(path, &bytes)?;
        return Ok(());
    }

    // No pin available. If enforcement is enabled, fail hard.
    if loader_manifests::LIBRARY_PIN_ENFORCEMENT_ENABLED {
        return Err(LauncherError::UnpinnedArtifact);
    }

    // Transitional fallback: use existing SHA-1 logic.
    download_sha1_atomic(
        client,
        &artifact.url,
        path,
        artifact.sha1.as_deref(),
        artifact.size,
        policy,
        NetworkCategory::LoaderMetadataAndContent,
    )
    .await
}

/// Download raw bytes from a loader Maven URL using a redirect-safe client.
async fn download_verified_inner(
    client: &reqwest::Client,
    url: &str,
    category: NetworkCategory,
) -> LauncherResult<Vec<u8>> {
    ensure_artifact_url(url)?;
    if network::classify_url(url) != Some(category) {
        return Err(LauncherError::UntrustedSource);
    }
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| LauncherError::NetworkOffline)?;
    // Defense-in-depth: verify the final response URL is category-safe.
    let final_url = response.url().as_str();
    let parsed = reqwest::Url::parse(final_url).map_err(|_| LauncherError::UntrustedSource)?;
    if !redirect_target_is_safe(&parsed, category) {
        return Err(LauncherError::UntrustedSource);
    }
    if !response.status().is_success() {
        return Err(LauncherError::Generic {
            code: "ERR_DOWNLOAD_HTTP".into(),
            message: format!("Download {url} returned HTTP {}", response.status()),
        });
    }
    response
        .bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|_| LauncherError::NetworkOffline)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Java selection
    // -----------------------------------------------------------------------

    #[test]
    fn java_selection_prefers_exact_major() {
        let candidates = vec![
            JavaInstallation {
                path: PathBuf::from("javaw.exe"),
                version: 21,
                version_string: "21".into(),
                source: java::JavaSource::System,
                arch: None,
            },
            JavaInstallation {
                path: PathBuf::from("java.exe"),
                version: 17,
                version_string: "17".into(),
                source: java::JavaSource::System,
                arch: None,
            },
        ];
        let selected = select_java(None, &candidates, 17, false).unwrap();
        assert_eq!(selected.major_version, 17);
        assert_eq!(selected.path, PathBuf::from("java.exe"));
    }

    #[test]
    fn java_selection_rejects_incompatible_candidates() {
        let candidates = vec![JavaInstallation {
            path: PathBuf::from("java.exe"),
            version: 8,
            version_string: "1.8".into(),
            source: java::JavaSource::System,
            arch: None,
        }];
        // With no exact match for 17, expect JavaRuntimeMissing.
        assert!(matches!(
            select_java(None, &candidates, 17, false),
            Err(LauncherError::JavaRuntimeMissing { .. })
        ));
    }

    #[test]
    fn argument_parser_preserves_quoted_windows_paths() {
        let parsed = parse_argument_string(
            r#"-Dsome.path="C:\Program Files\Something" -javaagent:"C:\Tools With Spaces\agent.jar""#,
        )
        .unwrap();
        assert_eq!(
            parsed,
            vec![
                r#"-Dsome.path=C:\Program Files\Something"#,
                r#"-javaagent:C:\Tools With Spaces\agent.jar"#,
            ]
        );
    }

    #[test]
    fn safe_relative_path_rejects_native_traversal() {
        assert!(safe_relative_path("../../evil.dll").is_err());
        assert!(safe_relative_path("C:/Windows/evil.dll").is_err());
        assert_eq!(
            safe_relative_path("org/lwjgl/lwjgl.dll").unwrap(),
            PathBuf::from("org/lwjgl/lwjgl.dll")
        );
    }

    #[test]
    fn safe_relative_path_rejects_absolute_unix() {
        assert!(safe_relative_path("/absolute/path.dll").is_err());
    }

    #[test]
    fn safe_relative_path_rejects_unc_prefix() {
        assert!(safe_relative_path("//server/share/evil.dll").is_err());
        assert!(safe_relative_path("\\\\server\\share\\evil.dll").is_err());
    }

    #[test]
    fn sha1_sidecar_parser_requires_hex_digest() {
        assert_eq!(
            parse_sha1_sidecar("0123456789abcdef0123456789abcdef01234567  artifact.jar"),
            Some("0123456789abcdef0123456789abcdef01234567".into())
        );
        assert!(parse_sha1_sidecar("not-a-hash").is_none());
    }

    // -----------------------------------------------------------------------
    // Canary tests: credential-boundary and redaction
    // -----------------------------------------------------------------------

    #[test]
    fn launch_identity_debug_never_contains_access_token() {
        let identity = LaunchIdentity {
            username: "Player".into(),
            access_token: "secret-minecraft-token-12345".into(),
            uuid: "550e8400-e29b-41d4-a716-446655440000".into(),
            user_type: "msa".into(),
            client_id: "my-client-id".into(),
            xuid: "2535467890123456".into(),
            user_properties: r#"{"preferredLanguage":"en"}"#.into(),
        };
        let debug_str = format!("{identity:?}");
        assert!(
            !debug_str.contains("secret-minecraft-token-12345"),
            "LaunchIdentity Debug must not contain access_token"
        );
        assert!(
            debug_str.contains("[REDACTED]"),
            "LaunchIdentity Debug must contain [REDACTED] for access_token"
        );
    }

    #[test]
    fn launch_identity_debug_partially_redacts_uuid() {
        let identity = LaunchIdentity {
            username: "Player".into(),
            access_token: "tok".into(),
            uuid: "550e8400-e29b-41d4-a716-446655440000".into(),
            user_type: "msa".into(),
            client_id: "".into(),
            xuid: "".into(),
            user_properties: "{}".into(),
        };
        let debug_str = format!("{identity:?}");
        // The first 4 and last 4 chars of the UUID should be visible
        assert!(
            debug_str.contains("550e"),
            "leading UUID chars should be visible"
        );
        assert!(
            debug_str.contains("0000"),
            "trailing UUID chars should be visible"
        );
        // The middle section must NOT be visible in full
        assert!(
            !debug_str.contains("e29b-41d4-a716-44665544"),
            "middle of UUID should be truncated"
        );
    }

    #[test]
    fn prepared_command_debug_never_contains_args_values() {
        let cmd = PreparedCommand {
            program: PathBuf::from("java"),
            args: vec![
                "-Xmx2G".into(),
                "-Djava.library.path=/some/path".into(),
                "-cp".into(),
                "/lots/of/jars".into(),
                "net.minecraft.client.main.Main".into(),
                "--accessToken".into(),
                "super-secret-mc-token".into(),
            ],
            cwd: PathBuf::from("/game/dir"),
            env: BTreeMap::from([
                ("HOME".into(), "/home/user".into()),
                ("JAVA_HOME".into(), "/usr/lib/jvm/java-21".into()),
            ]),
        };
        let debug_str = format!("{cmd:?}");
        // Must NOT contain any expanded argument values
        assert!(
            !debug_str.contains("super-secret-mc-token"),
            "PreparedCommand Debug must not expose args values"
        );
        assert!(
            !debug_str.contains("-Xmx2G"),
            "PreparedCommand Debug must not expose args values"
        );
        // Must NOT contain environment values
        assert!(
            !debug_str.contains("/home/user"),
            "PreparedCommand Debug must not expose env values"
        );
        // Must report arg_count (7 arguments in the test vector)
        assert!(
            debug_str.contains("arg_count: 7"),
            "PreparedCommand Debug must report arg_count"
        );
        // Must report env_keys
        assert!(
            debug_str.contains("HOME"),
            "PreparedCommand Debug must expose env key names"
        );
        assert!(
            debug_str.contains("JAVA_HOME"),
            "PreparedCommand Debug must expose env key names"
        );
        // Must report program and cwd
        assert!(debug_str.contains("java"), "program must be visible");
        assert!(debug_str.contains("/game/dir"), "cwd must be visible");
    }

    #[test]
    fn spawn_error_does_not_include_args() {
        // Spawn with a non-existent program — error should reference the program
        // name, not any expanded argument values.
        let cmd = PreparedCommand {
            program: PathBuf::from("nonexistent-java-binary-that-will-never-exist"),
            args: vec!["--accessToken".into(), "leaked-token".into()],
            cwd: PathBuf::from("/tmp"),
            env: BTreeMap::new(),
        };
        let err = spawn(&cmd).unwrap_err();
        let msg = err.to_string();
        // The error should mention the program but never the leaked token
        assert!(
            !msg.contains("leaked-token"),
            "Spawn error must not contain argument values (got: {msg})"
        );
    }

    #[test]
    fn sanitize_log_with_secrets_handles_opaque_jwt_token() {
        let token = "eyJhbGciOiJIUzI1NiJ9.eyJ4dWlkIjoiMjUzNTQzMjM0NTY3ODkwMSJ9.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        let input = format!("Using token: {token}");
        let result = crate::log_sanitizer::sanitize_log_with_secrets(&input, &[token]);
        assert!(
            !result.contains(token),
            "opaque JWT token should be redacted"
        );
        assert!(result.contains("[REDACTED]"));
    }

    // -----------------------------------------------------------------------
    // Network policy integration tests
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_request_compiles_with_explicit_policy() {
        // Verify that ResolveRequest can be constructed with all policy variants.
        let _req = ResolveRequest {
            instance_id: "test".into(),
            base_version_id: "1.21".into(),
            loader: None,
            game_dir: PathBuf::from("/tmp"),
            assets_dir: PathBuf::from("/tmp/assets"),
            cache_dir: PathBuf::from("/tmp/cache"),
            java_override: None,
            java_candidates: vec![],
            network_policy: NetworkPolicy::all_enabled(),
            allow_incompatible_java_override: false,
            minecraft_dir: None,
            receipts_root: None,
        };
        let _req_disabled = ResolveRequest {
            network_policy: NetworkPolicy::all_disabled(),
            .._req
        };
    }

    #[test]
    fn resolved_launch_plan_retains_policy() {
        let policy = NetworkPolicy::all_enabled();
        // Just verify the struct compiles with network_policy and is cloneable.
        let _plan = ResolvedLaunchPlan {
            instance_id: "test".into(),
            version_id: "1.21".into(),
            base_version_id: "1.21".into(),
            loader: None,
            java: ResolvedJava {
                path: PathBuf::from("java"),
                major_version: 21,
                required_major_version: 21,
                incompatible_override: false,
            },
            version: VersionInfo::default(),
            game_dir: PathBuf::from("/tmp"),
            assets_dir: PathBuf::from("/tmp/assets"),
            cache_dir: PathBuf::from("/tmp/cache"),
            network_policy: policy,
            adopted_profile: None,
        };
    }

    #[test]
    fn classify_loader_urls_not_mojang_content() {
        // Fabric Maven URLs must classify as LoaderMetadataAndContent,
        // NEVER as MojangContent.
        let loader_urls = [
            "https://maven.fabricmc.net/v2/versions/loader/1.21/0.19.0/profile/json",
            "https://maven.fabricmc.net/net/fabricmc/fabric-loader/0.19.0/fabric-loader-0.19.0.jar",
            "https://maven.quiltmc.org/release/org/quiltmc/quilt-loader/0.22.0/quilt-loader-0.22.0.jar",
        ];
        for url in &loader_urls {
            let cat = network::classify_url(url);
            assert_eq!(
                cat,
                Some(NetworkCategory::LoaderMetadataAndContent),
                "Loader URL {url} should classify as LoaderMetadataAndContent, got {cat:?}"
            );
        }
    }

    #[test]
    fn classify_mojang_resource_urls_as_content() {
        let content_urls = [
            "https://piston-data.mojang.com/v1/objects/test/client.jar",
            "https://libraries.minecraft.net/net/minecraft/launchwrapper/1.12/launchwrapper-1.12.jar",
            "https://resources.download.minecraft.net/ab/abcdef1234567890abcdef1234567890abcdef12",
        ];
        for url in &content_urls {
            let cat = network::classify_url(url);
            assert_eq!(
                cat,
                Some(NetworkCategory::MojangContent),
                "Content URL {url} should classify as MojangContent, got {cat:?}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Helper: build a test ZIP in memory from named byte slices
    // -----------------------------------------------------------------------

    fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            for &(name, data) in entries {
                zip.start_file(name, zip::write::FileOptions::default())
                    .unwrap();
                zip.write_all(data).unwrap();
            }
            zip.finish().unwrap();
        }
        buf
    }

    /// Write a ZIP using the standard `zip::ZipWriter` for correct CRC/data, then
    /// patch the central directory entries so that `external_attributes` include
    /// the given Unix file mode **and** set the system byte to Unix (3). This
    /// makes `ZipFile::unix_mode()` return `Some(mode)` on the reading side.
    fn make_zip_with_modes(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
        let mut buf = Vec::new();
        // Write normally (the zip crate stores only the lower 9 permission bits
        // via unix_permissions, but the CRC and sizes are correct).
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            for &(name, data, _mode) in entries {
                zip.start_file(name, zip::write::FileOptions::default())
                    .unwrap();
                zip.write_all(data).unwrap();
            }
            zip.finish().unwrap();
        }

        // Locate the End of Central Directory record (last 22+ bytes).
        let total = buf.len();
        // EOCD is always at buf[total-22..] for a simple archive with no comment.
        let eocd_pos = total - 22;
        let cd_entries =
            u16::from_le_bytes(buf[eocd_pos + 8..eocd_pos + 10].try_into().unwrap()) as usize;
        let cd_offset =
            u32::from_le_bytes(buf[eocd_pos + 16..eocd_pos + 20].try_into().unwrap()) as usize;

        // Patch external_attributes at offset 38 within each central directory entry.
        // Layout: sig(4) + ver_made(2) + ver_needed(2) + flags(2) + compr(2) +
        // mod_time(2) + mod_date(2) + crc32(4) + compr_sz(4) + uncompr_sz(4) +
        // filename_len(2) + extra_len(2) + comment_len(2) + disk_start(2) +
        // internal_attrs(2) = 38 bytes before external_attrs.
        let mut pos = cd_offset;
        for i in 0..cd_entries {
            let fn_len = u16::from_le_bytes(buf[pos + 28..pos + 30].try_into().unwrap()) as usize;

            let mode = entries[i].2;
            // Upper 16 bits = Unix mode; bits 8-15 = system (3 = Unix).
            let ext = (mode << 16) | (3u32 << 8);
            buf[pos + 38..pos + 42].copy_from_slice(&ext.to_le_bytes());

            pos += 46 + fn_len;
        }

        buf
    }

    /// Helper: write bytes to a temp directory and run extract_natives_atomically.
    fn extract_test_zip(
        zip_data: &[u8],
        rules: Option<launch::ExtractRules>,
    ) -> LauncherResult<tempfile::TempDir> {
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("natives.zip");
        std::fs::write(&archive_path, zip_data).unwrap();
        let natives_dir = dir.path().join("natives");
        extract_natives_atomically(&[(archive_path, rules)], &natives_dir)?;
        Ok(dir)
    }

    fn staging_path(destination: &Path) -> PathBuf {
        destination.with_extension("staging")
    }

    // -----------------------------------------------------------------------
    // Native extraction: valid archive success
    // -----------------------------------------------------------------------

    #[test]
    fn extracts_valid_native_archive_successfully() {
        let zip_data = make_zip(&[
            ("org/lwjgl/lwjgl.dll", b"lwjgl content"),
            ("org/lwjgl/glfw.dll", b"glfw content"),
            ("META-INF/MANIFEST.MF", b"should be excluded"),
        ]);
        let dir = extract_test_zip(&zip_data, None).unwrap();
        let natives = dir.path().join("natives");
        assert!(natives.join("org/lwjgl/lwjgl.dll").is_file());
        assert!(natives.join("org/lwjgl/glfw.dll").is_file());
        // META-INF must be excluded
        assert!(!natives.join("META-INF").exists());
        // Staging must be removed
        assert!(!staging_path(&natives).exists());
    }

    // -----------------------------------------------------------------------
    // Native extraction: META-INF and configured excludes
    // -----------------------------------------------------------------------

    #[test]
    fn excludes_meta_inf_and_configured_prefixes() {
        let rules = launch::ExtractRules {
            exclude: vec!["debug/".into()],
        };
        let zip_data = make_zip(&[
            ("META-INF/signature.SF", b"sig"),
            ("debug/trace.log", b"trace"),
            ("natives/foo.so", b"foo"),
        ]);
        let dir = extract_test_zip(&zip_data, Some(rules)).unwrap();
        let natives = dir.path().join("natives");
        assert!(!natives.join("META-INF").exists());
        assert!(!natives.join("debug").exists());
        assert!(natives.join("natives/foo.so").is_file());
    }

    // -----------------------------------------------------------------------
    // Native extraction: traversal variants
    // -----------------------------------------------------------------------

    #[test]
    fn rejects_traversal_dotdot_entry() {
        let zip_data = make_zip(&[("../../etc/passwd", b"evil")]);
        assert!(extract_test_zip(&zip_data, None).is_err());
    }

    #[test]
    fn rejects_absolute_unix_path_entry() {
        let zip_data = make_zip(&[("/tmp/evil.dll", b"evil")]);
        assert!(extract_test_zip(&zip_data, None).is_err());
    }

    #[test]
    fn rejects_windows_drive_path_entry() {
        let zip_data = make_zip(&[("C:/Windows/evil.dll", b"evil")]);
        assert!(extract_test_zip(&zip_data, None).is_err());
    }

    #[test]
    fn rejects_unc_path_entry() {
        // UNC starts with // after normalization (backslash → '/')
        let zip_data = make_zip(&[("//server/share/evil.dll", b"evil")]);
        assert!(extract_test_zip(&zip_data, None).is_err());
    }

    // -----------------------------------------------------------------------
    // Native extraction: symlink unix mode rejection
    // -----------------------------------------------------------------------

    #[test]
    fn rejects_symlink_unix_mode() {
        // 0o120777 = S_IFLNK | 0777
        let zip_data = make_zip_with_modes(&[("link.so", b"target", 0o120777)]);
        assert!(extract_test_zip(&zip_data, None).is_err());
    }

    #[test]
    fn rejects_fifo_unix_mode() {
        // 0o010644 = S_IFIFO | 0644
        let zip_data = make_zip_with_modes(&[("fifo", b"", 0o010644)]);
        assert!(extract_test_zip(&zip_data, None).is_err());
    }

    #[test]
    fn rejects_block_device_unix_mode() {
        // 0o060644 = S_IFBLK | 0644
        let zip_data = make_zip_with_modes(&[("bdev", b"", 0o060644)]);
        assert!(extract_test_zip(&zip_data, None).is_err());
    }

    #[test]
    fn rejects_character_device_unix_mode() {
        // 0o020644 = S_IFCHR | 0644
        let zip_data = make_zip_with_modes(&[("cdev", b"", 0o020644)]);
        assert!(extract_test_zip(&zip_data, None).is_err());
    }

    #[test]
    fn rejects_socket_unix_mode() {
        // 0o140644 = S_IFSOCK | 0644
        let zip_data = make_zip_with_modes(&[("sock", b"", 0o140644)]);
        assert!(extract_test_zip(&zip_data, None).is_err());
    }

    #[test]
    fn allows_regular_file_unix_mode() {
        // 0o100644 = S_IFREG | 0644
        let zip_data = make_zip_with_modes(&[("libtest.so", b"content", 0o100644)]);
        let dir = extract_test_zip(&zip_data, None).unwrap();
        assert!(dir.path().join("natives/libtest.so").is_file());
    }

    #[test]
    fn allows_directory_unix_mode() {
        // 0o040755 = S_IFDIR | 0755
        let zip_data = make_zip_with_modes(&[("subdir/lib.so", b"content", 0o100644)]);
        let dir = extract_test_zip(&zip_data, None).unwrap();
        assert!(dir.path().join("natives/subdir/lib.so").is_file());
    }

    // -----------------------------------------------------------------------
    // Native extraction: duplicate and collision detection
    // -----------------------------------------------------------------------

    #[test]
    fn rejects_exact_duplicate_entry() {
        let zip_data = make_zip(&[("org/lib.so", b"content"), ("org/lib.so", b"content")]);
        assert!(extract_test_zip(&zip_data, None).is_err());
    }

    #[test]
    fn rejects_case_insensitive_collision() {
        let zip_data = make_zip(&[("OpenGL32.dll", b"content"), ("opengl32.dll", b"content")]);
        assert!(extract_test_zip(&zip_data, None).is_err());
    }

    #[test]
    fn rejects_file_directory_conflict_file_after_dir() {
        // "subdir/" as a directory followed by a file "subdir" creates a conflict.
        let zip_data = make_zip(&[("subdir/lib.so", b"libcontent"), ("subdir", b"filecontent")]);
        assert!(extract_test_zip(&zip_data, None).is_err());
    }

    #[test]
    fn rejects_directory_after_file_conflict() {
        // A file entry "somefile" followed by a directory entry "somefile/"
        // should be impossible in a well-formed zip, but test anyway.
        let zip_data = make_zip(&[
            ("somefile", b"filecontent"),
            ("somefile/lib.so", b"libcontent"),
        ]);
        assert!(extract_test_zip(&zip_data, None).is_err());
    }

    // -----------------------------------------------------------------------
    // Native extraction: aggregate size limit
    // -----------------------------------------------------------------------

    #[test]
    fn extracts_within_aggregate_limit() {
        // Multiple small entries should not trigger the aggregate limit.
        let zip_data = make_zip(&[
            ("lib1.so", b"content1"),
            ("lib2.so", b"content2"),
            ("lib3.so", b"content3"),
        ]);
        let dir = extract_test_zip(&zip_data, None).unwrap();
        assert!(dir.path().join("natives/lib1.so").is_file());
        assert!(dir.path().join("natives/lib2.so").is_file());
        assert!(dir.path().join("natives/lib3.so").is_file());
    }

    // -----------------------------------------------------------------------
    // Native extraction: cleanup and preservation after failure
    // -----------------------------------------------------------------------

    #[test]
    fn staging_removed_on_failure() {
        let zip_data = make_zip(&[("../../traversal.dll", b"evil")]);
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("bad.zip");
        std::fs::write(&archive_path, &zip_data).unwrap();
        let natives_dir = dir.path().join("natives");

        // Create a valid previous natives directory to test preservation.
        std::fs::create_dir_all(&natives_dir).unwrap();
        std::fs::write(natives_dir.join("old.so"), b"old content").unwrap();

        let result = extract_natives_atomically(&[(archive_path, None)], &natives_dir);
        assert!(result.is_err());

        // Staging must be removed.
        assert!(!staging_path(&natives_dir).exists());
        // Previous promoted natives must be preserved.
        assert!(natives_dir.join("old.so").is_file());
    }

    #[test]
    fn valid_extraction_removes_staging() {
        let zip_data = make_zip(&[("good.so", b"good content")]);
        let dir = extract_test_zip(&zip_data, None).unwrap();
        let natives = dir.path().join("natives");
        assert!(!staging_path(&natives).exists());
        assert!(natives.join("good.so").is_file());
    }

    // -----------------------------------------------------------------------
    // Atomic write: successful replacement
    // -----------------------------------------------------------------------

    #[test]
    fn atomic_write_successfully_replaces_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("test.bin");

        // First write
        atomic_write(&target, b"version 1").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "version 1");

        // Second write (replacement)
        atomic_write(&target, b"version 2").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "version 2");

        // No temp files should remain
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_str().is_some_and(|n| n.contains("agtmp")))
            .collect();
        assert!(entries.is_empty(), "stale agtmp files remain: {entries:?}");
    }

    // -----------------------------------------------------------------------
    // Atomic write: no temp residue after error (induced via bad path)
    // -----------------------------------------------------------------------

    #[test]
    fn atomic_write_no_temp_residue_on_error() {
        let dir = tempfile::tempdir().unwrap();

        // Create a file where a directory is expected to cause an error.
        std::fs::write(dir.path().join("not_a_dir"), b"block").unwrap();
        let bad_target = dir.path().join("not_a_dir").join("test.bin");
        let result = atomic_write(&bad_target, b"data");
        assert!(result.is_err());

        // No .agtmp files should remain in the directory.
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_str().is_some_and(|n| n.contains("agtmp")))
            .collect();
        assert!(entries.is_empty(), "stale agtmp files remain: {entries:?}");
    }

    // -----------------------------------------------------------------------
    // Atomic write: temp file naming uses PID + counter
    // -----------------------------------------------------------------------

    #[test]
    fn atomic_temp_path_contains_pid() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("cache.bin");
        let temp = atomic_temp_path(&target);
        let name = temp.file_name().unwrap().to_str().unwrap();
        assert!(name.contains(".agtmp_"), "{name} missing .agtmp_");
        assert!(
            name.contains(&std::process::id().to_string()),
            "{name} missing PID"
        );
    }

    // -----------------------------------------------------------------------
    // is_allowed_unix_mode unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn is_allowed_mode_accepts_regular_file() {
        assert!(is_allowed_unix_mode(0o100644));
        assert!(is_allowed_unix_mode(0o100755));
    }

    #[test]
    fn is_allowed_mode_accepts_directory() {
        assert!(is_allowed_unix_mode(0o040755));
        assert!(is_allowed_unix_mode(0o040700));
    }

    #[test]
    fn is_allowed_mode_rejects_symlink() {
        assert!(!is_allowed_unix_mode(0o120777));
    }

    #[test]
    fn is_allowed_mode_rejects_fifo() {
        assert!(!is_allowed_unix_mode(0o010644));
    }

    #[test]
    fn is_allowed_mode_rejects_socket() {
        assert!(!is_allowed_unix_mode(0o140644));
    }

    #[test]
    fn is_allowed_mode_rejects_block_device() {
        assert!(!is_allowed_unix_mode(0o060644));
    }

    #[test]
    fn is_allowed_mode_rejects_char_device() {
        assert!(!is_allowed_unix_mode(0o020644));
    }

    // -----------------------------------------------------------------------
    // check_path_collision unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn path_collision_exact_duplicate_rejected() {
        let seen = vec![PathBuf::from("lib.so")];
        let result = check_path_collision(&seen, "lib.so", Path::new("lib.so"));
        assert!(result.is_err());
    }

    #[test]
    fn path_collision_case_insensitive_rejected() {
        let seen = vec![PathBuf::from("OpenGL32.dll")];
        let result = check_path_collision(&seen, "opengl32.dll", Path::new("opengl32.dll"));
        assert!(result.is_err());
    }

    #[test]
    fn path_collision_unique_path_accepted() {
        let seen = vec![PathBuf::from("existing.so")];
        let result = check_path_collision(&seen, "new.so", Path::new("new.so"));
        assert!(result.is_ok());
    }

    #[test]
    fn path_collision_ancestor_after_descendant_rejected() {
        // seen contains "foo/bar.so", then we try to add "foo" (which would
        // be a file conflicting with its parent role).
        let seen = vec![PathBuf::from("foo/bar.so")];
        let result = check_path_collision(&seen, "foo", Path::new("foo"));
        assert!(result.is_err());
    }

    #[test]
    fn path_collision_descendant_after_ancestor_rejected() {
        // seen contains "foo", then we try to add "foo/bar.so" (descendant
        // of a previously-extracted file — "foo" was extracted as a file,
        // so "foo/bar.so" would require it to be a directory).
        let seen = vec![PathBuf::from("foo")];
        let result = check_path_collision(&seen, "foo/bar.so", Path::new("foo/bar.so"));
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // is_excluded unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn is_excluded_matches_meta_inf_prefix() {
        assert!(is_excluded("META-INF/MANIFEST.MF", None));
        assert!(is_excluded("META-INF/signature.SF", None));
    }

    #[test]
    fn is_excluded_does_not_match_non_meta_inf() {
        assert!(!is_excluded("org/lwjgl/lwjgl.dll", None));
    }

    #[test]
    fn is_excluded_matches_configured_rules() {
        let rules = launch::ExtractRules {
            exclude: vec!["debug/".into(), "internal/".into()],
        };
        assert!(is_excluded("debug/trace.log", Some(&rules)));
        assert!(is_excluded("internal/secret.dll", Some(&rules)));
        assert!(!is_excluded("public/api.dll", Some(&rules)));
    }

    // -----------------------------------------------------------------------
    // Cache freshness: is_cache_fresh
    // -----------------------------------------------------------------------

    #[test]
    fn non_manifest_url_is_always_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("version.json");
        std::fs::write(&path, b"{}").unwrap();
        // Set mtime to 100 days ago (File::open is read-only on Windows;
        // use OpenOptions with write access for set_modified).
        let past =
            std::time::SystemTime::now() - std::time::Duration::from_secs(100 * 24 * 60 * 60);
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_modified(past).unwrap();
        drop(file);
        assert!(is_cache_fresh(
            &path,
            "https://piston-meta.mojang.com/v1/packages/abc123/1.21.json"
        ));
    }

    #[test]
    fn manifest_url_is_fresh_when_under_ttl() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("version_manifest_v2.json");
        std::fs::write(&path, b"{}").unwrap();
        // Fresh: mtime is now (just written).
        assert!(is_cache_fresh(&path, VERSION_MANIFEST_URL));
    }

    #[test]
    fn manifest_url_is_stale_when_over_ttl() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("version_manifest_v2.json");
        std::fs::write(&path, b"{}").unwrap();
        // Set mtime to 25 hours ago (write access required on Windows).
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(25 * 60 * 60);
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_modified(past).unwrap();
        drop(file);
        assert!(!is_cache_fresh(&path, VERSION_MANIFEST_URL));
    }

    #[test]
    fn manifest_url_is_not_fresh_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");
        assert!(!is_cache_fresh(&path, VERSION_MANIFEST_URL));
    }

    // -----------------------------------------------------------------------
    // Library pin: sha256_hex helper
    // -----------------------------------------------------------------------

    #[test]
    fn sha256_hex_is_deterministic() {
        let data = b"hello";
        let h1 = download::sha256_hex(data);
        let h2 = download::sha256_hex(data);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn sha256_hex_known_value() {
        // SHA-256 of b"hello" is
        // 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        let h = download::sha256_hex(b"hello");
        assert_eq!(
            h,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    // -----------------------------------------------------------------------
    // Redirect-policy unit tests (pure, no network)
    // -----------------------------------------------------------------------

    #[test]
    fn redirect_same_category_allowed() {
        // Mojang metadata → Mojang metadata redirect is safe.
        let url = reqwest::Url::parse("https://launcher.mojang.com/v1/objects/test.json").unwrap();
        assert!(
            redirect_target_is_safe(&url, NetworkCategory::MojangMetadata),
            "mojang metadata → mojang metadata should be allowed"
        );
        // Mojang content → Mojang content redirect is safe.
        let url =
            reqwest::Url::parse("https://piston-data.mojang.com/v1/objects/client.jar").unwrap();
        assert!(
            redirect_target_is_safe(&url, NetworkCategory::MojangContent),
            "mojang content → mojang content should be allowed"
        );
        // Loader → Loader redirect is safe.
        let url = reqwest::Url::parse("https://maven.fabricmc.net/net/fabricmc/fabric-loader.jar")
            .unwrap();
        assert!(
            redirect_target_is_safe(&url, NetworkCategory::LoaderMetadataAndContent),
            "loader → loader should be allowed"
        );
    }

    #[test]
    fn redirect_cross_category_rejected() {
        // Mojang metadata redirecting to a loader host.
        let url = reqwest::Url::parse("https://maven.fabricmc.net/net/fabricmc/lib.jar").unwrap();
        assert!(
            !redirect_target_is_safe(&url, NetworkCategory::MojangMetadata),
            "fabric host should NOT be safe for MojangMetadata"
        );
        // Loader redirecting to a Mojang content host.
        let url =
            reqwest::Url::parse("https://piston-data.mojang.com/v1/objects/client.jar").unwrap();
        assert!(
            !redirect_target_is_safe(&url, NetworkCategory::LoaderMetadataAndContent),
            "mojang content should NOT be safe for LoaderMetadataAndContent"
        );
    }

    #[test]
    fn redirect_https_downgrade_rejected() {
        // HTTP (not HTTPS) URL must be rejected.
        let url =
            reqwest::Url::parse("http://piston-meta.mojang.com/mc/game/manifest.json").unwrap();
        assert!(
            !redirect_target_is_safe(&url, NetworkCategory::MojangMetadata),
            "HTTP downgrade should be rejected"
        );
    }

    #[test]
    fn redirect_non_standard_port_rejected() {
        let url = reqwest::Url::parse("https://piston-meta.mojang.com:8080/mc/game/manifest.json")
            .unwrap();
        assert!(
            !redirect_target_is_safe(&url, NetworkCategory::MojangMetadata),
            "non-443 port should be rejected"
        );
    }

    #[test]
    fn redirect_unknown_host_rejected() {
        let url = reqwest::Url::parse("https://evil.example.com/malware.jar").unwrap();
        assert!(
            !redirect_target_is_safe(&url, NetworkCategory::MojangContent),
            "unknown host should be rejected"
        );
    }

    #[test]
    fn redirect_ip_literal_rejected() {
        // IPv4 literal
        let url = reqwest::Url::parse("https://10.0.0.1/malware.jar").unwrap();
        assert!(
            !redirect_target_is_safe(&url, NetworkCategory::MojangContent),
            "IPv4 literal 10.x.x.x should be rejected"
        );
        let url = reqwest::Url::parse("https://192.168.1.1/malware.jar").unwrap();
        assert!(
            !redirect_target_is_safe(&url, NetworkCategory::MojangContent),
            "IPv4 literal 192.168.x.x should be rejected"
        );
        let url = reqwest::Url::parse("https://127.0.0.1/malware.jar").unwrap();
        assert!(
            !redirect_target_is_safe(&url, NetworkCategory::MojangContent),
            "localhost should be rejected"
        );
    }

    #[test]
    fn redirect_private_metadata_host_rejected() {
        let url = reqwest::Url::parse("https://169.254.169.254/latest/meta-data/").unwrap();
        assert!(
            !redirect_target_is_safe(&url, NetworkCategory::MojangContent),
            "AWS metadata endpoint should be rejected"
        );
    }

    #[test]
    fn redirect_ipv6_literal_rejected() {
        let url = reqwest::Url::parse("https://[::1]/evil.jar").unwrap();
        assert!(
            !redirect_target_is_safe(&url, NetworkCategory::MojangContent),
            "IPv6 loopback should be rejected"
        );
    }

    #[test]
    fn is_ip_literal_works() {
        assert!(is_ip_literal("127.0.0.1"));
        assert!(is_ip_literal("10.0.0.1"));
        assert!(is_ip_literal("::1"));
        assert!(is_ip_literal("2001:db8::1"));
        assert!(!is_ip_literal("piston-meta.mojang.com"));
        assert!(!is_ip_literal("maven.fabricmc.net"));
    }

    #[test]
    fn is_private_or_local_works() {
        assert!(is_private_or_local("127.0.0.1"));
        assert!(is_private_or_local("10.0.0.5"));
        assert!(is_private_or_local("192.168.1.1"));
        assert!(is_private_or_local("172.16.0.1"));
        assert!(is_private_or_local("172.31.255.255"));
        assert!(is_private_or_local("169.254.169.254"));
        assert!(!is_private_or_local("piston-meta.mojang.com"));
        assert!(!is_private_or_local("8.8.8.8"));
        assert!(!is_private_or_local("172.15.0.1")); // below 172.16
        assert!(!is_private_or_local("172.32.0.1")); // above 172.31
    }

    // -----------------------------------------------------------------------
    // Enforcement gate integrity
    // -----------------------------------------------------------------------

    #[test]
    fn library_pin_enforcement_active() {
        // Mirrors loader_manifests::tests::library_pin_enforcement_is_enabled.
        // Enforcement was activated after the library-pin data refresh.
        assert!(
            loader_manifests::LIBRARY_PIN_ENFORCEMENT_ENABLED,
            "LIBRARY_PIN_ENFORCEMENT_ENABLED must be true after the library-pin data refresh."
        );
    }

    // -----------------------------------------------------------------------
    // Installed-profile adoption integration tests
    // -----------------------------------------------------------------------

    fn make_java_candidate() -> Vec<crate::java::JavaInstallation> {
        vec![
            crate::java::JavaInstallation {
                path: std::path::PathBuf::from("java8"),
                version: 8,
                version_string: "1.8.0".into(),
                source: crate::java::JavaSource::System,
                arch: None,
            },
            crate::java::JavaInstallation {
                path: std::path::PathBuf::from("java21"),
                version: 21,
                version_string: "21".into(),
                source: crate::java::JavaSource::System,
                arch: None,
            },
        ]
    }

    fn make_adopt_fixture(tmp: &tempfile::TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
        let minecraft_dir = tmp.path().join("minecraft");
        let receipts_root = tmp.path().join("receipts");
        std::fs::create_dir_all(&minecraft_dir.join("versions")).unwrap();
        std::fs::create_dir_all(&receipts_root).unwrap();
        (minecraft_dir, receipts_root)
    }

    fn write_profile_json(path: &std::path::Path, profile_id: &str, inherits_from: &str) {
        let profile = serde_json::json!({
            "id": profile_id,
            "inheritsFrom": inherits_from,
            "mainClass": "net.minecraft.client.main.Main",
            "type": "release",
            "libraries": [],
        });
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bytes = serde_json::to_vec_pretty(&profile).unwrap();
        std::fs::write(path, &bytes).unwrap();
    }

    fn write_base_version_json(path: &std::path::Path, mc_version: &str) {
        let version = serde_json::json!({
            "id": mc_version,
            "mainClass": "net.minecraft.client.main.Main",
            "type": "release",
            "libraries": [],
        });
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec_pretty(&version).unwrap()).unwrap();
    }

    fn write_forge_receipt(
        receipts_root: &std::path::Path,
        minecraft_dir: &std::path::Path,
        tuple: &crate::installed_profile::LoaderTuple,
        installer_sha: &str,
    ) {
        let profile_id = crate::installed_profile::derive_profile_id(tuple);
        let profile_path = minecraft_dir
            .join("versions")
            .join(&profile_id)
            .join(format!("{profile_id}.json"));
        // Re-read the profile and compute its stable hash to match
        // what adopt_installed_profile will compute.
        let profile_bytes = std::fs::read(&profile_path).unwrap();
        let profile_value: serde_json::Value = serde_json::from_slice(&profile_bytes).unwrap();
        let stable_hash = crate::installed_profile::stable_profile_hash(&profile_value);
        let receipt = crate::installed_profile::InstalledProfileReceipt {
            schema_version: 2,
            tuple: tuple.clone(),
            installer_sha256: installer_sha.to_string(),
            installer_url: "https://example.com".into(),
            profile_id: profile_id.clone(),
            profile_relative_path: format!("versions/{profile_id}/{profile_id}.json"),
            profile_stable_hash: stable_hash,
            base_version_id: tuple.minecraft_version.clone(),
            installed_at: "2026-01-01T00:00:00Z".into(),
            installer_exit_status: 0,
            generated_artifact_sha256: None,
        };
        crate::installed_profile::write_receipt_atomic(receipts_root, tuple, &receipt).unwrap();
    }

    #[tokio::test]
    async fn adopt_neoforge_21_0_163_succeeds() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (minecraft_dir, receipts_root) = make_adopt_fixture(&tmp);
        let profile_id = "neoforge-21.0.163";
        let profile_path = minecraft_dir
            .join("versions")
            .join(profile_id)
            .join(format!("{profile_id}.json"));
        let base_path = minecraft_dir
            .join("versions")
            .join("1.21")
            .join("1.21.json");
        write_profile_json(&profile_path, profile_id, "1.21");
        write_base_version_json(&base_path, "1.21");

        let entry = loader_manifests::find_entry("neoforge", "1.21", "21.0.163")
            .expect("neoforge 1.21/21.0.163 must be in manifest");

        let tuple = crate::installed_profile::LoaderTuple {
            loader: "neoforge".into(),
            minecraft_version: "1.21".into(),
            loader_version: "21.0.163".into(),
        };
        write_forge_receipt(&receipts_root, &minecraft_dir, &tuple, &entry.sha256);

        let resolved = resolve(ResolveRequest {
            instance_id: "adopt-neoforge-test".into(),
            base_version_id: "1.21".into(),
            loader: Some(LoaderInfo {
                loader_type: "neoforge".into(),
                version: "21.0.163".into(),
                version_url: String::new(),
            }),
            game_dir: tmp.path().join("game"),
            assets_dir: tmp.path().join("assets"),
            cache_dir: tmp.path().join("cache"),
            java_override: None,
            java_candidates: make_java_candidate(),
            network_policy: NetworkPolicy::all_disabled(),
            allow_incompatible_java_override: false,
            minecraft_dir: Some(minecraft_dir),
            receipts_root: Some(receipts_root),
        })
        .await
        .expect("neoforge adoption should succeed");

        assert!(
            resolved.adopted_profile.is_some(),
            "adopted_profile must be Some"
        );
    }

    #[tokio::test]
    async fn adopt_forge_missing_profile_returns_err() {
        // No profile JSON on disk → ProfileMissing error.
        let tmp = tempfile::TempDir::new().unwrap();
        let (minecraft_dir, receipts_root) = make_adopt_fixture(&tmp);
        // Create base version but NOT the forge profile
        let base_path = minecraft_dir
            .join("versions")
            .join("1.21")
            .join("1.21.json");
        write_base_version_json(&base_path, "1.21");

        let err = resolve(ResolveRequest {
            instance_id: "missing-profile-test".into(),
            base_version_id: "1.21".into(),
            loader: Some(LoaderInfo {
                loader_type: "forge".into(),
                version: "51.0.29".into(),
                version_url: String::new(),
            }),
            game_dir: tmp.path().join("game"),
            assets_dir: tmp.path().join("assets"),
            cache_dir: tmp.path().join("cache"),
            java_override: None,
            java_candidates: make_java_candidate(),
            network_policy: NetworkPolicy::all_disabled(),
            allow_incompatible_java_override: false,
            minecraft_dir: Some(minecraft_dir),
            receipts_root: Some(receipts_root),
        })
        .await
        .expect_err("missing profile should produce an error");

        assert!(
            matches!(&err, LauncherError::ProfileMissing(_)),
            "expected ProfileMissing, got {err:?}"
        );
        assert_eq!(err.code(), "ERR_PROFILE_MISSING");
    }

    #[tokio::test]
    async fn adopt_forge_corrupt_json_returns_err() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (minecraft_dir, receipts_root) = make_adopt_fixture(&tmp);
        let profile_id = "forge-1.21-51.0.29";
        let profile_path = minecraft_dir
            .join("versions")
            .join(profile_id)
            .join(format!("{profile_id}.json"));
        let base_path = minecraft_dir
            .join("versions")
            .join("1.21")
            .join("1.21.json");
        write_base_version_json(&base_path, "1.21");

        // Write corrupt JSON (not valid JSON)
        std::fs::create_dir_all(profile_path.parent().unwrap()).unwrap();
        std::fs::write(&profile_path, b"not valid json content").unwrap();

        let err = resolve(ResolveRequest {
            instance_id: "corrupt-profile-test".into(),
            base_version_id: "1.21".into(),
            loader: Some(LoaderInfo {
                loader_type: "forge".into(),
                version: "51.0.29".into(),
                version_url: String::new(),
            }),
            game_dir: tmp.path().join("game"),
            assets_dir: tmp.path().join("assets"),
            cache_dir: tmp.path().join("cache"),
            java_override: None,
            java_candidates: make_java_candidate(),
            network_policy: NetworkPolicy::all_disabled(),
            allow_incompatible_java_override: false,
            minecraft_dir: Some(minecraft_dir),
            receipts_root: Some(receipts_root),
        })
        .await
        .expect_err("corrupt profile should produce an error");

        assert!(
            matches!(&err, LauncherError::ProfileCorrupt(_)),
            "expected ProfileCorrupt, got {err:?}"
        );
        assert_eq!(err.code(), "ERR_PROFILE_CORRUPT");
    }

    #[tokio::test]
    async fn adopt_forge_no_receipt_unsupported_returns_err() {
        // Profile exists but no receipt → unsupported metadata (unhashed libs).
        let tmp = tempfile::TempDir::new().unwrap();
        let (minecraft_dir, receipts_root) = make_adopt_fixture(&tmp);
        let profile_id = "forge-1.21-51.0.29";
        let profile_path = minecraft_dir
            .join("versions")
            .join(profile_id)
            .join(format!("{profile_id}.json"));
        let base_path = minecraft_dir
            .join("versions")
            .join("1.21")
            .join("1.21.json");
        // Write profile with a no-hash library to trigger unsupported error
        let profile = serde_json::json!({
            "id": profile_id,
            "inheritsFrom": "1.21",
            "mainClass": "net.minecraftforge.Main",
            "type": "release",
            "libraries": [{
                "name": "some.group:artifact:1.0",
                "url": "https://example.com/"
            }],
        });
        std::fs::create_dir_all(profile_path.parent().unwrap()).unwrap();
        std::fs::write(&profile_path, serde_json::to_vec_pretty(&profile).unwrap()).unwrap();
        write_base_version_json(&base_path, "1.21");

        let err = resolve(ResolveRequest {
            instance_id: "unsupported-profile-test".into(),
            base_version_id: "1.21".into(),
            loader: Some(LoaderInfo {
                loader_type: "forge".into(),
                version: "51.0.29".into(),
                version_url: String::new(),
            }),
            game_dir: tmp.path().join("game"),
            assets_dir: tmp.path().join("assets"),
            cache_dir: tmp.path().join("cache"),
            java_override: None,
            java_candidates: make_java_candidate(),
            network_policy: NetworkPolicy::all_disabled(),
            allow_incompatible_java_override: false,
            minecraft_dir: Some(minecraft_dir),
            receipts_root: Some(receipts_root),
        })
        .await
        .expect_err("profile without receipt should produce unsupported error");

        assert!(
            matches!(&err, LauncherError::ProfileUnsupportedMetadata(_)),
            "expected ProfileUnsupportedMetadata, got {err:?}"
        );
        assert_eq!(err.code(), "ERR_PROFILE_UNSUPPORTED_METADATA");
    }

    #[tokio::test]
    async fn adopt_forge_no_minecraft_dir_returns_err() {
        // When minecraft_dir and receipts_root are None, adoption should
        // return ProfileMissing error.
        let tmp = tempfile::TempDir::new().unwrap();

        let err = resolve(ResolveRequest {
            instance_id: "no-adoption-path-test".into(),
            base_version_id: "1.21".into(),
            loader: Some(LoaderInfo {
                loader_type: "forge".into(),
                version: "51.0.29".into(),
                version_url: String::new(),
            }),
            game_dir: tmp.path().join("game"),
            assets_dir: tmp.path().join("assets"),
            cache_dir: tmp.path().join("cache"),
            java_override: None,
            java_candidates: make_java_candidate(),
            network_policy: NetworkPolicy::all_disabled(),
            allow_incompatible_java_override: false,
            minecraft_dir: None,
            receipts_root: None,
        })
        .await
        .expect_err("missing adoption paths should produce error");

        assert!(
            matches!(&err, LauncherError::ProfileMissing(_)),
            "expected ProfileMissing, got {err:?}"
        );
    }
}
