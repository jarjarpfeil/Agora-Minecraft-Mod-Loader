use crate::db;
use crate::error::{LauncherError, LauncherResult};
use crate::instances::{self, CreateInstanceRequest, InstanceDetail, LoaderVersionSummary};
use crate::models::InstanceRow;
use crate::state::LauncherState;

#[tauri::command]
pub async fn greet(name: String) -> String {
    format!("Hello, {}!", name)
}

/// Execute a parameterized SELECT against the cached read-only registry.db.
///
/// Only SELECT statements are accepted. Query parameters must be bound positionally.
#[tauri::command]
pub async fn query_registry(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    sql: String,
    params: Option<Vec<serde_json::Value>>,
) -> LauncherResult<Vec<serde_json::Value>> {
    tokio::task::spawn_blocking(move || {
        let sql_trim = sql.trim().to_lowercase();
        if !sql_trim.starts_with("select") {
            return Err(LauncherError::Generic {
                code: "ERR_INVALID_QUERY".to_string(),
                message: "query_registry only accepts SELECT statements.".to_string(),
            });
        }

        let registry_path =
            crate::paths::registry_db_path(&app).map_err(|_| LauncherError::RegistryMissing)?;
        if !registry_path.exists() {
            return Err(LauncherError::RegistryMissing);
        }

        let conn = rusqlite::Connection::open_with_flags(
            &registry_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|_| LauncherError::RegistryMissing)?;

        conn.pragma_update(None, "query_only", "ON")
            .map_err(|_| LauncherError::Generic {
                code: "ERR_INVALID_QUERY".to_string(),
                message: "Failed to set read-only mode.".to_string(),
            })?;

        let mut stmt = conn.prepare(&sql).map_err(|e| LauncherError::Generic {
            code: "ERR_INVALID_QUERY".to_string(),
            message: e.to_string(),
        })?;

        let columns = stmt.column_count();
        let mut names: Vec<String> = Vec::with_capacity(columns);
        for i in 0..columns {
            names.push(stmt.column_name(i).unwrap_or("").to_string());
        }
        let params = params.unwrap_or_default();
        let rows = stmt
            .query_map(rusqlite::params_from_iter(params.iter().map(value_to_sql)), |row| {
                let mut obj = serde_json::Map::new();
                for (i, name) in names.iter().enumerate() {
                    let val: serde_json::Value = row_get(row, i);
                    obj.insert(name.clone(), val);
                }
                Ok(serde_json::Value::Object(obj))
            })
            .map_err(|e| LauncherError::Generic {
                code: "ERR_INVALID_QUERY".to_string(),
                message: e.to_string(),
            })?;

        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| LauncherError::Generic {
                code: "ERR_INVALID_QUERY".to_string(),
                message: e.to_string(),
            })?);
        }
        Ok(out)
    })
    .await
    .map_err(|_| LauncherError::Generic {
        code: "ERR_INVALID_QUERY".to_string(),
        message: "Registry query task failed.".to_string(),
    })?
}

/// List all user instances from `local_state.db`.
#[tauri::command]
pub async fn list_instances(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
) -> LauncherResult<Vec<InstanceRow>> {
    tokio::task::spawn_blocking(move || instances::list_instances(&app))
        .await
        .map_err(|_| LauncherError::LocalStateFailed)?
}

/// Fetch a single instance plus its on-disk manifest.
#[tauri::command]
pub async fn get_instance_detail(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    instance_id: String,
) -> LauncherResult<Option<InstanceDetail>> {
    tokio::task::spawn_blocking(move || instances::get_instance_detail(&app, &instance_id))
        .await
        .map_err(|_| LauncherError::LocalStateFailed)?
}

/// Create a custom instance and inject its modloader.
#[tauri::command]
pub async fn create_instance(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    request: CreateInstanceRequest,
) -> LauncherResult<InstanceRow> {
    instances::create_instance(app, request).await
}

/// Delete an instance, moving its directory to the OS trash.
#[tauri::command]
pub async fn delete_instance(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    instance_id: String,
) -> LauncherResult<()> {
    tokio::task::spawn_blocking(move || instances::delete_instance(&app, &instance_id))
        .await
        .map_err(|_| LauncherError::LocalStateFailed)?
}

/// Launch an instance via the official Mojang launcher delegation.
#[tauri::command]
pub async fn launch_instance(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    instance_id: String,
) -> LauncherResult<()> {
    tokio::task::spawn_blocking(move || instances::launch_instance(&app, &instance_id))
        .await
        .map_err(|_| LauncherError::LocalStateFailed)?
}

/// List pinned loader versions for a loader + Minecraft version.
#[tauri::command]
pub async fn list_loader_versions(
    _state: tauri::State<'_, LauncherState>,
    loader: String,
    mc_version: String,
) -> LauncherResult<Vec<LoaderVersionSummary>> {
    Ok(instances::list_loader_versions(&loader, &mc_version))
}

/// Read a JSON-encoded setting from `local_state.db`.
#[tauri::command]
pub async fn get_setting(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    key: String,
) -> LauncherResult<Option<serde_json::Value>> {
    tokio::task::spawn_blocking(move || {
        let conn = db::local_state_connection(&app).map_err(|_| LauncherError::LocalStateFailed)?;
        db::get_setting(&conn, &key).map_err(|_| LauncherError::LocalStateFailed)
    })
    .await
    .map_err(|_| LauncherError::LocalStateFailed)?
}

/// Upsert a JSON-encoded setting into `local_state.db`.
#[tauri::command]
pub async fn set_setting(
    app: tauri::AppHandle,
    _state: tauri::State<'_, LauncherState>,
    key: String,
    value: serde_json::Value,
) -> LauncherResult<()> {
    tokio::task::spawn_blocking(move || {
        let conn = db::local_state_connection(&app).map_err(|_| LauncherError::LocalStateFailed)?;
        db::set_setting(&conn, &key, &value).map_err(|_| LauncherError::LocalStateFailed)
    })
    .await
    .map_err(|_| LauncherError::LocalStateFailed)?
}

fn value_to_sql(v: &serde_json::Value) -> Box<dyn rusqlite::ToSql> {
    match v {
        serde_json::Value::Null => Box::new(rusqlite::types::Null),
        serde_json::Value::Bool(b) => Box::new(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Box::new(i)
            } else if let Some(f) = n.as_f64() {
                Box::new(f)
            } else {
                Box::new(n.to_string())
            }
        }
        serde_json::Value::String(s) => Box::new(s.clone()),
        other => Box::new(other.to_string()),
    }
}

fn row_get(row: &rusqlite::Row<'_>, idx: usize) -> serde_json::Value {
    let val: rusqlite::Result<String> = row.get_ref(idx).and_then(|v| match v {
        rusqlite::types::ValueRef::Null => Ok(String::new()),
        rusqlite::types::ValueRef::Integer(i) => Ok(i.to_string()),
        rusqlite::types::ValueRef::Real(f) => Ok(f.to_string()),
        rusqlite::types::ValueRef::Text(t) => Ok(String::from_utf8_lossy(t).to_string()),
        rusqlite::types::ValueRef::Blob(b) => Ok(format!("<blob {} bytes>", b.len())),
    });
    match val {
        Ok(s) => serde_json::Value::String(s),
        Err(_) => serde_json::Value::Null,
    }
}
