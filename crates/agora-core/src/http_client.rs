//! Category-aware HTTP clients with per-category timeouts, host allowlists,
//! URL scheme/port/IP validation, and redirect re-validation.
//!
//! **All production requests must go through a checked helper** — never call
//! `clients.get(category).get(url).send()` directly, as that bypasses the
//! URL validation, category allowlist, redirect re-validation, and response
//! size limits enforced here.
//!
//! Available helpers:
//! - [`checked_request`] — async GET with full enforcement, returns response.
//! - [`checked_get_bytes`] — async GET, returns validated bytes.
//! - [`checked_request_with_headers`] — async GET with custom headers (auth).
//! - [`blocking_checked_request`] — blocking GET with full enforcement.
//! - [`blocking_checked_get_bytes`] — blocking GET, returns validated bytes.
//!
//! Per-artifact hash verification remains at individual call sites.
//!
//! # Construction
//!
//! - [`HttpClients::new()`] returns `LauncherResult<Self>` — fails if the
//!   TLS backend cannot be initialised.
//! - [`HttpClients::for_testing()`] provides a single-client wrapper for tests
//!   (bypasses policy — never use in production).

use crate::error::{LauncherError, LauncherResult};
use crate::network;
use std::io::Read;
use std::net::IpAddr;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Category-to-allowlist mapping
// ---------------------------------------------------------------------------

/// Per-category host allowlist (first-party domains only).
///
/// Unknown public hosts are rejected in production helpers.
/// Dynamic user-approved endpoints (e.g. custom AI assistant host) must use
/// an explicit policy path.
pub(crate) fn category_allowlist(category: ClientCategory) -> &'static [&'static str] {
    match category {
        ClientCategory::MojangMetadata => &[
            "piston-meta.mojang.com",
            "launchermeta.mojang.com",
            "launcher.mojang.com",
        ],
        ClientCategory::MojangContent => &[
            "resources.download.minecraft.net",
            "libraries.minecraft.net",
            "piston-data.mojang.com",
        ],
        ClientCategory::Loader => &[
            "meta.fabricmc.net",
            "maven.fabricmc.net",
            "maven.quiltmc.org",
            "files.minecraftforge.net",
            "maven.minecraftforge.net",
            "maven.neoforged.net",
            "repo.spongepowered.org",
            "raw.githubusercontent.com",
        ],
        ClientCategory::Modrinth => &["api.modrinth.com", "cdn.modrinth.com"],
        ClientCategory::Modpack => &[
            "cdn.modrinth.com",
            "github.com",
            "objects.githubusercontent.com",
            "releases.githubusercontent.com",
            "release-assets.githubusercontent.com",
        ],
        ClientCategory::GitHub | ClientCategory::Registry => &[
            "github.com",
            "api.github.com",
            "objects.githubusercontent.com",
            "releases.githubusercontent.com",
            "release-assets.githubusercontent.com",
            "raw.githubusercontent.com",
        ],
        ClientCategory::JavaRuntime => &[
            "api.adoptium.net",
            "github.com",
            "objects.githubusercontent.com",
            "release-assets.githubusercontent.com",
        ],
        ClientCategory::Microsoft => &[
            "login.live.com",
            "login.microsoftonline.com",
            "sisu.xboxlive.com",
            "api.minecraftservices.com",
            "xsts.auth.xboxlive.com",
            "user.auth.xboxlive.com",
            "device.auth.xboxlive.com",
        ],
        ClientCategory::AiAssistant => &[
            // Explicitly approved Copilot API hosts. Other user-configured
            // AI endpoints require a separate approval policy and are not
            // accepted by this production helper.
            "api.individual.githubcopilot.com",
            "api.githubcopilot.com",
        ],
    }
}

/// Friendly name for each HTTP client category, used in logging and errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClientCategory {
    /// Mojang metadata: version manifest, version JSON, asset index.
    MojangMetadata,
    /// Mojang content: client JAR, libraries, natives, assets, logging config.
    MojangContent,
    /// Loader metadata and content: pinned profiles, Maven artifacts.
    Loader,
    /// Modrinth API and CDN.
    Modrinth,
    /// Modpack archives, which are substantially larger than individual mods.
    Modpack,
    /// GitHub API and release assets.
    GitHub,
    /// Microsoft/ Xbox Live authentication (MSA).
    Microsoft,
    /// Registry database download from GitHub Releases.
    Registry,
    /// AI assistant / OpenAI-compatible API.
    AiAssistant,
    /// Managed Java runtime metadata and archives.
    JavaRuntime,
}

