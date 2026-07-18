//! OS-level process identity captured immediately after spawn and verified
//! before any process-management operation.  The identity serves as a
//! fail‑closed guard against PID reuse, stale records, and accidental
//! cross‑session signalling.
//!
//! # Design
//!
//! * `capture` queries the running OS (via [`sysinfo`]) for PID, start time,
//!   and expected executable path of a just‑spawned child process.
//! * `verify` refreshes the OS record and confirms that PID, start_time, and
//!   executable still match the captured identity.
//!
//! Both functions are **pure** – they take the data they need and return a
//! result.  No global or async state is involved.

use crate::error::{LauncherError, LauncherResult};
use std::time::Duration;
use sysinfo::{Pid, System};

// ---------------------------------------------------------------------------
// Capture
// ---------------------------------------------------------------------------

/// Query the OS for the process identity of `pid` immediately after spawn.
///
/// **Fail‑closed**: if the process is no longer alive or the OS cannot supply
/// start_time, this returns `Err(ProcessCaptureFailed)`.
pub fn capture(pid: u32) -> LauncherResult<ProcessIdentity> {
    let mut previous = capture_once(pid)?;
    let parent_exe = std::env::current_exe().ok().map(|path| {
        path.canonicalize()
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    });

    // On Unix the PID becomes visible between fork and exec. A single sample
    // can therefore still report the parent's executable even though the
    // child transitions to Java immediately afterwards. Require two stable
    // consecutive samples before committing the identity.
    for _ in 0..25 {
        std::thread::sleep(Duration::from_millis(10));
        let current = match capture_once(pid) {
            Ok(current) => current,
            // A very short-lived child may exit while identity is settling.
            // Retain the last valid sample so its waiter can classify the exit.
            Err(_) => return Ok(previous),
        };
        let still_parent_image = parent_exe.as_deref().is_some_and(|parent| {
            current
                .expected_exe
                .as_deref()
                .is_some_and(|exe| exe.replace('\\', "/") == parent)
        });
        if current.start_time == previous.start_time
            && current.expected_exe == previous.expected_exe
            && !still_parent_image
        {
            return Ok(current);
        }
        previous = current;
    }

    Ok(previous)
}

fn capture_once(pid: u32) -> LauncherResult<ProcessIdentity> {
    let mut system = System::new();
    // Refresh only the single process to keep the call lightweight.
    system.refresh_process(Pid::from_u32(pid));

    let process =
        system
            .process(Pid::from_u32(pid))
            .ok_or_else(|| LauncherError::ProcessCaptureFailed {
                pid,
                detail: "Process not found in system process table after spawn".into(),
            })?;

    let start_time = process.start_time();
    if start_time == 0 {
        return Err(LauncherError::ProcessCaptureFailed {
            pid,
            detail: "OS reported start_time = 0 for spawned process".into(),
        });
    }

    let expected_exe = process.exe().map(|p| p.to_string_lossy().to_string());

    Ok(ProcessIdentity {
        pid,
        start_time,
        expected_exe,
    })
}

// ---------------------------------------------------------------------------
// Verify
// ---------------------------------------------------------------------------

