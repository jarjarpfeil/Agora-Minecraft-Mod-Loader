use serde::{Deserialize, Serialize};

use std::time::Duration;

use crate::error::{LauncherError, LauncherResult};

pub const AGORA_OAUTH_CLIENT_ID: &str = match option_env!("AGORA_OAUTH_CLIENT_ID") {
    Some(v) => v,
    None => "Iv23ctVA40Yy1ZUkvemh",
};

const KEYRING_SERVICE: &str = "com.agoramc";
const KEYRING_ACCOUNT: &str = "github-token";

/// Fallback token file name (in app data dir) for when OS keyring is unavailable.
const TOKEN_FALLBACK_FILE: &str = "tokens.enc";

/// PBKDF2 iterations for key derivation in the keyring fallback.
const PBKDF2_ITERATIONS: u32 = 200_000;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DeviceFlowResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GithubProfile {
    pub login: String,
    pub avatar_url: String,
}

#[derive(Debug, Deserialize)]
struct DeviceFlowPollResponse {
    access_token: Option<String>,
    error: Option<String>,
    interval: Option<u64>,
}

/// Log a line to stderr (replaced the old temp-file logger that wrote to
/// %TEMP%/agora-device-flow.log).
pub fn log_line(line: &str) {
    eprintln!("[auth] {line}");
}

pub async fn start_device_flow() -> LauncherResult<DeviceFlowResponse> {
    if AGORA_OAUTH_CLIENT_ID.is_empty() {
        return Err(LauncherError::Generic {
            code: "ERR_AUTH_NOT_CONFIGURED".to_string(),
            message: "GitHub OAuth is not configured. Set the AGORA_OAUTH_CLIENT_ID environment \
                      variable before building/running Tauri (e.g. \
                      $env:AGORA_OAUTH_CLIENT_ID='Iv1.xxxxxxxx'; npm run tauri:dev). Register \
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

    let params = [("client_id", AGORA_OAUTH_CLIENT_ID)];

    let resp = client
        .post("https://github.com/login/device/code")
        .header("Accept", "application/json")
        .form(&params)
        .send()
        .await
        .map_err(|e| {
            eprintln!("[auth] device-code request network error: {e}");
            LauncherError::NetworkOffline
        })?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    // Device-flow responses contain a device code. Do not emit response
    // bodies to logs, which are often collected by launchers and support
    // tools outside the OS credential boundary.
    eprintln!("[auth] device-code response status={status}");

    if !status.is_success() {
        return Err(LauncherError::Generic {
            code: "ERR_AUTH_DEVICE_CODE".to_string(),
            message: format!("GitHub rejected the device code request (status {status})."),
        });
    }

    serde_json::from_str::<DeviceFlowResponse>(&body).map_err(|e| {
        eprintln!("[auth] device-code parse error: {e}");
        LauncherError::Generic {
            code: "ERR_AUTH_DEVICE_CODE".to_string(),
            message: "Failed to parse GitHub device code response.".to_string(),
        }
    })
}

pub async fn poll_device_flow(device_code: String, mut interval: u64) -> LauncherResult<Option<String>> {
    eprintln!(
        "[auth] poll_device_flow ENTERED device_code_len={} interval={}s",
        device_code.len(),
        interval
    );
    let client = reqwest::Client::builder()
        .user_agent("agora-launcher")
        .build()
        .map_err(|e| {
            eprintln!("[auth] poll HTTP client build error: {e}");
            LauncherError::Generic {
                code: "ERR_AUTH_HTTP_CLIENT".to_string(),
                message: "Failed to build HTTP client for token polling.".to_string(),
            }
        })?;

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
                let body = r.text().await.unwrap_or_default();
                // Successful polling responses contain the OAuth access
                // token. Log only status metadata, never the body.
                eprintln!("[auth] poll status={status}");

                let parsed: Option<DeviceFlowPollResponse> =
                    serde_json::from_str(&body).ok();

                if let Some(parsed) = parsed {
                    if let Some(token) = parsed.access_token {
                        eprintln!("[auth] token obtained");
                        return Ok(Some(token));
                    }
                    if let Some(err) = parsed.error.as_deref() {
                        match err {
                            "authorization_pending" => {
                                eprintln!("[auth] awaiting user authorization (interval={})",
                                    parsed.interval.unwrap_or(interval));
                                if let Some(next) = parsed.interval {
                                    interval = next;
                                }
                            }
                            "slow_down" => {
                                interval = interval.saturating_add(5);
                                eprintln!("[auth] slow_down; interval now {interval}s");
                            }
                            "expired_token" => {
                                eprintln!("[auth] device code expired");
                                return Ok(None);
                            }
                            "access_denied" => {
                                eprintln!("[auth] user denied authorization");
                                return Ok(None);
                            }
                            other => {
                                eprintln!("[auth] unknown error from GitHub: {other}");
                            }
                        }
                    } else if let Some(next) = parsed.interval {
                        interval = next;
                    }
                } else {
                    eprintln!("[auth] could not parse poll response as JSON");
                }
            }
            Err(e) => {
                eprintln!("[auth] network error during poll: {e}");
            }
        }

        tokio::time::sleep(Duration::from_secs(interval.max(1))).await;
    }
}

