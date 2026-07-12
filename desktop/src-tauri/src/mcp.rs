#![allow(dead_code)]

use std::borrow::Cow;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

use crate::crash_diagnostics;
use crate::crash_investigator;
use crate::db;
use crate::error::LauncherError;
use crate::instances;
use crate::models::InstanceManifest;
use crate::paths;
use crate::registry;

// ---------------------------------------------------------------------------
// Baked-in MCP skill guide
// ---------------------------------------------------------------------------

/// The Agora MCP skill guide, baked into the app so users can copy it
/// from Settings without finding it on disk.
pub const MCP_SKILL_CONTENT: &str = include_str!("../skills/agora-mcp/SKILL.md");

// ---------------------------------------------------------------------------
// Session ID generation (no uuid crate â€” SystemTime + counter)
// ---------------------------------------------------------------------------

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

fn generate_session_id() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let counter = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:016x}{:016x}", secs, counter)
}

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    method: String,
    #[serde(default)]
    params: Option<serde_json::Value>,
    #[serde(default)]
    id: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
    id: serde_json::Value,
}

impl JsonRpcResponse {
    fn success(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: Some(result),
            error: None,
            id,
        }
    }

    fn error(id: serde_json::Value, code: i32, message: &str) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.to_string(),
            }),
            id,
        }
    }
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

const JSONRPC_ERROR_METHOD_NOT_FOUND: i32 = -32601;
const JSONRPC_ERROR_INTERNAL_ERROR: i32 = -32603;
const MCP_ERR_TOO_MANY_REQUESTS: &str = "ERR_MCP_TOO_MANY_REQUESTS";

// MCP error codes (application-level)
const MCP_ERR_DENIED: &str = "ERR_MCP_DENIED";
const MCP_TOKEN_KEY: &str = "mcp_bearer_token";

fn generate_token() -> String {
    use rand::Rng;
    let bytes: [u8; 32] = rand::thread_rng().gen();
    hex::encode(bytes)
}

fn get_or_create_mcp_token(app: &AppHandle) -> Option<String> {
    let conn = db::local_state_connection(app).ok()?;
    match db::get_setting(&conn, MCP_TOKEN_KEY) {
        Ok(Some(serde_json::Value::String(t))) if !t.is_empty() => Some(t),
        _ => {
            let token = generate_token();
            if db::set_setting(
                &conn,
                MCP_TOKEN_KEY,
                &serde_json::Value::String(token.clone()),
            )
            .is_ok()
            {
                if let Ok(app_data) = paths::app_data_dir(app) {
                    write_token_file(&app_data, &token);
                }
                Some(token)
            } else {
                None
            }
        }
    }
}

fn write_token_file(app_data_dir: &std::path::Path, token: &str) {
    let path = app_data_dir.join("mcp_token");
    if let Ok(mut f) = std::fs::File::create(&path) {
        let _ = std::io::Write::write_all(&mut f, token.as_bytes());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
        }
    }
}

fn read_stored_token(app: &AppHandle) -> Option<String> {
    let conn = db::local_state_connection(app).ok()?;
    match db::get_setting(&conn, MCP_TOKEN_KEY) {
        Ok(Some(serde_json::Value::String(t))) if !t.is_empty() => Some(t),
        _ => None,
    }
}

fn extract_bearer_token(
    headers: &std::collections::HashMap<String, String>,
    full_path: &str,
) -> Option<String> {
    if let Some(auth) = headers.get("authorization") {
        if let Some(t) = auth.strip_prefix("Bearer ") {
            return Some(t.trim().to_string());
        }
        if let Some(t) = auth.strip_prefix("bearer ") {
            return Some(t.trim().to_string());
        }
    }
    if let Some(q) = full_path.find('?') {
        for pair in full_path[q + 1..].split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                if k == "token" {
                    return urlencoding::decode(v).ok().map(|s| s.to_string());
                }
            }
        }
    }
    None
}

fn validate_token(
    app: &AppHandle,
    headers: &std::collections::HashMap<String, String>,
    full_path: &str,
) -> bool {
    match read_stored_token(app) {
        Some(expected) => match extract_bearer_token(headers, full_path) {
            Some(t) => t == expected,
            None => false,
        },
        None => false,
    }
}

// ---------------------------------------------------------------------------
// SSE session store
// ---------------------------------------------------------------------------

type SessionStore = Arc<std::sync::Mutex<HashMap<String, tokio::sync::mpsc::Sender<String>>>>;

// ---------------------------------------------------------------------------
// Rate limiter
// ---------------------------------------------------------------------------

struct RateLimiter {
    requests: Vec<u64>, // timestamps in seconds
}

impl RateLimiter {
    fn new() -> Self {
        Self {
            requests: Vec::new(),
        }
    }

    fn allow(&mut self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.requests.retain(|&t| now.wrapping_sub(t) < 60);
        if self.requests.len() >= 100 {
            return false;
        }
        self.requests.push(now);
        true
    }
}

// ---------------------------------------------------------------------------
// Approval check
// ---------------------------------------------------------------------------

/// Decision result for the pure approval logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApprovalResult {
    Allowed,
    Denied,
}

/// Pure approval decision: given a stored grant state and whether the tool
/// is destructive, decide if the call is allowed.
///
/// Read-only tools (is_destructive=false) are always allowed (safe default).
/// Destructive tools require an explicit grant.
fn check_approval_grant(state: Option<&str>, is_destructive: bool) -> ApprovalResult {
    if !is_destructive {
        return ApprovalResult::Allowed;
    }
    match state {
        Some("always_allow") | Some("session") => ApprovalResult::Allowed,
        Some("always_deny") => ApprovalResult::Denied,
        None => ApprovalResult::Denied,
        Some(_) => ApprovalResult::Denied,
    }
}

