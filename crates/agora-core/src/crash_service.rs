//! Core-owned CrashService — safe enable/disable operations for mod files,
//! crash-diagnostics shim operations (check for fresh reports, list, read,
//! triage with optional registry augmentation), and crash investigation
//! pipeline (fingerprinting, scoring, ruled-out/confirmation/survival
//! decisions, and SuggestedAction determination).
//!
//! Owns: instance and filename validation, instance locking, atomic rename,
//! manifest update, no-clobber enforcement for mod enable/disable, and
//! crash-diagnostics disk/database access via Ctx paths.

use std::collections::{HashMap, HashSet};

use crate::crash_diagnostics::{CrashReportInfo, CrashTriageResult};
use crate::ctx::Ctx;
use crate::error::{LauncherError, LauncherResult};
use crate::helpers::{rename_in_content_dir, set_enabled_in_all_arrays};
use crate::lock_manager::LockResource;
use crate::models::InstanceManifest;
use crate::registry;
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct CrashService {
    ctx: Ctx,
}

impl CrashService {
    pub fn new(ctx: Ctx) -> Self {
        Self { ctx }
    }

    /// Rename `mods/<filename>` to `mods/<filename>.disabled` atomically.
    ///
    /// Validates instance ID and filename, acquires an instance lock, and
    /// updates the manifest to mark the mod as disabled. Never clobbers an
    /// existing `.disabled` file. Returns `Ok(())` if the mod is already
    /// disabled or if the source file does not exist.
    pub fn disable_mod(&self, instance_id: &str, filename: &str) -> LauncherResult<()> {
        let sanitized = validate_instance_id(instance_id)?;
        validate_filename(filename)?;

        let _guard = self
            .ctx
            .lock_manager
            .acquire(LockResource::Instance(sanitized.clone()), "disable_mod")?;

        let mods_dir = self.ctx.paths.instance_dir(&sanitized)?.join("mods");

        let source = mods_dir.join(filename);
        let dest = mods_dir.join(format!("{}.disabled", filename));

        if dest.exists() {
            return Ok(());
        }

        if !source.exists() {
            return Ok(());
        }

        std::fs::rename(&source, &dest).map_err(|e| LauncherError::Generic {
            code: "ERR_MOD_DISABLE".into(),
            message: format!("Failed to rename mod file '{filename}': {e}"),
        })?;

        let manifest_path = self.ctx.paths.instance_manifest(&sanitized)?;
        update_manifest_disable(&manifest_path, filename)?;

        Ok(())
    }

    /// Reverse of `disable_mod`: rename `mods/<filename>.disabled` back to
    /// `mods/<filename>` atomically.
    ///
    /// Validates instance ID and filename, acquires an instance lock, and
    /// updates the manifest to mark the mod as enabled. Never clobbers an
    /// existing target file. Returns `Ok(())` if the mod is already enabled.
    pub fn enable_mod(&self, instance_id: &str, filename: &str) -> LauncherResult<()> {
        let sanitized = validate_instance_id(instance_id)?;
        validate_filename(filename)?;

        let _guard = self
            .ctx
            .lock_manager
            .acquire(LockResource::Instance(sanitized.clone()), "enable_mod")?;

        let mods_dir = self.ctx.paths.instance_dir(&sanitized)?.join("mods");

        let disabled_path = mods_dir.join(format!("{}.disabled", filename));
        let source = mods_dir.join(filename);

        if !disabled_path.exists() {
            return Ok(());
        }

        if source.exists() {
            return Ok(());
        }

        std::fs::rename(&disabled_path, &source).map_err(|e| LauncherError::Generic {
            code: "ERR_MOD_ENABLE".into(),
            message: format!("Failed to rename mod file '{filename}': {e}"),
        })?;

        let manifest_path = self.ctx.paths.instance_manifest(&sanitized)?;
        update_manifest_enable(&manifest_path, filename)?;

        Ok(())
    }

    /// Disable an artifact (any content type) by renaming the file to
    /// `<filename>.disabled` and setting `enabled: false` in the manifest.
    ///
    /// Searches all content subdirectories (mods, resourcepacks, shaderpacks,
    /// datapacks, saves). Returns an error if the file is not found.
    pub fn disable_artifact(&self, instance_id: &str, filename: &str) -> LauncherResult<()> {
        let sanitized = crate::paths::sanitize_id(instance_id);
        let _guard = self.ctx.lock_manager.acquire(
            LockResource::Instance(sanitized.clone()),
            "disable_artifact",
        )?;
        let dir = self.ctx.paths.instance_dir(&sanitized)?;

        let renamed = rename_in_content_dir(&dir, filename, false).is_some();
        if !renamed {
            return Err(LauncherError::Generic {
                code: "ERR_MOD_FILE_NOT_FOUND".to_string(),
                message: format!("File '{filename}' not found in any content directory."),
            });
        }

        let manifest_path = self.ctx.paths.instance_manifest(&sanitized)?;
        let result = if manifest_path.exists() {
            (|| -> LauncherResult<()> {
                let text = std::fs::read_to_string(&manifest_path)
                    .map_err(|_| LauncherError::InstanceCreateFailed)?;
                let mut manifest: crate::models::InstanceManifest =
                    serde_json::from_str(&text).map_err(|_| LauncherError::InstanceCreateFailed)?;
                set_enabled_in_all_arrays(&mut manifest, filename, false);
                crate::helpers::atomic_write_manifest(&manifest_path, &manifest)
            })()
        } else {
            Ok(())
        };
        if let Err(error) = result {
            let _ = rename_in_content_dir(&dir, filename, true);
            return Err(error);
        }

        Ok(())
    }

