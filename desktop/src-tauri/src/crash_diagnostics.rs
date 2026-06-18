use crate::db;
use crate::error::{LauncherError, LauncherResult};
use crate::paths;
use crate::registry;
use rusqlite::Connection;
use serde::Serialize;

/// Per §2.4.1, crash signature regex patterns longer than this are rejected.
const MAX_REGEX_LEN: usize = 256;

/// Summary of a single crash report file on disk.
#[derive(Debug, Clone, Serialize)]
pub struct CrashReportInfo {
    pub filename: String,
    pub modified_at: String,
    pub size_bytes: u64,
}

/// Result of matching a crash log against the curated signature set.
#[derive(Debug, Clone, Serialize)]
pub struct CrashTriageResult {
    pub matched: bool,
    pub signature_name: Option<String>,
    pub solution_markdown: Option<String>,
    pub action_button_json: Option<String>,
}

impl CrashTriageResult {
    fn no_match() -> Self {
        Self {
            matched: false,
            signature_name: None,
            solution_markdown: None,
            action_button_json: None,
        }
    }
}

/// A row from the registry `crash_signatures` table.
struct CrashSignatureRow {
    name: String,
    regex_pattern: String,
    solution_markdown: Option<String>,
    action_button_json: Option<String>,
}

/// Check whether a fresh crash report appeared after the instance's
/// `last_launched_at`. Returns the newest qualifying file.
///
/// Reads `last_launched_at` from `local_state.db`, lists files in
/// `instances/<id>/crash-reports/`, and returns the newest file whose mtime is
/// strictly newer than `last_launched_at`. If the instance was never launched
/// or no newer crash report exists, returns `None`.
pub fn check_for_crash<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
) -> LauncherResult<Option<CrashReportInfo>> {
    let sanitized = paths::sanitize_id(instance_id);

    let conn = db::local_state_connection(app).map_err(|_| LauncherError::LocalStateFailed)?;
    let row = db::get_instance(&conn, &sanitized)
        .map_err(|_| LauncherError::LocalStateFailed)?;

    let last_launched_at = match row.and_then(|r| r.last_launched_at) {
        Some(ts) => ts,
        None => return Ok(None),
    };
    let last_launched = parse_rfc3339(&last_launched_at);

    let reports_dir = match crash_reports_dir(app, &sanitized) {
        Ok(dir) => dir,
        Err(_) => return Ok(None),
    };
    if !reports_dir.exists() {
        return Ok(None);
    }

    let mut newest: Option<(CrashReportInfo, std::time::SystemTime)> = None;
    let entries = match std::fs::read_dir(&reports_dir) {
        Ok(e) => e,
        Err(_) => return Ok(None),
    };
    for entry in entries.flatten() {
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !meta.is_file() {
            continue;
        }
        let mtime = match meta.modified() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if let Some(ref last) = last_launched {
            if mtime <= *last {
                continue;
            }
        }
        let filename = entry.file_name().to_string_lossy().to_string();
        let info = CrashReportInfo {
            filename: filename.clone(),
            modified_at: system_time_to_rfc3339(mtime),
            size_bytes: meta.len(),
        };
        match &newest {
            Some((_, best_mtime)) if mtime <= *best_mtime => {}
            _ => newest = Some((info, mtime)),
        }
    }

    Ok(newest.map(|(info, _)| info))
}