fn check_approval(
    app: &AppHandle,
    tool_name: &str,
    instance_id: &str,
    is_destructive: bool,
) -> Result<(), LauncherError> {
    if !is_destructive {
        return Ok(());
    }

    let conn = match db::local_state_connection(app) {
        Ok(c) => c,
        Err(_) => return Err(LauncherError::LocalStateFailed),
    };

    let mut stmt = match conn
        .prepare("SELECT state FROM mcp_approval_grants WHERE tool_name = ?1 AND instance_id = ?2")
    {
        Ok(s) => s,
        Err(_) => return Err(LauncherError::LocalStateFailed),
    };

    let state: Option<String> = match stmt.query_row([tool_name, instance_id], |row| row.get(0)) {
        Ok(v) => Some(v),
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        Err(_) => return Err(LauncherError::LocalStateFailed),
    };

    match check_approval_grant(state.as_deref(), is_destructive) {
        ApprovalResult::Allowed => Ok(()),
        ApprovalResult::Denied => {
            // Determine the specific denial reason for the error message.
            match state.as_deref() {
                Some("always_deny") => Err(LauncherError::McpDenied),
                None => Err(LauncherError::Generic {
                    code: MCP_ERR_DENIED.to_string(),
                    message: format!(
                        "Approval required: grant '{}' for instance '{}' in Agora Settings",
                        tool_name, instance_id
                    ),
                }),
                Some(other) => Err(LauncherError::Generic {
                    code: MCP_ERR_DENIED.to_string(),
                    message: format!("Unknown approval state: {}", other),
                }),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tool definitions
// ---------------------------------------------------------------------------

fn tool_definitions() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "name": "list_instances",
            "description": "List all Minecraft instances managed by Agora, including their IDs, names, and loader configurations.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
        serde_json::json!({
            "name": "list_instance_mods",
            "description": "List all installed mods for a specific instance, including filenames, versions, sources, and dependency information.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "instance_id": {
                        "type": "string",
                        "description": "The instance ID to list mods for."
                    }
                },
                "required": ["instance_id"]
            }
        }),
        serde_json::json!({
            "name": "disable_mod",
            "description": "Disable a mod in an instance by renaming its .jar file to .jar.disabled. Destructive â€” requires approval.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "instance_id": {
                        "type": "string",
                        "description": "The instance ID."
                    },
                    "filename": {
                        "type": "string",
                        "description": "The mod filename to disable."
                    }
                },
                "required": ["instance_id", "filename"]
            }
        }),
        serde_json::json!({
            "name": "search_crash_signatures",
            "description": "Search the curated crash signature database for patterns matching the provided crash text. Returns matching signatures and fix hints.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "crash_text": {
                        "type": "string",
                        "description": "The crash log text to search against."
                    }
                },
                "required": ["crash_text"]
            }
        }),
        serde_json::json!({
            "name": "suggest_mod_incompatibility",
            "description": "Analyze crash text against installed mods in an instance and return ranked suspect mods that may be causing the crash.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "instance_id": {
                        "type": "string",
                        "description": "The instance ID."
                    },
                    "crash_text": {
                        "type": "string",
                        "description": "The crash log text to analyze."
                    }
                },
                "required": ["instance_id", "crash_text"]
            }
        }),
        serde_json::json!({
            "name": "get_system_context",
            "description": "Return a markdown summary of the current Agora system state, including instances, installed mods, and recent crashes.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
        serde_json::json!({
            "name": "read_latest_crash",
            "description": "Read the most recent crash report or log for an instance. Returns the last 200 lines of the newest crash file.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "instance_id": {
                        "type": "string",
                        "description": "The instance ID to read crash reports for."
                    }
                },
                "required": ["instance_id"]
            }
        }),
        serde_json::json!({
            "name": "read_mod_manifest",
            "description": "Fetch curated metadata for a specific mod from the local SQLite registry, including curator notes, categories, compatibility data, and license info.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "mod_id": {
                        "type": "string",
                        "description": "The registry ID of the mod (e.g. 'sodium', 'iris')."
                    }
                },
                "required": ["mod_id"]
            }
        }),
        serde_json::json!({
            "name": "enable_mod",
            "description": "Re-enable a previously disabled mod in an instance by renaming its .jar.disabled file back to .jar. Destructive -- requires approval.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "instance_id": {
                        "type": "string",
                        "description": "The instance ID."
                    },
                    "filename": {
                        "type": "string",
                        "description": "The mod filename to re-enable."
                    }
                },
                "required": ["instance_id", "filename"]
            }
        }),
        serde_json::json!({
            "name": "search_knowledge_base",
            "description": "Search the curated registry for mods matching a natural-language query. Uses LIKE matching across mod names and descriptions in the local SQLite database. Returns the top 5 matches with their curator metadata.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Natural-language search string, e.g. 'performance rendering optimization' or 'magic mod'."
                    }
                },
                "required": ["query"]
            }
        }),
    ]
}

// ---------------------------------------------------------------------------
// Tool implementations
// ---------------------------------------------------------------------------

fn read_manifest(
    app: &AppHandle,
    instance_id: &str,
) -> Result<Option<InstanceManifest>, LauncherError> {
    let path = paths::instance_manifest_path(app, instance_id)
        .map_err(|_| LauncherError::LocalStateFailed)?;
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).map_err(|_| LauncherError::LocalStateFailed)?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|_| LauncherError::LocalStateFailed)
}

fn build_system_context(app: &AppHandle) -> String {
    let mut lines: Vec<String> = Vec::new();

    lines.push("# Agora System Context".to_string());
    lines.push("".to_string());

    // Instances
    lines.push("## Instances".to_string());
    let instance_rows = instances::list_instances(app);
    match instance_rows {
        Ok(rows) => {
            if rows.is_empty() {
                lines.push("No instances configured.".to_string());
            } else {
                for row in &rows {
                    lines.push(format!(
                        "- **{}** (`{}`) â€” Minecraft {}, Loader: {} {}",
                        row.name,
                        row.instance_id,
                        row.minecraft_version,
                        row.loader,
                        row.loader_version
                    ));
                }
            }
        }
        Err(e) => {
            lines.push(format!("Error listing instances: {}", e));
        }
    }
    lines.push("".to_string());

    // Installed mods
    lines.push("## Installed Mods".to_string());
    let instance_rows = instances::list_instances(app);
    match instance_rows {
        Ok(rows) => {
            let mut total_mods = 0usize;
            for row in &rows {
                let manifest = read_manifest(app, &row.instance_id);
                match manifest {
                    Ok(Some(m)) => {
                        lines.push(format!("- **{}** â€” {} mods", row.name, m.mods.len()));
                        total_mods += m.mods.len();
                        for mod_ in &m.mods {
                            let ver = mod_.version.as_deref().unwrap_or("unknown");
                            lines.push(format!(
                                "  - {} v{} (source: {})",
                                mod_.filename, ver, mod_.source
                            ));
                        }
                    }
                    Ok(None) => {
                        lines.push(format!("- **{}** â€” manifest not found", row.name));
                    }
                    Err(_) => {
                        lines.push(format!("- **{}** â€” could not read manifest", row.name));
                    }
                }
            }
            if total_mods == 0 && rows.is_empty() {
                lines.push("No installed mods.".to_string());
            }
        }
        Err(e) => {
            lines.push(format!("Error listing instances: {}", e));
        }
    }
    lines.push("".to_string());

    // Recent crashes
    lines.push("## Recent Crashes".to_string());
    let instance_rows = instances::list_instances(app);
    match instance_rows {
        Ok(rows) => {
            let mut found_crashes = false;
            for row in &rows {
                let instance_path = paths::instance_dir(app, &row.instance_id);
                if let Ok(dir) = instance_path {
                    let crash_dir = dir.join("crash-reports");
                    if crash_dir.exists() {
                        if let Ok(entries) = std::fs::read_dir(&crash_dir) {
                            let mut crash_files: Vec<_> = entries
                                .filter_map(|e| e.ok())
                                .filter(|e| {
                                    let fname = e.file_name();
                                    let name = fname.to_string_lossy();
                                    name.ends_with(".log") || name.ends_with(".txt")
                                })
                                .collect();
                            // Sort by modified time descending (newest first).
                            crash_files.sort_by(|a, b| {
                                let ma = a.metadata().and_then(|m| m.modified()).ok();
                                let mb = b.metadata().and_then(|m| m.modified()).ok();
                                mb.cmp(&ma) // descending
                            });
                            for entry in crash_files.iter().take(3) {
                                let fname = entry.file_name().to_string_lossy().to_string();
                                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                                lines.push(format!("- {} ({} bytes)", fname, size));
                                found_crashes = true;
                            }
                        }
                    }
                }
            }
            if !found_crashes {
                lines.push("No recent crash reports found.".to_string());
            }
        }
        Err(e) => {
            lines.push(format!("Error listing instances: {}", e));
        }
    }

    lines.join("\n")
}