impl ClientCategory {
    fn timeout(&self) -> Duration {
        match self {
            ClientCategory::MojangMetadata => Duration::from_secs(30),
            ClientCategory::MojangContent => Duration::from_secs(120),
            ClientCategory::Loader => Duration::from_secs(60),
            ClientCategory::Modrinth => Duration::from_secs(30),
            ClientCategory::Modpack => Duration::from_secs(5 * 60),
            ClientCategory::GitHub => Duration::from_secs(30),
            ClientCategory::Microsoft => Duration::from_secs(30),
            ClientCategory::Registry => Duration::from_secs(60),
            ClientCategory::AiAssistant => Duration::from_secs(60),
            ClientCategory::JavaRuntime => Duration::from_secs(120),
        }
    }

    fn user_agent(&self) -> &'static str {
        "AgoraLauncher/1.0"
    }

    fn max_response_bytes(&self) -> Option<u64> {
        match self {
            ClientCategory::MojangContent => Some(200 * 1024 * 1024),
            ClientCategory::Modrinth => Some(200 * 1024 * 1024),
            ClientCategory::Modpack => Some(500 * 1024 * 1024),
            ClientCategory::Loader => Some(100 * 1024 * 1024),
            ClientCategory::Registry => Some(100 * 1024 * 1024),
            ClientCategory::JavaRuntime => Some(512 * 1024 * 1024),
            _ => Some(10 * 1024 * 1024),
        }
    }
}

// ---------------------------------------------------------------------------
// HttpClients
// ---------------------------------------------------------------------------

/// A set of pre-built HTTP clients for different network categories.
///
/// Construct via [`HttpClients::new()`] (production) or
/// [`HttpClients::for_testing()`] (tests only — no policy enforcement).
#[derive(Debug, Clone)]
pub struct HttpClients {
    mojang_metadata: reqwest::Client,
    mojang_content: reqwest::Client,
    loader: reqwest::Client,
    modrinth: reqwest::Client,
    github: reqwest::Client,
    microsoft: reqwest::Client,
    registry: reqwest::Client,
    ai_assistant: reqwest::Client,
    java_runtime: reqwest::Client,
}

impl HttpClients {
    /// Build a full set of category-aware clients.
    ///
    /// Returns `Err` if the TLS backend cannot be initialised (fatal).
    pub fn new() -> LauncherResult<Self> {
        Ok(Self {
            mojang_metadata: Self::build_client(ClientCategory::MojangMetadata)?,
            mojang_content: Self::build_client(ClientCategory::MojangContent)?,
            loader: Self::build_client(ClientCategory::Loader)?,
            modrinth: Self::build_client(ClientCategory::Modrinth)?,
            github: Self::build_client(ClientCategory::GitHub)?,
            microsoft: Self::build_client(ClientCategory::Microsoft)?,
            registry: Self::build_client(ClientCategory::Registry)?,
            ai_assistant: Self::build_client(ClientCategory::AiAssistant)?,
            java_runtime: Self::build_client(ClientCategory::JavaRuntime)?,
        })
    }

    /// Build with a single client used for all categories (**testing only**).
    ///
    /// This bypasses category-specific timeouts and policies — never use in
    /// production code. When the actual policy matters, use [`new()`](Self::new)
    /// and override individual clients with `with_*`.
    pub fn for_testing(client: reqwest::Client) -> Self {
        Self {
            mojang_metadata: client.clone(),
            mojang_content: client.clone(),
            loader: client.clone(),
            modrinth: client.clone(),
            github: client.clone(),
            microsoft: client.clone(),
            registry: client.clone(),
            ai_assistant: client.clone(),
            java_runtime: client,
        }
    }

    /// Get the raw client for a category.
    ///
    /// Prefer [`checked_request`] or [`checked_get_bytes`] instead of using
    /// this directly, to ensure policy enforcement.
    pub fn get(&self, category: ClientCategory) -> &reqwest::Client {
        match category {
            ClientCategory::MojangMetadata => &self.mojang_metadata,
            ClientCategory::MojangContent => &self.mojang_content,
            ClientCategory::Loader => &self.loader,
            ClientCategory::Modrinth => &self.modrinth,
            ClientCategory::Modpack => &self.modrinth,
            ClientCategory::GitHub => &self.github,
            ClientCategory::Microsoft => &self.microsoft,
            ClientCategory::Registry => &self.registry,
            ClientCategory::AiAssistant => &self.ai_assistant,
            ClientCategory::JavaRuntime => &self.java_runtime,
        }
    }

    fn build_client(category: ClientCategory) -> LauncherResult<reqwest::Client> {
        // Redirects are handled by checked_request's manual per-hop loop
        // with re-validation. The client itself follows none to prevent
        // any accidental bypass when callers use .get() directly.
        reqwest::Client::builder()
            .timeout(category.timeout())
            .user_agent(category.user_agent())
            .redirect(reqwest::redirect::Policy::none())
            .pool_max_idle_per_host(4)
            .build()
            .map_err(|e| LauncherError::Generic {
                code: "ERR_HTTP_CLIENT_BUILD".into(),
                message: format!("Failed to build HTTP client for {category:?}: {e}"),
            })
    }

