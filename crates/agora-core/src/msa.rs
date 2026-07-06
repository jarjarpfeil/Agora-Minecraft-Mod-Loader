//! Microsoft Account (MSA) authentication for direct Minecraft launching.
//!
//! Implements the Xbox Live device-token flow (p256 ECDSA signed requests)
//! adapted from the Theseus/Modrinth App reference implementation. This is the
//! security-critical Phase 5 module — NO tokens are ever logged, rendered in
//! UI redacted form, or persisted outside the OS keyring (+ aes-gcm fallback).
//!
//! The flow (9 steps, see plan Phase 5):
//!   1. Generate p256 ECDSA key pair → device token
//!   2. SISU authenticate → redirect URI for browser login
//!   3. User completes browser login → auth code
//!   4. Exchange auth code → OAuth token (access + refresh)
//!   5. SISU authorize → Xbox title + user tokens
//!   6. XSTS authorize → Xbox Live token (extract user hash)
//!   7. Minecraft launcher login → MC access token
//!   8. Minecraft entitlements check
//!   9. Minecraft profile (username + UUID)

use crate::db;
use crate::error::{LauncherError, LauncherResult};
use base64::Engine;
use chrono::{DateTime, Utc};
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use p256::ecdsa::signature::Signer;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Check a network enable setting from the local state DB.
fn check_network_enabled(setting_key: &str, disabled_msg: &str) -> LauncherResult<()> {
    let app_data_dir = dirs::data_local_dir()
        .ok_or_else(|| LauncherError::Generic {
            code: "ERR_NO_DATA_DIR".into(),
            message: "Could not determine local data directory.".into(),
        })?
        .join("agora");
    let db_path = app_data_dir.join("local_state.db");
    let conn = db::local_state_connection(&db_path).map_err(|e| LauncherError::Generic {
        code: "ERR_DB".into(),
        message: e.to_string(),
    })?;
    if !db::is_network_enabled(&conn, setting_key) {
        return Err(LauncherError::Generic {
            code: "ERR_NETWORK_DISABLED".into(),
            message: disabled_msg.into(),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Constants (from Theseus — public, well-known Microsoft identifiers)
// ---------------------------------------------------------------------------

const MICROSOFT_CLIENT_ID: &str = "00000000402b5328";
const AUTH_REPLY_URL: &str = "https://login.live.com/oauth20_desktop.srf";
const REQUESTED_SCOPE: &str = "service::user.auth.xboxlive.com::MBI_SSL";
const TITLE_ID: &str = "1794566092";
const USER_AGENT: &str = "Agora Launcher (https://github.com/Kilo-Org/agora)";

// Endpoints
const DEVICE_AUTH_URL: &str = "https://device.auth.xboxlive.com/device/authenticate";
const SISU_AUTH_URL: &str = "https://sisu.xboxlive.com/authenticate";
const OAUTH_TOKEN_URL: &str = "https://login.live.com/oauth20_token.srf";
const SISU_AUTHORIZE_URL: &str = "https://sisu.xboxlive.com/authorize";
const XSTS_AUTHORIZE_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_LOGIN_URL: &str = "https://api.minecraftservices.com/launcher/login";
const MC_ENTITLEMENTS_URL: &str = "https://api.minecraftservices.com/entitlements/license";
const MC_PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";

// Keyring storage
const KEYRING_SERVICE: &str = "com.agoramc";
const KEYRING_ACCOUNT: &str = "msa-credentials";

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Persisted Minecraft credentials stored in the OS keyring.
#[derive(Clone, Serialize, Deserialize)]
pub struct MsaCredentials {
    pub username: String,
    pub uuid: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires: DateTime<Utc>,
}

impl std::fmt::Debug for MsaCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MsaCredentials")
            .field("username", &self.username)
            .field("uuid", &self.uuid)
            .field("access_token", &"[REDACTED]")
            .field("refresh_token", &"[REDACTED]")
            .field("expires", &self.expires)
            .finish()
    }
}

impl MsaCredentials {
    pub fn is_expired(&self) -> bool {
        Utc::now() >= self.expires
    }

    pub fn needs_refresh(&self) -> bool {
        // 5-minute margin
        Utc::now() + chrono::Duration::minutes(5) >= self.expires
    }
}

/// The login flow state returned by `begin_login`. The caller opens
/// `auth_uri` in a browser, captures the `?code=` from the redirect, then
/// passes it to `finish_login`.
#[derive(Clone)]
pub struct LoginFlow {
    pub auth_uri: String,
    verifier: String,
    session_id: Option<String>,
    key_json: String,
    device_token: String,
    state: String,
}

impl std::fmt::Debug for LoginFlow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginFlow")
            .field("auth_uri", &self.auth_uri)
            .field("verifier", &"[REDACTED]")
            .field("session_id", &self.session_id.as_deref().map(|_| "[PRESENT]"))
            .field("key_json", &"[REDACTED]")
            .field("device_token", &"[REDACTED]")
            .field("state", &"[REDACTED]")
            .finish()
    }
}