fn handle_list_instances(app: &AppHandle) -> serde_json::Value {
    match instances::list_instances(app) {
        Ok(rows) => {
            let items: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|r| {
                    serde_json::json!({
                        "instance_id": r.instance_id,
                        "name": r.name,
                        "minecraft_version": r.minecraft_version,
                        "loader": r.loader,
                        "loader_version": r.loader_version,
                    })
                })
                .collect();
            serde_json::json!({ "instances": items })
        }
        Err(e) => serde_json::json!({ "error": "ERR_MCP_INTERNAL", "message": e.to_string() }),
    }
}

fn handle_list_instance_mods(app: &AppHandle, instance_id: &str) -> serde_json::Value {
    let manifest = match read_manifest(app, instance_id) {
        Ok(m) => m,
        Err(e) => {
            return serde_json::json!({
                "error": "ERR_MCP_INTERNAL",
                "message": e.to_string(),
            });
        }
    };
    match manifest {
        Some(m) => {
            let mods: Vec<serde_json::Value> = m
                .mods
                .into_iter()
                .map(|mod_| {
                    serde_json::json!({
                        "filename": mod_.filename,
                        "version": mod_.version,
                        "source": mod_.source,
                        "mod_jar_id": mod_.mod_jar_id,
                        "depends_on": mod_.depends_on,
                        "optional_deps": mod_.optional_deps,
                        "java_packages": mod_.java_packages,
                    })
                })
                .collect();
            serde_json::json!({ "mods": mods })
        }
        None => serde_json::json!({ "mods": [] }),
    }
}

fn handle_disable_mod(app: &AppHandle, instance_id: &str, filename: &str) -> serde_json::Value {
    match crash_investigator::disable_mod(app, instance_id, filename) {
        Ok(()) => serde_json::json!({
            "content": [{
                "type": "text",
                "text": format!("Mod {} disabled in instance {}. Restart the game to apply.", filename, instance_id),
            }],
            "isError": false,
        }),
        Err(e) => serde_json::json!({
            "content": [{
                "type": "text",
                "text": e.to_string(),
            }],
            "isError": true,
        }),
    }
}

// ---------------------------------------------------------------------------
// search_crash_signatures implementation
// ---------------------------------------------------------------------------

async fn handle_search_crash_signatures(app: &AppHandle, crash_text: &str) -> serde_json::Value {
    let app_clone = app.clone();
    let text = crash_text.to_string();
    let matches: Vec<serde_json::Value> = match tokio::task::spawn_blocking(move || {
        perform_signature_search(&app_clone, &text)
    })
    .await
    {
        Ok(Ok(m)) => m,
        Ok(Err(_)) | Err(_) => Vec::new(),
    };

    serde_json::json!({ "matches": matches })
}

fn perform_signature_search(
    app: &AppHandle,
    crash_text: &str,
) -> Result<Vec<serde_json::Value>, LauncherError> {
    let conn = registry::open_registry(app)?;
    let mut stmt = conn
        .prepare(
            "SELECT id, name, regex_pattern, solution_markdown \
         FROM crash_signatures",
        )
        .map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?;

    let rows = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let name: String = row.get(1)?;
            let pattern: String = row.get(2)?;
            let solution: String = row.get(3)?;
            Ok((id, name, pattern, solution))
        })
        .map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?;

    let mut matches: Vec<serde_json::Value> = Vec::new();
    for row in rows {
        let (_id, name, pattern, solution) = match row {
            Ok(v) => v,
            Err(_) => continue,
        };

        let regex = match regex::Regex::new(&pattern) {
            Ok(r) => r,
            Err(_) => continue, // Skip invalid patterns silently
        };

        if regex.is_match(crash_text) {
            matches.push(serde_json::json!({
                "id": _id,
                "name": name,
                "fix_hint": solution,
            }));
        }
    }

    Ok(matches)
}

// ---------------------------------------------------------------------------
// suggest_mod_incompatibility implementation
// ---------------------------------------------------------------------------

