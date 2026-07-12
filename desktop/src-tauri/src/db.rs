use crate::models::InstanceRow;
use crate::paths;
use rusqlite::Connection;
use serde::Serialize;

/// Expected schema version for the mutable local SQLite database.
/// Migrations are applied sequentially on startup.
pub const LOCAL_STATE_SCHEMA_VERSION: i64 = 3;

/// Open a connection to the mutable local state database, creating it if needed.
// Re-exported from agora-core for desktop-internal callers.
pub use agora_core::db::{get_flag_rate_limit_status, normalize_pair, record_co_crash};

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

/// Open a connection to the downloaded read-only registry database.
pub fn registry_connection<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> anyhow::Result<Connection> {
    let db_path = paths::registry_db_path(app)?;
    let conn = Connection::open(&db_path)?;
    conn.execute_batch("PRAGMA journal_mode = WAL;")?;
    Ok(conn)
}

/// Initialize the local SQLite database on first run and apply migrations.
pub fn init_local_state_db<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> anyhow::Result<()> {
    let conn = local_state_connection(app)?;
    run_migrations(&conn)?;
    Ok(())
}

/// Apply sequential migrations up to [LOCAL_STATE_SCHEMA_VERSION].
/// Delegates to agora-core for the actual migration SQL.
fn run_migrations(conn: &Connection) -> anyhow::Result<()> {
    agora_core::db::run_migrations(conn)
}

/// Read a JSON-encoded setting from user_settings.
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

/// Upsert a JSON-encoded setting into user_settings.
pub fn set_setting(conn: &Connection, key: &str, value: &serde_json::Value) -> anyhow::Result<()> {
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
             jvm_memory_mb, jvm_gc, jvm_custom_args, jvm_always_pre_touch, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
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
             jvm_custom_args = excluded.jvm_custom_args,
             jvm_always_pre_touch = excluded.jvm_always_pre_touch",
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
            row.jvm_always_pre_touch as i64,
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
                jvm_memory_mb, jvm_gc, jvm_custom_args, jvm_always_pre_touch, created_at
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
                jvm_memory_mb, jvm_gc, jvm_custom_args, jvm_always_pre_touch, created_at
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

/// Update last_launched_at for an instance.
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

// ---------------------------------------------------------------------------
// Set-locked helper
// ---------------------------------------------------------------------------

pub fn set_locked(conn: &Connection, instance_id: &str, locked: bool) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE user_instances SET is_locked = ?1 WHERE instance_id = ?2",
        rusqlite::params![locked, instance_id],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Version cache helpers
// ---------------------------------------------------------------------------

/// Read the stored Modrinth version page from settings.
pub fn get_version_page(conn: &Connection, mod_id: &str) -> anyhow::Result<Option<String>> {
    let key = format!("modrinth_versions_page_{}", mod_id);
    get_setting(conn, &key).map(|v| v.and_then(|j| j.as_str().map(String::from)))
}

/// Store a Modrinth version page in settings.
pub fn set_version_page(conn: &Connection, mod_id: &str, data: &str) -> anyhow::Result<()> {
    let key = format!("modrinth_versions_page_{}", mod_id);
    set_setting(conn, &key, &serde_json::Value::String(data.to_string()))
}

/// Delete the stored version page for a mod.
pub fn delete_version_page(conn: &Connection, mod_id: &str) -> anyhow::Result<()> {
    let key = format!("modrinth_versions_page_{}", mod_id);
    conn.execute(
        "DELETE FROM user_settings WHERE key = ?1",
        rusqlite::params![key],
    )?;
    Ok(())
}

/// Store a registry item's info (name, icon_url, etc.) in settings so we don't
/// need the registry db on every UI render.
pub fn set_item_cache(conn: &Connection, item_id: &str, data: &str) -> anyhow::Result<()> {
    let key = format!("item_cache_{}", item_id);
    set_setting(conn, &key, &serde_json::Value::String(data.to_string()))
}

/// Read a cached registry item.
pub fn get_item_cache(conn: &Connection, item_id: &str) -> anyhow::Result<Option<String>> {
    let key = format!("item_cache_{}", item_id);
    get_setting(conn, &key).map(|v| v.and_then(|j| j.as_str().map(String::from)))
}

// ---------------------------------------------------------------------------
// Crash investigator tables (v3 schema)
// ---------------------------------------------------------------------------

/// Insert a crash event and return the new row id.
pub fn insert_crash_event(
    conn: &Connection,
    instance_id: &str,
    fingerprint: &str,
    exception_class: &str,
    top_frames_json: &str,
    signature_name: Option<&str>,
) -> anyhow::Result<i64> {
    let now = chrono::Utc::now().to_rfc3339();
    let rows = conn.execute(
        "INSERT INTO crash_events (instance_id, fingerprint, exception_class, top_frames_json, signature_name, occurred_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![instance_id, fingerprint, exception_class, top_frames_json, signature_name, now],
    )?;
    Ok(rows as i64)
}

/// Insert a survival (successful launch) with associated mod ids.
pub fn insert_survival(
    conn: &Connection,
    instance_id: &str,
    mod_ids: &[String],
) -> anyhow::Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO crash_survivals (instance_id, occurred_at) VALUES (?1, ?2)",
        rusqlite::params![instance_id, now],
    )?;
    let survival_id = conn.last_insert_rowid();
    for mod_id in mod_ids {
        conn.execute(
            "INSERT INTO crash_survival_mods (survival_id, mod_id) VALUES (?1, ?2)",
            rusqlite::params![survival_id, mod_id],
        )?;
    }
    Ok(())
}