    // ------------------------------------------------------------------
    // Builder override helpers (testing)
    // ------------------------------------------------------------------

    /// Replace the Modrinth client (e.g., with a mock).
    pub fn with_modrinth_client(mut self, client: reqwest::Client) -> Self {
        self.modrinth = client;
        self
    }

    /// Replace the GitHub client (e.g., with a mock).
    pub fn with_github_client(mut self, client: reqwest::Client) -> Self {
        self.github = client;
        self
    }
}

// ---------------------------------------------------------------------------
// URL validation (shared by checked_request and checked_get_bytes)
// ---------------------------------------------------------------------------

/// Validate a URL for a given category against all policies.
///
/// Checks:
/// 1. URL parses successfully.
/// 2. Scheme is HTTPS (not HTTP, not file, not data, not anything else).
/// 3. Port is 443 (default HTTPS) — no non-standard ports.
/// 4. No userinfo component (prevents `https://user:pass@host/`).
/// 5. Host is not an IP literal (must be a domain name).
/// 6. Host is on the category allowlist.
/// 7. Host resolves to a non-private/reserved IP (loopback, private,
///    link-local, multicast, unspecified — SSRF protection).
///
/// The allowlist and IP check together reduce but do not fully eliminate
/// DNS rebinding / SSRF risk. Hostname-based allowlists do not protect
/// against an attacker-controlled DNS that resolves an allowlisted domain
/// to a private IP after our check. This is an unavoidable platform
/// limitation without DNS-level pinning (HSTS, CAA, DANE).
pub fn check_request_url(category: ClientCategory, url: &str) -> LauncherResult<reqwest::Url> {
    let parsed = reqwest::Url::parse(url).map_err(|_| LauncherError::Generic {
        code: "ERR_INVALID_URL".into(),
        message: format!("Cannot parse URL: {url}"),
    })?;

    // 1. Scheme must be HTTPS.
    if parsed.scheme() != "https" {
        return Err(LauncherError::Generic {
            code: "ERR_HTTP_SCHEME".into(),
            message: format!(
                "URL scheme must be HTTPS: {}",
                network::sanitized_url_for_log(url)
            ),
        });
    }

    // 2. Port must be default HTTPS (443).
    if let Some(port) = parsed.port() {
        if port != 443 {
            return Err(LauncherError::Generic {
                code: "ERR_HTTP_PORT".into(),
                message: format!(
                    "Non-standard port {port} blocked for {}",
                    network::sanitized_url_for_log(url)
                ),
            });
        }
    }

    // 3. No userinfo (username:password in URL).
    if parsed.username() != "" || parsed.password().is_some() {
        return Err(LauncherError::Generic {
            code: "ERR_HTTP_USERINFO".into(),
            message: "URL must not contain userinfo (username:password).".into(),
        });
    }

    let host = parsed.host_str().ok_or_else(|| LauncherError::Generic {
        code: "ERR_INVALID_URL".into(),
        message: format!("URL has no host: {}", network::sanitized_url_for_log(url)),
    })?;

    // 4. Reject IP literals — require domain names.
    if host.parse::<IpAddr>().is_ok() {
        return Err(LauncherError::Generic {
            code: "ERR_HTTP_IP_LITERAL".into(),
            message: format!("IP literal {host} blocked; domain name required"),
        });
    }

    // 5. Category host allowlist.
    //
    // An empty allowlist rejects ALL hosts (the category has no approved
    // endpoints). Non-empty allowlists support exact and subdomain matching
    // (e.g. "objects.githubusercontent.com" matches when "github.com" is in
    // the list via ends_with(".github.com")).
    let allowlist = category_allowlist(category);
    let host_ok = allowlist
        .iter()
        .any(|allowed| host == *allowed || host.ends_with(&format!(".{allowed}")));
    if !host_ok {
        return Err(LauncherError::Generic {
            code: "ERR_HTTP_HOST_NOT_ALLOWED".into(),
            message: format!("Host {host} is not in the {category:?} allowlist",),
        });
    }

    // 6. SSRF protection: DNS resolution check for private IPs.
    //    Note: this is best-effort — the IP may change between check and
    //    connect. DNS-level hostname validation without DNSSEC cannot
    //    fully prevent DNS rebinding on attacker-controlled infrastructure.
    //    For first-party allowlisted domains this risk is negligible.
    if let Some(addr) = parsed
        .socket_addrs(|| None)
        .ok()
        .and_then(|addrs| addrs.into_iter().next())
    {
        let ip = addr.ip();
        let blocked = ip.is_loopback()
            || ip.is_unspecified()
            || ip.is_multicast()
            || match ip {
                IpAddr::V4(v4) => v4.is_private() || v4.is_link_local(),
                IpAddr::V6(_) => false,
            };
        if blocked {
            return Err(LauncherError::Generic {
                code: "ERR_SSRF_BLOCKED".into(),
                message: format!("Request blocked: {host} resolves to a private/reserved address"),
            });
        }
    }

    Ok(parsed)
}