async fn suggest_mod_incompatibility_impl(
    app: &AppHandle,
    instance_id: &str,
    crash_text: &str,
) -> serde_json::Value {
    // Check for a parseable crash fingerprint first.
    if crash_investigator::parse_crash_log(crash_text).is_none() {
        return serde_json::json!({
            "content": [{
                "type": "text",
                "text": "No crash fingerprint detected in the provided text.",
            }],
            "isError": false,
        });
    }

    let manifest = match read_manifest(app, instance_id) {
        Ok(m) => m,
        Err(e) => {
            return serde_json::json!({
                "content": [{
                    "type": "text",
                    "text": e.to_string(),
                }],
                "isError": true,
            });
        }
    };
    let manifest = match manifest {
        Some(m) => m,
        None => {
            return serde_json::json!({
                "content": [{
                    "type": "text",
                    "text": format!("Instance '{}' not found", instance_id),
                }],
                "isError": true,
            });
        }
    };

    match tokio::runtime::Handle::try_current() {
        Ok(_handle) => {
            let app_clone = app.clone();
            let instance_id = instance_id.to_string();
            let text = crash_text.to_string();
            let mods: Vec<crate::models::InstalledMod> = manifest.mods.clone();
            match tokio::task::spawn_blocking(move || {
                crash_investigator::score_suspects(&app_clone, &instance_id, &text, &mods)
            })
            .await
            {
                Ok(Ok(suspects)) => {
                    let results: Vec<serde_json::Value> = suspects
                        .into_iter()
                        .map(|s| {
                            serde_json::json!({
                                "mod_id": s.mod_id,
                                "filename": s.filename,
                                "total_score": s.total_score,
                                "is_dependent_of": s.is_dependent_of,
                                "breakdown": s.breakdown,
                            })
                        })
                        .collect();
                    serde_json::json!({ "suspects": results })
                }
                Ok(Err(e)) => serde_json::json!({
                    "content": [{
                        "type": "text",
                        "text": e.to_string(),
                    }],
                    "isError": true,
                }),
                Err(_) => serde_json::json!({
                    "content": [{
                        "type": "text",
                        "text": "Scoring task panicked",
                    }],
                    "isError": true,
                }),
            }
        }
        Err(_) => {
            // No async runtime â€” run synchronously.
            match crash_investigator::score_suspects(app, instance_id, crash_text, &manifest.mods) {
                Ok(suspects) => {
                    let results: Vec<serde_json::Value> = suspects
                        .into_iter()
                        .map(|s| {
                            serde_json::json!({
                                "mod_id": s.mod_id,
                                "filename": s.filename,
                                "total_score": s.total_score,
                                "is_dependent_of": s.is_dependent_of,
                                "breakdown": s.breakdown,
                            })
                        })
                        .collect();
                    serde_json::json!({ "suspects": results })
                }
                Err(e) => serde_json::json!({
                    "content": [{
                        "type": "text",
                        "text": e.to_string(),
                    }],
                    "isError": true,
                }),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// read_latest_crash handler
// ---------------------------------------------------------------------------

fn handle_read_latest_crash(app: &AppHandle, instance_id: &str) -> serde_json::Value {
    let reports = match crash_diagnostics::list_crash_reports(app, instance_id) {
        Ok(r) => r,
        Err(e) => {
            return serde_json::json!({
                "content": [{"type": "text", "text": format!("Error listing crash reports: {}", e)}],
                "isError": true,
            })
        }
    };
    let newest = match reports.first() {
        Some(r) => r.filename.clone(),
        None => {
            return serde_json::json!({
                "content": [{"type": "text", "text": format!("No crash reports found for instance '{}'", instance_id)}],
                "isError": false,
            })
        }
    };
    let full = match crash_diagnostics::read_crash_log(app, instance_id, &newest) {
        Ok(t) => t,
        Err(e) => {
            return serde_json::json!({
                "content": [{"type": "text", "text": format!("Error reading crash log: {}", e)}],
                "isError": true,
            })
        }
    };
    // Return the last 200 lines (most relevant for diagnosis).
    let lines: Vec<&str> = full.lines().collect();
    let start = if lines.len() > 200 {
        lines.len() - 200
    } else {
        0
    };
    let tail: Vec<&str> = lines[start..].to_vec();
    serde_json::json!({
        "content": [{"type": "text", "text": tail.join("\n")}],
        "isError": false,
        "filename": newest,
        "total_lines": lines.len(),
    })
}

// ---------------------------------------------------------------------------
// read_mod_manifest handler
// ---------------------------------------------------------------------------

fn handle_read_mod_manifest(app: &AppHandle, mod_id: &str) -> serde_json::Value {
    let conn = match registry::open_registry(app) {
        Ok(c) => c,
        Err(e) => {
            return serde_json::json!({
                "content": [{"type": "text", "text": format!("Could not open registry: {}", e)}],
                "isError": true,
            })
        }
    };
    let item = match registry::get_item_by_id(&conn, mod_id) {
        Ok(Some(i)) => i,
        Ok(None) => {
            return serde_json::json!({
                "content": [{"type": "text", "text": format!("Mod '{}' not found in curated registry", mod_id)}],
                "isError": true,
            })
        }
        Err(e) => {
            return serde_json::json!({
                "content": [{"type": "text", "text": format!("Registry query error: {}", e)}],
                "isError": true,
            })
        }
    };
    serde_json::json!({
        "id": item.id,
        "name": item.name,
        "content_type": item.content_type,
        "download_strategy": item.download_strategy,
        "source_identifier": item.source_identifier,
        "sha256": item.sha256,
        "license_id": item.license_id,
        "description": item.description,
        "body_markdown": item.body_markdown,
        "page_url": item.page_url,
        "icon_url": item.icon_url,
        "upvotes": item.upvotes,
        "downvotes": item.downvotes,
        "net_score": item.net_score,
        "velocity": item.velocity,
        "status": item.status,
        "is_immune": item.is_immune,
        "immunity_reason": item.immunity_reason,
        "date_added": item.date_added,
        "compatible_versions_json": item.compatible_versions_json,
        "modrinth_id": item.modrinth_id,
    })
}

// ---------------------------------------------------------------------------
// search_knowledge_base handler
// ---------------------------------------------------------------------------

fn handle_search_knowledge_base(app: &AppHandle, query: &str) -> serde_json::Value {
    let conn = match registry::open_registry(app) {
        Ok(c) => c,
        Err(e) => {
            return serde_json::json!({
                "content": [{"type": "text", "text": format!("Could not open registry: {}", e)}],
                "isError": true,
            })
        }
    };
    let like_pattern = format!("%{}%", query);
    let sql = "SELECT id, name, content_type, description                FROM registry_items                WHERE (description IS NOT NULL AND description LIKE ?1)                   OR (name LIKE ?1)                LIMIT 5";
    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(e) => {
            return serde_json::json!({
                "content": [{"type": "text", "text": format!("Query prepare error: {}", e)}],
                "isError": true,
            })
        }
    };
    let rows = match stmt.query_map(
        [&like_pattern],
        |row| -> rusqlite::Result<serde_json::Value> {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "name": row.get::<_, String>(1)?,
                "content_type": row.get::<_, String>(2)?,
                "description": row.get::<_, Option<String>>(3)?,
            }))
        },
    ) {
        Ok(r) => r,
        Err(e) => {
            return serde_json::json!({
                "content": [{"type": "text", "text": format!("Query error: {}", e)}],
                "isError": true,
            })
        }
    };
    let mut results: Vec<serde_json::Value> = Vec::new();
    for row in rows {
        if let Ok(v) = row {
            results.push(v);
        }
    }
    serde_json::json!({
        "results": results,
        "query": query,
    })
}

