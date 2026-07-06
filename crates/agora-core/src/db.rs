use crate::models::InstanceRow;
use rusqlite::Connection;
use serde::Serialize;

/// Expected schema version for the mutable local SQLite database.
/// Migrations are applied sequentially on startup.
pub const LOCAL_STATE_SCHEMA_VERSION: i64 = 3;

/// Open a read-write connection to the local state database.
///
/// Caller is responsible for ensuring the directory exists; this function
/// does not create parent directories.
pub fn local_state_connection(db_path: &std::path::Path) -> anyhow::Result<Connection> {
    let conn = Connection::open(db_path)?;
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
    Ok(conn)
}

/// Open a read-only connection to the cached registry database.
///
/// Caller is responsible for ensuring the file exists.
pub fn registry_connection(db_path: &std::path::Path) -> anyhow::Result<Connection> {
    let conn = Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.execute_batch("PRAGMA query_only = ON;")?;
    Ok(conn)
}

/// Initialize the local SQLite database on first run and apply migrations.
pub fn init_local_state_db(db_path: &std::path::PathBuf) -> anyhow::Result<()> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(db_path)?;
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
    run_migrations(&conn)?;

    // Network feature toggles are stored as JSON strings ("true"/"false")
    // because is_network_enabled reads them with .as_str().
    for key in [
        "network_modrinth_enabled",
        "network_modrinth_cdn_enabled",
        "network_registry_sync_enabled",
        "network_github_oauth_enabled",
        "network_msa_enabled",
        "network_adoptium_enabled",
    ] {
        if get_setting(&conn, key).ok().flatten().is_none() {
            set_setting(&conn, key, &serde_json::Value::String("true".to_string()))?;
        }
    }

    // Feature toggles are stored as genuine JSON booleans so that readers
    // comparing with  == true /  == &Value::Bool(true) match correctly.
    for key in [
        "modrinth_enabled",
        "ai_chat_enabled",
        "ai_mcp_enabled",
    ] {
        if get_setting(&conn, key).ok().flatten().is_none() {
            set_setting(&conn, key, &serde_json::Value::Bool(true))?;
        }
    }

    Ok(())
}

/// Apply sequential migrations up to [LOCAL_STATE_SCHEMA_VERSION].
pub fn run_migrations(conn: &Connection) -> anyhow::Result<()> {
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
                 jvm_always_pre_touch INTEGER NOT NULL DEFAULT 1,
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
              );

              CREATE TABLE IF NOT EXISTS flag_submissions (
                  id INTEGER PRIMARY KEY AUTOINCREMENT,
                  timestamp INTEGER NOT NULL
              );",
        )?;
        conn.execute("INSERT OR IGNORE INTO schema_version (version) VALUES (1)", [])?;
    }

    // Migration v2: add flag_submissions table for comment-flag rate limiting (§5.5).
    if current < 2 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS flag_submissions (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 timestamp INTEGER NOT NULL
             );",
        )?;
        conn.execute("INSERT OR IGNORE INTO schema_version (version) VALUES (2)", [])?;
    }

    // Migration v3: add crash-investigator tables for dynamic scoring algorithm.
    if current < 3 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS crash_events (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 instance_id TEXT NOT NULL,
                 fingerprint TEXT NOT NULL,
                 exception_class TEXT NOT NULL,
                 top_frames_json TEXT NOT NULL,
                 signature_name TEXT,
                 occurred_at TEXT NOT NULL
             );

             CREATE TABLE IF NOT EXISTS crash_survivals (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 instance_id TEXT NOT NULL,
                 occurred_at TEXT NOT NULL
             );

             CREATE TABLE IF NOT EXISTS crash_survival_mods (
                 survival_id INTEGER NOT NULL,
                 mod_id TEXT NOT NULL,
                 PRIMARY KEY (survival_id, mod_id),
                 FOREIGN KEY (survival_id) REFERENCES crash_survivals(id)
             );

             CREATE TABLE IF NOT EXISTS crash_attribution (
                 fingerprint TEXT NOT NULL,
                 mod_id TEXT NOT NULL,
                 confirm_count INTEGER NOT NULL DEFAULT 0,
                 last_confirmed_at TEXT,
                 PRIMARY KEY (fingerprint, mod_id)
             );

             CREATE TABLE IF NOT EXISTS crash_ruled_out (
                 fingerprint TEXT NOT NULL,
                 mod_id TEXT NOT NULL,
                 ruled_out_at TEXT NOT NULL,
                 PRIMARY KEY (fingerprint, mod_id)
             );",
        )?;
        conn.execute("INSERT OR IGNORE INTO schema_version (version) VALUES (3)", [])?;
    }

    // Migration: add jvm_always_pre_touch column to existing databases.
    if current >= 1 {
        let _ = conn.execute(
            "ALTER TABLE user_instances ADD COLUMN jvm_always_pre_touch INTEGER NOT NULL DEFAULT 1",
            [],
        );
    }

    if current > target {
        anyhow::bail!("local_state.db schema version {current} is newer than supported {target}");
    }
    Ok(())
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

