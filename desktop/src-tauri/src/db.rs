use crate::models::InstanceRow;
use crate::paths;
use rusqlite::Connection;

/// Expected schema version for the downloaded read-only registry database.
/// The launcher compares this against `SELECT version FROM schema_version`.
pub const REGISTRY_SCHEMA_VERSION: i64 = 1;

/// Expected schema version for the mutable local SQLite database.
/// Migrations are applied sequentially on startup.
pub const LOCAL_STATE_SCHEMA_VERSION: i64 = 1;

/// Open a connection to the mutable local state database, creating it if needed.
pub fn local_state_connection<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> anyhow::Result<Connection> {
    let db_path = paths::local_state_db_path(app)?;
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(&db_path)?;
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
    Ok(conn)
}

/// Initialize the local SQLite database on first run and apply migrations.
pub fn init_local_state_db<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> anyhow::Result<()> {
    let conn = local_state_connection(app)?;
    run_migrations(&conn)?;
    Ok(())
}

/// Apply sequential migrations up to [`LOCAL_STATE_SCHEMA_VERSION`].
fn run_migrations(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
             version INTEGER PRIMARY KEY
         );",
    )?;

    let current: i64 = conn
        .query_row("SELECT COALESCE(MAX(version), 0) FROM schema_version", [], |row| {
            row.get(0)
        })?;
    let target = LOCAL_STATE_SCHEMA_VERSION;

    if current < 1 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS user_settings (
                 key TEXT PRIMARY KEY,
                 value_json TEXT NOT NULL
             );

             CREATE TABLE IF NOT EXISTS user_instances (
                 instance_id TEXT PRIMARY KEY,
                 name TEXT NOT NULL,
                 minecraft_version TEXT NOT NULL,
                 loader TEXT NOT NULL,
                 loader_version TEXT NOT NULL,
                 is_modpack BOOLEAN NOT NULL DEFAULT 0,
                 is_locked BOOLEAN NOT NULL DEFAULT 0,
                 last_launched_at TEXT,
                 jvm_memory_mb INTEGER NOT NULL DEFAULT 4096,
                 jvm_gc TEXT NOT NULL DEFAULT 'g1gc',
                 jvm_custom_args TEXT NOT NULL DEFAULT '',
                 created_at TEXT NOT NULL DEFAULT (datetime('now'))
             );

             CREATE TABLE IF NOT EXISTS local_crash_telemetry (
                 mod_a_id TEXT NOT NULL,
                 mod_b_id TEXT NOT NULL,
                 crash_count INTEGER NOT NULL DEFAULT 1,
                 last_seen_at TEXT NOT NULL,
                 PRIMARY KEY (mod_a_id, mod_b_id)
             );

             CREATE TABLE IF NOT EXISTS mcp_approval_grants (
                 tool_name TEXT NOT NULL,
                 instance_id TEXT NOT NULL,
                 state TEXT NOT NULL,
                 granted_at TEXT NOT NULL,
                 expires_at TEXT,
                 PRIMARY KEY (tool_name, instance_id)
             );",
        )?;
        conn.execute("INSERT OR IGNORE INTO schema_version (version) VALUES (1)", [])?;
    }

    if current > target {
        anyhow::bail!("local_state.db schema version {current} is newer than supported {target}");
    }
    Ok(())
}

/// Read a JSON-encoded setting from `user_settings`.
pub fn get_setting(conn: &Connection, key: &str) -> anyhow::Result<Option<serde_json::Value>> {
    let mut stmt = conn.prepare("SELECT value_json FROM user_settings WHERE key = ?1")?;
    let mut rows = stmt.query([key])?;
    if let Some(row) = rows.next()? {
        let text: String = row.get(0)?;
        let value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
        Ok(Some(value))
    } else {
        Ok(None)
    }
}

/// Upsert a JSON-encoded setting into `user_settings`.
pub fn set_setting(
    conn: &Connection,
    key: &str,
    value: &serde_json::Value,
) -> anyhow::Result<()> {
    let text = serde_json::to_string(value)?;
    conn.execute(
        "INSERT INTO user_settings (key, value_json) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
        rusqlite::params![key, text],
    )?;
    Ok(())
}