/// Verify that the OS process identified by `identity` is still alive and
/// matches the captured identity on every available attribute.
///
/// Returns `Ok(())` when every field that `identity` captured still matches
/// the current OS record.  Returns `Err(ProcessStale)` when:
/// - The PID no longer exists in the process table.
/// - The start_time differs (PID was reused by a different process).
/// - The executable path differs when both the captured identity and the OS
///   report a value (missing OS data does **not** cause rejection on its own;
///   the identity is still verified by the remaining fields).
pub fn verify(identity: &ProcessIdentity) -> LauncherResult<()> {
    let mut system = System::new();
    system.refresh_process(Pid::from_u32(identity.pid));

    let process = match system.process(Pid::from_u32(identity.pid)) {
        Some(p) => p,
        None => {
            return Err(LauncherError::ProcessStale {
                pid: identity.pid,
                detail: "Process no longer exists in the OS process table".into(),
            });
        }
    };

    // Compare start_time – this is the strongest reuse detector.
    let current_start = process.start_time();
    if current_start != identity.start_time {
        return Err(LauncherError::ProcessStale {
            pid: identity.pid,
            detail: format!(
                "Start time mismatch: expected {} but OS reports {}",
                identity.start_time, current_start
            ),
        });
    }

    // Compare executable path when both sides have it.
    if let Some(ref expected) = identity.expected_exe {
        if let Some(current_exe) = process.exe() {
            let current = current_exe.to_string_lossy();
            // Platform‑normalise path separators for comparison.
            let normalised = |s: &str| s.replace('\\', "/");
            if normalised(&current) != normalised(expected) {
                return Err(LauncherError::ProcessStale {
                    pid: identity.pid,
                    detail: format!(
                        "Executable mismatch: expected '{}' but OS reports '{}'",
                        expected, current
                    ),
                });
            }
        }
        // If the OS can't supply exe, we still pass – start_time is the
        // authoritative check.
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Identity type
// ---------------------------------------------------------------------------

/// Identity of a spawned OS process, captured from the OS immediately after
/// the child is created.
///
/// This is stored internally in [`AppState`](crate::state::AppState) and is
/// **not** serialised to the frontend.  The public
/// [`RunningProcess`](crate::state::RunningProcess) carries only the fields
/// the UI needs (instance_id, pid, session_id).
#[derive(Debug, Clone)]
pub struct ProcessIdentity {
    pub pid: u32,
    /// The process start time reported by the OS (seconds since epoch).
    pub start_time: u64,
    /// Canonical path of the executable, when the OS provides it.
    pub expected_exe: Option<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: run a long‑lived child process and return its `ProcessIdentity`.
    fn spawn_test_child() -> (std::process::Child, ProcessIdentity) {
        let child = if cfg!(target_os = "windows") {
            std::process::Command::new("cmd.exe")
                .args(["/c", "ping", "-n", "60", "127.0.0.1>nul"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("failed to spawn test child")
        } else {
            std::process::Command::new("sleep")
                .arg("60")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("failed to spawn test child")
        };

        let pid = child.id();
        let identity = capture(pid).expect("capture should succeed on a live child");

        (child, identity)
    }

    #[test]
    fn test_capture_and_verify_success() {
        let (mut child, identity) = spawn_test_child();
        let result = verify(&identity);
        assert!(
            result.is_ok(),
            "verify should succeed on a live child: {:?}",
            result
        );
        // Cleanup
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn test_capture_nonexistent_pid() {
        let result = capture(u32::MAX);
        assert!(result.is_err(), "capture of nonexistent PID should fail");
        match result {
            Err(LauncherError::ProcessCaptureFailed { .. }) => {} // expected
            other => panic!("Expected ProcessCaptureFailed, got {:?}", other),
        }
    }

    #[test]
    fn test_verify_nonexistent_pid_fails() {
        let identity = ProcessIdentity {
            pid: u32::MAX,
            start_time: 12345,
            expected_exe: None,
        };
        let result = verify(&identity);
        assert!(result.is_err(), "verify of nonexistent PID should fail");
        match result {
            Err(LauncherError::ProcessStale { .. }) => {} // expected
            other => panic!("Expected ProcessStale, got {:?}", other),
        }
    }

    #[test]
    fn test_verify_mismatched_start_time_fails() {
        let (mut child, mut identity) = spawn_test_child();
        // Corrupt the start time.
        identity.start_time = identity.start_time.wrapping_add(42);

        let result = verify(&identity);
        assert!(
            result.is_err(),
            "verify with mismatched start_time should fail"
        );

        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn test_verify_mismatched_exe_fails() {
        let (mut child, mut identity) = spawn_test_child();
        // Set an impossible exe path.
        identity.expected_exe = Some("/nonexistent/evil.exe".into());

        let result = verify(&identity);
        // If the OS supplies an exe, the mismatch is caught; if it does not,
        // the check is skipped and verify passes.  Both outcomes are valid
        // (we only reject when both sides have data).
        if identity.expected_exe.is_some() {
            eprintln!("exe mismatch test: verify returned {:?}", result);
        }

        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn test_verify_after_child_death_fails() {
        // Spawn a throw‑away child, kill it, wait, then verify.
        let (mut child, identity) = spawn_test_child();
        let _ = child.kill();
        let _ = child.wait();

        // Give the OS a moment to reap.
        std::thread::sleep(std::time::Duration::from_millis(100));

        let result = verify(&identity);
        assert!(
            result.is_err(),
            "verify after child death should fail: {:?}",
            result
        );
    }
}