// ---------------------------------------------------------------------------
// Checked request helpers (with per-hop redirect re-validation)
// ---------------------------------------------------------------------------

/// Perform a GET with full URL validation and per-hop redirect re-validation.
///
/// Validates the initial URL and each redirect hop against the same category
/// policies. Returns the final response.
pub async fn checked_request(
    clients: &HttpClients,
    category: ClientCategory,
    url: &str,
) -> LauncherResult<reqwest::Response> {
    // Validate initial URL against category policies.
    let _validated = check_request_url(category, url)?;

    let client = clients.get(category);

    // Build a custom redirect policy that re-validates each hop.
    let mut remaining_redirects: u8 = 10;
    let mut current_url = url.to_string();

    loop {
        let response =
            client
                .get(&current_url)
                .send()
                .await
                .map_err(|e| LauncherError::Generic {
                    code: "ERR_NETWORK".into(),
                    message: format!(
                        "HTTP GET failed for {}: {e}",
                        network::sanitized_url_for_log(&current_url)
                    ),
                })?;

        // Check if the response is a redirect.
        let status = response.status();
        if status.is_redirection() {
            if remaining_redirects == 0 {
                return Err(LauncherError::Generic {
                    code: "ERR_TOO_MANY_REDIRECTS".into(),
                    message: format!(
                        "Too many redirects for {}",
                        network::sanitized_url_for_log(url)
                    ),
                });
            }

            // Get the Location header.
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| LauncherError::Generic {
                    code: "ERR_REDIRECT_NO_LOCATION".into(),
                    message: "Redirect without Location header".to_string(),
                })?;

            // Resolve relative redirects against the current URL.
            let next_url = reqwest::Url::parse(&current_url)
                .ok()
                .and_then(|base| {
                    reqwest::Url::parse(location)
                        .or_else(|_| base.join(location))
                        .ok()
                })
                .ok_or_else(|| LauncherError::Generic {
                    code: "ERR_INVALID_REDIRECT".into(),
                    message: format!("Cannot resolve redirect Location: {location}"),
                })?;

            // Re-validate the redirect target against category policies.
            let _ = check_request_url(category, next_url.as_str())?;

            remaining_redirects -= 1;
            current_url = next_url.to_string();
            continue;
        }

        return Ok(response);
    }
}

/// Send an HTTP request through the category policy path.
///
/// Custom headers are applied only to the initial request. A request body is
/// replayed only for 307/308 redirects; 301/302/303 redirects become GETs so
/// credentials or form bodies are never silently replayed to a new endpoint.
pub async fn checked_send(
    clients: &HttpClients,
    category: ClientCategory,
    method: reqwest::Method,
    url: &str,
    headers: &[(String, String)],
    body: Option<Vec<u8>>,
    content_type: Option<&str>,
) -> LauncherResult<reqwest::Response> {
    check_request_url(category, url)?;
    let client = clients.get(category);
    let mut current_method = method;
    let mut current_url = url.to_string();
    let mut current_body = body;
    let mut first_request = true;
    let mut remaining_redirects = 10u8;

    loop {
        let mut request = client.request(current_method.clone(), &current_url);
        if first_request {
            for (key, value) in headers {
                request = request.header(key, value);
            }
            if let Some(content_type) = content_type {
                request = request.header(reqwest::header::CONTENT_TYPE, content_type);
            }
        }
        if let Some(body) = current_body.clone() {
            request = request.body(body);
        }

        let response = request.send().await.map_err(|e| LauncherError::Generic {
            code: "ERR_NETWORK".into(),
            message: format!(
                "HTTP request failed for {}: {e}",
                network::sanitized_url_for_log(&current_url)
            ),
        })?;
        if !response.status().is_redirection() {
            return Ok(response);
        }
        if remaining_redirects == 0 {
            return Err(LauncherError::Generic {
                code: "ERR_TOO_MANY_REDIRECTS".into(),
                message: format!(
                    "Too many redirects for {}",
                    network::sanitized_url_for_log(url)
                ),
            });
        }
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| LauncherError::Generic {
                code: "ERR_REDIRECT_NO_LOCATION".into(),
                message: "Redirect without Location header".into(),
            })?;
        let next_url = reqwest::Url::parse(&current_url)
            .ok()
            .and_then(|base| {
                reqwest::Url::parse(location)
                    .or_else(|_| base.join(location))
                    .ok()
            })
            .ok_or_else(|| LauncherError::Generic {
                code: "ERR_INVALID_REDIRECT".into(),
                message: "Cannot resolve redirect Location".into(),
            })?;
        check_request_url(category, next_url.as_str())?;

        match response.status().as_u16() {
            307 | 308 => {}
            _ => {
                current_method = reqwest::Method::GET;
                current_body = None;
            }
        }
        current_url = next_url.to_string();
        first_request = false;
        remaining_redirects -= 1;
    }
}