/// Check if a specific network feature is enabled in settings.
/// Returns `true` if enabled (or setting not found, for backward compatibility),
/// `false` if explicitly disabled.
/// Setting keys: `network_modrinth_enabled`, `network_modrinth_cdn_enabled`,
/// `network_registry_sync_enabled`, `network_github_oauth_enabled`,
/// `network_msa_enabled`, `network_adoptium_enabled`.
pub fn is_network_enabled(conn: &Connection, key: &str) -> bool {
    get_setting(conn, key)
        .ok()
        .flatten()
        .and_then(|v| v.as_str().map(|s| s == "true"))
        .unwrap_or(true)
}

/// Upsert a JSON-encoded setting into user_settings.
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

/// Set the locked flag for an instance.
pub fn set_locked(conn: &Connection, instance_id: &str, locked: bool) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE user_instances SET is_locked = ?1 WHERE instance_id = ?2",
        rusqlite::params![locked, instance_id],
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

/// Normalize a mod pair so the lexicographically smaller ID always comes first.
/// This ensures (sodium, iris) and (iris, sodium) map to the same row.
pub fn normalize_pair<'a>(a: &'a str, b: &'a str) -> (&'a str, &'a str) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// Record a co-crash for a pair of mods (§4.1b).
pub fn record_co_crash(conn: &Connection, mod_a: &str, mod_b: &str) -> anyhow::Result<()> {
    let (a, b) = normalize_pair(mod_a, mod_b);
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO local_crash_telemetry (mod_a_id, mod_b_id, crash_count, last_seen_at)
         VALUES (?1, ?2, 1, ?3)
         ON CONFLICT(mod_a_id, mod_b_id) DO UPDATE SET
             crash_count = crash_count + 1,
             last_seen_at = excluded.last_seen_at",
        rusqlite::params![a, b, now],
    )?;
    Ok(())
}

/// Purge stale crash telemetry records per §4.1b retention rules:
/// - Records older than 90 days.
/// - Pairs with crash_count < 2.
pub fn purge_stale_crash_telemetry(conn: &Connection) -> anyhow::Result<()> {
    let cutoff = chrono::Utc::now() - chrono::Duration::days(90);
    conn.execute(
        "DELETE FROM local_crash_telemetry
         WHERE last_seen_at < ?1 OR crash_count < 2",
        rusqlite::params![cutoff.to_rfc3339()],
    )?;
    Ok(())
}

/// Record a flag submission for rate-limit tracking (§5.5).
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

/// Rate-limit status for comment-flag submissions (§5.5).
#[derive(Serialize)]
pub struct FlagRateLimit {
    pub remaining_hour: i64,
    pub remaining_day: i64,
    pub reset_hour_at_unix: i64,
    pub reset_day_at_unix: i64,
    pub can_flag: bool,
}

const MAX_FLAGS_PER_HOUR: i64 = 5;
const MAX_FLAGS_PER_DAY: i64 = 20;

/// Compute the current flag rate-limit status for a connection at `now_unix`.
pub fn get_flag_rate_limit_status(
    conn: &Connection,
    now_unix: i64,
) -> anyhow::Result<FlagRateLimit> {
    let hour_window_start = now_unix - 3600;
    let day_window_start = now_unix - 86400;

    let hour_count = count_flags_since(conn, hour_window_start)?;
    let day_count = count_flags_since(conn, day_window_start)?;

    let remaining_hour = (MAX_FLAGS_PER_HOUR - hour_count).max(0);
    let remaining_day = (MAX_FLAGS_PER_DAY - day_count).max(0);

    let reset_hour_at_unix = if hour_count > 0 {
        let mut stmt = conn.prepare(
            "SELECT MIN(timestamp) FROM flag_submissions WHERE timestamp >= ?1",
        )?;
        let oldest_hour: i64 = stmt.query_row([hour_window_start], |row| row.get(0))?;
        oldest_hour + 3600
    } else {
        now_unix + 3600
    };

    let reset_day_at_unix = if day_count > 0 {
        let mut stmt = conn.prepare(
            "SELECT MIN(timestamp) FROM flag_submissions WHERE timestamp >= ?1",
        )?;
        let oldest_day: i64 = stmt.query_row([day_window_start], |row| row.get(0))?;
        oldest_day + 86400
    } else {
        now_unix + 86400
    };

    let can_flag = remaining_hour > 0 && remaining_day > 0;

    Ok(FlagRateLimit {
        remaining_hour,
        remaining_day,
        reset_hour_at_unix,
        reset_day_at_unix,
        can_flag,
    })
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
        jvm_always_pre_touch: row.get::<_, i64>(11)? != 0,
        created_at: row.get(12)?,
    })
}

