use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::db;
use crate::error::{LauncherError, LauncherResult};

const COPILOT_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
const COPILOT_DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const COPILOT_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const COPILOT_USER_URL: &str = "https://api.github.com/user";
const COPILOT_INTERNAL_USER_URL: &str = "https://api.github.com/copilot_internal/user";
const COPILOT_TOKEN_EXCHANGE_URL: &str = "https://api.github.com/copilot_internal/v2/token";
const COPILOT_INDIVIDUAL_API_BASE: &str = "https://api.individual.githubcopilot.com";
const COPILOT_ENTERPRISE_API_BASE: &str = "https://api.githubcopilot.com";
const COPILOT_KEYRING_SERVICE: &str = "agora.copilot";
const COPILOT_KEYRING_ACCOUNT: &str = "token";
/// Conservative fallback for how long a Copilot session token (from the
/// `copilot_internal/v2/token` exchange) stays valid, used only when the
/// exchange response doesn't include its own `expires_at`. Real tokens are
/// typically good for ~25-30 minutes; we refresh a little early to be safe.
const COPILOT_SESSION_TOKEN_TTL_MINUTES: i64 = 20;

/// Check a network enable setting from the local state DB.
/// If the DB file doesn't exist yet, the feature is allowed (default-enabled).
fn check_network_enabled(setting_key: &str, disabled_msg: &str) -> LauncherResult<()> {
    let app_data_dir = match dirs::data_local_dir() {
        Some(d) => d.join("agora"),
        None => {
            return Err(LauncherError::Generic {
                code: "ERR_NO_DATA_DIR".into(),
                message: "Could not determine local data directory.".into(),
            })
        }
    };
    let db_path = app_data_dir.join("local_state.db");
    if !db_path.exists() {
        // DB hasn't been initialised yet — feature is enabled by default.
        return Ok(());
    }
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatResponse {
    pub content: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AiContext {
    pub instance_id: Option<String>,
    pub crash_log: Option<String>,
    pub crash_signatures: Option<String>,
    pub suspects: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopilotDeviceFlowResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CopilotToken {
    pub access_token: String,
    pub copilot_token: Option<String>,
    pub endpoint: String,
    pub plan: String,
    pub username: String,
    pub stored_at: chrono::DateTime<chrono::Utc>,
    /// When `copilot_token` stops being valid, if known. `None` for tokens
    /// stored by a pre-patch version of Agora (hence `serde(default)`), or
    /// when we fell back to the raw OAuth token, which doesn't expire on
    /// this timescale.
    #[serde(default)]
    pub copilot_token_expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl std::fmt::Debug for CopilotToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CopilotToken")
            .field("access_token", &"[REDACTED]")
            .field("copilot_token", &"[REDACTED]")
            .field("endpoint", &self.endpoint)
            .field("plan", &self.plan)
            .field("username", &self.username)
            .field("stored_at", &self.stored_at)
            .field("copilot_token_expires_at", &self.copilot_token_expires_at)
            .finish()
    }
}

pub fn build_system_prompt(mcp_skill_content: &str) -> String {
    format!(
        "You are Agora's built-in AI assistant for Minecraft modding. \
         You help users find mods, packs, shaders, resource packs, servers, \
         and datapacks, recommend settings and configurations, and diagnose crashes.\n\n\
         ## Agora Tools\n\n\
         The following tools are available via the Agora MCP server. \
         In this chat interface, you don't call tools directly — instead, \
         use your training knowledge to give the user a clear recommendation.\n\n\
         {}\n\n\
         ## Capabilities\n\
         - **Finding content**: Recommend mods, modpacks, shaders, resource packs, \
         servers, and datapacks for what the user wants to do (e.g. \"I want \
         better inventory management\", \"something that makes the Nether more \
         interesting\"). Be specific about what each option does.\n\
         - **Configuration advice**: Suggest settings changes, config tweaks, \
         and performance optimizations. If the user mentions a mod you know, \
         recommend relevant in-game or file-based settings.\n\
         - **Mod compatibility**: Flag known conflicts between mods and suggest \
         alternatives or fixes.\n\
         - **Crash diagnosis**: When a crash log is provided, identify the \
         likely cause and recommend a fix.\n\n\
         ## Guidelines\n\
         - Be concise but thorough. Lead with your best recommendation first.\n\
         - When you identify a suspect mod in a crash, explain WHY (cite the signal evidence).\n\
         - If you are unsure about a recommendation, suggest the user connect \
         an AI agent via the MCP server (Settings → AI / MCP Server → Enable), \
         which gives the agent direct control over Agora to list instances, \
         browse the registry, disable mods, and analyze crashes on its own. \
         Offer to explain the setup steps if they want them.\n\
         - When recommending a mod from the Agora registry, mention it is \
         community-curated. Suggest the user search for it in the Browse tab.\n\
         - If no mod-related cause is found in a crash, suggest other \
         possibilities (game engine, world corruption, shaders, GPU drivers, etc.).\n\n\
         - Refer to the below MCP setup guide whenever you discuss the MCP server or AI agents. 
         Do not assume the user knows what MCP, AI agents, or SSE are. Provide clear instructions 
         for enabling the server, obtaining the Bearer token, and configuring their AI client.\n\n\
         - Suggest Claude Desktop (https://claude.com/download) or Opencode Desktop (https://opencode.ai/download) 
         as some decent recommendations for AI clients, but suggest that the user research and choose their own 
         MCP capable AI client if they prefer. Do not assume the user knows what these are, and provide links to 
         their official websites if possible.\n\n\
         ## MCP Setup Guide\n\n\
         When you suggest the user connect an AI agent via the MCP server, \
         include this setup information. The user needs to:\n\n\
         1. **Enable the server** — Open Agora Settings, find \"AI / MCP Server\", \
         and toggle it on. This starts a local HTTP server on `127.0.0.1:39741`.\n\n\
         2. **Get the Bearer token** — Once enabled, go to Settings → Integrations → \
         MCP Server. A token is generated automatically and displayed there. \
         Copy it — every AI client that connects needs this token for authentication.\n\n\
         3. **Configure the AI client** — Add Agora's MCP server URL using the \
         SSE transport. The URL is `http://127.0.0.1:39741/sse` with an \
         `Authorization: Bearer <token>` header. For Claude Desktop, add this to \
         `claude_desktop_config.json` under `mcpServers`:\n\
         ```json\n\
         {{\"mcpServers\": {{\"agora\": {{\"url\": \"http://127.0.0.1:39741/sse\", \"headers\": {{\"Authorization\": \"Bearer <your-token>\"}}}}}}}}\n\
         ```\n\
         For Cursor or other MCP-compatible clients, use the same URL and \
         Authorization header pattern. Kilo Code users can find the Agora MCP \
         client setting in `.kilo/kilo.json` under `mcpServers`.\n\n\
         4. **Approve destructive actions** — The agent can disable/enable mods \
         for testing, but this requires per-instance approval. In Settings → MCP → \
         Approvals, grant `disable_mod` permission for the instances the agent \
         should manage. The agent will tell the user if permission is missing.\n\n\
         5. **Restart Agora** — MCP connections are established at app startup. \
         If you just enabled MCP, restart Agora for the server to listen.\n\n\
         **What the agent can do:** 10 tools — list instances and their mods, \
         read crash logs, search crash signatures, rank suspect mods by a \
         weighted scoring algorithm, disable/enable mods, read curated mod \
         manifests from the registry, search the knowledge base by name or \
         description, and get a full system context overview. All tools run \
         locally with zero network calls.\n\n\
         **Security:** The server only accepts connections from `127.0.0.1` \
         (loopback). The Bearer token is the primary authentication boundary. \
         Rate limit is 100 requests per 60 seconds per session.",
        mcp_skill_content
    )
}

/// Start the GitHub Copilot device code flow.
pub async fn start_copilot_flow(client: &reqwest::Client) -> LauncherResult<CopilotDeviceFlowResponse> {
    check_network_enabled("network_github_oauth_enabled", "GitHub Copilot is disabled in Privacy settings.")?;
    let params = [
        ("client_id", COPILOT_CLIENT_ID),
        ("scope", "read:user"),
    ];

    let resp = client
        .post(COPILOT_DEVICE_CODE_URL)
        .header("Accept", "application/json")
        .form(&params)
        .send()
        .await
        .map_err(|_| LauncherError::NetworkOffline)?;

    let status = resp.status();
    if !status.is_success() {
        return Err(LauncherError::Generic {
            code: "ERR_COPILOT_DEVICE_FLOW".to_string(),
            message: format!("Device code flow returned HTTP {}", status.as_u16()),
        });
    }

    resp.json::<CopilotDeviceFlowResponse>().await.map_err(|e| LauncherError::Generic {
        code: "ERR_COPILOT_DEVICE_FLOW_PARSE".to_string(),
        message: format!("Failed to parse device flow response: {}", e),
    })
}

/// Poll the device flow until the user approves or it expires.
pub async fn poll_copilot_flow(
    client: &reqwest::Client,
    device_code: &str,
    interval: u64,
) -> LauncherResult<String> {
    check_network_enabled("network_github_oauth_enabled", "GitHub Copilot is disabled in Privacy settings.")?;
    let params = [
        ("client_id", COPILOT_CLIENT_ID),
        ("device_code", device_code),
        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
    ];

    let mut current_interval = interval;

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(current_interval)).await;

        let resp = client
            .post(COPILOT_TOKEN_URL)
            .header("Accept", "application/json")
            .form(&params)
            .send()
            .await
            .map_err(|_| LauncherError::NetworkOffline)?;

        let status = resp.status();
        if !status.is_success() {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let error = body.get("error").and_then(|v| v.as_str()).unwrap_or("");

            match error {
                "authorization_pending" => continue,
                "slow_down" => {
                    current_interval = current_interval.saturating_add(5);
                    continue;
                }
                "expired_token" => {
                    return Err(LauncherError::Generic {
                        code: "ERR_COPILOT_FLOW_EXPIRED".to_string(),
                        message: "Device code expired. Please restart the login process.".to_string(),
                    });
                }
                "access_denied" => {
                    return Err(LauncherError::Generic {
                        code: "ERR_COPILOT_FLOW_DENIED".to_string(),
                        message: "Login cancelled by user.".to_string(),
                    });
                }
                _ => {
                    return Err(LauncherError::Generic {
                        code: "ERR_COPILOT_FLOW_ERROR".to_string(),
                        message: format!("Device flow error: {}", error),
                    });
                }
            }
        }

        let body: serde_json::Value = resp.json().await.map_err(|e| LauncherError::Generic {
            code: "ERR_COPILOT_POLL_PARSE".to_string(),
            message: format!("Failed to parse poll response: {}", e),
        })?;

        if let Some(token) = body.get("access_token").and_then(|v| v.as_str()) {
            return Ok(token.to_string());
        }

        let error = body.get("error").and_then(|v| v.as_str()).unwrap_or("unknown");
        match error {
            "authorization_pending" => continue,
            "slow_down" => {
                current_interval = current_interval.saturating_add(5);
                continue;
            }
            "expired_token" => {
                return Err(LauncherError::Generic {
                    code: "ERR_COPILOT_FLOW_EXPIRED".to_string(),
                    message: "Device code expired. Please restart the login process.".to_string(),
                });
            }
            "access_denied" => {
                return Err(LauncherError::Generic {
                    code: "ERR_COPILOT_FLOW_DENIED".to_string(),
                    message: "Login cancelled by user.".to_string(),
                });
            }
            _ => {
                return Err(LauncherError::Generic {
                    code: "ERR_COPILOT_FLOW_ERROR".to_string(),
                    message: format!("Device flow error: {}", error),
                });
            }
        }
    }
}

/// Detect which Copilot endpoint to use and resolve the full token.
///
/// This mirrors the handshake Copilot's own editor integrations perform:
/// 1. `GET copilot_internal/user` to read the account's plan, its
///    entitlement API host, and whether Copilot Chat is enabled at all.
/// 2. Exchange the long-lived GitHub OAuth token for a short-lived Copilot
///    session token via `GET copilot_internal/v2/token`.
///
/// Step 2 used to only run for Business/Enterprise plans here, which is why
/// Free/individual accounts never ended up with a usable token: the
/// chat/completions endpoints reject a raw GitHub OAuth token on every
/// plan, not just the paid org ones. We now attempt the exchange
/// unconditionally. A handful of individual-plan users have reported this
/// endpoint 404ing for their account while GitHub reworks its Copilot plan
/// structure — if that happens, we fall back to the raw OAuth token against
/// the account's own entitlement endpoint instead of failing outright.
pub async fn resolve_copilot_endpoint(
    client: &reqwest::Client,
    ghu_token: &str,
) -> LauncherResult<CopilotToken> {
    check_network_enabled("network_github_oauth_enabled", "GitHub Copilot is disabled in Privacy settings.")?;
    let resp = client
        .get(COPILOT_INTERNAL_USER_URL)
        .header("Authorization", format!("Bearer {}", ghu_token))
        .header("Accept", "application/json")
        .header("User-Agent", "Agora-Launcher/1.0")
        .header("Editor-Version", "vscode/1.95.0")
        .send()
        .await
        .map_err(|_| LauncherError::NetworkOffline)?;

    let status = resp.status();
    if !status.is_success() {
        return Err(LauncherError::Generic {
            code: "ERR_COPILOT_INTERNAL_USER".to_string(),
            message: format!("Copilot internal user endpoint returned HTTP {}", status.as_u16()),
        });
    }

    let internal_user: serde_json::Value = resp.json().await.map_err(|e| LauncherError::Generic {
        code: "ERR_COPILOT_INTERNAL_USER_PARSE".to_string(),
        message: format!("Failed to parse copilot_internal/user response: {}", e),
    })?;

    let plan = internal_user
        .get("copilot_plan")
        .and_then(|v| v.as_str())
        .unwrap_or("free")
        .to_string();

    // A brand-new Free-tier signup may not have accepted the Copilot Chat
    // terms yet. Catch that here with an actionable message instead of
    // letting it surface later as a confusing 401/403 from completions.
    if internal_user.get("chat_enabled").and_then(|v| v.as_bool()) == Some(false) {
        return Err(LauncherError::Generic {
            code: "ERR_COPILOT_CHAT_DISABLED".to_string(),
            message: "Copilot Chat isn't enabled for this GitHub account yet. Enable it at github.com/settings/copilot, then try again.".to_string(),
        });
    }

    // Prefer the host GitHub actually hands us for this account over a
    // hardcoded guess — plan-to-domain mapping has shifted before, and
    // GitHub is actively restructuring individual plans as of 2026.
    let api_base = internal_user
        .get("endpoints")
        .and_then(|e| e.get("api"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim_end_matches('/').to_string())
        .unwrap_or_else(|| match plan.as_str() {
            "business" | "enterprise" => COPILOT_ENTERPRISE_API_BASE.to_string(),
            _ => COPILOT_INDIVIDUAL_API_BASE.to_string(),
        });

    eprintln!("[copilot] plan={plan} api_base={api_base}");

    // Always attempt the session-token exchange, regardless of plan — see
    // the doc comment above.
    let exchange_resp = client
        .post(COPILOT_TOKEN_EXCHANGE_URL)
        .header("Authorization", format!("token {}", ghu_token))
        .header("Accept", "application/json")
        .header("User-Agent", "GitHubCopilotChat/1.95.0")
        .header("Editor-Version", "vscode/1.95.0")
        .send()
        .await
        .map_err(|_| LauncherError::NetworkOffline)?;

    let exchange_status = exchange_resp.status();
    let (copilot_token, copilot_token_expires_at) = if exchange_status.is_success() {
        eprintln!("[copilot] token exchange OK — using Copilot session token");
        let token_json: serde_json::Value = exchange_resp.json().await.map_err(|e| LauncherError::Generic {
            code: "ERR_COPILOT_TOKEN_EXCHANGE_PARSE".to_string(),
            message: format!("Failed to parse token exchange response: {}", e),
        })?;

        let session_token = token_json
            .get("token")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if session_token.is_empty() {
            return Err(LauncherError::Generic {
                code: "ERR_COPILOT_TOKEN_EXCHANGE_EMPTY".to_string(),
                message: "Copilot token exchange succeeded but returned no token.".to_string(),
            });
        }

        let expires_at = token_json
            .get("expires_at")
            .and_then(|v| v.as_i64())
            .and_then(|secs| chrono::DateTime::from_timestamp(secs, 0))
            .unwrap_or_else(|| {
                chrono::Utc::now() + chrono::Duration::minutes(COPILOT_SESSION_TOKEN_TTL_MINUTES)
            });

        (Some(session_token), Some(expires_at))
    } else if exchange_status == reqwest::StatusCode::NOT_FOUND {
        let body_text = exchange_resp.text().await.unwrap_or_default();
        eprintln!("[copilot] token exchange returned 404 body={}", body_text);
        (None, None)
    } else {
        let body_text = exchange_resp.text().await.unwrap_or_default();
        eprintln!("[copilot] token exchange returned {} body={}", exchange_status.as_u16(), body_text);
        return Err(LauncherError::Generic {
            code: "ERR_COPILOT_TOKEN_EXCHANGE".to_string(),
            message: format!("Token exchange returned HTTP {}", exchange_status.as_u16()),
        });
    };

    let endpoint = format!("{}/chat/completions", api_base);

    let resp = client
        .get(COPILOT_USER_URL)
        .header("Authorization", format!("Bearer {}", ghu_token))
        .header("Accept", "application/json")
        .header("User-Agent", "Agora-Launcher/1.0")
        .send()
        .await
        .map_err(|_| LauncherError::NetworkOffline)?;

    let status = resp.status();
    if !status.is_success() {
        return Err(LauncherError::Generic {
            code: "ERR_COPILOT_USER".to_string(),
            message: format!("GitHub user endpoint returned HTTP {}", status.as_u16()),
        });
    }

    let user_json: serde_json::Value = resp.json().await.map_err(|e| LauncherError::Generic {
        code: "ERR_COPILOT_USER_PARSE".to_string(),
        message: format!("Failed to parse user response: {}", e),
    })?;

    let username = user_json
        .get("login")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    Ok(CopilotToken {
        access_token: ghu_token.to_string(),
        copilot_token,
        endpoint,
        plan,
        username,
        stored_at: chrono::Utc::now(),
        copilot_token_expires_at,
    })
}

/// Store the Copilot token in the OS keyring.
pub fn store_copilot_token(token: &CopilotToken) -> LauncherResult<()> {
    let entry = keyring::Entry::new(COPILOT_KEYRING_SERVICE, COPILOT_KEYRING_ACCOUNT)
        .map_err(|e| LauncherError::Generic {
            code: "ERR_COPILOT_KEYRING".to_string(),
            message: format!("Failed to access keyring: {}", e),
        })?;

    let json = serde_json::to_string(token).unwrap_or_default();
    entry.set_password(&json).map_err(|e| LauncherError::Generic {
        code: "ERR_COPILOT_KEYRING_WRITE".to_string(),
        message: format!("Failed to write token to keyring: {}", e),
    })?;

    Ok(())
}

/// Load the stored Copilot token from the OS keyring, if any.
pub fn load_copilot_token() -> LauncherResult<Option<CopilotToken>> {
    let entry = keyring::Entry::new(COPILOT_KEYRING_SERVICE, COPILOT_KEYRING_ACCOUNT)
        .map_err(|e| LauncherError::Generic {
            code: "ERR_COPILOT_KEYRING".to_string(),
            message: format!("Failed to access keyring: {}", e),
        })?;

    match entry.get_password() {
        Ok(json) => {
            let token: CopilotToken = serde_json::from_str(&json).map_err(|e| LauncherError::Generic {
                code: "ERR_COPILOT_STORED_PARSE".to_string(),
                message: format!("Failed to parse stored token: {}", e),
            })?;
            Ok(Some(token))
        }
        Err(e) => {
            if matches!(e, keyring::Error::NoEntry) {
                Ok(None)
            } else {
                Err(LauncherError::Generic {
                    code: "ERR_COPILOT_KEYRING_READ".to_string(),
                    message: format!("Failed to read keyring: {}", e),
                })
            }
        }
    }
}

/// Clear the stored Copilot token from the OS keyring.
pub fn clear_copilot_token() -> LauncherResult<()> {
    let entry = keyring::Entry::new(COPILOT_KEYRING_SERVICE, COPILOT_KEYRING_ACCOUNT)
        .map_err(|e| LauncherError::Generic {
            code: "ERR_COPILOT_KEYRING".to_string(),
            message: format!("Failed to access keyring: {}", e),
        })?;

    match entry.delete_password() {
        Ok(_) => Ok(()),
        Err(e) => {
            if matches!(e, keyring::Error::NoEntry) {
                Ok(())
            } else {
                Err(LauncherError::Generic {
                    code: "ERR_COPILOT_KEYRING_DELETE".to_string(),
                    message: format!("Failed to delete token: {}", e),
                })
            }
        }
    }
}

pub async fn chat_completion(
    messages: Vec<ChatMessage>,
    token: &CopilotToken,
) -> LauncherResult<ChatResponse> {
    check_network_enabled("network_github_oauth_enabled", "GitHub Copilot is disabled in Privacy settings.")?;

    let client = reqwest::Client::builder()
        .user_agent("GitHubCopilotChat/1.95.0") // Keep verified extension UA to ensure modern routing
        .build()
        .map_err(|_| LauncherError::Generic {
            code: "ERR_AI_HTTP_CLIENT".to_string(),
            message: "Failed to build HTTP client for Copilot.".to_string(),
        })?;

    let mut token = token.clone();
    let needs_refresh = token.copilot_token.is_some()
        && token
            .copilot_token_expires_at
            .map(|exp| chrono::Utc::now() + chrono::Duration::minutes(2) >= exp)
            .unwrap_or(false);

    if needs_refresh {
        if let Ok(refreshed) = resolve_copilot_endpoint(&client, &token.access_token).await {
            let _ = store_copilot_token(&refreshed);
            token = refreshed;
        }
    }

    // Direct assignment instead of candidate arrays and loops
    let model = "gpt-4o";
    let auth_token = token.copilot_token.as_deref().unwrap_or(&token.access_token);
    let auth_source = if token.copilot_token.is_some() { "session" } else { "oauth" };

    let body = serde_json::json!({
        "messages": messages,
        "model": model,
        "temperature": 0.3,
        "max_tokens": 2000,
    });

    eprintln!(
        "[copilot] POST {} model={} plan={} auth={} token_age={}s",
        token.endpoint,
        model,
        token.plan,
        auth_source,
        chrono::Utc::now()
            .signed_duration_since(token.stored_at)
            .num_seconds(),
    );

    let resp = client
        .post(&token.endpoint)
        .header("Authorization", format!("Bearer {}", auth_token))
        .header("Editor-Version", "vscode/1.95.0")
        .header("User-Agent", "GitHubCopilotChat/1.95.0")
        .header("Openai-Intent", "conversation-edits")
        .header("X-Initiator", "agent")
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|_| LauncherError::NetworkOffline)?;

    let status = resp.status();
    eprintln!("[copilot] response status={}", status.as_u16());

    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(LauncherError::Generic {
            code: "ERR_AI_RATE_LIMIT".to_string(),
            message: format!(
                "You've reached the usage limit for your Copilot {} plan for this billing period.",
                token.plan
            ),
        });
    }

    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(LauncherError::Generic {
            code: "ERR_AI_AUTH_EXPIRED".to_string(),
            message: "GitHub Copilot token expired. Please re-login.".to_string(),
        });
    }

    if status.is_success() {
        let parsed = resp
            .json::<serde_json::Value>()
            .await
            .map_err(|_| LauncherError::Generic {
                code: "ERR_AI_PARSE".to_string(),
                message: "Failed to parse Copilot response.".to_string(),
            })?;

        let content = parsed
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();

        let response_model = parsed
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or("copilot")
            .to_string();

        return Ok(ChatResponse {
            content,
            model: response_model,
        });
    }

    // Handle generic failures cleanly without loop fallbacks
    let body_text = resp.text().await.unwrap_or_default();
    eprintln!("[copilot] error body: {}", body_text);

    Err(LauncherError::Generic {
        code: "ERR_AI_REQUEST".to_string(),
        message: format!("Copilot returned status {}: {}", status.as_u16(), body_text),
    })
}