    /// Re-enable a disabled artifact by renaming `<filename>.disabled` back to
    /// `<filename>` and setting `enabled: true` in the manifest.
    ///
    /// Searches all content subdirectories. Returns `Ok(())` if the file is
    /// already enabled or not found.
    pub fn enable_artifact(&self, instance_id: &str, filename: &str) -> LauncherResult<()> {
        let sanitized = crate::paths::sanitize_id(instance_id);
        let _guard = self
            .ctx
            .lock_manager
            .acquire(LockResource::Instance(sanitized.clone()), "enable_artifact")?;
        let dir = self.ctx.paths.instance_dir(&sanitized)?;

        let renamed = rename_in_content_dir(&dir, filename, true).is_some();

        let manifest_path = self.ctx.paths.instance_manifest(&sanitized)?;
        let result = if manifest_path.exists() {
            (|| -> LauncherResult<()> {
                let text = std::fs::read_to_string(&manifest_path)
                    .map_err(|_| LauncherError::InstanceCreateFailed)?;
                let mut manifest: crate::models::InstanceManifest =
                    serde_json::from_str(&text).map_err(|_| LauncherError::InstanceCreateFailed)?;
                set_enabled_in_all_arrays(&mut manifest, filename, true);
                crate::helpers::atomic_write_manifest(&manifest_path, &manifest)
            })()
        } else {
            Ok(())
        };
        if let Err(error) = result {
            if renamed {
                let _ = rename_in_content_dir(&dir, filename, false);
            }
            return Err(error);
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Crash-diagnostics shim operations
    // -----------------------------------------------------------------------

    /// Open a connection to the local state database via Ctx paths.
    fn connection(&self) -> LauncherResult<rusqlite::Connection> {
        crate::db::local_state_connection(&self.ctx.paths.local_state_db()).map_err(|error| {
            LauncherError::Generic {
                code: "ERR_LOCAL_STATE_FAILED".into(),
                message: error.to_string(),
            }
        })
    }

    /// Check whether a fresh crash report appeared after the instance's
    /// `last_launched_at`. Returns the newest qualifying file.
    ///
    /// Gracefully degrades: missing instance, absent `last_launched_at`, or
    /// inaccessible crash-reports directory all yield `Ok(None)`.
    pub fn check_for_crash(&self, instance_id: &str) -> LauncherResult<Option<CrashReportInfo>> {
        let sanitized = validate_instance_id(instance_id)?;
        let conn = match self.connection() {
            Ok(c) => c,
            Err(_) => return Ok(None),
        };
        let last_launched_at = match crate::db::get_instance(&conn, &sanitized)
            .ok()
            .and_then(|r| r)
            .and_then(|r| r.last_launched_at)
        {
            Some(ts) => ts,
            None => return Ok(None),
        };
        let dir = match self.ctx.paths.instance_dir(&sanitized) {
            Ok(d) => d.join("crash-reports"),
            Err(_) => return Ok(None),
        };
        crate::crash_diagnostics::check_for_crash_from_path(&dir, &last_launched_at)
    }

    /// List all crash report files for an instance, newest first.
    ///
    /// Gracefully degrades: returns an empty vec when the instance directory
    /// or crash-reports subdirectory is inaccessible.
    pub fn list_reports(&self, instance_id: &str) -> LauncherResult<Vec<CrashReportInfo>> {
        let sanitized = validate_instance_id(instance_id)?;
        let dir = match self.ctx.paths.instance_dir(&sanitized) {
            Ok(d) => d.join("crash-reports"),
            Err(_) => return Ok(Vec::new()),
        };
        Ok(crate::crash_diagnostics::list_reports_from_dir(&dir))
    }

    /// Read the content of a named crash report file.
    ///
    /// Validates the filename for path-traversal safety before resolving it
    /// against the instance's crash-reports directory. Returns
    /// `ERR_CRASH_LOG_READ` when the file cannot be read.
    pub fn read_crash_log(&self, instance_id: &str, filename: &str) -> LauncherResult<String> {
        let sanitized = validate_instance_id(instance_id)?;
        validate_filename(filename)?;
        let safe_name = std::path::Path::new(filename)
            .file_name()
            .ok_or_else(|| LauncherError::Generic {
                code: "ERR_CRASH_LOG_PATH".to_string(),
                message: "Invalid crash log filename.".to_string(),
            })?
            .to_string_lossy()
            .to_string();
        let dir = match self.ctx.paths.instance_dir(&sanitized) {
            Ok(d) => d.join("crash-reports"),
            Err(_) => {
                return Err(LauncherError::Generic {
                    code: "ERR_CRASH_LOG_READ".to_string(),
                    message: "Could not read the crash log file.".to_string(),
                })
            }
        };
        crate::crash_diagnostics::read_crash_log_from_path(&dir.join(&safe_name))
    }

    /// Triage a crash log against the curated signature set, optionally
    /// augmented by `registry.db` signatures when available.
    ///
    /// Gracefully degrades: returns `no_match` when the crash-reports directory
    /// or the named file is inaccessible. The registry DB is opened as a
    /// read-only best-effort — triage always succeeds against the embedded
    /// corpus even without a registry database.
    pub fn triage_crash(
        &self,
        instance_id: &str,
        filename: &str,
    ) -> LauncherResult<CrashTriageResult> {
        let sanitized = validate_instance_id(instance_id)?;
        validate_filename(filename)?;
        let safe_name = std::path::Path::new(filename)
            .file_name()
            .ok_or_else(|| LauncherError::Generic {
                code: "ERR_CRASH_LOG_PATH".to_string(),
                message: "Invalid crash log filename.".to_string(),
            })?
            .to_string_lossy()
            .to_string();
        let dir = match self.ctx.paths.instance_dir(&sanitized) {
            Ok(d) => d.join("crash-reports"),
            Err(_) => return Ok(CrashTriageResult::no_match()),
        };
        let text = match std::fs::read_to_string(dir.join(&safe_name)) {
            Ok(t) => t,
            Err(_) => return Ok(CrashTriageResult::no_match()),
        };
        let conn_opt = if self.ctx.paths.registry_db().exists() {
            crate::db::registry_connection(&self.ctx.paths.registry_db()).ok()
        } else {
            None
        };
        Ok(crate::crash_diagnostics::triage_with_db(
            &text,
            conn_opt.as_ref(),
        ))
    }
}

// -----------------------------------------------------------------------
// CrashFingerprint — compact crash log fingerprint
// -----------------------------------------------------------------------

/// A compact fingerprint derived from a crash log: the root exception class
/// and up to 3 top stack frames.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrashFingerprint {
    pub exception_class: String,
    pub top_frames: Vec<String>,
}

impl CrashFingerprint {
    /// Return a stable hash key like `"java.lang.NullPointerException|at
    /// me.jellysquid...|at net.minecraft..."` — join top_frames with `|`,
    /// capped at 3.
    pub fn fingerprint_str(&self) -> String {
        let frames: Vec<String> = self.top_frames.iter().take(3).cloned().collect();
        format!("{}|{}", self.exception_class, frames.join("|"))
    }
}

// -----------------------------------------------------------------------
// SuspectScore — ranked suspect mod result
// -----------------------------------------------------------------------

/// A ranked suspect mod with its score and per-signal breakdown.
#[derive(Debug, Clone, Serialize)]
pub struct SuspectScore {
    pub mod_id: String,
    pub filename: String,
    pub total_score: f64,
    pub breakdown: serde_json::Value,
    #[serde(default)]
    pub is_dependent_of: Option<String>,
}

// -----------------------------------------------------------------------
// parse_crash_log — extract CrashFingerprint from crash text
// -----------------------------------------------------------------------

/// Parse a crash log text and extract a `CrashFingerprint`.
///
/// Finds the root cause exception by looking for the LAST `Caused by:` line
/// (which is the deepest/primary cause), then extracts the exception class and
/// up to 3 top stack frames (lines starting with `\t at <package.Class>.<method>`).
///
/// Returns `None` if no exception is found. Never panics on malformed logs.
pub fn parse_crash_log(text: &str) -> Option<CrashFingerprint> {
    let lines: Vec<&str> = text.lines().collect();

    // Find the last "Caused by:" line — that's the root cause.
    let mut last_cause_idx: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        if line.contains("Caused by:") {
            last_cause_idx = Some(i);
        }
    }

    let cause_line = match last_cause_idx {
        Some(idx) => lines[idx],
        None => {
            let first_exc = lines.iter().position(|line| {
                line.contains("java.lang.")
                    || (line.contains(" at ")
                        && line
                            .split(' ')
                            .any(|w| w.ends_with("Exception") || w.ends_with("Error")))
            });
            lines[first_exc?]
        }
    };

    let exception_class = cause_line
        .split("Caused by:")
        .nth(1)
        .and_then(|rest| rest.split(':').next())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            cause_line
                .split_whitespace()
                .find(|w| w.ends_with("Exception") || w.ends_with("Error"))
        })
        .map(String::from);

    let exception_class = exception_class?;
    if exception_class.is_empty() {
        return None;
    }

    let mut top_frames: Vec<String> = Vec::new();
    let start_idx = last_cause_idx.unwrap_or(0);

    for line in lines.iter().skip(start_idx + 1) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("at ") {
            top_frames.push(trimmed.to_string());
            if top_frames.len() >= 3 {
                break;
            }
        } else if trimmed.is_empty() || trimmed.starts_with("...") {
            continue;
        } else {
            break;
        }
    }

    Some(CrashFingerprint {
        exception_class,
        top_frames,
    })
}

// -----------------------------------------------------------------------
// Evidence read methods for the desktop scoring pipeline
// -----------------------------------------------------------------------

impl CrashService {
    /// Return confirmed attributions for a fingerprint, ordered by confirm_count DESC.
    pub fn get_confirmed_attribution(
        &self,
        fingerprint: &str,
    ) -> LauncherResult<Vec<crate::db::CrashAttribution>> {
        let conn = self.connection()?;
        crate::db::get_confirmed_attribution(&conn, fingerprint)
            .map_err(|_| LauncherError::LocalStateFailed)
    }

    /// Return the mod_ids ruled out for a fingerprint.
    pub fn get_ruled_out_mods(&self, fingerprint: &str) -> LauncherResult<Vec<String>> {
        let conn = self.connection()?;
        crate::db::get_ruled_out_mods(&conn, fingerprint)
            .map_err(|_| LauncherError::LocalStateFailed)
    }

    /// Check whether a mod has been ruled out for a fingerprint.
    pub fn is_ruled_out(&self, fingerprint: &str, mod_id: &str) -> LauncherResult<bool> {
        let conn = self.connection()?;
        crate::db::is_ruled_out(&conn, fingerprint, mod_id)
            .map_err(|_| LauncherError::LocalStateFailed)
    }

    /// Return the number of survivals a mod appears in.
    pub fn get_mod_survival_count(&self, mod_id: &str) -> LauncherResult<i64> {
        let conn = self.connection()?;
        crate::db::get_mod_survival_count(&conn, mod_id)
            .map_err(|_| LauncherError::LocalStateFailed)
    }

    /// Return the number of survivals where both mods a and b appear together.
    pub fn get_pair_survival_count(&self, a: &str, b: &str) -> LauncherResult<i64> {
        let conn = self.connection()?;
        crate::db::get_pair_survival_count(&conn, a, b).map_err(|_| LauncherError::LocalStateFailed)
    }

    /// Return the co-crash count for a mod pair from local_crash_telemetry.
    pub fn get_pair_crash_count(&self, a: &str, b: &str) -> LauncherResult<i64> {
        let conn = self.connection()?;
        crate::db::get_pair_crash_count(&conn, a, b).map_err(|_| LauncherError::LocalStateFailed)
    }

    /// Return the total number of crash_survivals rows.
    pub fn get_total_survival_count(&self) -> LauncherResult<i64> {
        let conn = self.connection()?;
        crate::db::get_total_survival_count(&conn).map_err(|_| LauncherError::LocalStateFailed)
    }
}

