use crate::browse_cache::SharedBrowseCache;
use crate::error::LauncherResult;
use crate::models::ModVersionCandidate;
use crate::msa::LoginFlow;
use crate::process_identity::{self, ProcessIdentity};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Information about the current directly-launched Minecraft process.
///
/// This is the public representation sent to the frontend.  Internal OS-level
/// identity (start time, executable path) is kept in [`AppState::process_identity`]
/// and verified before any process-management operation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RunningProcess {
    pub instance_id: String,
    pub pid: u32,
    /// Monotonically increasing launch session ID, used to disambiguate
    /// late events from a previous launch of the same instance.
    pub session_id: u64,
}

/// Reservation held while a direct launch is preparing network assets and the
/// Java command.  It closes the check-then-spawn race without keeping the
/// application-state mutex locked across asynchronous work.
#[derive(Debug, Clone)]
pub struct LaunchReservation {
    pub instance_id: String,
}

/// A short-lived release-resolution result shared by explicit update scans.
#[derive(Clone)]
pub struct UpdateCandidateCacheEntry {
    pub fetched_at: std::time::Instant,
    pub candidates: Vec<ModVersionCandidate>,
}

/// Lightweight shared application state.
pub struct AppState {
    /// Shared HTTP client for all network operations (MSA, Modrinth, etc.)
    pub client: reqwest::Client,
    /// In-flight MSA login flow (ephemeral — only alive between begin/finish).
    /// If the app crashes, the flow is lost and the user re-authenticates.
    pub login_flow: Option<LoginFlow>,
    /// Shared browse cache for paginated Modrinth + registry results.
    pub browse_cache: SharedBrowseCache,
    /// Tracked directly-launched process, stored so the frontend can recover
    /// running state after navigation or reload.
    pub running_process: Option<RunningProcess>,
    /// Internal OS-level identity of the tracked process (start time,
    /// executable path).  This is **not** serialised to the frontend and is
    /// verified before any process-management operation (kill, state query).
    ///
    /// **Backend-restart semantics**: `AppState::new()` starts with no process
    /// identity, and persisted PIDs from a previous launcher session are never
    /// adopted.  If the launcher restarts while a game is running, the game
    /// process is considered detached (non-actionable) — the frontend will
    /// report phase = 'idle' until the user launches again.
    pub process_identity: Option<ProcessIdentity>,
    /// A launch that has exclusive ownership but has not spawned Java yet.
    pub launch_reservation: Option<LaunchReservation>,
    /// Sessions for which the user explicitly requested termination.  The exit
    /// classifier consumes these so a user stop is never reported as a crash.
    pub user_cancelled_launches: HashSet<u64>,
    /// Instance IDs with an active install transaction.
    pub active_install_instances: HashSet<String>,
    /// Per-instance serialization for LKG read/modify/write promotion. Delegated
    /// monitors can overlap a newer direct launch, so the global launch lock is
    /// not sufficient for protecting `lkg.json`.
    pub lkg_locks: HashMap<String, Arc<Mutex<()>>>,
    /// Release candidates shared across explicit update scans for five minutes.
    pub update_candidate_cache: HashMap<String, UpdateCandidateCacheEntry>,
}

impl AppState {
    pub fn new() -> Self {
        // AppState client is used by Tauri-hosted commands.
        // The timeout is short because the AppState client is for lightweight
        // health checks and status queries. Heavy downloads use HttpClients
        // from the managed CoreContext.
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("AgoraLauncher/1.0")
            .build()
            .unwrap_or_else(|_| {
                reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .expect("reqwest::Client::builder().build() must succeed")
            });
        Self {
            client,
            login_flow: None,
            browse_cache: crate::browse_cache::new_cache(),
            running_process: None,
            process_identity: None,
            launch_reservation: None,
            user_cancelled_launches: HashSet::new(),
            active_install_instances: HashSet::new(),
            lkg_locks: HashMap::new(),
            update_candidate_cache: HashMap::new(),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// Tauri-managed wrapper around the shared application state.
pub type LauncherState = Arc<Mutex<AppState>>;

// ---------------------------------------------------------------------------
// Pure verification helper — callable from any async context without holding
// the AppState mutex.
// ---------------------------------------------------------------------------

/// Capture the OS identity for a just‑spawned PID and return it.
///
/// On failure the caller should kill the owned child and abort the launch.
pub fn capture_identity(pid: u32) -> LauncherResult<ProcessIdentity> {
    process_identity::capture(pid)
}

/// Verify that the captured identity still matches the live OS process.
///
/// This is a pure, synchronous helper intended to be called **outside** the
/// AppState async mutex.  Pass the [`ProcessIdentity`] you snapshotted from
/// state and re‑check session freshness after re‑acquiring the lock.
pub fn verify_identity(identity: &ProcessIdentity) -> LauncherResult<()> {
    process_identity::verify(identity)
}