// Internal: the p256 signing key pair (serialized for the LoginFlow)
struct DeviceTokenKey {
    id: Uuid,
    key: SigningKey,
    x: String,
    y: String,
}

impl std::fmt::Debug for DeviceTokenKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceTokenKey")
            .field("id", &self.id)
            .field("key", &"[REDACTED]")
            .field("x", &self.x)
            .field("y", &self.y)
            .finish()
    }
}

impl DeviceTokenKey {
    fn generate() -> Self {
        let id = Uuid::new_v4();
        let signing_key = SigningKey::random(&mut OsRng);
        let public_key = VerifyingKey::from(&signing_key);
        let point = public_key.to_encoded_point(false);

        let b64 = |bytes: &[u8]| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
        };

        let x = point.x().map(|x| b64(x.as_slice())).unwrap_or_default();
        let y = point.y().map(|y| b64(y.as_slice())).unwrap_or_default();

        Self { id, key: signing_key, x, y }
    }

    /// Serialize the key for storage in LoginFlow (the key must survive across
    /// async calls between begin_login and finish_login).
    fn to_json(&self) -> String {
        let der = p256::pkcs8::EncodePrivateKey::to_pkcs8_der(&self.key)
            .map(|d| d.as_bytes().to_vec())
            .unwrap_or_default();
        serde_json::json!({
            "id": self.id,
            "x": self.x,
            "y": self.y,
            "der": base64::engine::general_purpose::STANDARD.encode(&der),
        })
        .to_string()
    }

    fn from_json(json: &str) -> LauncherResult<Self> {
        let v: serde_json::Value = serde_json::from_str(json).map_err(|_| LauncherError::Generic {
            code: "ERR_MSA_KEY_DECODE".into(),
            message: "Failed to decode device token key.".into(),
        })?;
        let id: Uuid = v["id"].as_str()
            .and_then(|s| Uuid::parse_str(s).ok())
            .unwrap_or_else(Uuid::new_v4);
        let x = v["x"].as_str().unwrap_or("").to_string();
        let y = v["y"].as_str().unwrap_or("").to_string();
        let der_b64 = v["der"].as_str().unwrap_or("");
        let der = base64::engine::general_purpose::STANDARD.decode(der_b64).unwrap_or_default();
        let key = p256::pkcs8::DecodePrivateKey::from_pkcs8_der(&der)
            .map_err(|_| LauncherError::Generic {
                code: "ERR_MSA_KEY_DECODE".into(),
                message: "Failed to reconstruct signing key from PKCS8 DER.".into(),
            })?;
        Ok(Self { id, key, x, y })
    }
}

/// Response shapes (PascalCase = Xbox Live convention)
mod xbox_types {
    use serde::Deserialize;
    use chrono::{DateTime, Utc};
    use std::collections::HashMap;

    #[derive(Deserialize, Clone)]
    #[serde(rename_all = "PascalCase")]
    pub struct XboxToken {
        pub issue_instant: DateTime<Utc>,
        pub not_after: DateTime<Utc>,
        pub token: String,
        pub display_claims: HashMap<String, serde_json::Value>,
    }