/// Triage a crash log against curated signatures. Reads the crash log text,
/// queries `crash_signatures` from `registry.db` (read-only), and returns the
/// first match. Per §2.4.1, patterns longer than 256 chars are rejected.
///
/// The crash log is identified by `(instance_id, filename)` and sanitized
/// the same way as `read_crash_log` to prevent path traversal.
pub fn triage_crash<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
    filename: &str,
) -> LauncherResult<CrashTriageResult> {
    let sanitized = paths::sanitize_id(instance_id);
    let reports_dir = crash_reports_dir(app, &sanitized)?;
    let safe_name = std::path::Path::new(filename)
        .file_name()
        .ok_or_else(|| LauncherError::Generic {
            code: "ERR_CRASH_LOG_PATH".to_string(),
            message: "Invalid crash log filename.".to_string(),
        })?
        .to_string_lossy()
        .to_string();
    let crash_path = reports_dir.join(&safe_name);

    let text = match std::fs::read_to_string(&crash_path) {
        Ok(t) => t,
        Err(_) => return Ok(CrashTriageResult::no_match()),
    };

    let conn = match registry::open_registry(app) {
        Ok(c) => c,
        Err(_) => return Ok(CrashTriageResult::no_match()),
    };
    let signatures = match load_signatures(&conn) {
        Ok(s) => s,
        Err(_) => return Ok(CrashTriageResult::no_match()),
    };

    for sig in signatures {
        if sig.regex_pattern.chars().count() > MAX_REGEX_LEN {
            continue;
        }
        let re = match regex::Regex::new(&sig.regex_pattern) {
            Ok(re) => re,
            Err(_) => continue,
        };
        if re.is_match(&text) {
            return Ok(CrashTriageResult {
                matched: true,
                signature_name: Some(sig.name),
                solution_markdown: sig.solution_markdown,
                action_button_json: sig.action_button_json,
            });
        }
    }

    Ok(CrashTriageResult::no_match())
}

/// List all crash report files for an instance with modification times and sizes.
pub fn list_crash_reports<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
) -> LauncherResult<Vec<CrashReportInfo>> {
    let sanitized = paths::sanitize_id(instance_id);
    let reports_dir = match crash_reports_dir(app, &sanitized) {
        Ok(dir) => dir,
        Err(_) => return Ok(Vec::new()),
    };
    if !reports_dir.exists() {
        return Ok(Vec::new());
    }

    let mut out: Vec<(CrashReportInfo, std::time::SystemTime)> = Vec::new();
    let entries = match std::fs::read_dir(&reports_dir) {
        Ok(e) => e,
        Err(_) => return Ok(Vec::new()),
    };
    for entry in entries.flatten() {
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !meta.is_file() {
            continue;
        }
        let mtime = match meta.modified() {
            Ok(t) => t,
            Err(_) => continue,
        };
        out.push((
            CrashReportInfo {
                filename: entry.file_name().to_string_lossy().to_string(),
                modified_at: system_time_to_rfc3339(mtime),
                size_bytes: meta.len(),
            },
            mtime,
        ));
    }
    out.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(out.into_iter().map(|(info, _)| info).collect())
}

/// Read the content of a specific crash report file.
pub fn read_crash_log<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
    filename: &str,
) -> LauncherResult<String> {
    let sanitized = paths::sanitize_id(instance_id);
    let reports_dir = crash_reports_dir(app, &sanitized)?;
    let safe_name = std::path::Path::new(filename)
        .file_name()
        .ok_or_else(|| LauncherError::Generic {
            code: "ERR_CRASH_LOG_PATH".to_string(),
            message: "Invalid crash log filename.".to_string(),
        })?
        .to_string_lossy()
        .to_string();
    let path = reports_dir.join(&safe_name);
    std::fs::read_to_string(&path).map_err(|_| LauncherError::Generic {
        code: "ERR_CRASH_LOG_READ".to_string(),
        message: "Could not read the crash log file.".to_string(),
    })
}

fn crash_reports_dir<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
) -> LauncherResult<std::path::PathBuf> {
    let dir = paths::instance_dir(app, instance_id).map_err(|_| LauncherError::LocalStateFailed)?;
    Ok(dir.join("crash-reports"))
}

fn load_signatures(conn: &Connection) -> anyhow::Result<Vec<CrashSignatureRow>> {
    let mut stmt = conn.prepare(
        "SELECT name, regex_pattern, solution_markdown, action_button_json
         FROM crash_signatures",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(CrashSignatureRow {
            name: row.get(0)?,
            regex_pattern: row.get(1)?,
            solution_markdown: row.get(2)?,
            action_button_json: row.get(3)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

fn parse_rfc3339(ts: &str) -> Option<std::time::SystemTime> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| std::time::SystemTime::from(dt.with_timezone(&chrono::Utc)))
}

fn system_time_to_rfc3339(t: std::time::SystemTime) -> String {
    let dt: chrono::DateTime<chrono::Utc> = t.into();
    dt.to_rfc3339()
}