// ---------------------------------------------------------------------------
// Tool call handler
// ---------------------------------------------------------------------------

async fn handle_tool_call(
    app: &AppHandle,
    tool_name: &str,
    params: &serde_json::Value,
) -> serde_json::Value {
    let get_str = |key: &str| -> Option<String> {
        params
            .get(key)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    };

    let instance_id = get_str("instance_id").unwrap_or_default();
    let filename = get_str("filename").unwrap_or_default();
    let _crash_text = get_str("crash_text").unwrap_or_default();

    match tool_name {
        "list_instances" => {
            let result = handle_list_instances(app);
            serde_json::json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string(&result).unwrap_or_default(),
                }],
                "isError": result.get("error").is_some(),
            })
        }
        "list_instance_mods" => {
            let result = handle_list_instance_mods(app, &instance_id);
            serde_json::json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string(&result).unwrap_or_default(),
                }],
                "isError": result.get("error").is_some(),
            })
        }
        "disable_mod" => {
            if let Err(e) = check_approval(app, "disable_mod", &instance_id, true) {
                return serde_json::json!({
                    "content": [{
                        "type": "text",
                        "text": format!("Approval denied: {}", e),
                    }],
                    "isError": true,
                });
            }
            let result = handle_disable_mod(app, &instance_id, &filename);
            serde_json::json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string(&result).unwrap_or_default(),
                }],
                "isError": result.get("error").is_some(),
            })
        }
        "search_crash_signatures" => {
            let crash_text = get_str("crash_text").unwrap_or_default();
            let result = handle_search_crash_signatures(app, &crash_text).await;
            serde_json::json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string(&result).unwrap_or_default(),
                }],
                "isError": false,
            })
        }
        "suggest_mod_incompatibility" => {
            let instance_id = get_str("instance_id").unwrap_or_default();
            let crash_text = get_str("crash_text").unwrap_or_default();
            let result = suggest_mod_incompatibility_impl(app, &instance_id, &crash_text).await;
            serde_json::json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string(&result).unwrap_or_default(),
                }],
                "isError": false,
            })
        }
        "get_system_context" => {
            let md = build_system_context(app);
            serde_json::json!({
                "content": [{
                    "type": "text",
                    "text": md,
                }],
                "isError": false,
            })
        }
        "read_latest_crash" => {
            let instance_id = get_str("instance_id").unwrap_or_default();
            handle_read_latest_crash(app, &instance_id)
        }
        "read_mod_manifest" => {
            let mod_id = get_str("mod_id").unwrap_or_default();
            handle_read_mod_manifest(app, &mod_id)
        }
        "enable_mod" => {
            if let Err(e) = check_approval(app, "enable_mod", &instance_id, true) {
                return serde_json::json!({
                    "content": [{
                        "type": "text",
                        "text": format!("Approval denied: {}", e),
                    }],
                    "isError": true,
                });
            }
            match crash_investigator::enable_mod(app, &instance_id, &filename) {
                Ok(()) => serde_json::json!({
                    "content": [{
                        "type": "text",
                        "text": format!("Mod {} re-enabled in instance {}. Restart the game to apply.", filename, instance_id),
                    }],
                    "isError": false,
                }),
                Err(e) => serde_json::json!({
                    "content": [{
                        "type": "text",
                        "text": e.to_string(),
                    }],
                    "isError": true,
                }),
            }
        }
        "search_knowledge_base" => {
            let query = get_str("query").unwrap_or_default();
            handle_search_knowledge_base(app, &query)
        }
        _ => serde_json::json!({
            "content": [{
                "type": "text",
                "text": format!("Tool '{}' not found", tool_name),
            }],
            "isError": true,
        }),
    }
}

// ---------------------------------------------------------------------------
// MCP method handler
// ---------------------------------------------------------------------------

async fn handle_mcp_method(
    app: &AppHandle,
    method: &str,
    params: Option<&serde_json::Value>,
) -> serde_json::Value {
    match method {
        "initialize" => {
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {},
                    "resources": {}
                },
                "serverInfo": {
                    "name": "agora",
                    "version": "0.1.0"
                }
            })
        }
        "tools/list" => {
            serde_json::json!({
                "tools": tool_definitions()
            })
        }
        "tools/call" => {
            let params = params.unwrap_or(&serde_json::Value::Null);
            let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let tool_params = params.get("arguments").unwrap_or(&serde_json::Value::Null);
            handle_tool_call(app, tool_name, tool_params).await
        }
        "resources/list" => {
            serde_json::json!({
                "resources": [{
                    "uri": "system_context.md",
                    "name": "System Context",
                    "mimeType": "text/markdown",
                    "description": "Current Agora system state",
                }]
            })
        }
        "resources/read" => {
            let params = params.unwrap_or(&serde_json::Value::Null);
            let uri = params.get("uri").and_then(|v| v.as_str()).unwrap_or("");

            if uri == "system_context.md" {
                let md = build_system_context(app);
                serde_json::json!({
                    "contents": [{
                        "uri": "system_context.md",
                        "mimeType": "text/markdown",
                        "text": md,
                    }]
                })
            } else {
                serde_json::json!({
                    "error": {
                        "code": -32602,
                        "message": format!("Unknown resource URI: {}", uri),
                    }
                })
            }
        }
        _ => serde_json::json!({
            "error": {
                "code": JSONRPC_ERROR_METHOD_NOT_FOUND,
                "message": format!("Unknown method: {}", method),
            }
        }),
    }
}

// ---------------------------------------------------------------------------
// HTTP parsing helpers
// ---------------------------------------------------------------------------

fn parse_request_line(line: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() >= 2 {
        Some((parts[0].to_string(), parts[1].to_string()))
    } else {
        None
    }
}

fn parse_query_params(path: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();
    if let Some(query) = path.split('?').nth(1) {
        for pair in query.split('&') {
            let mut kv = pair.splitn(2, '=');
            if let (Some(k), Some(v)) = (kv.next(), kv.next()) {
                params.insert(
                    urlencoding::decode(k)
                        .unwrap_or_else(|_| Cow::Borrowed(k))
                        .into_owned(),
                    urlencoding::decode(v)
                        .unwrap_or_else(|_| Cow::Borrowed(v))
                        .into_owned(),
                );
            }
        }
    }
    params
}