    impl std::fmt::Debug for XboxToken {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("XboxToken")
                .field("issue_instant", &self.issue_instant)
                .field("not_after", &self.not_after)
                .field("token", &"[REDACTED]")
                .field("display_claims", &"[PRESENT]")
                .finish()
        }
    }

    #[derive(Deserialize, Debug, Clone)]
    pub struct SisuRedirect {
        #[serde(rename = "MsaOauthRedirect")]
        pub msa_oauth_redirect: String,
    }

    #[derive(Deserialize, Clone)]
    #[serde(rename_all = "PascalCase")]
    pub struct SisuAuthorize {
        pub title_token: XboxToken,
        pub user_token: XboxToken,
    }

    impl std::fmt::Debug for SisuAuthorize {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("SisuAuthorize")
                .field("title_token", &"[REDACTED]")
                .field("user_token", &"[REDACTED]")
                .finish()
        }
    }

    #[derive(Deserialize, Clone)]
    pub struct MinecraftToken {
        pub access_token: String,
    }

    impl std::fmt::Debug for MinecraftToken {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("MinecraftToken")
                .field("access_token", &"[REDACTED]")
                .finish()
        }
    }

    #[derive(Deserialize, Debug, Clone)]
    pub struct MinecraftProfile {
        pub id: String,
        pub name: String,
    }
}

// ---------------------------------------------------------------------------
// ECDSA signed request engine
// ---------------------------------------------------------------------------

/// Build the signature buffer and sign it with p256 ECDSA.
///
/// Buffer format (from Xbox Live authentication protocol):
/// [1_u32 BE][0_u8][time u128 BE][0_u8]"POST"[0_u8][path][0_u8][auth][0_u8][body][0_u8]
///
/// time = ((unix_timestamp + 11644473600) * 10000000) — Windows file time.
///
/// Wire format: [1_i32 BE][time 8 BE][r_bytes][s_bytes], base64-standard encoded.
fn sign_request(
    key: &DeviceTokenKey,
    url_path: &str,
    authorization: Option<&str>,
    body: &str,
    current_date: DateTime<Utc>,
) -> String {
    let time = ((current_date.timestamp() as u128) + 11644473600) * 10000000;

    let mut buffer = Vec::new();
    buffer.extend_from_slice(&1u32.to_be_bytes());
    buffer.push(0);
    buffer.extend_from_slice(&(time as u64).to_be_bytes()); // 8 bytes
    buffer.push(0);
    buffer.extend_from_slice(b"POST");
    buffer.push(0);
    buffer.extend_from_slice(url_path.as_bytes());
    buffer.push(0);
    buffer.extend_from_slice(authorization.unwrap_or("").as_bytes());
    buffer.push(0);
    buffer.extend_from_slice(body.as_bytes());
    buffer.push(0);

    let signature: Signature = key.key.sign(&buffer);
    let sig_bytes = signature.to_bytes();

    let mut wire = Vec::new();
    wire.extend_from_slice(&1i32.to_be_bytes());
    wire.extend_from_slice(&(time as u64).to_be_bytes());
    wire.extend_from_slice(&sig_bytes[..32]);  // r
    wire.extend_from_slice(&sig_bytes[32..]);   // s

    base64::engine::general_purpose::STANDARD.encode(&wire)
}

/// Send a signed request to an Xbox Live endpoint.
async fn send_signed_request<T: for<'de> serde::Deserialize<'de>>(
    client: &reqwest::Client,
    url: &str,
    url_path: &str,
    body: serde_json::Value,
    key: &DeviceTokenKey,
    authorization: Option<&str>,
    include_contract_version: bool,
) -> LauncherResult<(DateTime<Utc>, T)> {
    let current_date = Utc::now();
    let body_str = serde_json::to_string(&body).unwrap_or_default();
    let sig = sign_request(key, url_path, authorization, &body_str, current_date);

    let mut req = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("Signature", &sig);

    if include_contract_version {
        req = req.header("x-xbl-contract-version", "1");
    }
    if let Some(auth) = authorization {
        req = req.header("Authorization", auth);
    }

    let resp = req
        .body(body_str)
        .send()
        .await
        .map_err(|e| LauncherError::Generic {
            code: "ERR_MSA_REQUEST".into(),
            message: format!("Request to {} failed: {}", url, e),
        })?;

    let status = resp.status();
    let raw = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        return Err(LauncherError::Generic {
            code: "ERR_MSA_HTTP_ERROR".into(),
            message: format!("{} returned HTTP {} (response suppressed)", url, status),
        });
    }

    let parsed: T = serde_json::from_str(&raw).map_err(|e| LauncherError::Generic {
        code: "ERR_MSA_DESERIALIZE".into(),
        message: format!("Failed to parse response from {}: {} (response suppressed)", url, e),
    })?;

    Ok((current_date, parsed))
}