/// Send an URL-encoded form through the checked policy path.
pub async fn checked_post_form(
    clients: &HttpClients,
    category: ClientCategory,
    url: &str,
    fields: &[(&str, &str)],
    headers: &[(String, String)],
) -> LauncherResult<reqwest::Response> {
    let encoded = fields
        .iter()
        .map(|(key, value)| {
            format!(
                "{}={}",
                urlencoding::encode(key),
                urlencoding::encode(value)
            )
        })
        .collect::<Vec<_>>()
        .join("&");
    checked_send(
        clients,
        category,
        reqwest::Method::POST,
        url,
        headers,
        Some(encoded.into_bytes()),
        Some("application/x-www-form-urlencoded"),
    )
    .await
}

/// Send a JSON body through the checked policy path.
pub async fn checked_post_json(
    clients: &HttpClients,
    category: ClientCategory,
    url: &str,
    body: &serde_json::Value,
    headers: &[(String, String)],
) -> LauncherResult<reqwest::Response> {
    let body = serde_json::to_vec(body).map_err(|e| LauncherError::Generic {
        code: "ERR_JSON_ENCODE".into(),
        message: format!("Failed to encode request body: {e}"),
    })?;
    checked_send(
        clients,
        category,
        reqwest::Method::POST,
        url,
        headers,
        Some(body),
        Some("application/json"),
    )
    .await
}

/// Fetch JSON through the checked policy path.
pub async fn checked_get_json<T: serde::de::DeserializeOwned>(
    clients: &HttpClients,
    category: ClientCategory,
    url: &str,
) -> LauncherResult<T> {
    let bytes = checked_get_bytes(clients, category, url).await?;
    serde_json::from_slice(&bytes).map_err(|e| LauncherError::Generic {
        code: "ERR_JSON_DECODE".into(),
        message: format!("Failed to decode response JSON: {e}"),
    })
}

/// Download all bytes from a category-routed URL with full validation.
///
/// Validates the initial URL and each redirect hop, verifies HTTP success,
/// and enforces per-category response size limits. Pre-checks
/// Content-Length (fast reject) and validates the full body against the cap.
pub async fn checked_get_bytes(
    clients: &HttpClients,
    category: ClientCategory,
    url: &str,
) -> LauncherResult<Vec<u8>> {
    checked_get_bytes_with_progress(clients, category, url, |_downloaded, _total| {}).await
}

/// Download bytes while reporting cumulative body progress after each chunk.
///
/// The callback is synchronous and runs on the async task that owns the
/// response. It must remain lightweight and must not perform blocking I/O.
pub async fn checked_get_bytes_with_progress<F>(
    clients: &HttpClients,
    category: ClientCategory,
    url: &str,
    mut on_progress: F,
) -> LauncherResult<Vec<u8>>
where
    F: FnMut(u64, Option<u64>),
{
    let mut response = checked_request(clients, category, url).await?;

    if !response.status().is_success() {
        log_response_error(&response, &format!("{category:?} GET"));
        return Err(LauncherError::Generic {
            code: "ERR_HTTP_STATUS".into(),
            message: format!(
                "HTTP {} for {}",
                response.status(),
                network::sanitized_url_for_log(url)
            ),
        });
    }

    let max = category.max_response_bytes().unwrap_or(10 * 1024 * 1024) as usize;

    // Pre-check content-length header if available (fast reject).
    if let Some(cl) = response.content_length() {
        if cl as usize > max {
            return Err(LauncherError::Generic {
                code: "ERR_RESPONSE_TOO_LARGE".into(),
                message: format!("Content-Length {cl} exceeds maximum {max}"),
            });
        }
    }

    let cap = response.content_length().unwrap_or(0).min(max as u64) as usize;
    let mut data = Vec::with_capacity(cap);
    let mut total = 0usize;
    let content_length = response.content_length();
    on_progress(0, content_length);
    loop {
        let chunk = response.chunk().await.map_err(|e| LauncherError::Generic {
            code: "ERR_NETWORK".into(),
            message: format!("Failed to read response chunk: {e}"),
        })?;
        let Some(chunk) = chunk else { break };
        total = total.saturating_add(chunk.len());
        if total > max {
            return Err(LauncherError::Generic {
                code: "ERR_RESPONSE_TOO_LARGE".into(),
                message: format!("Response exceeds {max} bytes (read {total})"),
            });
        }
        data.extend_from_slice(&chunk);
        on_progress(total as u64, content_length);
    }
    Ok(data)
}

