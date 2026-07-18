//! Governance network actions: pure logic layer.
//!
//! Tauri-coupled functions that require `&tauri::AppHandle` (fetch_triage_poll,
//! flag_review) live in the desktop crate's governance shim. This module hosts
//! the constants, types, pure DB-bound logic, and the core-owned
//! [`GovernanceService`] for rate-limit operations against `local_state.db`.

use crate::ctx::Ctx;
use crate::db::FlagRateLimit;
use crate::error::{LauncherError, LauncherResult};
use rusqlite::Connection;
use serde::Serialize;

// --- Repository resolution ---

/// Resolve the governance repository with the same priority as the registry repo:
/// 1. CLI override (passed as parameter)
/// 2. `AGORA_REGISTRY_REPO` environment variable
/// 3. Built-in default: `"jarjarpfeil/Agora-Launcher"`
pub fn governance_repo(cli_override: Option<&str>) -> String {
    cli_override
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("AGORA_REGISTRY_REPO").ok())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "jarjarpfeil/Agora-Launcher".into())
}

/// Admin-alerts repo is where curator flag issues are filed. Configurable at
/// build time via AGORA_ADMIN_ALERTS_REPO; defaults to the same owner as the
/// governance/registry repo.
pub const AGORA_ADMIN_ALERTS_REPO: &str = match option_env!("AGORA_ADMIN_ALERTS_REPO") {
    Some(v) => v,
    None => "jarjarpfeil/admin-alerts",
};

// --- Types ---

/// A live triage poll for a given mod, fetched from GitHub Discussions.
#[derive(Debug, Serialize, Clone)]
pub struct TriagePoll {
    pub discussion_url: Option<String>,
    pub keep_votes: i64,
    pub remove_votes: i64,
}

// --- Rate limit status ---

/// Return the current flag rate-limit status for a local state connection.
pub fn get_flag_rate_limit(conn: &Connection) -> LauncherResult<FlagRateLimit> {
    let now_unix = chrono::Utc::now().timestamp();
    crate::db::get_flag_rate_limit_status(conn, now_unix)
        .map_err(|_| LauncherError::LocalStateFailed)
}

// ---------------------------------------------------------------------------
// GovernanceService — core-owned typed service
// ---------------------------------------------------------------------------

/// Core-owned service for flag rate-limit operations against `local_state.db`.
///
/// Every method opens a fresh connection via `Ctx` paths; desktop adapters
/// must use this service rather than opening the database directly.
#[derive(Clone)]
pub struct GovernanceService {
    ctx: Ctx,
}

impl GovernanceService {
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

    /// Return the current flag rate-limit status.
    pub fn get_flag_rate_limit(&self) -> LauncherResult<FlagRateLimit> {
        let conn = self.connection()?;
        let now_unix = chrono::Utc::now().timestamp();
        crate::db::get_flag_rate_limit_status(&conn, now_unix)
            .map_err(|_| LauncherError::LocalStateFailed)
    }

    /// Record a flag submission for rate-limit tracking.
    pub fn record_flag_submission(&self, timestamp: i64) -> LauncherResult<()> {
        let conn = self.connection()?;
        crate::db::record_flag_submission(&conn, timestamp)
            .map_err(|_| LauncherError::LocalStateFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded_ctx() -> (Ctx, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "agora-governance-service-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let ctx = Ctx::for_testing(root.clone());
        crate::db::init_local_state_db(&ctx.paths.local_state_db()).unwrap();
        let conn = crate::db::local_state_connection(&ctx.paths.local_state_db()).unwrap();
        // Seed a submission so rate-limit queries return non-zero counts.
        crate::db::record_flag_submission(&conn, 1000).unwrap();
        conn.close().unwrap();
        (ctx, root)
    }

    #[test]
    fn service_get_flag_rate_limit_returns_ok() {
        let (ctx, root) = seeded_ctx();
        let svc = GovernanceService::new(ctx);
        let limit = svc.get_flag_rate_limit().unwrap();
        // One submission at unix 1000; now is wall-clock so hour/day windows
        // will not overlap that seed. Remaining should be at max.
        assert!(limit.remaining_hour > 0);
        assert!(limit.remaining_day > 0);
        assert!(limit.can_flag);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn service_record_flag_submission_updates_limit() {
        let (ctx, root) = seeded_ctx();
        let svc = GovernanceService::new(ctx);
        let before = svc.get_flag_rate_limit().unwrap();
        assert!(before.can_flag);

        // Record a submission in the recent window.
        let now = chrono::Utc::now().timestamp();
        svc.record_flag_submission(now).unwrap();

        let after = svc.get_flag_rate_limit().unwrap();
        // The after count should reflect at least the one we just inserted
        // (the seed at unix 1000 is outside any real-time window).
        assert!(
            after.remaining_hour < before.remaining_hour
                || after.remaining_day < before.remaining_day
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