// ---------------------------------------------------------------------------
// Crash Investigator tables (v3 schema)
// ---------------------------------------------------------------------------

/// An attribution row: (mod_id, confirm_count, last_confirmed_at).
#[derive(Debug, Clone, Serialize)]
pub struct CrashAttribution {
    pub mod_id: String,
    pub confirm_count: i64,
    pub last_confirmed_at: Option<String>,
}

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

/// Return confirmed attributions for a fingerprint, ordered by confirm_count DESC.
pub fn get_confirmed_attribution(
    conn: &Connection,
    fingerprint: &str,
) -> anyhow::Result<Vec<CrashAttribution>> {
    let mut stmt = conn.prepare(
        "SELECT mod_id, confirm_count, last_confirmed_at
         FROM crash_attribution
         WHERE fingerprint = ?1
         ORDER BY confirm_count DESC",
    )?;
    let rows = stmt.query_map([fingerprint], |row| {
        Ok(CrashAttribution {
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
pub fn add_ruled_out(
    conn: &Connection,
    fingerprint: &str,
    mod_id: &str,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT OR IGNORE INTO crash_ruled_out (fingerprint, mod_id, ruled_out_at) VALUES (?1, ?2, ?3)",
        rusqlite::params![fingerprint, mod_id, now],
    )?;
    Ok(())
}

/// Check whether a mod has been ruled out for a fingerprint.
pub fn is_ruled_out(
    conn: &Connection,
    fingerprint: &str,
    mod_id: &str,
) -> anyhow::Result<bool> {
    let mut stmt = conn.prepare(
        "SELECT 1 FROM crash_ruled_out WHERE fingerprint = ?1 AND mod_id = ?2",
    )?;
    Ok(stmt.exists([fingerprint, mod_id])?)
}

/// Return the mod_ids ruled out for a fingerprint.
pub fn get_ruled_out_mods(
    conn: &Connection,
    fingerprint: &str,
) -> anyhow::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT mod_id FROM crash_ruled_out WHERE fingerprint = ?1",
    )?;
    let rows = stmt.query_map([fingerprint], |row| row.get(0))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
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
pub fn get_pair_survival_count(
    conn: &Connection,
    a: &str,
    b: &str,
) -> anyhow::Result<i64> {
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
    conn.query_row(
        "SELECT COUNT(*) FROM crash_survivals",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Helper: create a unique temp-file-backed test database with migrations applied.
    fn test_db() -> (Connection, PathBuf) {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir()
            .join(format!("agora-test-{}.db", n));
        let _ = std::fs::remove_file(&path);
        init_local_state_db(&path).expect("failed to init test db");
        let conn = Connection::open(&path).expect("failed to open test db");
        (conn, path)
    }

    // ---- normalize_pair (pure, no DB) ----

    #[test]
    fn test_normalize_pair_lexicographic() {
        let (a, b) = normalize_pair("zinc", "sodium");
        assert_eq!(a, "sodium");
        assert_eq!(b, "zinc");
    }

    #[test]
    fn test_normalize_pair_already_ordered() {
        let (a, b) = normalize_pair("a", "b");
        assert_eq!(a, "a");
        assert_eq!(b, "b");
    }

    #[test]
    fn test_normalize_pair_symmetric() {
        assert_eq!(normalize_pair("a", "b"), normalize_pair("b", "a"));
    }

    #[test]
    fn test_normalize_pair_same_id() {
        let (a, b) = normalize_pair("x", "x");
        assert_eq!(a, "x");
        assert_eq!(b, "x");
    }

    // ---- get_setting / set_setting ----

    #[test]
    fn test_get_setting_absent_returns_none() {
        let (conn, _path) = test_db();
        assert!(get_setting(&conn, "nonexistent").unwrap().is_none());
    }

    #[test]
    fn test_set_setting_roundtrip() {
        let (conn, _path) = test_db();
        set_setting(&conn, "key", &serde_json::json!("value")).unwrap();
        let val = get_setting(&conn, "key").unwrap();
        assert_eq!(val, Some(serde_json::json!("value")));
    }

    #[test]
    fn test_set_setting_overwrite() {
        let (conn, _path) = test_db();
        set_setting(&conn, "key", &serde_json::json!("v1")).unwrap();
        set_setting(&conn, "key", &serde_json::json!("v2")).unwrap();
        let val = get_setting(&conn, "key").unwrap();
        assert_eq!(val, Some(serde_json::json!("v2")));
    }

    // ---- record_co_crash ----

    #[test]
    fn test_record_co_crash_increments() {
        let (conn, _path) = test_db();
        record_co_crash(&conn, "a", "b").unwrap();
        record_co_crash(&conn, "a", "b").unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT crash_count FROM local_crash_telemetry WHERE mod_a_id = ?1 AND mod_b_id = ?2",
                rusqlite::params!["a", "b"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_record_co_crash_symmetric() {
        let (conn, _path) = test_db();
        record_co_crash(&conn, "a", "b").unwrap();
        record_co_crash(&conn, "b", "a").unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT crash_count FROM local_crash_telemetry WHERE mod_a_id = ?1 AND mod_b_id = ?2",
                rusqlite::params!["a", "b"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }

    // ---- flag rate limiting ----

    #[test]
    fn test_flag_rate_limit_empty_can_flag() {
        let (conn, _path) = test_db();
        let now = 1_000_000_000i64;
        let status = get_flag_rate_limit_status(&conn, now).unwrap();
        assert!(status.can_flag);
        assert_eq!(status.remaining_hour, 5);
        assert_eq!(status.remaining_day, 20);
    }

    #[test]
    fn test_flag_rate_limit_after_three() {
        let (conn, _path) = test_db();
        let now = 1_000_000_000i64;
        for _ in 0..3 {
            record_flag_submission(&conn, now).unwrap();
        }
        let status = get_flag_rate_limit_status(&conn, now).unwrap();
        assert_eq!(status.remaining_hour, 2);
        assert_eq!(status.remaining_day, 17);
    }

    #[test]
    fn test_flag_rate_limit_hourly_exceeded() {
        let (conn, _path) = test_db();
        let now = 1_000_000_000i64;
        for _ in 0..5 {
            record_flag_submission(&conn, now).unwrap();
        }
        let status = get_flag_rate_limit_status(&conn, now).unwrap();
        assert!(!status.can_flag);
        assert_eq!(status.remaining_hour, 0);
    }

    #[test]
    fn test_flag_rate_limit_daily_exceeded() {
        let (conn, _path) = test_db();
        let now = 1_000_000_000i64;
        for _ in 0..20 {
            record_flag_submission(&conn, now).unwrap();
        }
        let status = get_flag_rate_limit_status(&conn, now).unwrap();
        assert!(!status.can_flag);
        assert_eq!(status.remaining_day, 0);
    }

    // ---- crash attribution ----

    #[test]
    fn test_increment_confirmation_inserts() {
        let (conn, _path) = test_db();
        increment_confirmation(&conn, "fp1", "mod_a").unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT confirm_count FROM crash_attribution WHERE fingerprint = ?1 AND mod_id = ?2",
                rusqlite::params!["fp1", "mod_a"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_increment_confirmation_increments() {
        let (conn, _path) = test_db();
        increment_confirmation(&conn, "fp1", "mod_a").unwrap();
        increment_confirmation(&conn, "fp1", "mod_a").unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT confirm_count FROM crash_attribution WHERE fingerprint = ?1 AND mod_id = ?2",
                rusqlite::params!["fp1", "mod_a"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_get_confirmed_attribution() {
        let (conn, _path) = test_db();
        increment_confirmation(&conn, "fp1", "mod_b").unwrap();
        increment_confirmation(&conn, "fp1", "mod_a").unwrap();
        increment_confirmation(&conn, "fp1", "mod_a").unwrap();
        let results = get_confirmed_attribution(&conn, "fp1").unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].mod_id, "mod_a");
        assert_eq!(results[0].confirm_count, 2);
        assert_eq!(results[1].mod_id, "mod_b");
        assert_eq!(results[1].confirm_count, 1);
    }
}

