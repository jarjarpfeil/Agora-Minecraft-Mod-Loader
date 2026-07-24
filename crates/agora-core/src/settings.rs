use rusqlite::Connection;

use crate::ctx::Ctx;
use crate::error::{LauncherError, LauncherResult};

// ---------------------------------------------------------------------------
// SettingsService — core-owned typed service
// ---------------------------------------------------------------------------

/// Core-owned service for reading user settings from `local_state.db`.
///
/// Every method opens a fresh read-only connection via `Ctx` paths; desktop
/// adapters must use this service rather than opening the database directly.
#[derive(Clone)]
pub struct SettingsService {
    ctx: Ctx,
}

impl SettingsService {
    pub fn new(ctx: Ctx) -> Self {
        Self { ctx }
    }

    fn connection(&self) -> LauncherResult<rusqlite::Connection> {
        crate::db::local_state_connection(&self.ctx.paths.local_state_db()).map_err(|error| {
            LauncherError::Generic {
                code: "ERR_LOCAL_STATE_FAILED".into(),
                message: error.to_string(),
            }
        })
    }

    /// Get a setting value by key. Returns `Ok(None)` when the key does not
    /// exist.
    pub fn get(&self, key: &str) -> LauncherResult<Option<serde_json::Value>> {
        let conn = self.connection()?;
        get(&conn, key).map_err(|error| LauncherError::Generic {
            code: "ERR_LOCAL_STATE_FAILED".into(),
            message: error.to_string(),
        })
    }

    /// Set a setting value by key. Serializes `value` to JSON text and
    /// upserts it into `local_state.db`.
    pub fn set(&self, key: &str, value: &serde_json::Value) -> LauncherResult<()> {
        let conn = self.connection()?;
        let text = serde_json::to_string(value).map_err(|error| LauncherError::Generic {
            code: "ERR_LOCAL_STATE_FAILED".into(),
            message: error.to_string(),
        })?;
        set(&conn, key, &text).map_err(|error| LauncherError::Generic {
            code: "ERR_LOCAL_STATE_FAILED".into(),
            message: error.to_string(),
        })?;
        Ok(())
    }

    /// Get a setting as a boolean. Accepts genuine JSON booleans and legacy
    /// string/number encodings left by older frontend versions. Returns
    /// `false` for missing or unexpected values.
    pub fn get_bool(&self, key: &str) -> LauncherResult<bool> {
        Ok(self.get(key)?.map(parse_bool_value).unwrap_or(false))
    }

    /// Get a setting as an optional string. Returns `Ok(None)` when the key
    /// is absent or the value is not a JSON string.
    pub fn get_string(&self, key: &str) -> LauncherResult<Option<String>> {
        Ok(self.get(key)?.and_then(|v| v.as_str().map(String::from)))
    }

    /// List all settings as (key, raw JSON string) pairs.
    pub fn list(&self) -> LauncherResult<Vec<(String, String)>> {
        let conn = self.connection()?;
        list(&conn).map_err(|error| LauncherError::Generic {
            code: "ERR_LOCAL_STATE_FAILED".into(),
            message: error.to_string(),
        })
    }

    /// List all settings as (key, parsed JSON value) pairs.
    pub fn list_parsed(&self) -> LauncherResult<Vec<(String, serde_json::Value)>> {
        let conn = self.connection()?;
        list_parsed(&conn).map_err(|error| LauncherError::Generic {
            code: "ERR_LOCAL_STATE_FAILED".into(),
            message: error.to_string(),
        })
    }
}