// -----------------------------------------------------------------------
// Telemetry mutation methods
// -----------------------------------------------------------------------

impl CrashService {
    /// Purge stale crash telemetry records (older than 90 days, count < 2).
    pub fn purge_stale_telemetry(&self) -> LauncherResult<()> {
        let conn = self.connection()?;
        crate::db::purge_stale_crash_telemetry(&conn).map_err(|_| LauncherError::LocalStateFailed)
    }

    /// Record a crash event in the local state database and wire the co-crash
    /// table for every mod pair in `mod_ids`. Returns the new crash event row id.
    pub fn record_crash_event(
        &self,
        instance_id: &str,
        fingerprint: &CrashFingerprint,
        mod_ids: &[String],
        signature_name: Option<&str>,
    ) -> LauncherResult<i64> {
        let conn = self.connection()?;

        let top_frames_json =
            serde_json::to_string(&fingerprint.top_frames).map_err(|_| LauncherError::Generic {
                code: "ERR_JSON_SERIALIZE".to_string(),
                message: "Failed to serialize top_frames.".to_string(),
            })?;

        crate::db::insert_crash_event(
            &conn,
            instance_id,
            &fingerprint.fingerprint_str(),
            &fingerprint.exception_class,
            &top_frames_json,
            signature_name,
        )
        .map_err(|_| LauncherError::LocalStateFailed)?;

        // Wire the co-crash table for every pair.
        for a in mod_ids {
            for b in mod_ids {
                if a < b {
                    let _ = crate::db::record_co_crash(&conn, a, b);
                }
            }
        }

        Ok(conn.last_insert_rowid())
    }

    /// Record that the instance survived a launch with the given mods installed.
    pub fn record_survival(&self, instance_id: &str, mod_ids: &[String]) -> LauncherResult<()> {
        let conn = self.connection()?;
        crate::db::insert_survival(&conn, instance_id, mod_ids)
            .map_err(|_| LauncherError::LocalStateFailed)
    }

    /// Increment the confirmation count for a mod_id matching a fingerprint.
    pub fn confirm_attribution(
        &self,
        fingerprint: &CrashFingerprint,
        mod_id: &str,
    ) -> LauncherResult<()> {
        let conn = self.connection()?;
        crate::db::increment_confirmation(&conn, &fingerprint.fingerprint_str(), mod_id)
            .map_err(|_| LauncherError::LocalStateFailed)
    }

    /// Mark a mod as ruled out for a given fingerprint.
    pub fn rule_out(&self, fingerprint: &CrashFingerprint, mod_id: &str) -> LauncherResult<()> {
        let conn = self.connection()?;
        crate::db::add_ruled_out(&conn, &fingerprint.fingerprint_str(), mod_id)
            .map_err(|_| LauncherError::LocalStateFailed)
    }
}

// ---------------------------------------------------------------------------
// SuggestedAction — recommended next action for the triage UI
// ---------------------------------------------------------------------------

/// The recommended next action for the triage UI.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum SuggestedAction {
    GuidedDisable { next_suspect: SuspectScore },
    ConfidenceAutoDisable { mod_id: String, filename: String },
    ShowTriageBanner { mod_id: String },
    NoSuspects,
}

// ---------------------------------------------------------------------------
// InvestigationResult — complete result of a crash investigation step
// ---------------------------------------------------------------------------

/// The complete result of a crash investigation step.
#[derive(Debug, Clone, Serialize)]
pub struct InvestigationResult {
    pub fingerprint: Option<CrashFingerprint>,
    pub signature_name: Option<String>,
    pub suspects: Vec<SuspectScore>,
    pub suggested_action: SuggestedAction,
    pub ruled_out: Vec<String>,
}

// ---------------------------------------------------------------------------
// continue_investigation — full investigation pipeline
// ---------------------------------------------------------------------------

/// Run the full investigation pipeline: score suspects, fetch ruled-out mods,
/// filter them out, pick the top remaining, and build an `InvestigationResult`.
///
/// If the top suspect's registry_id is `under_review` in the registry, the
/// suggested action is `ShowTriageBanner`.
pub fn continue_investigation(
    ctx: &Ctx,
    fingerprint: &CrashFingerprint,
    installed: &[crate::models::InstalledMod],
    crash_text: &str,
) -> LauncherResult<InvestigationResult> {
    let crash_svc = CrashService::new(ctx.clone());
    let registry_svc = crate::registry::RegistryService::new(ctx.clone());

    // Score suspects.
    let mut suspects = score_suspects(ctx, crash_text, installed)?;

    // Fetch ruled-out mods for this fingerprint.
    let ruled_out = crash_svc
        .get_ruled_out_mods(&fingerprint.fingerprint_str())
        .unwrap_or_default();

    // Filter out ruled-out mods.
    let ruled_out_set: std::collections::HashSet<String> = ruled_out.iter().cloned().collect();
    suspects.retain(|s| !ruled_out_set.contains(&s.mod_id));

    // Build the ruled_out list for the result (sorted for determinism).
    let mut ruled_out_sorted = ruled_out.clone();
    ruled_out_sorted.sort();

    // Determine suggested action.
    let suggested_action = if suspects.is_empty() {
        SuggestedAction::NoSuspects
    } else {
        let top = &suspects[0];
        let is_under_review = registry_svc.is_under_review(&top.mod_id).unwrap_or(false);

        if is_under_review {
            SuggestedAction::ShowTriageBanner {
                mod_id: top.mod_id.clone(),
            }
        } else {
            SuggestedAction::GuidedDisable {
                next_suspect: top.clone(),
            }
        }
    };

    Ok(InvestigationResult {
        fingerprint: Some(fingerprint.clone()),
        signature_name: None,
        suspects,
        suggested_action,
        ruled_out: ruled_out_sorted,
    })
}

// ---------------------------------------------------------------------------
// compute_mod_score — pure per-mod scoring
// ---------------------------------------------------------------------------

/// Pure per-mod scoring computation.
///
/// Computes a `SuspectScore` for a single mod from pre-gathered inputs.
#[allow(clippy::too_many_arguments)]
pub fn compute_mod_score(
    mod_id: String,
    filename: String,
    java_packages: &[String],
    crashed_packages: &[String],
    installed_ids: &[String],
    known_conflicts: &[registry::KnownConflict],
    confirmed_count: i64,
    total_survivals: i64,
    mod_survival_count: i64,
    pair_crash_counts: &[(String, i64)],
    pair_survival_counts: &[(String, i64)],
) -> SuspectScore {
    let ubiquity_denom = if total_survivals == 0 {
        1
    } else {
        total_survivals
    };

    // --- Signal A (stack-frame match) ---
    let base_a: f64 = java_packages
        .iter()
        .filter(|pkg_mod| {
            crashed_packages.iter().any(|pkg_crash| {
                pkg_crash == pkg_mod.as_str()
                    || pkg_mod.starts_with(pkg_crash.as_str())
                    || pkg_crash.starts_with(pkg_mod.as_str())
            })
        })
        .count() as f64;

    // --- Signal G (curated conflict) ---
    let mut g_contributions: Vec<(f64, String)> = Vec::new();
    for conflict in known_conflicts {
        let (other_id, severity) = if conflict.mod_a_id == mod_id {
            (&conflict.mod_b_id, &conflict.severity)
        } else if conflict.mod_b_id == mod_id {
            (&conflict.mod_a_id, &conflict.severity)
        } else {
            continue;
        };

        if !installed_ids.contains(other_id) {
            continue;
        }

        let is_mitigated = conflict
            .mitigated_by
            .iter()
            .any(|mit| installed_ids.contains(mit));
        if is_mitigated {
            continue;
        }

        let contribution = match severity.as_str() {
            "hard" => 1.0,
            "weak" => 0.3,
            _ => 0.0,
        };
        g_contributions.push((contribution, other_id.clone()));
    }

    // --- Signal E (confirmed prior) ---
    let base_e: f64 = (confirmed_count as f64 * 0.5).min(1.0);

    // --- Signal B (fingerprint recurrence) ---
    let base_b: f64 = if confirmed_count > 0 { 0.2 } else { 0.0 };

    // --- Signal C (co-crash telemetry) ---
    let max_pair_crash: i64 = pair_crash_counts
        .iter()
        .map(|(_, count)| *count)
        .max()
        .unwrap_or(0);
    let base_c = (max_pair_crash as f64 * 0.1).min(0.5);

    // -------------------------------------------------------------------
    // Confounders
    // -------------------------------------------------------------------

    // D (survival ubiquity) — dampens A and C
    let ubiquity = (mod_survival_count as f64 / ubiquity_denom as f64).clamp(0.0, 1.0);
    let dampener_d = 1.0 - (ubiquity * 0.7);

    let a_final = base_a * dampener_d;
    let c_final = base_c * dampener_d;

    // Survival co-decay on G
    let mut g_final = 0.0f64;
    for (contrib, other_id) in &g_contributions {
        let pair_surv = pair_survival_counts
            .iter()
            .find(|(id, _)| id == other_id)
            .map(|(_, count)| *count)
            .unwrap_or(0);
        let g_mod = if pair_surv >= 3 {
            (1.0 - (pair_surv as f64 * 0.15)).max(0.1)
        } else {
            1.0
        };
        g_final += contrib * g_mod;
    }

    // -------------------------------------------------------------------
    // Total + NaN gate
    // -------------------------------------------------------------------
    let mut total_score = g_final + base_e + a_final + base_b + c_final;
    if total_score.is_nan() {
        total_score = 0.0;
    }

    let breakdown = if total_score > 0.0 {
        let conflict_pairs: Vec<serde_json::Value> = g_contributions
            .iter()
            .map(|(contrib, other_id)| {
                serde_json::json!({
                    "partner": other_id,
                    "contribution": contrib
                })
            })
            .collect();

        serde_json::json!({
            "G": g_final,
            "E": base_e,
            "A": a_final,
            "B": base_b,
            "C": c_final,
            "ubiquity_dampener": dampener_d,
            "conflict_pairs": conflict_pairs,
        })
    } else {
        serde_json::json!({})
    };

    SuspectScore {
        mod_id,
        filename,
        total_score,
        breakdown,
        is_dependent_of: None,
    }
}