fn extract_route(path: &str) -> &str {
    path.split('?').next().unwrap_or(path)
}

// Extract session_id from query string
fn extract_session_id(path: &str) -> Option<String> {
    parse_query_params(path).remove("session_id")
}

// ---------------------------------------------------------------------------
// Connection handler â€” the core of the MCP server
// ---------------------------------------------------------------------------

async fn handle_connection(
    stream: tokio::net::TcpStream,
    store: SessionStore,
    app: AppHandle,
) -> std::io::Result<()> {
    let (raw_read, mut write_half) = stream.into_split();
    let mut read_half = BufReader::new(raw_read);

    // Read request line
    let mut request_line = String::new();
    if read_half.read_line(&mut request_line).await? == 0 {
        return Ok(());
    }

    let (method, full_path) = match parse_request_line(&request_line) {
        Some(v) => v,
        None => return Ok(()),
    };

    // Read headers
    let mut headers = HashMap::new();
    loop {
        let mut header_line = String::new();
        let bytes = read_half.read_line(&mut header_line).await?;
        if bytes == 0 {
            break;
        }
        let header_line = header_line.trim();
        if header_line.is_empty() {
            break;
        }
        if let Some((key, value)) = header_line.split_once(':') {
            headers.insert(key.trim().to_lowercase(), value.trim().to_string());
        }
    }

    let route = extract_route(&full_path);

    // Token auth: reject unauthenticated connections (spec 10.0 #2, B2 2026-07-05).
    if !validate_token(&app, &headers, &full_path) {
        let body = br#"{"jsonrpc":"2.0","id":null,"error":{"code":-32001,"message":"Unauthorized: MCP Bearer token required. Copy it from Settings > Integrations > MCP Server."}}"#;
        let msg = format!(
            "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            std::str::from_utf8(body).unwrap_or(""),
        );
        let _ = write_half.write_all(msg.as_bytes()).await;
        return Ok(());
    }

    match (method.as_str(), route) {
        ("GET", "/sse") => handle_sse(write_half, store, app).await,
        ("POST", "/messages") => {
            handle_post_messages(full_path, store, app, headers, read_half, write_half).await
        }
        // Streamable HTTP transport (POST to /mcp or POST to /sse).
        // Modern MCP clients (Kilo Code, etc.) try Streamable HTTP first by POSTing
        // directly to the server URL before falling back to the legacy SSE transport.
        // We handle both /mcp and /sse POST paths to support either URL in config.
        ("POST", "/mcp") | ("POST", "/sse") => {
            handle_streamable_http(app, headers, read_half, write_half).await
        }
        _ => {
            let mut w = tokio::io::BufWriter::new(write_half);
            let _ = w
                .write_all(
                    "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}"
                        .as_bytes(),
                )
                .await;
            Ok(())
        }
    }
}

async fn handle_sse(
    writer: tokio::net::tcp::OwnedWriteHalf,
    store: SessionStore,
    app: AppHandle,
) -> std::io::Result<()> {
    let session_id = generate_session_id();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(64);

    // Register session
    {
        let mut sessions = store.lock().unwrap();
        sessions.insert(session_id.clone(), tx);
    }

    let mut writer = tokio::io::BufWriter::new(writer);

    // Send SSE headers
    let sse_headers = "HTTP/1.1 200 OK\r\n\
        Content-Type: text/event-stream\r\n\
        Cache-Control: no-cache\r\n\
        Connection: keep-alive\r\n\
        X-Accel-Buffering: no\r\n\r\n";
    writer.write_all(sse_headers.as_bytes()).await?;
    writer.flush().await?;

    // Send endpoint event
    let endpoint_event = format!(
        "event: endpoint\ndata: /messages?session_id={}\n\n",
        session_id
    );
    writer.write_all(endpoint_event.as_bytes()).await?;
    writer.flush().await?;

    // Keep alive loop: wait for messages from the SSE channel.
    // The connection stays open; we send responses via the channel.
    // When the client disconnects, the write will fail and we exit.
    let mut alive = true;
    while alive {
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    Some(data) => {
                        let event = format!("data: {}\n\n", data);
                        if let Err(e) = writer.write_all(event.as_bytes()).await {
                            let _ = e;
                            alive = false;
                        } else {
                            let _ = writer.flush().await;
                        }
                    }
                    None => {
                        // Channel closed
                        alive = false;
                    }
                }
            }
            // Use a sleep to prevent busy-looping since we don't have the read half.
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {
                // Keep the loop alive, check for messages
            }
        }
    }

    // Clean up session
    {
        let mut sessions = store.lock().unwrap();
        sessions.remove(&session_id);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Streamable HTTP transport handler
// ---------------------------------------------------------------------------
//
// Per the 2025-03-26 MCP spec, Streamable HTTP clients POST JSON-RPC requests
// to a single endpoint (e.g. /mcp or the same /sse URL) and receive the
// JSON-RPC response directly in the HTTP response body (200 OK).
// No separate SSE session is required for request/response pairs.

async fn handle_streamable_http(
    app: AppHandle,
    headers: HashMap<String, String>,
    mut read_half: tokio::io::BufReader<tokio::net::tcp::OwnedReadHalf>,
    mut write_half: tokio::net::tcp::OwnedWriteHalf,
) -> std::io::Result<()> {
    // Read POST body
    let content_length = headers
        .get("content-length")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);

    let mut body = Vec::with_capacity(content_length);
    if content_length > 0 {
        body.resize(content_length, 0u8);
        let _ = read_half.read_exact(&mut body).await;
    }

    if body.is_empty() {
        // No body — return 204 No Content.
        let _ = write_half
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
            .await;
        return Ok(());
    }

    let request: JsonRpcRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            let resp = JsonRpcResponse::error(
                serde_json::Value::Null,
                -32700,
                &format!("JSON parse error: {}", e),
            );
            let resp_bytes = serde_json::to_vec(&resp).unwrap_or_default();
            let _ = write_half
                .write_all(
                    format!(
                        "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        resp_bytes.len(),
                        String::from_utf8_lossy(&resp_bytes)
                    )
                    .as_bytes(),
                )
                .await;
            return Ok(());
        }
    };

    // Notifications (no id) must not receive a response per JSON-RPC 2.0 spec.
    if request.id.is_none() {
        let _ = write_half
            .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n")
            .await;
        return Ok(());
    }

    let result = handle_mcp_method(&app, &request.method, request.params.as_ref()).await;
    let response = JsonRpcResponse::success(
        request.id.clone().unwrap_or(serde_json::Value::Null),
        result,
    );
    let resp_bytes = response.to_json_bytes();

    let _ = write_half
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                resp_bytes.len(),
                String::from_utf8_lossy(&resp_bytes)
            )
            .as_bytes(),
        )
        .await;

    Ok(())
}

