use crate::ctx::Ctx;
use crate::error::{LauncherError, LauncherResult};
use crate::java::{self, JavaInstallation};
use crate::network::{NetworkCategory, NetworkPolicy};
use crate::runtime_catalog::RuntimeCatalog;
use crate::runtime_manager::{self, RuntimeProgress};
use std::sync::Arc;

#[derive(Clone)]
pub struct RuntimeService {
    ctx: Ctx,
}

impl RuntimeService {
    pub fn new(ctx: Ctx) -> Self {
        Self { ctx }
    }

    fn registry_conn(&self) -> Option<rusqlite::Connection> {
        let path = self.ctx.paths.registry_db();
        if path.exists() {
            rusqlite::Connection::open_with_flags(
                &path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                    | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .ok()
        } else {
            None
        }
    }

    pub fn effective_catalog(&self) -> RuntimeCatalog {
        let reg_conn = self.registry_conn();
        RuntimeCatalog::effective(reg_conn.as_ref())
    }

    pub fn list_candidates(&self) -> LauncherResult<Vec<JavaInstallation>> {
        let runtimes_root = self.ctx.paths.java_runtimes_root();
        let minecraft_dir = crate::paths::minecraft_dir();
        Ok(java::detect_java_candidates(
            Some(&runtimes_root),
            minecraft_dir.as_deref(),
        ))
    }

    pub async fn ensure_runtime(
        &self,
        major: u32,
        policy: NetworkPolicy,
        progress: Arc<dyn RuntimeProgress>,
    ) -> LauncherResult<JavaInstallation> {
        let mode = self.get_java_runtime_mode()?;
        if mode != "automatic" {
            return Err(LauncherError::Generic {
                code: "ERR_JAVA_RUNTIME_MODE".into(),
                message: format!(
                    "Java runtime mode is '{mode}'. \
                     Set it to 'automatic' to allow managed provisioning, \
                     or use a system-installed JRE matching Java {major}."
                ),
            });
        }

        policy.check(NetworkCategory::JavaRuntime)?;

        let runtimes_root = self.ctx.paths.java_runtimes_root();
        let catalog = self.effective_catalog();
        let lock_manager = self.ctx.lock_manager.clone();
        tokio::task::spawn_blocking(move || {
            runtime_manager::ensure_runtime(
                &runtimes_root,
                major,
                &catalog,
                &policy,
                Some(progress.as_ref()),
                Some(&lock_manager),
            )
        })
        .await
        .map_err(|error| LauncherError::Generic {
            code: "ERR_JAVA_PROVISION".into(),
            message: format!("Java provisioning task failed: {error}"),
        })?
    }

    pub fn remove_unused(&self) -> LauncherResult<usize> {
        let runtimes_root = self.ctx.paths.java_runtimes_root();
        let catalog = self.effective_catalog();
        runtime_manager::remove_unused(&runtimes_root, &catalog, &[])
    }

    pub fn get_java_runtime_mode(&self) -> LauncherResult<String> {
        let db_path = self.ctx.paths.local_state_db();
        if !db_path.exists() {
            return Ok("automatic".to_string());
        }
        let conn = crate::db::local_state_connection(&db_path).map_err(|error| {
            LauncherError::Generic {
                code: "ERR_LOCAL_STATE_FAILED".into(),
                message: error.to_string(),
            }
        })?;
        Ok(crate::db::get_setting(&conn, "java_runtime_mode")
            .ok()
            .flatten()
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "automatic".to_string()))
    }

    pub fn inspect(&self, path: &std::path::Path) -> LauncherResult<JavaInstallation> {
        crate::java::inspect_java(path).ok_or_else(|| LauncherError::Generic {
            code: "ERR_JAVA_INSPECT".into(),
            message: format!("Could not inspect Java executable at '{}'", path.display()),
        })
    }

    pub fn network_policy(&self) -> LauncherResult<NetworkPolicy> {
        let db_path = self.ctx.paths.local_state_db();
        if !db_path.exists() {
            return Ok(NetworkPolicy::all_enabled());
        }
        let conn = crate::db::local_state_connection(&db_path).map_err(|error| {
            LauncherError::Generic {
                code: "ERR_LOCAL_STATE_FAILED".into(),
                message: error.to_string(),
            }
        })?;
        Ok(NetworkPolicy::from_db(&conn))
    }
}