// ---------------------------------------------------------------------------
// Auth steps
// ---------------------------------------------------------------------------

fn proof_key_json(key: &DeviceTokenKey) -> serde_json::Value {
    serde_json::json!({
        "kty": "EC",
        "x": key.x,
        "y": key.y,
        "crv": "P-256",
        "alg": "ES256",
        "use": "sig"
    })
}

/// Step 1: Get device token from Xbox Live.
async fn get_device_token(
    client: &reqwest::Client,
    key: &DeviceTokenKey,
) -> LauncherResult<xbox_types::XboxToken> {
    let body = serde_json::json!({
        "Properties": {
            "AuthMethod": "ProofOfPossession",
            "Id": key.id.to_string(),
            "DeviceType": "Win32",
            "Version": "10.16.0",
            "ProofKey": proof_key_json(key),
        },
        "RelyingParty": "http://auth.xboxlive.com",
        "TokenType": "JWT"
    });

    let (_, token) = send_signed_request(
        client, DEVICE_AUTH_URL, "/device/authenticate",
        body, key, None, false,
    ).await?;
    Ok(token)
}

// ---------------------------------------------------------------------------
// PKCE challenge generation
// ---------------------------------------------------------------------------

fn generate_pkce_verifier() -> String {
    let bytes: Vec<u8> = (0..64).map(|_| rand::random::<u8>()).collect();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn pkce_challenge(verifier: &str) -> String {
    let hash = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash)
}

// ---------------------------------------------------------------------------
// Steps 2-3: SISU authenticate → redirect URI
// ---------------------------------------------------------------------------

/// Step 2: SISU authenticate with the device token. Returns (session_id, redirect_uri, state).
async fn sisu_authenticate(
    client: &reqwest::Client,
    device_token: &str,
    challenge: &str,
    key: &DeviceTokenKey,
) -> LauncherResult<(Option<String>, String, String)> {
    let state: String = (0..32).map(|_| format!("{:x}", rand::random::<u8>() % 16)).collect();

    let body = serde_json::json!({
        "AppId": MICROSOFT_CLIENT_ID,
        "DeviceToken": device_token,
        "Offers": [REQUESTED_SCOPE],
        "Query": {
            "code_challenge": challenge,
            "code_challenge_method": "S256",
            "state": state,
            "prompt": "select_account"
        },
        "RedirectUri": AUTH_REPLY_URL,
        "Sandbox": "RETAIL",
        "TokenType": "code",
        "TitleId": TITLE_ID
    });

    let body_str = serde_json::to_string(&body).unwrap_or_default();
    let sig = sign_request(key, "/authenticate", None, &body_str, Utc::now());

    let resp = client
        .post(SISU_AUTH_URL)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("Signature", &sig)
        .body(body_str)
        .send()
        .await
        .map_err(|e| LauncherError::Generic {
            code: "ERR_MSA_SISU_AUTH".into(),
            message: format!("SISU authenticate failed: {}", e),
        })?;

    let session_id = resp
        .headers()
        .get("X-SessionId")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let redirect: xbox_types::SisuRedirect = resp.json().await.map_err(|e| LauncherError::Generic {
        code: "ERR_MSA_SISU_AUTH_PARSE".into(),
        message: format!("Failed to parse SISU redirect: {}", e),
    })?;

    Ok((session_id, redirect.msa_oauth_redirect, state))
}

// ---------------------------------------------------------------------------
// Step 4: Exchange auth code for OAuth token
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct OAuthToken {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
}

impl std::fmt::Debug for OAuthToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthToken")
            .field("access_token", &"[REDACTED]")
            .field("refresh_token", &"[REDACTED]")
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

