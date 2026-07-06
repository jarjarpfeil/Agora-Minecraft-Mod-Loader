//! Global GitHub API rate limiter.
//!
//! Provides three guarantees:
//! 1. **Concurrency cap** — at most 4 simultaneous outbound requests to any
//!    `api.github.com` endpoint, enforced via a shared [`Semaphore`].
//! 2. **Cooldown propagation** — when any caller receives a `429` or a `403`
//!    with `x-ratelimit-remaining: 0`, it calls [`report_rate_limit`] which
//!    freezes ALL subsequent GitHub traffic for `Retry-After` seconds (plus
//!    jitter).  This prevents the "penalty box reset" loop where every new
//!    request extends GitHub's ban timer.
//! 3. **Shared HTTP client** — a single [`reqwest::Client`] with a 30-second
//!    timeout and proper `User-Agent`, reused by every call-site so we don't
//!    open a fresh TCP/TLS connection pool per request.

use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

// ---------------------------------------------------------------------------
// Tuning knobs
// ---------------------------------------------------------------------------

/// Hard ceiling on concurrent outbound GitHub API requests.
const MAX_CONCURRENT_GITHUB: usize = 4;

/// Default cooldown (seconds) when the `Retry-After` header is absent.
const DEFAULT_COOLDOWN_SECS: u64 = 65;

/// Maximum jitter added on top of `Retry-After` (10-25 % of the base, capped
/// at 30 s so we never wait absurdly long).
const MAX_JITTER_SECS: u64 = 30;

// ---------------------------------------------------------------------------
// Shared client
// ---------------------------------------------------------------------------

static GITHUB_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent("AgoraLauncher/1.0")
        .timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(MAX_CONCURRENT_GITHUB)
        .build()
        .expect("failed to build shared GitHub HTTP client")
});

/// Returns the shared [`reqwest::Client`] used for **all** GitHub API calls.
pub fn github_client() -> &'static reqwest::Client {
    &GITHUB_CLIENT
}

// ---------------------------------------------------------------------------
// Concurrency semaphore
// ---------------------------------------------------------------------------

static GITHUB_SEM: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(MAX_CONCURRENT_GITHUB)));

/// Acquire a permit before making **any** GitHub API request.
///
/// The returned [`OwnedSemaphorePermit`] enforces the concurrency cap.  It is
/// automatically released when dropped (i.e. when the request completes).
///
/// If a cooldown is active this function sleeps until it expires, *then*
/// acquires the semaphore — so callers never fire a request into a known ban.
pub async fn acquire_github_permit() -> OwnedSemaphorePermit {
    // 1. Wait out any active cooldown.
    wait_for_cooldown().await;
    // 2. Grab a concurrency slot.
    GITHUB_SEM
        .clone()
        .acquire_owned()
        .await
        .expect("github semaphore closed")
}

// ---------------------------------------------------------------------------
// Cooldown state
// ---------------------------------------------------------------------------

static COOLDOWN_UNTIL: LazyLock<Mutex<Option<Instant>>> =
    LazyLock::new(|| Mutex::new(None));

/// Record a rate-limit response.  Sets a global cooldown so that **every**
/// GitHub call in the process blocks until the ban lifts.
///
/// `retry_after_header` is the numeric value of the `Retry-After` response
/// header, if present.  When absent we fall back to [`DEFAULT_COOLDOWN_SECS`].
pub async fn report_rate_limit(retry_after_header: Option<u64>) {
    let base = retry_after_header.unwrap_or(DEFAULT_COOLDOWN_SECS);
    // Deterministic-ish jitter (10-25 % of base, capped).
    let jitter_range = (base / 4).min(MAX_JITTER_SECS).max(1);
    let jitter = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64)
        % jitter_range;
    let total = base + jitter;
    let expiry = Instant::now() + Duration::from_secs(total);

    let mut lock = COOLDOWN_UNTIL.lock().await;
    // Only ever *extend* the cooldown, never shorten it.
    if lock.map_or(true, |cur| expiry > cur) {
        *lock = Some(expiry);
    }
    eprintln!("[GITHUB_RATE_LIMIT] cooldown for {total}s — all GitHub traffic paused");
}

/// Clear the cooldown (e.g. after a successful request confirms we're unbanned).
pub async fn clear_rate_limit() {
    let mut lock = COOLDOWN_UNTIL.lock().await;
    if lock.is_some() {
        eprintln!("[GITHUB_RATE_LIMIT] cooldown cleared");
    }
    *lock = None;
}

/// Returns `true` if a cooldown is currently active.
pub async fn is_rate_limited() -> bool {
    let lock = COOLDOWN_UNTIL.lock().await;
    lock.map_or(false, |until| Instant::now() < until)
}

/// Sleep until the cooldown expires.  Returns immediately if no cooldown is
/// active.
async fn wait_for_cooldown() {
    loop {
        let sleep_for = {
            let lock = COOLDOWN_UNTIL.lock().await;
            match *lock {
                Some(until) if Instant::now() < until => Some(until - Instant::now()),
                Some(_) => {
                    // Expired — clear it and proceed.
                    drop(lock);
                    let mut w = COOLDOWN_UNTIL.lock().await;
                    *w = None;
                    return;
                }
                None => return,
            }
        };
        if let Some(dur) = sleep_for {
            eprintln!("[GITHUB_RATE_LIMIT] waiting {dur:?} for cooldown to expire");
            tokio::time::sleep(dur).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Response helpers
// ---------------------------------------------------------------------------

/// Parse the `Retry-After` header from a GitHub response (seconds).
pub fn parse_retry_after(resp: &reqwest::Response) -> Option<u64> {
    resp.headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
}

/// Returns `true` if the response indicates a rate-limit that should trigger
/// the global cooldown.  Handles both primary limits (hourly quota exhausted)
/// and secondary abuse limits (burst too fast).
pub fn is_rate_limit_response(resp: &reqwest::Response) -> bool {
    let status = resp.status();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return true;
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        // Secondary abuse limits always include a `retry-after` header even
        // when `x-ratelimit-remaining` is still high.
        if resp.headers().contains_key("retry-after") {
            return true;
        }
        // Primary rate limits (hourly quota exhausted).
        if let Some(remaining) = resp.headers().get("x-ratelimit-remaining") {
            if remaining == "0" {
                return true;
            }
        }
    }
    false
}
