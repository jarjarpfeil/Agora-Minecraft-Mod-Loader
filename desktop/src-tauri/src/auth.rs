use serde::{Deserialize, Serialize};
use std::io::Write;
use std::time::Duration;

use crate::error::{LauncherError, LauncherResult};

/// Append a diagnostic line to `agora-device-flow.log` in the OS temp dir.
///
/// On Windows the Tauri exe detaches from the launching terminal, so stderr
/// vanishes. File logging lets the user inspect what GitHub is actually
/// returning during the OAuth Device Flow poll. Lines are timestamped and
/// flushed synchronously. Public so other modules (e.g. `commands.rs`) can
/// share the same log sink.
pub fn log_line(line: &str) {
    let stamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let entry = format!("[{stamp}] {line}\n");
    // std::env::temp_dir() = %TEMP% on Windows, /tmp on Unix. Always writable.
    let path = std::env::temp_dir().join("agora-device-flow.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = f.write_all(entry.as_bytes());
        let _ = f.flush();
    }
}

/// GitHub OAuth Device Flow client id. Empty string means OAuth is not configured
/// for this build; callers that need a value should check for emptiness first.
pub const AGORA_OAUTH_CLIENT_ID: &str = match option_env!("AGORA_OAUTH_CLIENT_ID") {
    Some(v) => v,
    None => "Iv23ctVA40Yy1ZUkvemh",
};

const KEYRING_SERVICE: &str = "io.agora-mc";
const KEYRING_ACCOUNT: &str = "github-token";

/// Response from `POST https://github.com/login/device/code`.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DeviceFlowResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

/// Public GitHub profile fields surfaced after a successful login.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GithubProfile {
    pub login: String,
    pub avatar_url: String,
}

/// Internal shape of the GitHub access-token polling response.
#[derive(Debug, Deserialize)]
struct DeviceFlowPollResponse {
    access_token: Option<String>,
    error: Option<String>,
    interval: Option<u64>,
}

/// Start the GitHub OAuth Device Flow by requesting a device code.
pub async fn start_device_flow() -> LauncherResult<DeviceFlowResponse> {
    // Fail fast with a clear, actionable error instead of letting GitHub
    // reject the empty client_id request downstream.
    if AGORA_OAUTH_CLIENT_ID.is_empty() {
        return Err(LauncherError::Generic {
            code: "ERR_AUTH_NOT_CONFIGURED".to_string(),
            message: "GitHub OAuth is not configured. Set the AGORA_OAUTH_CLIENT_ID environment \
                      variable before building/running Tauri (e.g. \
                      `$env:AGORA_OAUTH_CLIENT_ID='Iv1.xxxxxxxx'; npm run tauri:dev`). Register \
                      an OAuth app at https://github.com/settings/developers (Authorization type: \
                      GitHub App, Device Flow enabled)."
                .to_string(),
        });
    }

    let client = reqwest::Client::builder()
        .user_agent("agora-launcher")
        .build()
        .map_err(|_| LauncherError::Generic {
            code: "ERR_AUTH_HTTP_CLIENT".to_string(),
            message: "Failed to build HTTP client for device flow.".to_string(),
        })?;

    // NOTE: GitHub Apps ignore the `scope` parameter — permissions are
    // determined by what's granted to the app in its settings UI at
    // https://github.com/settings/apps/<app-slug>/permissions. Do NOT send
    // OAuth-App-style scopes (e.g. `public_repo read:org`); they are silently
    // ignored for GitHub Apps and mislead readers about the trust model.
    // See README.md > "GitHub OAuth (in-app sign-in)" for the permission list.
    let params = [("client_id", AGORA_OAUTH_CLIENT_ID)];

    let resp = client
        .post("https://github.com/login/device/code")
        .header("Accept", "application/json")
        .form(&params)
        .send()
        .await
        .map_err(|e| {
            log_line(&format!(
                "device-code request network error: {e}"
            ));
            LauncherError::NetworkOffline
        })?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    log_line(&format!(
        "device-code response status={status} body={body}"
    ));

    if !status.is_success() {
        return Err(LauncherError::Generic {
            code: "ERR_AUTH_DEVICE_CODE".to_string(),
            message: format!("GitHub rejected the device code request (status {status})."),
        });
    }

    serde_json::from_str::<DeviceFlowResponse>(&body).map_err(|e| {
        log_line(&format!("device-code parse error: {e}"));
        LauncherError::Generic {
            code: "ERR_AUTH_DEVICE_CODE".to_string(),
            message: "Failed to parse GitHub device code response.".to_string(),
        }
    })
}