pub fn build_context_message(context: &AiContext) -> String {
    let mut parts: Vec<String> = Vec::new();

    if let Some(ref crash_log) = context.crash_log {
        parts.push(format!("## Crash Log\n\n```\n{}\n```", crash_log));
    }

    if let Some(ref crash_signatures) = context.crash_signatures {
        parts.push(format!(
            "## Curated Crash Signatures Matched\n\n{}",
            crash_signatures
        ));
    }

    if let Some(ref suspects) = context.suspects {
        parts.push(format!("## Ranked Suspect Mods\n\n{}", suspects));
    }

    if parts.is_empty() {
        return "The user has not provided any crash data. Help them with their Minecraft modding questions — finding mods, configuration advice, or general recommendations.".to_string();
    }

    parts.push(
        "## Your Task\n\nBased on the above information, answer the user's question or diagnose the crash, recommending fixes or content as appropriate."
            .to_string(),
    );

    parts.join("\n\n")
}

pub fn build_context_message_with_app(
    manifest_path: Option<std::path::PathBuf>,
    context: &AiContext,
) -> String {
    let mut parts: Vec<String> = Vec::new();

    if let Some(ref crash_log) = context.crash_log {
        parts.push(format!("## Crash Log\n\n```\n{}\n```", crash_log));
    }

    if let Some(ref crash_signatures) = context.crash_signatures {
        parts.push(format!(
            "## Curated Crash Signatures Matched\n\n{}",
            crash_signatures
        ));
    }

    if let Some(ref suspects) = context.suspects {
        parts.push(format!("## Ranked Suspect Mods\n\n{}", suspects));
    }

    if let Some(ref manifest_path) = manifest_path {
        if manifest_path.exists() {
            if let Ok(text) = std::fs::read_to_string(manifest_path) {
                if let Ok(manifest) = serde_json::from_str::<crate::models::InstanceManifest>(&text) {
                    let mut mod_lines: Vec<String> = Vec::new();
                    for mod_ in &manifest.mods {
                        let ver = mod_.version.as_deref().unwrap_or("unknown");
                        mod_lines.push(format!(
                            "- {} v{} (source: {})",
                            mod_.filename, ver, mod_.source
                        ));
                    }
                    if !mod_lines.is_empty() {
                        parts.push(format!(
                            "## Instance Mods\n\n{}\n\n### {}\n\n{}",
                            mod_lines.join("\n"),
                            manifest.name,
                            context.instance_id.as_deref().unwrap_or("unknown"),
                        ));
                    }
                }
            }
        }
    }

    if parts.is_empty() {
        return "The user has not provided any crash data. Help them with their Minecraft modding questions — finding mods, configuration advice, or general recommendations.".to_string();
    }

    parts.push(
        "## Your Task\n\nBased on the above information, answer the user's question or diagnose the crash, recommending fixes or content as appropriate."

            .to_string(),
    );

    parts.join("\n\n")

}