/// Derive a 256-bit key using PBKDF2-HMAC-SHA256.
/// Salt is derived from the OS username and a stable machine identifier.
fn derive_fallback_key() -> Vec<u8> {
    use pbkdf2::pbkdf2_hmac;
    use sha2::Sha256;

    let username = dirs::home_dir()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "unknown".to_string());

    // TODO: use a stronger machine identifier (e.g. machine-id on Linux,
    // MachineGuid on Windows) when available.
    let salt = format!("agora-fallback:{}:{}", username, std::env::consts::OS);

    let mut key = vec![0u8; 32];
    pbkdf2_hmac::<Sha256>(b"agora-mcp-keyring-fallback", salt.as_bytes(), PBKDF2_ITERATIONS, &mut key);
    key
}

/// Encrypt the token using AES-256-GCM with a random 12-byte nonce.
/// Returns (nonce || ciphertext || tag).
fn encrypt_token(token: &str, key: &[u8]) -> LauncherResult<Vec<u8>> {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};

    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| LauncherError::Generic {
        code: "ERR_AUTH_ENCRYPT".to_string(),
        message: "Failed to create AES cipher for token encryption.".to_string(),
    })?;

    use rand::Rng;
    let nonce_bytes: [u8; 12] = rand::thread_rng().gen();
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, token.as_bytes())
        .map_err(|_| LauncherError::Generic {
            code: "ERR_AUTH_ENCRYPT".to_string(),
            message: "AES-GCM encryption failed.".to_string(),
        })?;

    let mut out = Vec::with_capacity(12 + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt a token from (nonce || ciphertext || tag).
fn decrypt_token(data: &[u8], key: &[u8]) -> Option<String> {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};

    if data.len() < 12 {
        return None;
    }

    let (nonce_bytes, ciphertext) = data.split_at(12);
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher.decrypt(nonce, ciphertext).ok()?;
    String::from_utf8(plaintext).ok()
}

/// Return the path to the fallback token file.
fn fallback_token_path() -> Option<std::path::PathBuf> {
    dirs::data_local_dir().map(|d| d.join("agora").join(TOKEN_FALLBACK_FILE))
}

/// Returns 	rue — the fallback is always available on all platforms.
/// This signal is used by Settings to show the spec-mandated "less secure" warning.
pub fn keyring_fallback_available() -> bool {
    true
}

pub fn store_token(token: &str) -> LauncherResult<()> {
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT) {
        if entry.set_password(token).is_ok() {
            return Ok(());
        }
    }

    // Keyring creation or write failed — fall back to AES-256-GCM encrypted
    // local storage. Secret Service commonly fails at write time on headless
    // Linux, so only falling back when Entry::new fails strands users.
    let path = fallback_token_path().ok_or_else(|| LauncherError::Generic {
        code: "ERR_AUTH_FALLBACK_PATH".to_string(),
        message: "Could not determine data directory for fallback token storage."
            .to_string(),
    })?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|_| LauncherError::Generic {
            code: "ERR_AUTH_FALLBACK_WRITE".to_string(),
            message: "Failed to create fallback token directory.".to_string(),
        })?;
    }

    let key = derive_fallback_key();
    let encrypted = encrypt_token(token, &key)?;
    std::fs::write(&path, encrypted).map_err(|_| LauncherError::Generic {
        code: "ERR_AUTH_FALLBACK_WRITE".to_string(),
        message: "Failed to write fallback token file.".to_string(),
    })?;

    Ok(())
}

pub fn get_token() -> Option<String> {
    // Try OS keyring first.
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT) {
        if let Ok(token) = entry.get_password() {
            return Some(token);
        }
    }

    // Fallback: try reading from encrypted file.
    let path = fallback_token_path()?;
    if !path.exists() {
        return None;
    }
    let data = std::fs::read(&path).ok()?;
    let key = derive_fallback_key();
    decrypt_token(&data, &key)
}

pub fn clear_token() -> Result<(), String> {
    // Clear from OS keyring.
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT) {
        match entry.delete_password() {
            Ok(()) => {}
            Err(keyring::Error::NoEntry) => {}
            Err(e) => return Err(format!("Failed to delete GitHub token: {}", e)),
        }
    }

    // Also remove the fallback encrypted file if present.
    if let Some(path) = fallback_token_path() {
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
    }

    Ok(())
}

pub fn is_authenticated() -> bool {
    get_token().is_some()
}

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