/// Return the number of survivals a mod appears in.
pub fn get_mod_survival_count(conn: &Connection, mod_id: &str) -> anyhow::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM crash_survival_mods WHERE mod_id = ?1",
        rusqlite::params![mod_id],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

/// Return the number of survivals where both mods a and b appear together.
pub fn get_pair_survival_count(conn: &Connection, a: &str, b: &str) -> anyhow::Result<i64> {
    use agora_core::db::normalize_pair;
    let (first, second) = normalize_pair(a, b);
    conn.query_row(
        "SELECT COUNT(*) FROM crash_survival_mods x
         JOIN crash_survival_mods y ON x.survival_id = y.survival_id
         WHERE x.mod_id = ?1 AND y.mod_id = ?2",
        rusqlite::params![first, second],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

/// Return the total number of crash_survivals rows.
pub fn get_total_survival_count(conn: &Connection) -> anyhow::Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM crash_survivals", [], |row| row.get(0))
        .map_err(Into::into)
}

/// Return confirmed attributions for a fingerprint, ordered by confirm_count DESC.
pub fn get_confirmed_attribution(
    conn: &Connection,
    fingerprint: &str,
) -> anyhow::Result<Vec<agora_core::db::CrashAttribution>> {
    let mut stmt = conn.prepare(
        "SELECT mod_id, confirm_count, last_confirmed_at
         FROM crash_attribution
         WHERE fingerprint = ?1
         ORDER BY confirm_count DESC",
    )?;
    let rows = stmt.query_map([fingerprint], |row| {
        Ok(agora_core::db::CrashAttribution {
            mod_id: row.get(0)?,
            confirm_count: row.get(1)?,
            last_confirmed_at: row.get(2)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Upsert a confirmed attribution: insert with confirm_count=1 or increment + update last_confirmed_at.
pub fn increment_confirmation(
    conn: &Connection,
    fingerprint: &str,
    mod_id: &str,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO crash_attribution (fingerprint, mod_id, confirm_count, last_confirmed_at)
         VALUES (?1, ?2, 1, ?3)
         ON CONFLICT(fingerprint, mod_id) DO UPDATE SET
             confirm_count = confirm_count + 1,
             last_confirmed_at = excluded.last_confirmed_at",
        rusqlite::params![fingerprint, mod_id, now],
    )?;
    Ok(())
}

/// Idempotently add a ruled-out mod for a fingerprint.
pub fn add_ruled_out(conn: &Connection, fingerprint: &str, mod_id: &str) -> anyhow::Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT OR IGNORE INTO crash_ruled_out (fingerprint, mod_id, ruled_out_at) VALUES (?1, ?2, ?3)",
        rusqlite::params![fingerprint, mod_id, now],
    )?;
    Ok(())
}

/// Check whether a mod has been ruled out for a fingerprint.
pub fn is_ruled_out(conn: &Connection, fingerprint: &str, mod_id: &str) -> anyhow::Result<bool> {
    let mut stmt =
        conn.prepare("SELECT 1 FROM crash_ruled_out WHERE fingerprint = ?1 AND mod_id = ?2")?;
    Ok(stmt.exists([fingerprint, mod_id])?)
}

/// Return the mod_ids ruled out for a fingerprint.
pub fn get_ruled_out_mods(conn: &Connection, fingerprint: &str) -> anyhow::Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT mod_id FROM crash_ruled_out WHERE fingerprint = ?1")?;
    let rows = stmt.query_map([fingerprint], |row| row.get(0))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Flag rate limiting (§5.5)
// ---------------------------------------------------------------------------

/// Record a flag submission for rate-limit tracking.
pub fn record_flag_submission(conn: &Connection, now_unix: i64) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO flag_submissions (timestamp) VALUES (?1)",
        rusqlite::params![now_unix],
    )?;
    Ok(())
}

/// Return the number of flag submissions at or after since_unix.
pub fn count_flags_since(conn: &Connection, since_unix: i64) -> anyhow::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM flag_submissions WHERE timestamp >= ?1",
        rusqlite::params![since_unix],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

/// Purge stale crash telemetry records per retention rules.
pub fn purge_stale_crash_telemetry(conn: &Connection) -> anyhow::Result<()> {
    let cutoff = chrono::Utc::now() - chrono::Duration::days(90);
    conn.execute(
        "DELETE FROM local_crash_telemetry
         WHERE last_seen_at < ?1 OR crash_count < 2",
        rusqlite::params![cutoff.to_rfc3339()],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Row mapping
// ---------------------------------------------------------------------------

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
        jvm_always_pre_touch: row.get::<_, i64>(11)? != 0,
        created_at: row.get(12)?,
    })
}