// ---------------------------------------------------------------------------
// Legacy SSE POST /messages handler
// ---------------------------------------------------------------------------

async fn handle_post_messages(
    full_path: String,
    store: SessionStore,
    app: AppHandle,
    headers: HashMap<String, String>,
    mut read_half: tokio::io::BufReader<tokio::net::tcp::OwnedReadHalf>,
    mut write_half: tokio::net::tcp::OwnedWriteHalf,
) -> std::io::Result<()> {
    // Parse session_id from query params
    let session_id = extract_session_id(&full_path).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "Missing session_id")
    })?;

    // Read POST body
    let content_length = headers
        .get("content-length")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);

    let mut body = Vec::with_capacity(content_length);
    if content_length > 0 {
        body.resize(content_length, 0u8);
        let _ = read_half.read_exact(&mut body).await;
    }

    // Parse JSON-RPC request
    let request: JsonRpcRequest = if !body.is_empty() {
        match serde_json::from_slice(&body) {
            Ok(r) => r,
            Err(e) => {
                let resp = JsonRpcResponse::error(
                    serde_json::Value::Null,
                    -32700, // Parse error
                    &format!("JSON parse error: {}", e),
                );
                let resp_bytes = serde_json::to_vec(&resp).unwrap_or_default();
                let _ = write_half
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                            resp_bytes.len(),
                            String::from_utf8_lossy(&resp_bytes)
                        )
                        .as_bytes(),
                    )
                    .await;
                return Ok(());
            }
        }
    } else {
        return Ok(());
    };

    // Rate limit check
    let mut rate_limiter = RateLimiter::new();
    if !rate_limiter.allow() {
        let resp = JsonRpcResponse::error(
            request.id.unwrap_or(serde_json::Value::Null),
            -32000,
            MCP_ERR_TOO_MANY_REQUESTS,
        );
        let resp_bytes = serde_json::to_vec(&resp).unwrap_or_default();
        let _ = write_half
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    resp_bytes.len(),
                    String::from_utf8_lossy(&resp_bytes)
                )
                .as_bytes(),
            )
            .await;
        return Ok(());
    }

    // Notifications (no id) must not receive a response per JSON-RPC 2.0 spec.
    // Return 202 Accepted with no body and no SSE push.
    if request.id.is_none() {
        let _ = write_half
            .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n")
            .await;
        return Ok(());
    }

    // Dispatch the JSON-RPC method
    let response = match request.method.as_str() {
        "initialize" | "tools/list" | "tools/call" | "resources/list" | "resources/read" => {
            let result = handle_mcp_method(&app, &request.method, request.params.as_ref()).await;
            JsonRpcResponse::success(
                request.id.clone().unwrap_or(serde_json::Value::Null),
                result,
            )
        }
        _ => JsonRpcResponse::error(
            request.id.unwrap_or(serde_json::Value::Null),
            JSONRPC_ERROR_METHOD_NOT_FOUND,
            &format!("Unknown method: {}", request.method),
        ),
    };

    let resp_bytes = response.to_json_bytes();

    // Per the legacy SSE transport spec, POST /messages returns 202 Accepted
    // and the actual JSON-RPC response travels via the SSE channel.
    // We also acknowledge via HTTP for clients that read the body directly.
    let _ = write_half
        .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n")
        .await;

    // Send the response via the SSE channel (the primary delivery path).
    {
        let sender = store.lock().unwrap().get(&session_id).cloned();
        if let Some(sender) = sender {
            let _ = sender
                .send(String::from_utf8_lossy(&resp_bytes).to_string())
                .await;
        }
    }

    Ok(())
}

impl JsonRpcResponse {
    fn to_json_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// McpServer public API
// ---------------------------------------------------------------------------

pub struct McpServer {
    shutdown_tx: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    stopped_rx: std::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    running: Arc<AtomicBool>,
    port: u16,
}

impl McpServer {
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        let _ = self
            .shutdown_tx
            .lock()
            .unwrap()
            .take()
            .map(|tx| tx.send(()));
    }

    /// Take the one-shot completion signal for the listener task. This is
    /// consumed by the lifecycle commands before replacing a stopped server.
    pub fn take_stopped_rx(&self) -> Option<tokio::sync::oneshot::Receiver<()>> {
        self.stopped_rx.lock().unwrap().take()
    }
}

/// Long-lived Tauri state for the optional MCP listener. The listener itself
/// is replaceable, while this manager is registered once at app startup. That
/// avoids Tauri's deprecated `unmanage` API and makes Stop → Start reliable.
pub struct McpServerManager {
    lifecycle: tokio::sync::Mutex<()>,
    server: tokio::sync::Mutex<Option<McpServer>>,
}

impl Default for McpServerManager {
    fn default() -> Self {
        Self {
            lifecycle: tokio::sync::Mutex::new(()),
            server: tokio::sync::Mutex::new(None),
        }
    }
}

impl McpServerManager {
    /// Start the listener if it is not already running and return its port.
    pub async fn start(&self, app: AppHandle) -> Result<u16, std::io::Error> {
        let _lifecycle_guard = self.lifecycle.lock().await;

        let stale_server = {
            let mut server = self.server.lock().await;
            if let Some(running) = server.as_ref().filter(|server| server.is_running()) {
                return Ok(running.port());
            }
            server.take()
        };

        if let Some(stale_server) = stale_server {
            stale_server.stop();
            if let Some(stopped_rx) = stale_server.take_stopped_rx() {
                let _ = stopped_rx.await;
            }
        }

        let server = start_server(app).await?;
        let port = server.port();
        *self.server.lock().await = Some(server);
        Ok(port)
    }

    /// Stop the listener and wait until its TCP socket has been released.
    pub async fn stop(&self) {
        let _lifecycle_guard = self.lifecycle.lock().await;
        let server = self.server.lock().await.take();
        if let Some(server) = server {
            server.stop();
            if let Some(stopped_rx) = server.take_stopped_rx() {
                let _ = stopped_rx.await;
            }
        }
    }

    pub async fn port(&self) -> Option<u16> {
        self.server
            .lock()
            .await
            .as_ref()
            .filter(|server| server.is_running())
            .map(McpServer::port)
    }

    /// Best-effort shutdown for synchronous application-close callbacks.
    pub fn request_shutdown(&self) {
        if let Ok(mut server) = self.server.try_lock() {
            if let Some(server) = server.take() {
                server.stop();
            }
        }
    }
}