/// Read an already-validated response with the category size limit.
pub async fn checked_response_bytes(
    mut response: reqwest::Response,
    category: ClientCategory,
) -> LauncherResult<Vec<u8>> {
    let max = category.max_response_bytes().unwrap_or(10 * 1024 * 1024) as usize;
    if response
        .content_length()
        .is_some_and(|length| length as usize > max)
    {
        return Err(LauncherError::Generic {
            code: "ERR_RESPONSE_TOO_LARGE".into(),
            message: format!("Content-Length exceeds maximum {max}"),
        });
    }
    let mut data = Vec::new();
    let mut total = 0usize;
    loop {
        let chunk = response.chunk().await.map_err(|e| LauncherError::Generic {
            code: "ERR_NETWORK".into(),
            message: format!("Failed to read response chunk: {e}"),
        })?;
        let Some(chunk) = chunk else { break };
        total = total.saturating_add(chunk.len());
        if total > max {
            return Err(LauncherError::Generic {
                code: "ERR_RESPONSE_TOO_LARGE".into(),
                message: format!("Response exceeds {max} bytes"),
            });
        }
        data.extend_from_slice(&chunk);
    }
    Ok(data)
}

/// Read a checked response as UTF-8 with the category size limit.
pub async fn checked_response_text(
    response: reqwest::Response,
    category: ClientCategory,
) -> LauncherResult<String> {
    let bytes = checked_response_bytes(response, category).await?;
    String::from_utf8(bytes).map_err(|e| LauncherError::Generic {
        code: "ERR_RESPONSE_ENCODING".into(),
        message: format!("Response was not valid UTF-8: {e}"),
    })
}

/// Perform a GET with full URL validation, custom headers, and per-hop redirects.
///
/// Same as [`checked_request`] but accepts extra headers to inject
/// (e.g. `Authorization`, `Accept`). Headers are applied to the initial
/// request only; redirects get no custom headers (auth tokens are scoped
/// to the initial endpoint).
pub async fn checked_request_with_headers(
    clients: &HttpClients,
    category: ClientCategory,
    url: &str,
    headers: Vec<(String, String)>,
) -> LauncherResult<reqwest::Response> {
    let _validated = check_request_url(category, url)?;
    let client = clients.get(category);

    let mut remaining_redirects: u8 = 10;
    let mut current_url = url.to_string();
    let mut first = true;

    loop {
        let mut req = client.get(&current_url);
        if first {
            for (k, v) in &headers {
                req = req.header(k.as_str(), v.as_str());
            }
        }

        let response = req.send().await.map_err(|e| LauncherError::Generic {
            code: "ERR_NETWORK".into(),
            message: format!(
                "HTTP GET failed for {}: {e}",
                network::sanitized_url_for_log(&current_url)
            ),
        })?;

        if response.status().is_redirection() {
            if remaining_redirects == 0 {
                return Err(LauncherError::Generic {
                    code: "ERR_TOO_MANY_REDIRECTS".into(),
                    message: format!(
                        "Too many redirects for {}",
                        network::sanitized_url_for_log(url)
                    ),
                });
            }

            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| LauncherError::Generic {
                    code: "ERR_REDIRECT_NO_LOCATION".into(),
                    message: "Redirect without Location header".into(),
                })?;

            let next_url = reqwest::Url::parse(&current_url)
                .ok()
                .and_then(|base| {
                    reqwest::Url::parse(location)
                        .or_else(|_| base.join(location))
                        .ok()
                })
                .ok_or_else(|| LauncherError::Generic {
                    code: "ERR_INVALID_REDIRECT".into(),
                    message: format!("Cannot resolve redirect Location: {location}"),
                })?;

            let _ = check_request_url(category, next_url.as_str())?;
            remaining_redirects -= 1;
            current_url = next_url.to_string();
            first = false;
            continue;
        }

        return Ok(response);
    }
}