// ---------------------------------------------------------------------------
// score_suspects — full scoring pipeline
// ---------------------------------------------------------------------------

/// Dynamic-weighted suspicion-scoring algorithm.
///
/// For each installed mod, computes a `total_score` from five signals (G, E,
/// A, B, C) with two confounder dampeners (ubiquity D and survival co-decay
/// on G). Returns one `SuspectScore` per installed mod sorted by score
/// descending.
pub fn score_suspects(
    ctx: &Ctx,
    crash_text: &str,
    installed: &[crate::models::InstalledMod],
) -> LauncherResult<Vec<SuspectScore>> {
    let crash_svc = CrashService::new(ctx.clone());
    let registry_svc = crate::registry::RegistryService::new(ctx.clone());

    // -----------------------------------------------------------------------
    // Step 1 — parse the crash log
    // -----------------------------------------------------------------------
    let fingerprint = match parse_crash_log(crash_text) {
        Some(fp) => fp,
        None => {
            return Ok(installed
                .iter()
                .filter(|m| m.registry_id.is_some() || !m.filename.is_empty())
                .map(|m| SuspectScore {
                    mod_id: m.registry_id.clone().unwrap_or_else(|| m.filename.clone()),
                    filename: m.filename.clone(),
                    total_score: 0.0,
                    breakdown: serde_json::json!({}),
                    is_dependent_of: None,
                })
                .collect());
        }
    };

    // Collect package prefixes from crash stack frames.
    let mut crashed_packages: HashSet<String> = HashSet::new();
    for line in crash_text.lines() {
        let trimmed = line.trim_start();
        if let Some(after_at) = trimmed.strip_prefix("at ") {
            let class_part = match after_at.find('(') {
                Some(idx) => &after_at[..idx],
                None => after_at,
            };
            if let Some(dot_pos) = class_part.rfind('.') {
                let pkg = class_part[..dot_pos].replace('/', ".");
                crashed_packages.insert(pkg);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Step 2 — gather priors via services
    // -----------------------------------------------------------------------
    let confirmed_map: HashMap<String, i64> = crash_svc
        .get_confirmed_attribution(&fingerprint.fingerprint_str())
        .unwrap_or_default()
        .into_iter()
        .map(|r| (r.mod_id, r.confirm_count))
        .collect();

    let total_survivals: i64 = crash_svc.get_total_survival_count().unwrap_or(0);

    let known_conflicts: Vec<registry::KnownConflict> =
        registry_svc.known_conflicts().unwrap_or_default();

    let installed_ids: Vec<String> = installed
        .iter()
        .filter_map(|m| m.registry_id.clone().or_else(|| Some(m.filename.clone())))
        .collect();

    // -----------------------------------------------------------------------
    // Step 3 — per-mod scoring
    // -----------------------------------------------------------------------
    let mut suspects: Vec<SuspectScore> = Vec::with_capacity(installed.len());

    for mod_entry in installed {
        if mod_entry.registry_id.is_none() && mod_entry.filename.is_empty() {
            continue;
        }

        let mod_id = mod_entry
            .registry_id
            .clone()
            .unwrap_or_else(|| mod_entry.filename.clone());

        let surv_count: i64 = crash_svc.get_mod_survival_count(&mod_id).unwrap_or(0);

        // pair_crash_counts for each other installed mod
        let mut pair_crash_counts: Vec<(String, i64)> = Vec::new();
        for other_mod in installed {
            if other_mod.registry_id.as_deref() == Some(&mod_id) {
                continue;
            }
            let other_id = other_mod
                .registry_id
                .clone()
                .unwrap_or_else(|| other_mod.filename.clone());
            if other_id == mod_id {
                continue;
            }
            let pair_crash = crash_svc
                .get_pair_crash_count(&mod_id, &other_id)
                .unwrap_or(0);
            pair_crash_counts.push((other_id, pair_crash));
        }

        // pair_survival_counts for G partners in known conflicts
        let mut pair_survival_counts: Vec<(String, i64)> = Vec::new();
        for conflict in &known_conflicts {
            let other_id = if conflict.mod_a_id == mod_id {
                &conflict.mod_b_id
            } else if conflict.mod_b_id == mod_id {
                &conflict.mod_a_id
            } else {
                continue;
            };
            if !installed_ids.contains(other_id) {
                continue;
            }
            let pair_surv = crash_svc
                .get_pair_survival_count(&mod_id, other_id)
                .unwrap_or(0);
            pair_survival_counts.push((other_id.clone(), pair_surv));
        }

        let confirmed_count = *confirmed_map.get(&mod_id).unwrap_or(&0);

        let score = compute_mod_score(
            mod_id,
            mod_entry.filename.clone(),
            &mod_entry.java_packages,
            &crashed_packages.iter().cloned().collect::<Vec<_>>(),
            &installed_ids,
            &known_conflicts,
            confirmed_count,
            total_survivals,
            surv_count,
            &pair_crash_counts,
            &pair_survival_counts,
        );

        suspects.push(score);
    }

    // -------------------------------------------------------------------
    // Step 3b — indirect suspects
    // -------------------------------------------------------------------
    let mut reverse_deps: HashMap<String, Vec<String>> = HashMap::new();
    for mod_entry in installed {
        if let Some(ref _mod_jar_id) = mod_entry.mod_jar_id {
            for dep_on in &mod_entry.depends_on {
                reverse_deps.entry(dep_on.clone()).or_default().push(
                    mod_entry
                        .registry_id
                        .clone()
                        .unwrap_or_else(|| mod_entry.filename.clone()),
                );
            }
        }
    }

    let mut installed_by_id: HashMap<String, &crate::models::InstalledMod> = HashMap::new();
    for mod_entry in installed {
        let mid = mod_entry
            .registry_id
            .clone()
            .unwrap_or_else(|| mod_entry.filename.clone());
        installed_by_id.insert(mid, mod_entry);
    }

    let suspect_set: std::collections::HashSet<String> =
        suspects.iter().map(|s| s.mod_id.clone()).collect();

    let mut indirect_candidates: Vec<SuspectScore> = Vec::new();

    for s in &suspects {
        if s.total_score <= 0.0 {
            continue;
        }
        let parent_jar_id = installed_by_id
            .get(&s.mod_id)
            .and_then(|m| m.mod_jar_id.clone());

        let jar_id = match parent_jar_id {
            Some(jid) => jid,
            None => continue,
        };

        let dependents = match reverse_deps.get(&jar_id) {
            Some(deps) => deps,
            None => continue,
        };

        for dep_mod_id in dependents {
            if suspect_set.contains(dep_mod_id) {
                continue;
            }
            let dep_entry = match installed_by_id.get(dep_mod_id) {
                Some(e) => e,
                None => continue,
            };
            let indirect_score = s.total_score * 0.6;
            indirect_candidates.push(SuspectScore {
                mod_id: dep_mod_id.clone(),
                filename: dep_entry.filename.clone(),
                total_score: indirect_score,
                breakdown: serde_json::json!({
                    "indirect_via": s.mod_id,
                    "parent_score": s.total_score,
                }),
                is_dependent_of: Some(s.mod_id.clone()),
            });
        }
    }

    suspects.extend(indirect_candidates);

    // -------------------------------------------------------------------
    // Step 4 — sort
    // -------------------------------------------------------------------
    suspects.sort_by(|a, b| {
        let by_score = b
            .total_score
            .partial_cmp(&a.total_score)
            .unwrap_or(std::cmp::Ordering::Equal);
        if by_score != std::cmp::Ordering::Equal {
            return by_score;
        }
        a.mod_id.cmp(&b.mod_id)
    });

    Ok(suspects)
}

// ---------------------------------------------------------------------------
// Load instance manifest helper
// ---------------------------------------------------------------------------

impl CrashService {
    /// Read the instance manifest from disk via Ctx paths.
    fn load_instance_manifest(
        &self,
        instance_id: &str,
    ) -> LauncherResult<Option<InstanceManifest>> {
        let path = self.ctx.paths.instance_manifest(instance_id)?;
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path).map_err(|e| LauncherError::Generic {
            code: "ERR_LOCAL_STATE_FAILED".into(),
            message: format!("Cannot read instance manifest: {e}"),
        })?;
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| LauncherError::Generic {
                code: "ERR_LOCAL_STATE_FAILED".into(),
                message: format!("Cannot parse instance manifest: {e}"),
            })
    }
}

// ---------------------------------------------------------------------------
// suggest_mod_incompatibility — MCP-oriented entry point
// ---------------------------------------------------------------------------

impl CrashService {
    /// Analyze crash text against installed mods in an instance and return
    /// ranked suspect mods that may be causing the crash.
    ///
    /// Loads the instance manifest internally, parses the crash text for a
    /// fingerprint, and runs the full scoring pipeline. Returns an empty vec
    /// when the instance is not found.
    pub fn suggest_mod_incompatibility(
        &self,
        instance_id: &str,
        crash_text: &str,
    ) -> LauncherResult<Vec<SuspectScore>> {
        let manifest = self.load_instance_manifest(instance_id)?;
        let manifest = match manifest {
            Some(m) => m,
            None => {
                return Err(LauncherError::Generic {
                    code: "ERR_INSTANCE_NOT_FOUND".into(),
                    message: format!("Instance '{}' not found", instance_id),
                })
            }
        };
        score_suspects(&self.ctx, crash_text, &manifest.mods)
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Validate and normalize an instance ID.
fn validate_instance_id(instance_id: &str) -> LauncherResult<String> {
    let sanitized = crate::paths::sanitize_id(instance_id);
    crate::app_paths::validate_path_component(&sanitized).map_err(|_| LauncherError::Generic {
        code: "ERR_INVALID_INSTANCE".into(),
        message: "Invalid instance ID.".into(),
    })?;
    if sanitized.is_empty() {
        return Err(LauncherError::Generic {
            code: "ERR_INVALID_INSTANCE".into(),
            message: "Instance ID must not be empty.".into(),
        });
    }
    Ok(sanitized)
}

/// Validate a mod filename:
/// - Must not be empty.
/// - Must not contain path separators.
/// - Must not be `.` or `..` or composed only of dots/dashes.
/// - Must not be absolute.
fn validate_filename(filename: &str) -> LauncherResult<()> {
    if filename.is_empty() {
        return Err(LauncherError::Generic {
            code: "ERR_INVALID_FILENAME".into(),
            message: "Filename must not be empty.".into(),
        });
    }
    if filename.contains('/') || filename.contains('\\') {
        return Err(LauncherError::Generic {
            code: "ERR_INVALID_FILENAME".into(),
            message: "Filename must not contain path separators.".into(),
        });
    }
    let trimmed = filename.trim_matches(|c: char| c == '.' || c == ' ' || c == '-');
    if trimmed.is_empty() {
        return Err(LauncherError::Generic {
            code: "ERR_INVALID_FILENAME".into(),
            message: "Filename must not be composed only of dots, dashes, or spaces.".into(),
        });
    }
    if cfg!(windows) {
        if filename.len() >= 2
            && filename.as_bytes()[0].is_ascii_alphabetic()
            && filename.as_bytes()[1] == b':'
        {
            return Err(LauncherError::Generic {
                code: "ERR_INVALID_FILENAME".into(),
                message: "Filename must not start with a drive letter.".into(),
            });
        }
        if filename.starts_with('\\') || filename.starts_with("//") {
            return Err(LauncherError::Generic {
                code: "ERR_INVALID_FILENAME".into(),
                message: "Filename must not be a UNC path.".into(),
            });
        }
    } else {
        if filename.starts_with('/') {
            return Err(LauncherError::Generic {
                code: "ERR_INVALID_FILENAME".into(),
                message: "Filename must not be an absolute path.".into(),
            });
        }
    }
    Ok(())
}

/// Update the instance manifest to mark a mod as disabled.
///
/// Appends ` [disabled]` to the version string to preserve backward-compatible
/// semantics with the existing crash_investigator module.
fn update_manifest_disable(manifest_path: &std::path::Path, filename: &str) -> LauncherResult<()> {
    if !manifest_path.exists() {
        return Ok(());
    }
    let text = std::fs::read_to_string(manifest_path).map_err(|e| LauncherError::Generic {
        code: "ERR_MANIFEST_READ".into(),
        message: format!("Could not read manifest: {e}"),
    })?;
    let mut manifest: InstanceManifest =
        serde_json::from_str(&text).map_err(|e| LauncherError::Generic {
            code: "ERR_MANIFEST_PARSE".into(),
            message: format!("Invalid manifest: {e}"),
        })?;

    for entry in all_mod_entries_mut(&mut manifest) {
        if entry.filename == filename {
            entry.enabled = false;
            let v = entry.version.clone().unwrap_or_default();
            if !v.contains("[disabled]") {
                entry.version = if v.is_empty() {
                    Some("[disabled]".to_string())
                } else {
                    Some(format!("{v} [disabled]"))
                };
            }
            break;
        }
    }

    atomic_write_json(manifest_path, &manifest)
}

/// Update the instance manifest to mark a mod as enabled.
///
/// Strips ` [disabled]` from the version string.
fn update_manifest_enable(manifest_path: &std::path::Path, filename: &str) -> LauncherResult<()> {
    if !manifest_path.exists() {
        return Ok(());
    }
    let text = std::fs::read_to_string(manifest_path).map_err(|e| LauncherError::Generic {
        code: "ERR_MANIFEST_READ".into(),
        message: format!("Could not read manifest: {e}"),
    })?;
    let mut manifest: InstanceManifest =
        serde_json::from_str(&text).map_err(|e| LauncherError::Generic {
            code: "ERR_MANIFEST_PARSE".into(),
            message: format!("Invalid manifest: {e}"),
        })?;

    for entry in all_mod_entries_mut(&mut manifest) {
        if entry.filename == filename {
            entry.enabled = true;
            if let Some(ref v) = entry.version {
                if let Some(stripped) = v.strip_suffix(" [disabled]") {
                    if stripped.is_empty() {
                        entry.version = None;
                    } else {
                        entry.version = Some(stripped.to_string());
                    }
                } else if v == "[disabled]" {
                    entry.version = None;
                }
            }
            break;
        }
    }

    atomic_write_json(manifest_path, &manifest)
}

/// Iterate over all mod entries across content types.
fn all_mod_entries_mut(
    manifest: &mut InstanceManifest,
) -> impl Iterator<Item = &mut crate::models::InstalledMod> {
    manifest
        .mods
        .iter_mut()
        .chain(manifest.resourcepacks.iter_mut())
        .chain(manifest.shaders.iter_mut())
        .chain(manifest.datapacks.iter_mut())
        .chain(manifest.worlds.iter_mut())
}

/// Atomically write JSON: write to .tmp, then rename.
fn atomic_write_json(path: &std::path::Path, value: &impl serde::Serialize) -> LauncherResult<()> {
    let tmp_path = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(value).map_err(|e| LauncherError::Generic {
        code: "ERR_MANIFEST_SERIALIZE".into(),
        message: format!("Could not serialize manifest: {e}"),
    })?;
    std::fs::write(&tmp_path, &text).map_err(|e| LauncherError::Generic {
        code: "ERR_MANIFEST_WRITE".into(),
        message: format!("Could not write manifest: {e}"),
    })?;
    std::fs::rename(&tmp_path, path).map_err(|e| LauncherError::Generic {
        code: "ERR_MANIFEST_WRITE".into(),
        message: format!("Could not finalize manifest: {e}"),
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::CoreContext;
    use std::path::PathBuf;

    /// Per-test unique seed for temp dir names.
    static TEST_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    /// Create a temp directory and CoreContext unique to one test invocation.
    /// Initializes the local state database so connection-based methods work.
    fn setup_ctx() -> (CoreContext, PathBuf) {
        let seq = TEST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = std::env::temp_dir().join(format!("agora-crash-svc-test-{}", seq));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let ctx = CoreContext::for_testing(tmp.clone());
        // Initialize local_state.db so crash_service connection() works
        crate::db::init_local_state_db(&ctx.paths.local_state_db()).unwrap();
        (ctx, tmp)
    }

    fn create_instance(ctx: &CoreContext, instance_id: &str, mod_filenames: &[&str]) {
        let instance_dir = ctx.paths.instance_dir(instance_id).unwrap();
        std::fs::create_dir_all(instance_dir.join("mods")).unwrap();

        for fname in mod_filenames {
            std::fs::write(instance_dir.join("mods").join(fname), b"fake mod content").unwrap();
        }

        let manifest = crate::models::InstanceManifest {
            instance_id: instance_id.to_string(),
            name: instance_id.to_string(),
            created_from_pack: None,
            minecraft_version: "1.20".into(),
            loader: "fabric".into(),
            loader_version: "0.15.0".into(),
            is_locked: false,
            mods: mod_filenames
                .iter()
                .map(|fname| crate::models::InstalledMod {
                    filename: fname.to_string(),
                    registry_id: None,
                    modrinth_id: None,
                    source: "local".into(),
                    source_url: None,
                    version: Some("1.0.0".into()),
                    sha256: "abc".into(),
                    installed_at: "2024-01-01T00:00:00Z".into(),
                    java_packages: vec![],
                    mod_jar_id: None,
                    provided_mod_ids: vec![],
                    enabled: true,
                    content_type: "mod".into(),
                    depends_on: vec![],
                    optional_deps: vec![],
                    incompatible_deps: vec![],
                })
                .collect(),
            resourcepacks: vec![],
            shaders: vec![],
            datapacks: vec![],
            worlds: vec![],
            user_preferences: serde_json::Value::Null,
        };
        let manifest_path = ctx.paths.instance_manifest(instance_id).unwrap();
        std::fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();
    }

    fn assert_mod_exists(ctx: &CoreContext, instance_id: &str, filename: &str) {
        let path = ctx
            .paths
            .instance_dir(instance_id)
            .unwrap()
            .join("mods")
            .join(filename);
        assert!(path.exists(), "Expected {filename} to exist at {:?}", path);
    }

    fn assert_mod_not_exists(ctx: &CoreContext, instance_id: &str, filename: &str) {
        let path = ctx
            .paths
            .instance_dir(instance_id)
            .unwrap()
            .join("mods")
            .join(filename);
        assert!(
            !path.exists(),
            "Expected {filename} to not exist at {:?}",
            path
        );
    }

    fn assert_disabled_exists(ctx: &CoreContext, instance_id: &str, filename: &str) {
        let path = ctx
            .paths
            .instance_dir(instance_id)
            .unwrap()
            .join("mods")
            .join(format!("{filename}.disabled"));
        assert!(
            path.exists(),
            "Expected {filename}.disabled to exist at {:?}",
            path
        );
    }

    fn read_manifest(ctx: &CoreContext, instance_id: &str) -> crate::models::InstanceManifest {
        let path = ctx.paths.instance_manifest(instance_id).unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    // ------------------------------------------------------------------
    // disable_mod
    // ------------------------------------------------------------------

    #[test]
    fn test_disable_mod_renames_file_and_updates_manifest() {
        let (ctx, tmp) = setup_ctx();
        let svc = CrashService::new(ctx.clone());
        create_instance(&ctx, "test-instance", &["testmod.jar"]);

        svc.disable_mod("test-instance", "testmod.jar").unwrap();

        assert_mod_not_exists(&ctx, "test-instance", "testmod.jar");
        assert_disabled_exists(&ctx, "test-instance", "testmod.jar");

        let manifest = read_manifest(&ctx, "test-instance");
        let entry = manifest
            .mods
            .iter()
            .find(|m| m.filename == "testmod.jar")
            .unwrap();
        assert!(!entry.enabled);
        assert!(entry.version.as_deref().unwrap().contains("[disabled]"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_disable_mod_already_disabled_noop() {
        let (ctx, tmp) = setup_ctx();
        let svc = CrashService::new(ctx.clone());
        create_instance(&ctx, "test-instance", &["testmod.jar"]);

        svc.disable_mod("test-instance", "testmod.jar").unwrap();
        assert_disabled_exists(&ctx, "test-instance", "testmod.jar");

        svc.disable_mod("test-instance", "testmod.jar").unwrap();
        assert_disabled_exists(&ctx, "test-instance", "testmod.jar");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_disable_mod_missing_source_noop() {
        let (ctx, tmp) = setup_ctx();
        let svc = CrashService::new(ctx.clone());
        create_instance(&ctx, "test-instance", &["other.jar"]);

        svc.disable_mod("test-instance", "nonexistent.jar").unwrap();

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ------------------------------------------------------------------
    // enable_mod
    // ------------------------------------------------------------------

    #[test]
    fn test_enable_mod_renames_back_and_updates_manifest() {
        let (ctx, tmp) = setup_ctx();
        let svc = CrashService::new(ctx.clone());
        create_instance(&ctx, "test-instance", &["testmod.jar"]);

        svc.disable_mod("test-instance", "testmod.jar").unwrap();
        assert_disabled_exists(&ctx, "test-instance", "testmod.jar");

        svc.enable_mod("test-instance", "testmod.jar").unwrap();

        assert_mod_exists(&ctx, "test-instance", "testmod.jar");
        assert_mod_not_exists(&ctx, "test-instance", "testmod.jar.disabled");

        let manifest = read_manifest(&ctx, "test-instance");
        let entry = manifest
            .mods
            .iter()
            .find(|m| m.filename == "testmod.jar")
            .unwrap();
        assert!(entry.enabled);
        assert!(!entry
            .version
            .as_deref()
            .unwrap_or("")
            .contains("[disabled]"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_enable_mod_already_enabled_noop() {
        let (ctx, tmp) = setup_ctx();
        let svc = CrashService::new(ctx.clone());
        create_instance(&ctx, "test-instance", &["testmod.jar"]);

        svc.enable_mod("test-instance", "testmod.jar").unwrap();
        assert_mod_exists(&ctx, "test-instance", "testmod.jar");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_disable_artifact_rolls_back_rename_when_manifest_update_fails() {
        let (ctx, tmp) = setup_ctx();
        let svc = CrashService::new(ctx.clone());
        create_instance(&ctx, "test-instance", &["testmod.jar"]);
        std::fs::write(
            ctx.paths.instance_manifest("test-instance").unwrap(),
            b"not valid json",
        )
        .unwrap();

        assert!(svc
            .disable_artifact("test-instance", "testmod.jar")
            .is_err());
        assert_mod_exists(&ctx, "test-instance", "testmod.jar");
        assert_mod_not_exists(&ctx, "test-instance", "testmod.jar.disabled");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ------------------------------------------------------------------
    // Collision no-clobber
    // ------------------------------------------------------------------

    #[test]
    fn test_disable_mod_no_clobber_existing_disabled() {
        let (ctx, tmp) = setup_ctx();
        let svc = CrashService::new(ctx.clone());
        create_instance(&ctx, "test-instance", &["testmod.jar"]);

        let disabled_path = ctx
            .paths
            .instance_dir("test-instance")
            .unwrap()
            .join("mods")
            .join("testmod.jar.disabled");
        std::fs::write(&disabled_path, b"pre-existing disabled content").unwrap();

        svc.disable_mod("test-instance", "testmod.jar").unwrap();

        let content = std::fs::read_to_string(&disabled_path).unwrap();
        assert_eq!(content, "pre-existing disabled content");

        assert_mod_exists(&ctx, "test-instance", "testmod.jar");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_enable_mod_no_clobber_existing_target() {
        let (ctx, tmp) = setup_ctx();
        let svc = CrashService::new(ctx.clone());
        create_instance(&ctx, "test-instance", &["testmod.jar"]);

        svc.disable_mod("test-instance", "testmod.jar").unwrap();
        assert_disabled_exists(&ctx, "test-instance", "testmod.jar");

        let target_path = ctx
            .paths
            .instance_dir("test-instance")
            .unwrap()
            .join("mods")
            .join("testmod.jar");
        std::fs::write(&target_path, b"replacement content").unwrap();

        svc.enable_mod("test-instance", "testmod.jar").unwrap();

        assert_disabled_exists(&ctx, "test-instance", "testmod.jar");

        let content = std::fs::read_to_string(&target_path).unwrap();
        assert_eq!(content, "replacement content");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ------------------------------------------------------------------
    // Traversal rejection
    // ------------------------------------------------------------------

    #[test]
    fn test_disable_mod_rejects_traversal_filename() {
        let (ctx, tmp) = setup_ctx();
        let svc = CrashService::new(ctx.clone());
        create_instance(&ctx, "safe-instance", &["legit.jar"]);

        let result = svc.disable_mod("safe-instance", "../evil.jar");
        assert!(result.is_err());
        assert!(result.unwrap_err().code().contains("ERR_INVALID_FILENAME"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_disable_mod_rejects_absolute_filename() {
        let (ctx, tmp) = setup_ctx();
        let svc = CrashService::new(ctx.clone());
        create_instance(&ctx, "safe-instance", &["legit.jar"]);

        let result = svc.disable_mod("safe-instance", "/etc/passwd");
        assert!(result.is_err());
        assert!(result.unwrap_err().code().contains("ERR_INVALID_FILENAME"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_disable_mod_rejects_empty_filename() {
        let (ctx, tmp) = setup_ctx();
        let svc = CrashService::new(ctx.clone());
        create_instance(&ctx, "safe-instance", &["legit.jar"]);

        let result = svc.disable_mod("safe-instance", "");
        assert!(result.is_err());
        assert!(result.unwrap_err().code().contains("ERR_INVALID_FILENAME"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_disable_mod_rejects_separator_in_filename() {
        let (ctx, tmp) = setup_ctx();
        let svc = CrashService::new(ctx.clone());
        create_instance(&ctx, "safe-instance", &["legit.jar"]);

        let result = svc.disable_mod("safe-instance", "foo/bar.jar");
        assert!(result.is_err());
        assert!(result.unwrap_err().code().contains("ERR_INVALID_FILENAME"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_enable_mod_rejects_traversal_filename() {
        let (ctx, tmp) = setup_ctx();
        let svc = CrashService::new(ctx.clone());
        create_instance(&ctx, "safe-instance", &["legit.jar"]);

        let result = svc.enable_mod("safe-instance", "../../malicious.jar");
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_enable_mod_rejects_empty_filename() {
        let (ctx, tmp) = setup_ctx();
        let svc = CrashService::new(ctx.clone());
        create_instance(&ctx, "safe-instance", &["legit.jar"]);

        let result = svc.enable_mod("safe-instance", "");
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ------------------------------------------------------------------
    // validate_filename unit tests (no ctx needed)
    // ------------------------------------------------------------------

    #[test]
    fn test_validate_filename_accepts_normal() {
        assert!(validate_filename("mod.jar").is_ok());
        assert!(validate_filename("my-mod_1.0.jar").is_ok());
        assert!(validate_filename("a").is_ok());
    }

    #[test]
    fn test_validate_filename_rejects_empty() {
        assert!(validate_filename("").is_err());
    }

    #[test]
    fn test_validate_filename_rejects_separator() {
        assert!(validate_filename("a/b").is_err());
        assert!(validate_filename("a\\b").is_err());
    }

    #[test]
    fn test_validate_filename_rejects_dot_only() {
        assert!(validate_filename(".").is_err());
        assert!(validate_filename("..").is_err());
        assert!(validate_filename("...").is_err());
    }

    #[test]
    fn test_validate_filename_rejects_absolute_unix() {
        if !cfg!(windows) {
            assert!(validate_filename("/etc/passwd").is_err());
        }
    }

    #[test]
    fn test_validate_filename_rejects_windows_drive_letter() {
        if cfg!(windows) {
            assert!(validate_filename("C:foo.jar").is_err());
            assert!(validate_filename("D:\\bar.jar").is_err());
        }
    }

    // ------------------------------------------------------------------
    // Evidence read methods
    // ------------------------------------------------------------------

    #[test]
    fn test_evidence_get_confirmed_attribution() {
        let (ctx, tmp) = setup_ctx();
        let svc = CrashService::new(ctx);
        let fp = CrashFingerprint {
            exception_class: "java.lang.TestException".into(),
            top_frames: vec!["at com.example.Foo.bar".into()],
        };
        // No data yet
        let results = svc
            .get_confirmed_attribution(&fp.fingerprint_str())
            .unwrap();
        assert!(results.is_empty());

        // Record and verify
        svc.confirm_attribution(&fp, "mod-a").unwrap();
        svc.confirm_attribution(&fp, "mod-a").unwrap();
        svc.confirm_attribution(&fp, "mod-b").unwrap();
        let results = svc
            .get_confirmed_attribution(&fp.fingerprint_str())
            .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].mod_id, "mod-a");
        assert_eq!(results[0].confirm_count, 2);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_evidence_ruled_out_roundtrip() {
        let (ctx, tmp) = setup_ctx();
        let svc = CrashService::new(ctx);
        let fp = CrashFingerprint {
            exception_class: "java.lang.OtherException".into(),
            top_frames: vec![],
        };

        let ruled = svc.get_ruled_out_mods(&fp.fingerprint_str()).unwrap();
        assert!(ruled.is_empty());

        assert!(!svc.is_ruled_out(&fp.fingerprint_str(), "mod-x").unwrap());

        svc.rule_out(&fp, "mod-x").unwrap();
        svc.rule_out(&fp, "mod-y").unwrap();

        assert!(svc.is_ruled_out(&fp.fingerprint_str(), "mod-x").unwrap());
        let ruled = svc.get_ruled_out_mods(&fp.fingerprint_str()).unwrap();
        assert_eq!(ruled.len(), 2);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_evidence_survival_counts() {
        let (ctx, tmp) = setup_ctx();
        let svc = CrashService::new(ctx.clone());

        let total = svc.get_total_survival_count().unwrap();
        assert_eq!(total, 0);

        svc.record_survival("test-instance", &["mod-a".into(), "mod-b".into()])
            .unwrap();
        svc.record_survival("test-instance", &["mod-a".into()])
            .unwrap();

        let total = svc.get_total_survival_count().unwrap();
        assert_eq!(total, 2);

        let a_count = svc.get_mod_survival_count("mod-a").unwrap();
        assert_eq!(a_count, 2);

        let b_count = svc.get_mod_survival_count("mod-b").unwrap();
        assert_eq!(b_count, 1);

        let pair = svc.get_pair_survival_count("mod-a", "mod-b").unwrap();
        assert_eq!(pair, 1);

        let nonexistent = svc.get_mod_survival_count("mod-z").unwrap();
        assert_eq!(nonexistent, 0);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_evidence_pair_crash_count() {
        let (ctx, tmp) = setup_ctx();
        let svc = CrashService::new(ctx);

        let fp = CrashFingerprint {
            exception_class: "java.lang.TestException".into(),
            top_frames: vec![],
        };

        // record_crash_event wires co-crash for all pairs
        svc.record_crash_event(
            "test-instance",
            &fp,
            &["mod-a".into(), "mod-b".into(), "mod-c".into()],
            None,
        )
        .unwrap();

        let ab = svc.get_pair_crash_count("mod-a", "mod-b").unwrap();
        assert_eq!(ab, 1);

        let bc = svc.get_pair_crash_count("mod-b", "mod-c").unwrap();
        assert_eq!(bc, 1);

        // Non-existent pair
        let ac_unrelated = svc.get_pair_crash_count("mod-a", "mod-z").unwrap();
        assert_eq!(ac_unrelated, 0);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ------------------------------------------------------------------
    // Scoring tests (moved from desktop crash_investigator)
    // ------------------------------------------------------------------

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn make_conflict(a: &str, b: &str, severity: &str) -> crate::registry::KnownConflict {
        crate::registry::KnownConflict {
            mod_a_id: a.to_string(),
            mod_b_id: b.to_string(),
            severity: severity.to_string(),
            mitigated_by: vec![],
            notes: None,
        }
    }

    fn sample_crash_log() -> &'static str {
        r#"Java HotSpot(TM) 64-Bit Server VM warning: Sharing is only supported for boot loader classes
Exception in thread "Render thread" java.lang.RuntimeException: Crash!
    at net.minecraft.client.main.Main.main(SourceFile:42)
Caused by: java.lang.IllegalStateException: First cause
    at com.example.mod.Core.init(Core.java:10)
Caused by: java.lang.NullPointerException
    at me.jellysquid.nautilus.mixin.MixinFoo.render(MixinFoo.java:45)
    at com.example.mod.Core.onTick(Core.java:100)
    at net.minecraft.client.Minecraft.tick(Minecraft.java:520)
"#
    }

    #[test]
    fn test_fingerprint_str_stable() {
        let fp = CrashFingerprint {
            exception_class: "java.lang.NoClassDefFoundError".into(),
            top_frames: vec![
                "me.jellysquid.nautilus.Foo.bar".into(),
                "net.minecraft.client.Minecraft.tick".into(),
            ],
        };
        assert_eq!(
            fp.fingerprint_str(),
            "java.lang.NoClassDefFoundError|me.jellysquid.nautilus.Foo.bar|net.minecraft.client.Minecraft.tick"
        );
    }

    #[test]
    fn test_parse_crash_log_extracts_root_cause() {
        let result = parse_crash_log(sample_crash_log());
        assert!(
            result.is_some(),
            "parse_crash_log should return Some for a valid crash log"
        );
        let fp = result.unwrap();
        assert!(
            fp.exception_class.contains("NullPointerException"),
            "expected exception_class to contain NullPointerException, got '{}'",
            fp.exception_class
        );
    }

    #[test]
    fn test_parse_crash_log_no_exception_returns_none() {
        assert!(parse_crash_log("no crash here just text\n").is_none());
    }

    #[test]
    fn test_parse_crash_log_malformed_does_not_panic() {
        let garbage_inputs = [
            "",
            "   ",
            "null",
            "\x00\x01\x02\x7f\x7e",
            "Caused by:",
            "Caused by: ",
            "at no_space_here",
            "java.lang.\n",
            "Exception at foo.bar\n",
            "Caused by: java.lang.NullPointerException: \x00",
        ];
        for input in &garbage_inputs {
            let _ = parse_crash_log(input);
        }
    }

    #[test]
    fn test_score_a_stack_frame_hit() {
        let s = compute_mod_score(
            "sodium".into(),
            "sodium.jar".into(),
            &["me.jellysquid.nautilus".into()],
            &["me.jellysquid.nautilus.Foo.bar".into()],
            &[],
            &[],
            0,
            0,
            0,
            &[],
            &[],
        );
        assert!(
            s.total_score > 0.0,
            "expected A signal to produce score > 0, got {}",
            s.total_score
        );
    }

    #[test]
    fn test_score_a_no_match_zero() {
        let s = compute_mod_score(
            "unrelated".into(),
            "unrelated.jar".into(),
            &["com.unrelated".into()],
            &["me.jellysquid.nautilus.Foo.bar".into()],
            &[],
            &[],
            0,
            0,
            0,
            &[],
            &[],
        );
        assert!(
            approx(s.total_score, 0.0),
            "expected score ~0, got {}",
            s.total_score
        );
    }

    #[test]
    fn test_score_d_ubiquity_dampens_a() {
        let ubiquitous = compute_mod_score(
            "sodium".into(),
            "sodium.jar".into(),
            &["me.jellysquid.nautilus".into()],
            &["me.jellysquid.nautilus.Foo.bar".into()],
            &[],
            &[],
            0,
            10,
            8,
            &[],
            &[],
        );
        let rare = compute_mod_score(
            "sodium".into(),
            "sodium.jar".into(),
            &["me.jellysquid.nautilus".into()],
            &["me.jellysquid.nautilus.Foo.bar".into()],
            &[],
            &[],
            0,
            10,
            0,
            &[],
            &[],
        );
        assert!(ubiquitous.total_score > 0.0, "ubiquitous A must be > 0");
        assert!(rare.total_score > 0.0, "rare A must be > 0");
        assert!(
            ubiquitous.total_score < rare.total_score,
            "ubiquitous score ({}) should be strictly less than rare score ({})",
            ubiquitous.total_score,
            rare.total_score
        );
    }

    #[test]
    fn test_score_g_hard_conflict() {
        let conflict = make_conflict("optifine", "sodium", "hard");
        let s = compute_mod_score(
            "optifine".into(),
            "optifine.jar".into(),
            &[],
            &[],
            &["optifine".into(), "sodium".into()],
            &[conflict],
            0,
            0,
            0,
            &[],
            &[],
        );
        assert!(
            s.total_score >= 1.0,
            "hard conflict should yield score >= 1.0, got {}",
            s.total_score
        );
    }

    #[test]
    fn test_score_g_weak_conflict_lower_than_hard() {
        let conflict_hard = make_conflict("optifine", "sodium", "hard");
        let s_hard = compute_mod_score(
            "optifine".into(),
            "optifine.jar".into(),
            &[],
            &[],
            &["optifine".into(), "sodium".into()],
            &[conflict_hard],
            0,
            0,
            0,
            &[],
            &[],
        );

        let conflict_weak = make_conflict("optifine", "sodium", "weak");
        let s_weak = compute_mod_score(
            "optifine".into(),
            "optifine.jar".into(),
            &[],
            &[],
            &["optifine".into(), "sodium".into()],
            &[conflict_weak],
            0,
            0,
            0,
            &[],
            &[],
        );

        assert!(
            s_weak.total_score > 0.0,
            "weak conflict should be > 0, got {}",
            s_weak.total_score
        );
        assert!(
            s_weak.total_score < 1.0,
            "weak conflict should be < 1.0, got {}",
            s_weak.total_score
        );
        assert!(
            s_weak.total_score < s_hard.total_score,
            "weak ({}) should be strictly less than hard ({})",
            s_weak.total_score,
            s_hard.total_score
        );
    }

    #[test]
    fn test_score_g_mitigated_by_drops_to_zero() {
        let conflict = crate::registry::KnownConflict {
            mod_a_id: "optifine".into(),
            mod_b_id: "sodium".into(),
            severity: "hard".into(),
            mitigated_by: vec!["indium".into()],
            notes: None,
        };
        let s = compute_mod_score(
            "optifine".into(),
            "optifine.jar".into(),
            &[],
            &[],
            &["optifine".into(), "sodium".into(), "indium".into()],
            &[conflict],
            0,
            0,
            0,
            &[],
            &[],
        );
        assert!(
            approx(s.total_score, 0.0),
            "mitigated hard conflict should yield ~0, got {}",
            s.total_score
        );
    }

    #[test]
    fn test_score_survival_co_decay_overrides_stale_g() {
        let conflict = make_conflict("optifine", "sodium", "hard");
        let s = compute_mod_score(
            "optifine".into(),
            "optifine.jar".into(),
            &[],
            &[],
            &["optifine".into(), "sodium".into()],
            &[conflict],
            0,
            0,
            0,
            &[("sodium".into(), 0)],
            &[("sodium".into(), 5)],
        );
        assert!(
            s.total_score > 0.0,
            "decayed G should be > 0, got {}",
            s.total_score
        );
        assert!(
            s.total_score < 1.0,
            "decayed G should be < 1.0, got {}",
            s.total_score
        );
    }

    #[test]
    fn test_score_e_confirmed_prior() {
        let s = compute_mod_score(
            "mod".into(),
            "mod.jar".into(),
            &[],
            &[],
            &[],
            &[],
            3,
            0,
            0,
            &[],
            &[],
        );
        assert!(
            approx(s.total_score, 1.2),
            "expected total ~1.2 (E=1.0 + B=0.2), got {}",
            s.total_score
        );
    }

    #[test]
    fn test_score_b_proxy_when_confirmed() {
        let s = compute_mod_score(
            "mod".into(),
            "mod.jar".into(),
            &[],
            &[],
            &[],
            &[],
            1,
            0,
            0,
            &[],
            &[],
        );
        assert!(
            s.total_score > 0.0,
            "expected total > 0, got {}",
            s.total_score
        );
    }

    #[test]
    fn test_score_c_co_crash() {
        let s = compute_mod_score(
            "mod".into(),
            "mod.jar".into(),
            &[],
            &[],
            &[],
            &[],
            0,
            0,
            0,
            &[("other".into(), 10)],
            &[],
        );
        assert!(
            approx(s.total_score, 0.5),
            "expected C score ~0.5, got {}",
            s.total_score
        );
    }

    #[test]
    fn test_score_nan_coerced_to_zero() {
        let s = compute_mod_score(
            "mod".into(),
            "mod.jar".into(),
            &[],
            &[],
            &[],
            &[],
            0,
            0,
            0,
            &[],
            &[],
        );
        assert!(!s.total_score.is_nan(), "score should not be NaN");
        assert!(
            approx(s.total_score, 0.0),
            "expected score == 0.0, got {}",
            s.total_score
        );
    }

    #[test]
    fn test_suggested_action_serialization() {
        let sa = SuggestedAction::NoSuspects;
        let json = serde_json::to_string(&sa).unwrap();
        assert_eq!(json, r#"{"kind":"NoSuspects"}"#);

        let sa2 = SuggestedAction::GuidedDisable {
            next_suspect: SuspectScore {
                mod_id: "test-mod".into(),
                filename: "test.jar".into(),
                total_score: 0.5,
                breakdown: serde_json::json!({}),
                is_dependent_of: None,
            },
        };
        let json2 = serde_json::to_string(&sa2).unwrap();
        assert!(json2.contains("GuidedDisable"));
        assert!(json2.contains("test-mod"));
    }

    #[test]
    fn test_investigation_result_serialization() {
        let result = InvestigationResult {
            fingerprint: None,
            signature_name: None,
            suspects: vec![],
            suggested_action: SuggestedAction::NoSuspects,
            ruled_out: vec!["mod-a".into()],
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("NoSuspects"));
        assert!(json.contains("mod-a"));
    }
}