async fn exchange_oauth_token(
    client: &reqwest::Client,
    code: &str,
    verifier: &str,
) -> LauncherResult<OAuthToken> {
    let params = [
        ("client_id", MICROSOFT_CLIENT_ID),
        ("code", code),
        ("code_verifier", verifier),
        ("grant_type", "authorization_code"),
        ("redirect_uri", AUTH_REPLY_URL),
        ("scope", REQUESTED_SCOPE),
    ];

    let resp = client
        .post(OAUTH_TOKEN_URL)
        .form(&params)
        .send()
        .await
        .map_err(|e| LauncherError::Generic {
            code: "ERR_MSA_OAUTH_TOKEN".into(),
            message: format!("OAuth token exchange failed: {}", e),
        })?;

    let status = resp.status();
    if !status.is_success() {
        let _body = resp.text().await.unwrap_or_default();
        return Err(LauncherError::Generic {
            code: "ERR_MSA_OAUTH_TOKEN_HTTP".into(),
            message: format!("OAuth token endpoint returned HTTP {} (response suppressed)", status),
        });
    }

    resp.json::<OAuthToken>().await.map_err(|e| LauncherError::Generic {
        code: "ERR_MSA_OAUTH_TOKEN_PARSE".into(),
        message: format!("Failed to parse OAuth token: {}", e),
    })
}

async fn refresh_oauth_token(
    client: &reqwest::Client,
    refresh_token: &str,
) -> LauncherResult<OAuthToken> {
    let params = [
        ("client_id", MICROSOFT_CLIENT_ID),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
        ("redirect_uri", AUTH_REPLY_URL),
        ("scope", REQUESTED_SCOPE),
    ];

    let resp = client
        .post(OAUTH_TOKEN_URL)
        .form(&params)
        .send()
        .await
        .map_err(|e| LauncherError::Generic {
            code: "ERR_MSA_OAUTH_REFRESH".into(),
            message: format!("OAuth refresh failed: {}", e),
        })?;

    let status = resp.status();
    if !status.is_success() {
        let _body = resp.text().await.unwrap_or_default();
        return Err(LauncherError::Generic {
            code: "ERR_MSA_OAUTH_REFRESH_HTTP".into(),
            message: format!("OAuth refresh endpoint returned HTTP {} (response suppressed)", status),
        });
    }

    resp.json::<OAuthToken>().await.map_err(|e| LauncherError::Generic {
        code: "ERR_MSA_OAUTH_REFRESH_PARSE".into(),
        message: format!("Failed to parse refreshed OAuth token: {}", e),
    })
}

// ---------------------------------------------------------------------------
// Steps 5-6: SISU authorize → XSTS
// ---------------------------------------------------------------------------

async fn sisu_authorize(
    client: &reqwest::Client,
    session_id: Option<&str>,
    access_token: &str,
    device_token: &str,
    key: &DeviceTokenKey,
) -> LauncherResult<xbox_types::SisuAuthorize> {
    let body = serde_json::json!({
        "AccessToken": format!("t={}", access_token),
        "AppId": MICROSOFT_CLIENT_ID,
        "DeviceToken": device_token,
        "ProofKey": proof_key_json(key),
        "Sandbox": "RETAIL",
        "SessionId": session_id,
        "SiteName": "user.auth.xboxlive.com",
        "RelyingParty": "http://xboxlive.com",
        "UseModernGamertag": true
    });

    let (_, result) = send_signed_request(
        client, SISU_AUTHORIZE_URL, "/authorize",
        body, key, None, false,
    ).await?;
    Ok(result)
}