/// Blocking GET with full URL validation and per-hop redirect re-validation.
///
/// Uses `reqwest::blocking::Client` internally with the same policy as
/// [`checked_request`]. Use this in synchronous code paths only.
pub fn blocking_checked_request(
    _clients: &HttpClients,
    category: ClientCategory,
    url: &str,
) -> LauncherResult<reqwest::blocking::Response> {
    let _validated = check_request_url(category, url)?;

    // Build a blocking client with the same security posture.
    let client = reqwest::blocking::Client::builder()
        .timeout(category.timeout())
        .user_agent(category.user_agent())
        .redirect(reqwest::redirect::Policy::none())
        .pool_max_idle_per_host(4)
        .build()
        .map_err(|e| LauncherError::Generic {
            code: "ERR_HTTP_CLIENT_BUILD".into(),
            message: format!("Failed to build blocking HTTP client: {e}"),
        })?;

    let mut remaining_redirects: u8 = 10;
    let mut current_url = url.to_string();

    loop {
        let response = client
            .get(&current_url)
            .send()
            .map_err(|e| LauncherError::Generic {
                code: "ERR_NETWORK".into(),
                message: format!(
                    "HTTP GET failed for {}: {e}",
                    network::sanitized_url_for_log(url)
                ),
            })?;

        if response.status().is_redirection() {
            if remaining_redirects == 0 {
                return Err(LauncherError::Generic {
                    code: "ERR_TOO_MANY_REDIRECTS".into(),
                    message: format!(
                        "Too many redirects for {}",
                        network::sanitized_url_for_log(url)
                    ),
                });
            }

            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| LauncherError::Generic {
                    code: "ERR_REDIRECT_NO_LOCATION".into(),
                    message: "Redirect without Location header".into(),
                })?;

            let next_url = reqwest::Url::parse(&current_url)
                .ok()
                .and_then(|base| {
                    reqwest::Url::parse(location)
                        .or_else(|_| base.join(location))
                        .ok()
                })
                .ok_or_else(|| LauncherError::Generic {
                    code: "ERR_INVALID_REDIRECT".into(),
                    message: format!("Cannot resolve redirect Location: {location}"),
                })?;

            let _ = check_request_url(category, next_url.as_str())?;
            remaining_redirects -= 1;
            current_url = next_url.to_string();
            continue;
        }

        return Ok(response);
    }
}

/// Blocking GET returning validated bytes with size enforcement.
pub fn blocking_checked_get_bytes(
    clients: &HttpClients,
    category: ClientCategory,
    url: &str,
) -> LauncherResult<Vec<u8>> {
    let response = blocking_checked_request(clients, category, url)?;

    if !response.status().is_success() {
        return Err(LauncherError::Generic {
            code: "ERR_HTTP_STATUS".into(),
            message: format!(
                "HTTP {} for {}",
                response.status(),
                network::sanitized_url_for_log(url)
            ),
        });
    }

    let max = category.max_response_bytes().unwrap_or(10 * 1024 * 1024);
    let mut limited = response.take(max.saturating_add(1));
    let mut bytes = Vec::new();
    limited
        .read_to_end(&mut bytes)
        .map_err(|e| LauncherError::Generic {
            code: "ERR_NETWORK".into(),
            message: format!("Failed to read response body: {e}"),
        })?;
    if bytes.len() as u64 > max {
        return Err(LauncherError::Generic {
            code: "ERR_RESPONSE_TOO_LARGE".into(),
            message: format!(
                "Response exceeds {} bytes (downloaded {})",
                max,
                bytes.len()
            ),
        });
    }
    Ok(bytes.to_vec())
}