fn parse_bool_value(value: serde_json::Value) -> bool {
    match value {
        serde_json::Value::Bool(value) => value,
        serde_json::Value::String(value) => value == "true" || value == "1",
        serde_json::Value::Number(value) => value.as_i64() == Some(1),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Free functions (preserved, take &Connection)
// ---------------------------------------------------------------------------

/// List all settings as (key, raw_JSON_string) pairs.
pub fn list(conn: &Connection) -> anyhow::Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare("SELECT key, value_json FROM user_settings ORDER BY key")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// List all settings as (key, parsed_JSON_value) pairs.
///
/// Entries whose `value_json` is not valid JSON yield `Value::Null`.
pub fn list_parsed(conn: &Connection) -> anyhow::Result<Vec<(String, serde_json::Value)>> {
    let raw = list(conn)?;
    Ok(raw
        .into_iter()
        .map(|(k, v)| {
            let val = serde_json::from_str(&v).unwrap_or(serde_json::Value::Null);
            (k, val)
        })
        .collect())
}

/// Get a single setting by key.
///
/// Returns `None` when the key does not exist.
pub fn get(conn: &Connection, key: &str) -> anyhow::Result<Option<serde_json::Value>> {
    let mut stmt = conn.prepare("SELECT value_json FROM user_settings WHERE key = ?1")?;
    let mut rows = stmt.query([key])?;
    match rows.next()? {
        Some(row) => {
            let text: String = row.get(0)?;
            let value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
            Ok(Some(value))
        }
        None => Ok(None),
    }
}

/// Set a setting. The value string is parsed as JSON; if parsing fails it is
/// stored as a plain JSON string.  Returns the stored JSON value.
pub fn set(conn: &Connection, key: &str, value: &str) -> anyhow::Result<serde_json::Value> {
    let parsed: serde_json::Value =
        serde_json::from_str(value).unwrap_or(serde_json::Value::String(value.to_owned()));
    let text = serde_json::to_string(&parsed)?;
    conn.execute(
        "INSERT INTO user_settings (key, value_json) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
        rusqlite::params![key, text],
    )?;
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS user_settings (
                 key TEXT PRIMARY KEY,
                 value_json TEXT NOT NULL
             );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn list_empty_db() {
        let conn = mem_conn();
        let rows = list(&conn).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn list_parsed_empty_db() {
        let conn = mem_conn();
        let rows = list_parsed(&conn).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn set_and_get() {
        let conn = mem_conn();
        let stored = set(&conn, "test_key", "\"hello\"").unwrap();
        assert_eq!(stored, serde_json::Value::String("hello".into()));

        let got = get(&conn, "test_key").unwrap();
        assert_eq!(got, Some(serde_json::Value::String("hello".into())));
    }

    #[test]
    fn set_plain_string_fallback() {
        let conn = mem_conn();
        let stored = set(&conn, "plain", "hello").unwrap();
        assert_eq!(stored, serde_json::Value::String("hello".into()));

        let got = get(&conn, "plain").unwrap();
        assert_eq!(got, Some(serde_json::Value::String("hello".into())));
    }

    #[test]
    fn set_json_value() {
        let conn = mem_conn();
        let stored = set(&conn, "count", "42").unwrap();
        assert_eq!(stored, serde_json::Value::Number(42.into()));

        let got = get(&conn, "count").unwrap();
        assert_eq!(got, Some(serde_json::Value::Number(42.into())));
    }

    #[test]
    fn get_missing_key() {
        let conn = mem_conn();
        let got = get(&conn, "nope").unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn list_returns_all_keys() {
        let conn = mem_conn();
        set(&conn, "a", "\"1\"").unwrap();
        set(&conn, "b", "\"2\"").unwrap();
        let rows = list(&conn).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "a");
        assert_eq!(rows[1].0, "b");
    }

    #[test]
    fn set_overwrites_existing() {
        let conn = mem_conn();
        set(&conn, "k", "\"old\"").unwrap();
        set(&conn, "k", "\"new\"").unwrap();
        let got = get(&conn, "k").unwrap();
        assert_eq!(got, Some(serde_json::Value::String("new".into())));
    }

    #[test]
    fn list_parsed_returns_valid_json() {
        let conn = mem_conn();
        set(&conn, "bool", "true").unwrap();
        set(&conn, "num", "99").unwrap();
        set(&conn, "str", "\"text\"").unwrap();
        let rows = list_parsed(&conn).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].1, serde_json::Value::Bool(true));
        assert_eq!(rows[1].1, serde_json::Value::Number(99.into()));
        assert_eq!(rows[2].1, serde_json::Value::String("text".into()));
    }

    // -----------------------------------------------------------------------
    // SettingsService tests
    // -----------------------------------------------------------------------

    fn seeded_ctx() -> (Ctx, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "agora-settings-service-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let ctx = Ctx::for_testing(root.clone());
        crate::db::init_local_state_db(&ctx.paths.local_state_db()).unwrap();
        // Seed known values via the free function so the service can read them.
        let conn = crate::db::local_state_connection(&ctx.paths.local_state_db()).unwrap();
        set(&conn, "service_bool_key", "true").unwrap();
        set(&conn, "service_str_key", "\"hello\"").unwrap();
        conn.close().unwrap();
        (ctx, root)
    }

    #[test]
    fn service_get_returns_seeded_value() {
        let (ctx, root) = seeded_ctx();
        let svc = SettingsService::new(ctx);
        let val = svc.get("service_str_key").unwrap();
        assert_eq!(val, Some(serde_json::Value::String("hello".into())));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn service_get_missing_returns_none() {
        let (ctx, root) = seeded_ctx();
        let svc = SettingsService::new(ctx);
        let val = svc.get("nonexistent").unwrap();
        assert_eq!(val, None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn service_get_bool_true() {
        let (ctx, root) = seeded_ctx();
        let svc = SettingsService::new(ctx);
        assert!(svc.get_bool("service_bool_key").unwrap());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn service_get_bool_missing_is_false() {
        let (ctx, root) = seeded_ctx();
        let svc = SettingsService::new(ctx);
        assert!(!svc.get_bool("nonexistent").unwrap());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn service_get_bool_accepts_legacy_string_true() {
        let (ctx, root) = seeded_ctx();
        let conn = crate::db::local_state_connection(&ctx.paths.local_state_db()).unwrap();
        set(&conn, "string_true", "\"true\"").unwrap();
        conn.close().unwrap();
        let svc = SettingsService::new(ctx);
        assert!(svc.get_bool("string_true").unwrap());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn service_get_bool_accepts_legacy_number_one() {
        let (ctx, root) = seeded_ctx();
        let conn = crate::db::local_state_connection(&ctx.paths.local_state_db()).unwrap();
        set(&conn, "number_one", "1").unwrap();
        conn.close().unwrap();
        let svc = SettingsService::new(ctx);
        assert!(svc.get_bool("number_one").unwrap());
        let _ = std::fs::remove_dir_all(root);
    }
}