async fn xsts_authorize(
    client: &reqwest::Client,
    sisu: &xbox_types::SisuAuthorize,
    device_token: &str,
    key: &DeviceTokenKey,
    auth_date: DateTime<Utc>,
) -> LauncherResult<(String, String)> {
    let body = serde_json::json!({
        "RelyingParty": "rp://api.minecraftservices.com/",
        "TokenType": "JWT",
        "Properties": {
            "SandboxId": "RETAIL",
            "UserTokens": [sisu.user_token.token],
            "DeviceToken": device_token,
            "TitleToken": sisu.title_token.token
        }
    });

    let body_str = serde_json::to_string(&body).unwrap_or_default();
    let sig = sign_request(key, "/xsts/authorize", None, &body_str, auth_date);

    let resp = client
        .post(XSTS_AUTHORIZE_URL)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("Signature", &sig)
        .header("x-xbl-contract-version", "1")
        .body(body_str)
        .send()
        .await
        .map_err(|e| LauncherError::Generic {
            code: "ERR_MSA_XSTS".into(),
            message: format!("XSTS authorize failed: {}", e),
        })?;

    let status = resp.status();
    if !status.is_success() {
        let _body = resp.text().await.unwrap_or_default();
        return Err(LauncherError::Generic {
            code: "ERR_MSA_XSTS_HTTP".into(),
            message: format!("XSTS returned HTTP {} (response suppressed)", status),
        });
    }

    let token: xbox_types::XboxToken = resp.json().await.map_err(|e| LauncherError::Generic {
        code: "ERR_MSA_XSTS_PARSE".into(),
        message: format!("Failed to parse XSTS token: {}", e),
    })?;

    // Extract user hash (uhs) from display_claims
    let uhs = token
        .display_claims
        .get("xui")
        .and_then(|v| v.get(0))
        .and_then(|v| v.get("uhs"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| LauncherError::Generic {
            code: "ERR_MSA_NO_UHS".into(),
            message: "XSTS response did not contain user hash (uhs).".into(),
        })?
        .to_string();

    Ok((uhs, token.token))
}

// ---------------------------------------------------------------------------
// Steps 7-9: Minecraft token → entitlements → profile
// ---------------------------------------------------------------------------

async fn get_minecraft_token(
    client: &reqwest::Client,
    uhs: &str,
    xsts_token: &str,
) -> LauncherResult<String> {
    let body = serde_json::json!({
        "platform": "PC_LAUNCHER",
        "xtoken": format!("XBL3.0 x={};{}", uhs, xsts_token),
    });

    let resp = client
        .post(MC_LOGIN_URL)
        .header("Accept", "application/json")
        .header("User-Agent", USER_AGENT)
        .json(&body)
        .send()
        .await
        .map_err(|e| LauncherError::Generic {
            code: "ERR_MSA_MC_TOKEN".into(),
            message: format!("Minecraft login failed: {}", e),
        })?;

    let status = resp.status();
    if !status.is_success() {
        let _body = resp.text().await.unwrap_or_default();
        return Err(LauncherError::Generic {
            code: "ERR_MSA_MC_TOKEN_HTTP".into(),
            message: format!("Minecraft login returned HTTP {} (response suppressed)", status),
        });
    }

    let token: xbox_types::MinecraftToken = resp.json().await.map_err(|e| LauncherError::Generic {
        code: "ERR_MSA_MC_TOKEN_PARSE".into(),
        message: format!("Failed to parse Minecraft token: {}", e),
    })?;

    Ok(token.access_token)
}

async fn check_entitlements(
    client: &reqwest::Client,
    access_token: &str,
) -> LauncherResult<()> {
    let url = format!("{}?requestId={}", MC_ENTITLEMENTS_URL, Uuid::new_v4());
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .map_err(|e| LauncherError::Generic {
            code: "ERR_MSA_ENTITLEMENTS".into(),
            message: format!("Entitlements check failed: {}", e),
        })?;

    if !resp.status().is_success() {
        let _body = resp.text().await.unwrap_or_default();
        return Err(LauncherError::Generic {
            code: "ERR_MSA_NO_ENTITLEMENT".into(),
            message: "Your Microsoft account does not own Minecraft. Please purchase it at minecraft.net.".into(),
        });
    }

    Ok(())
}

async fn get_minecraft_profile(
    client: &reqwest::Client,
    access_token: &str,
) -> LauncherResult<(String, String)> {
    let resp = client
        .get(MC_PROFILE_URL)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .map_err(|e| LauncherError::Generic {
            code: "ERR_MSA_PROFILE".into(),
            message: format!("Profile fetch failed: {}", e),
        })?;

    let status = resp.status();
    if !status.is_success() {
        let _body = resp.text().await.unwrap_or_default();
        return Err(LauncherError::Generic {
            code: "ERR_MSA_PROFILE_HTTP".into(),
            message: format!("Profile endpoint returned HTTP {} (response suppressed)", status),
        });
    }

    let profile: xbox_types::MinecraftProfile = resp.json().await.map_err(|e| LauncherError::Generic {
        code: "ERR_MSA_PROFILE_PARSE".into(),
        message: format!("Failed to parse Minecraft profile: {}", e),
    })?;

    Ok((profile.name, profile.id))
}

// ---------------------------------------------------------------------------
// Public API: begin_login → finish_login → refresh → storage
// ---------------------------------------------------------------------------

/// Begin the MSA login flow. Returns a [`LoginFlow`] whose `auth_uri` should
/// be opened in a browser. The caller captures the `?code=` from the redirect
/// and passes it to [`finish_login`].
pub async fn begin_login(client: &reqwest::Client) -> LauncherResult<LoginFlow> {
    check_network_enabled("network_msa_enabled", "Microsoft account login is disabled in Privacy settings.")?;
    // Step 1: Generate p256 key + get device token
    let key = DeviceTokenKey::generate();
    let device_token = get_device_token(client, &key).await?;

    // Step 2: Generate PKCE challenge + SISU authenticate
    let verifier = generate_pkce_verifier();
    let challenge = pkce_challenge(&verifier);
    let (session_id, auth_uri, state) = sisu_authenticate(client, &device_token.token, &challenge, &key).await?;

    Ok(LoginFlow {
        auth_uri,
        verifier,
        session_id,
        key_json: key.to_json(),
        device_token: device_token.token,
        state,
    })
}

/// Complete the MSA login flow with the auth code from the browser redirect.
/// Runs steps 4-9 and returns persisted credentials.
///
/// The optional `state` parameter is passed from the browser redirect URL's
/// `?state=` query parameter. If it does not match the state generated during
/// `begin_login`, the login is rejected (CSRF protection).
pub async fn finish_login(
    client: &reqwest::Client,
    code: &str,
    flow: &LoginFlow,
    state: Option<&str>,
) -> LauncherResult<MsaCredentials> {
    check_network_enabled("network_msa_enabled", "Microsoft account login is disabled in Privacy settings.")?;
    // CSRF check: verify state parameter if provided and flow has one
    if let Some(passed_state) = state {
        if !flow.state.is_empty() && flow.state != passed_state {
            return Err(LauncherError::Generic {
                code: "ERR_MSA_STATE_MISMATCH".into(),
                message: "OAuth state parameter mismatch — possible CSRF attack. Aborting login.".into(),
            });
        }
    } else if !flow.state.is_empty() {
        return Err(LauncherError::Generic {
            code: "ERR_MSA_STATE_MISMATCH".into(),
            message: "OAuth state parameter missing — possible CSRF attack. Login aborted.".into(),
        });
    }

    let key = DeviceTokenKey::from_json(&flow.key_json)?;

    // Step 4: Exchange auth code for OAuth token
    let oauth = exchange_oauth_token(client, code, &flow.verifier).await?;
    let auth_date = Utc::now();

    // Step 5: SISU authorize
    let sisu = sisu_authorize(
        client,
        flow.session_id.as_deref(),
        &oauth.access_token,
        &flow.device_token,
        &key,
    ).await?;

    // Step 6: XSTS authorize → user hash + Xbox token
    let (uhs, xsts_token) = xsts_authorize(client, &sisu, &flow.device_token, &key, auth_date).await?;

    // Step 7: Minecraft login → access token
    let mc_access_token = get_minecraft_token(client, &uhs, &xsts_token).await?;

    // Step 8: Entitlements check
    check_entitlements(client, &mc_access_token).await?;

    // Step 9: Minecraft profile
    let (username, uuid) = get_minecraft_profile(client, &mc_access_token).await?;

    let creds = MsaCredentials {
        username,
        uuid,
        access_token: mc_access_token,
        refresh_token: oauth.refresh_token,
        expires: auth_date + chrono::Duration::seconds(oauth.expires_in as i64),
    };

    store_credentials(&creds)?;
    Ok(creds)
}

/// Refresh expired credentials using the refresh token.
/// Runs steps 4(refresh), 5, 6, 7.
pub async fn refresh_credentials(
    client: &reqwest::Client,
    creds: &MsaCredentials,
) -> LauncherResult<MsaCredentials> {
    check_network_enabled("network_msa_enabled", "Microsoft account login is disabled in Privacy settings.")?;
    // Step 4 (refresh): Get new OAuth token from refresh token
    let oauth = refresh_oauth_token(client, &creds.refresh_token).await?;
    let auth_date = Utc::now();

    // Regenerate device token + key (simpler + more secure than caching)
    let key = DeviceTokenKey::generate();
    let device_token = get_device_token(client, &key).await?;

    // Steps 5-7: SISU authorize → XSTS → Minecraft token
    let sisu = sisu_authorize(client, None, &oauth.access_token, &device_token.token, &key).await?;
    let (uhs, xsts_token) = xsts_authorize(client, &sisu, &device_token.token, &key, auth_date).await?;
    let mc_access_token = get_minecraft_token(client, &uhs, &xsts_token).await?;

    let refreshed = MsaCredentials {
        username: creds.username.clone(),
        uuid: creds.uuid.clone(),
        access_token: mc_access_token,
        refresh_token: oauth.refresh_token,
        expires: auth_date + chrono::Duration::seconds(oauth.expires_in as i64),
    };

    store_credentials(&refreshed)?;
    Ok(refreshed)
}

/// Get stored credentials from the OS keyring, or None if not authenticated.
pub fn load_credentials() -> LauncherResult<Option<MsaCredentials>> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)
        .map_err(|e| LauncherError::Generic {
            code: "ERR_MSA_KEYRING".into(),
            message: format!("Failed to access keyring: {}", e),
        })?;

    match entry.get_password() {
        Ok(json) => {
            let creds: MsaCredentials = serde_json::from_str(&json).map_err(|e| LauncherError::Generic {
                code: "ERR_MSA_STORED_PARSE".into(),
                message: format!("Failed to parse stored credentials: {}", e),
            })?;
            Ok(Some(creds))
        }
        Err(e) => {
            if matches!(e, keyring::Error::NoEntry) {
                Ok(None)
            } else {
                Err(LauncherError::Generic {
                    code: "ERR_MSA_KEYRING_READ".into(),
                    message: format!("Failed to read keyring: {}", e),
                })
            }
        }
    }
}