/// Log a sanitized summary of an HTTP response for diagnostics.
pub fn log_response_error(response: &reqwest::Response, context: &str) {
    let status = response.status();
    let url = response.url().as_str();
    eprintln!(
        "[http_client] {context}: HTTP {status} for {url}",
        context = context,
        status = status,
        url = network::sanitized_url_for_log(url),
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_succeeds() {
        let clients = HttpClients::new();
        assert!(clients.is_ok());
    }

    #[test]
    fn test_for_testing_does_not_panic() {
        let client = reqwest::Client::new();
        let clients = HttpClients::for_testing(client);
        assert!(clients.get(ClientCategory::GitHub) as *const _ as usize > 0);
    }

    #[test]
    fn test_categories_have_timeouts() {
        for cat in &[
            ClientCategory::MojangMetadata,
            ClientCategory::MojangContent,
            ClientCategory::Loader,
            ClientCategory::Modrinth,
            ClientCategory::Modpack,
            ClientCategory::GitHub,
            ClientCategory::Microsoft,
            ClientCategory::Registry,
            ClientCategory::AiAssistant,
        ] {
            assert!(
                cat.timeout() >= Duration::from_secs(1),
                "{:?} timeout too short",
                cat
            );
        }
    }

    #[test]
    fn test_modpack_category_allows_large_pack_entries() {
        assert_eq!(
            ClientCategory::Modpack.max_response_bytes(),
            Some(500 * 1024 * 1024)
        );
        assert!(
            ClientCategory::Modpack.max_response_bytes()
                > ClientCategory::Modrinth.max_response_bytes()
        );
    }

    // ------------------------------------------------------------------
    // URL validation tests
    // ------------------------------------------------------------------

    #[test]
    fn test_check_rejects_http() {
        let err =
            check_request_url(ClientCategory::GitHub, "http://github.com/file.jar").unwrap_err();
        assert_eq!(err.code(), "ERR_HTTP_SCHEME");
    }

    #[test]
    fn test_check_rejects_non_standard_port() {
        let err = check_request_url(ClientCategory::GitHub, "https://github.com:8080/file.jar")
            .unwrap_err();
        assert_eq!(err.code(), "ERR_HTTP_PORT");
    }

    #[test]
    fn test_check_rejects_userinfo() {
        let err = check_request_url(
            ClientCategory::GitHub,
            "https://user:pass@github.com/file.jar",
        )
        .unwrap_err();
        assert_eq!(err.code(), "ERR_HTTP_USERINFO");
    }

    #[test]
    fn test_check_rejects_ip_literal() {
        let err =
            check_request_url(ClientCategory::GitHub, "https://192.168.1.1/file.jar").unwrap_err();
        assert_eq!(err.code(), "ERR_HTTP_IP_LITERAL");
    }

    #[test]
    fn test_check_rejects_loopback() {
        let err =
            check_request_url(ClientCategory::GitHub, "https://127.0.0.1/file.jar").unwrap_err();
        assert_eq!(err.code(), "ERR_HTTP_IP_LITERAL");
    }

    #[test]
    fn test_check_allows_legitimate_github() {
        assert!(check_request_url(
            ClientCategory::GitHub,
            "https://github.com/owner/repo/releases/download/v1/file.jar"
        )
        .is_ok());
    }

    #[test]
    fn test_check_allows_legitimate_mojang() {
        assert!(check_request_url(
            ClientCategory::MojangMetadata,
            "https://piston-meta.mojang.com/manifest.json"
        )
        .is_ok());
    }

    #[test]
    fn test_check_rejects_unknown_host() {
        let err = check_request_url(
            ClientCategory::GitHub,
            "https://evil.example.com/malware.jar",
        )
        .unwrap_err();
        assert_eq!(err.code(), "ERR_HTTP_HOST_NOT_ALLOWED");
    }

    #[test]
    fn test_check_rejects_category_mismatch() {
        // github.com is in GitHub allowlist, not Modrinth.
        let err = check_request_url(
            ClientCategory::Modrinth,
            "https://github.com/owner/repo/file.jar",
        )
        .unwrap_err();
        assert_eq!(err.code(), "ERR_HTTP_HOST_NOT_ALLOWED");
    }

    #[test]
    fn test_check_allows_subdomain_of_allowlisted_host() {
        // Subdomains of allowlisted hosts are allowed (e.g., github.com
        // covers *.github.com since GitHub controls all its subdomains).
        assert!(check_request_url(
            ClientCategory::GitHub,
            "https://evil.github.com/malware.jar"
        )
        .is_ok());
    }

    #[test]
    fn test_check_rejects_host_not_in_allowlist_at_all() {
        // A completely unrelated host should be rejected.
        let err = check_request_url(ClientCategory::GitHub, "https://not-github.com/file.jar")
            .unwrap_err();
        assert_eq!(err.code(), "ERR_HTTP_HOST_NOT_ALLOWED");
    }

    #[test]
    fn test_check_allows_subdomain() {
        // objects.githubusercontent.com IS in the GitHub allowlist.
        assert!(check_request_url(
            ClientCategory::GitHub,
            "https://objects.githubusercontent.com/asset.zip"
        )
        .is_ok());
    }

    #[test]
    fn test_check_rejects_invalid_url() {
        let err = check_request_url(ClientCategory::GitHub, "not a url").unwrap_err();
        assert_eq!(err.code(), "ERR_INVALID_URL");
    }

    #[test]
    fn test_ai_assistant_rejects_unapproved_host() {
        // Only the explicitly approved Copilot hosts are accepted.
        let err = check_request_url(
            ClientCategory::AiAssistant,
            "https://api.openai.com/v1/chat/completions",
        )
        .unwrap_err();
        assert_eq!(err.code(), "ERR_HTTP_HOST_NOT_ALLOWED");
    }

    // ------------------------------------------------------------------
    // Allowlist contents
    // ------------------------------------------------------------------

    #[test]
    fn test_category_allowlist_nonempty() {
        assert!(!category_allowlist(ClientCategory::MojangMetadata).is_empty());
        assert!(!category_allowlist(ClientCategory::GitHub).is_empty());
        assert!(!category_allowlist(ClientCategory::Microsoft).is_empty());
        assert!(!category_allowlist(ClientCategory::Modrinth).is_empty());
        assert!(!category_allowlist(ClientCategory::Modpack).is_empty());
        assert!(!category_allowlist(ClientCategory::Loader).is_empty());
        assert!(!category_allowlist(ClientCategory::MojangContent).is_empty());
        assert!(!category_allowlist(ClientCategory::Registry).is_empty());
        assert!(!category_allowlist(ClientCategory::AiAssistant).is_empty());
    }
}