/// Insert or update an instance row.
pub fn upsert_instance(conn: &Connection, row: &InstanceRow) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO user_instances (
             instance_id, name, minecraft_version, loader, loader_version,
             is_modpack, is_locked, last_launched_at,
             jvm_memory_mb, jvm_gc, jvm_custom_args, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(instance_id) DO UPDATE SET
             name = excluded.name,
             minecraft_version = excluded.minecraft_version,
             loader = excluded.loader,
             loader_version = excluded.loader_version,
             is_modpack = excluded.is_modpack,
             is_locked = excluded.is_locked,
             last_launched_at = excluded.last_launched_at,
             jvm_memory_mb = excluded.jvm_memory_mb,
             jvm_gc = excluded.jvm_gc,
             jvm_custom_args = excluded.jvm_custom_args",
        rusqlite::params![
            row.instance_id,
            row.name,
            row.minecraft_version,
            row.loader,
            row.loader_version,
            row.is_modpack,
            row.is_locked,
            row.last_launched_at,
            row.jvm_memory_mb,
            row.jvm_gc,
            row.jvm_custom_args,
            row.created_at,
        ],
    )?;
    Ok(())
}

/// Delete an instance row.
pub fn delete_instance(conn: &Connection, instance_id: &str) -> anyhow::Result<()> {
    conn.execute(
        "DELETE FROM user_instances WHERE instance_id = ?1",
        rusqlite::params![instance_id],
    )?;
    Ok(())
}

/// List all instances, newest launched first.
pub fn list_instances(conn: &Connection) -> anyhow::Result<Vec<InstanceRow>> {
    let mut stmt = conn.prepare(
        "SELECT instance_id, name, minecraft_version, loader, loader_version,
                is_modpack, is_locked, last_launched_at,
                jvm_memory_mb, jvm_gc, jvm_custom_args, created_at
         FROM user_instances
         ORDER BY last_launched_at DESC NULLS LAST, created_at DESC",
    )?;
    let rows = stmt.query_map([], row_to_instance)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Fetch a single instance by id.
pub fn get_instance(conn: &Connection, instance_id: &str) -> anyhow::Result<Option<InstanceRow>> {
    let mut stmt = conn.prepare(
        "SELECT instance_id, name, minecraft_version, loader, loader_version,
                is_modpack, is_locked, last_launched_at,
                jvm_memory_mb, jvm_gc, jvm_custom_args, created_at
         FROM user_instances
         WHERE instance_id = ?1",
    )?;
    let mut rows = stmt.query_map([instance_id], row_to_instance)?;
    if let Some(r) = rows.next() {
        Ok(Some(r?))
    } else {
        Ok(None)
    }
}

/// Update `last_launched_at` for an instance.
pub fn touch_last_launched(
    conn: &Connection,
    instance_id: &str,
    timestamp: &str,
) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE user_instances SET last_launched_at = ?1 WHERE instance_id = ?2",
        rusqlite::params![timestamp, instance_id],
    )?;
    Ok(())
}

/// Count instances sharing a loader version (used to decide whether the loader
/// version JSON can be removed when deleting an instance).
pub fn count_instances_by_loader_version(
    conn: &Connection,
    loader: &str,
    minecraft_version: &str,
    loader_version: &str,
) -> anyhow::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM user_instances
         WHERE loader = ?1 AND minecraft_version = ?2 AND loader_version = ?3",
        rusqlite::params![loader, minecraft_version, loader_version],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn row_to_instance(row: &rusqlite::Row<'_>) -> rusqlite::Result<InstanceRow> {
    Ok(InstanceRow {
        instance_id: row.get(0)?,
        name: row.get(1)?,
        minecraft_version: row.get(2)?,
        loader: row.get(3)?,
        loader_version: row.get(4)?,
        is_modpack: row.get(5)?,
        is_locked: row.get(6)?,
        last_launched_at: row.get(7)?,
        jvm_memory_mb: row.get(8)?,
        jvm_gc: row.get(9)?,
        jvm_custom_args: row.get(10)?,
        created_at: row.get(11)?,
    })
}