/// Poll the token endpoint until the user authorizes the device or it expires.
///
/// Returns `Some(token)` on success, or `None` if the device code expired /
/// the user declined before a token was issued.
pub async fn poll_device_flow(device_code: String, mut interval: u64) -> LauncherResult<Option<String>> {
    log_line(&format!(
        "poll_device_flow ENTERED device_code_len={} interval={}s",
        device_code.len(),
        interval
    ));
    let client = reqwest::Client::builder()
        .user_agent("agora-launcher")
        .build()
        .map_err(|e| {
            log_line(&format!("poll HTTP client build error: {e}"));
            LauncherError::Generic {
                code: "ERR_AUTH_HTTP_CLIENT".to_string(),
                message: "Failed to build HTTP client for token polling.".to_string(),
            }
        })?;

    // Hard ceiling so a stalled poll cannot run forever. GitHub `expires_in`
    // is typically 900s; we allow generously beyond that before giving up.
    let deadline = std::time::Instant::now() + Duration::from_secs(1200);

    loop {
        if std::time::Instant::now() >= deadline {
            return Ok(None);
        }

        let params = [
            ("client_id", AGORA_OAUTH_CLIENT_ID),
            ("device_code", &device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ];

        let resp = client
            .post("https://github.com/login/oauth/access_token")
            .header("Accept", "application/json")
            .form(&params)
            .send()
            .await;

        match resp {
            Ok(r) => {
                let status = r.status();
                // Capture the raw body for diagnostics — the response is small
                // (access_token or error+interval), and logging it lets the user
                // see exactly what GitHub is returning each iteration.
                let body = r.text().await.unwrap_or_default();
                log_line(&format!("poll status={status} body={body}"));

                let parsed: Option<DeviceFlowPollResponse> =
                    serde_json::from_str(&body).ok();

                if let Some(parsed) = parsed {
                    if let Some(token) = parsed.access_token {
                        log_line("token obtained");
                        return Ok(Some(token));
                    }
                    if let Some(err) = parsed.error.as_deref() {
                        match err {
                            "authorization_pending" => {
                                log_line(&format!(
                                    "awaiting user authorization (interval={})",
                                    parsed.interval.unwrap_or(interval)
                                ));
                                if let Some(next) = parsed.interval {
                                    interval = next;
                                }
                            }
                            "slow_down" => {
                                interval = interval.saturating_add(5);
                                log_line(&format!(
                                    "slow_down; interval now {interval}s"
                                ));
                            }
                            "expired_token" => {
                                log_line("device code expired");
                                return Ok(None);
                            }
                            "access_denied" => {
                                log_line("user denied authorization");
                                return Ok(None);
                            }
                            other => {
                                log_line(&format!(
                                    "unknown error from GitHub: {other}"
                                ));
                            }
                        }
                    } else if let Some(next) = parsed.interval {
                        interval = next;
                    }
                } else {
                    log_line("could not parse poll response as JSON");
                }
            }
            Err(e) => {
                // Network blip; back off and retry.
                log_line(&format!("network error during poll: {e}"));
            }
        }

        tokio::time::sleep(Duration::from_secs(interval.max(1))).await;
    }
}

/// Persist the GitHub token to the OS keyring.
///
/// For now this is a plain keyring write. If the keyring is unavailable the
/// call fails — AES-256-GCM file-based fallback is intentionally not
/// implemented yet.
pub fn store_token(_app: &tauri::AppHandle, token: &str) -> LauncherResult<()> {
    match keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT) {
        Ok(entry) => entry
            .set_password(token)
            .map_err(|_| LauncherError::Generic {
                code: "ERR_AUTH_KEYRING_WRITE".to_string(),
                message: "Failed to store GitHub token in the OS keyring.".to_string(),
            }),
        Err(_) => Err(LauncherError::Generic {
            code: "ERR_AUTH_KEYRING_UNAVAILABLE".to_string(),
            message: "OS keyring is unavailable and no fallback is implemented yet.".to_string(),
        }),
    }
}

/// Read the GitHub token from the OS keyring, if present.
pub fn get_token(_app: &tauri::AppHandle) -> Option<String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).ok()?;
    entry.get_password().ok()
}

/// Delete the stored GitHub token from the OS keyring.
pub fn clear_token(_app: &tauri::AppHandle) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)
        .map_err(|e| format!("Failed to open keyring entry: {}", e))?;
    // `delete_password` returns NoEntry if there's nothing stored; treat as success.
    match entry.delete_password() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("Failed to delete GitHub token: {}", e)),
    }
}

/// Whether a GitHub token is currently stored.
pub fn is_authenticated(app: &tauri::AppHandle) -> bool {
    get_token(app).is_some()
}

/// Fetch the authenticated user's GitHub profile using the given token.
pub async fn get_github_user(token: String) -> LauncherResult<GithubProfile> {
    let client = reqwest::Client::builder()
        .user_agent("agora-launcher")
        .build()
        .map_err(|_| LauncherError::Generic {
            code: "ERR_AUTH_HTTP_CLIENT".to_string(),
            message: "Failed to build HTTP client for GitHub profile.".to_string(),
        })?;

    let resp = client
        .get("https://api.github.com/user")
        .header("Authorization", format!("Bearer {}", token))
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|_| LauncherError::NetworkOffline)?;

    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(LauncherError::AuthExpired);
    }
    if !resp.status().is_success() {
        return Err(LauncherError::Generic {
            code: "ERR_AUTH_PROFILE".to_string(),
            message: "GitHub rejected the profile request.".to_string(),
        });
    }

    #[derive(Debug, Deserialize)]
    struct GithubUserJson {
        login: String,
        avatar_url: String,
    }

    let parsed = resp
        .json::<GithubUserJson>()
        .await
        .map_err(|_| LauncherError::Generic {
            code: "ERR_AUTH_PROFILE".to_string(),
            message: "Failed to parse GitHub profile response.".to_string(),
        })?;

    Ok(GithubProfile {
        login: parsed.login,
        avatar_url: parsed.avatar_url,
    })
}