/// Store credentials in the OS keyring as JSON.
pub fn store_credentials(creds: &MsaCredentials) -> LauncherResult<()> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)
        .map_err(|e| LauncherError::Generic {
            code: "ERR_MSA_KEYRING".into(),
            message: format!("Failed to access keyring: {}", e),
        })?;

    let json = serde_json::to_string(creds).unwrap_or_default();
    entry.set_password(&json).map_err(|e| LauncherError::Generic {
        code: "ERR_MSA_KEYRING_WRITE".into(),
        message: format!("Failed to write credentials to keyring: {}", e),
    })?;
    Ok(())
}

/// Clear stored MSA credentials (sign out).
pub fn clear_credentials() -> LauncherResult<()> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)
        .map_err(|e| LauncherError::Generic {
            code: "ERR_MSA_KEYRING".into(),
            message: format!("Failed to access keyring: {}", e),
        })?;

    match entry.delete_password() {
        Ok(_) => Ok(()),
        Err(e) => {
            if matches!(e, keyring::Error::NoEntry) {
                Ok(())
            } else {
                Err(LauncherError::Generic {
                    code: "ERR_MSA_KEYRING_DELETE".into(),
                    message: format!("Failed to delete credentials: {}", e),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_verifier_is_hex_and_128_chars() {
        let v = generate_pkce_verifier();
        assert_eq!(v.len(), 128);
        assert!(v.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn pkce_challenge_is_base64url() {
        let v = generate_pkce_verifier();
        let c = pkce_challenge(&v);
        assert!(!c.is_empty());
        assert!(!c.contains('='));
    }

    #[test]
    fn device_token_key_roundtrip() {
        let key = DeviceTokenKey::generate();
        let json = key.to_json();
        let restored = DeviceTokenKey::from_json(&json).unwrap();
        assert_eq!(key.x, restored.x);
        assert_eq!(key.y, restored.y);
        assert_eq!(key.id, restored.id);
    }

    #[test]
    fn credentials_expiry_logic() {
        let creds = MsaCredentials {
            username: "test".into(),
            uuid: "abc".into(),
            access_token: "tok".into(),
            refresh_token: "ref".into(),
            expires: Utc::now() - chrono::Duration::hours(1),
        };
        assert!(creds.is_expired());
        assert!(creds.needs_refresh());

        let future = MsaCredentials {
            username: "test".into(),
            uuid: "abc".into(),
            access_token: "tok".into(),
            refresh_token: "ref".into(),
            expires: Utc::now() + chrono::Duration::hours(1),
        };
        assert!(!future.is_expired());
        assert!(!future.needs_refresh());
    }
}