pub async fn start_server(app: AppHandle) -> Result<McpServer, std::io::Error> {
    let port: u16 = 39741;
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();

    let listener = TcpListener::bind(addr).await?;
    // Ensure the MCP bearer token exists on server start (generates + persists on first call).
    let _ = get_or_create_mcp_token(&app);
    let session_store: SessionStore = Arc::new(std::sync::Mutex::new(HashMap::new()));

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let (stopped_tx, stopped_rx) = tokio::sync::oneshot::channel::<()>();
    let running = Arc::new(AtomicBool::new(true));

    let app_for_loop = app.clone();
    let running_for_loop = Arc::clone(&running);

    tokio::spawn(async move {
        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                                        Ok((mut stream, _addr)) => {
                            // Whitelist: only allow 127.0.0.1
                            let is_local = stream
                                .peer_addr()
                                .ok()
                                .map(|a| a.ip())
                                .unwrap_or_else(|| {
                                    // If we can't get the address, reject
                                    std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)
                                })
                                .is_loopback();

                            if !is_local {
                                let _ = stream.shutdown().await;
                                continue;
                            }

                            let store_clone = Arc::clone(&session_store);
                            let app_clone = app_for_loop.clone();
                            tokio::spawn(async move {
                                if let Err(e) = handle_connection(stream, store_clone, app_clone).await {
                                    eprintln!("MCP connection error: {}", e);
                                }
                            });
                        }
                        Err(e) => {
                            eprintln!("MCP accept error: {}", e);
                            break;
                        }
                    }
                }
                _ = &mut shutdown_rx => {
                    break;
                }
            }
        }
        running_for_loop.store(false, Ordering::SeqCst);
        let _ = stopped_tx.send(());
    });

    Ok(McpServer {
        shutdown_tx: std::sync::Mutex::new(Some(shutdown_tx)),
        stopped_rx: std::sync::Mutex::new(Some(stopped_rx)),
        running,
        port,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Approval logic (pure helper) ----

    #[test]
    fn test_approval_always_allow() {
        assert_eq!(
            check_approval_grant(Some("always_allow"), true),
            ApprovalResult::Allowed
        );
    }

    #[test]
    fn test_approval_always_deny() {
        assert_eq!(
            check_approval_grant(Some("always_deny"), true),
            ApprovalResult::Denied
        );
    }

    #[test]
    fn test_approval_session() {
        assert_eq!(
            check_approval_grant(Some("session"), true),
            ApprovalResult::Allowed
        );
    }

    #[test]
    fn test_approval_no_grant_readonly() {
        // No grant + non-destructive â†’ allowed (safe default).
        assert_eq!(check_approval_grant(None, false), ApprovalResult::Allowed);
    }

    #[test]
    fn test_approval_no_grant_destructive() {
        // No grant + destructive â†’ denied (safe default).
        assert_eq!(check_approval_grant(None, true), ApprovalResult::Denied);
    }

    // ---- JSON-RPC helpers ----

    #[test]
    fn test_jsonrpc_response_has_id() {
        let id = serde_json::json!(42);
        let resp = JsonRpcResponse::success(id.clone(), serde_json::json!({ "ok": true }));
        assert_eq!(resp.id, id);
        assert!(resp.error.is_none());
        assert!(resp.result.is_some());
    }

    #[test]
    fn test_jsonrpc_error_response() {
        let id = serde_json::json!(null);
        let resp = JsonRpcResponse::error(id.clone(), -32601, "Method not found");
        assert_eq!(resp.id, id);
        let err = resp.error.expect("expected error");
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "Method not found");
    }

    // ---- Session ID ----

    #[test]
    fn test_session_id_unique() {
        let a = generate_session_id();
        let b = generate_session_id();
        assert_ne!(a, b);
    }

    #[test]
    fn test_session_id_nonempty() {
        let id = generate_session_id();
        assert!(!id.is_empty());
    }

    // ---- HTTP parsing helpers ----

    #[test]
    fn test_parse_request_line_valid() {
        let (method, path) = parse_request_line("POST /messages HTTP/1.1").unwrap();
        assert_eq!(method, "POST");
        assert_eq!(path, "/messages");
    }

    #[test]
    fn test_parse_request_line_invalid() {
        assert!(parse_request_line("INVALID").is_none());
    }

    #[test]
    fn test_parse_query_params_basic() {
        let params = parse_query_params("/messages?session_id=abc&foo=bar");
        assert_eq!(params.get("session_id"), Some(&"abc".to_string()));
        assert_eq!(params.get("foo"), Some(&"bar".to_string()));
    }

    #[test]
    fn test_parse_query_params_no_query() {
        let params = parse_query_params("/messages");
        assert!(params.is_empty());
    }

    #[test]
    fn test_extract_route() {
        assert_eq!(extract_route("/sse"), "/sse");
        assert_eq!(extract_route("/sse?foo=bar"), "/sse");
        assert_eq!(extract_route("/messages"), "/messages");
    }

    #[test]
    fn test_extract_session_id_present() {
        assert_eq!(
            extract_session_id("/messages?session_id=abc123"),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn test_extract_session_id_absent() {
        assert!(extract_session_id("/messages").is_none());
    }

    // ---- Rate limiter (deterministic with mock time) ----

    #[test]
    fn test_rate_limiter_allows_under_limit() {
        let mut rl = RateLimiter::new();
        // Under 100 requests â€” all allowed.
        for _ in 0..100 {
            assert!(rl.allow());
        }
    }

    #[test]
    fn test_rate_limiter_blocks_over_limit() {
        let mut rl = RateLimiter::new();
        // Fill up to limit.
        for _ in 0..100 {
            assert!(rl.allow());
        }
        // Next request should be denied.
        assert!(!rl.allow());
    }

    #[test]
    fn test_server_stop_marks_listener_stopped_and_exposes_completion_signal() {
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();
        let (_stopped_tx, stopped_rx) = tokio::sync::oneshot::channel();
        let server = McpServer {
            shutdown_tx: std::sync::Mutex::new(Some(shutdown_tx)),
            stopped_rx: std::sync::Mutex::new(Some(stopped_rx)),
            running: Arc::new(AtomicBool::new(true)),
            port: 39741,
        };

        assert!(server.is_running());
        server.stop();

        assert!(!server.is_running());
        assert!(shutdown_rx.try_recv().is_ok());
        assert!(server.take_stopped_rx().is_some());
        assert!(server.take_stopped_rx().is_none());
    }
}
