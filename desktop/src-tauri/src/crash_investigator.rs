use crate::db;
use crate::error::{LauncherError, LauncherResult};
use crate::models::{InstalledMod, InstanceManifest};
use crate::paths;
use crate::registry;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json;
use std::collections::{HashMap, HashSet};

use std::path::Path;

// ---------------------------------------------------------------------------
// 1. JAR package parsing & metadata extraction
// ---------------------------------------------------------------------------

// JAR metadata parsing (mod ID, dependencies, incompatibilities) has been
// deduplicated to `agora_core::jar_metadata::parse_jar_metadata`. Only the
// thin Java-package extraction wrapper below remains here.

/// Open a .jar file as a zip archive and extract Java package directories
/// from `.class` entry paths.
///
/// An entry like `me/jellysquid/nautilus/Foo.class` yields the package
/// `me.jellysquid.nautilus` — the full directory path with the filename
/// stripped, segments joined by `.`. A minimum of 2 directory segments before
/// the filename is required to avoid noise from single-segment paths like
/// `Foo.class` at the zip root.
///
/// On ANY error (not a zip, io failure, etc.), returns `vec![]`. Never panics.
pub fn parse_jar_packages(jar_path: &Path) -> Vec<String> {
    agora_core::jar_metadata::parse_jar_metadata(jar_path).java_packages
}

// ---------------------------------------------------------------------------
// 2. CrashFingerprint
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// 3. Crash log parsing
// ---------------------------------------------------------------------------

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
            // Fallback: look for the first line matching java.lang.* or similar
            // exception pattern
            let first_exc = lines.iter().position(|line| {
                line.contains("java.lang.")
                    || (line.contains(" at ")
                        && line
                            .split(' ')
                            .any(|w| w.ends_with("Exception") || w.ends_with("Error")))
            });
            match first_exc {
                Some(idx) => lines[idx],
                None => return None,
            }
        }
    };

    // Extract the exception class name from the "Caused by:" line.
    // Format: "Caused by: fully.qualified.ExceptionName: message"
    // or: "Caused by: fully.qualified.ExceptionName"
    let exception_class = cause_line
        .split("Caused by:")
        .nth(1)
        .and_then(|rest| rest.split(':').next())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            // Fallback: extract from the line itself if it looks like an exception class
            cause_line
                .split_whitespace()
                .find(|w| w.ends_with("Exception") || w.ends_with("Error"))
        })
        .map(String::from);

    let exception_class = match exception_class {
        Some(ec) => ec,
        None => return None,
    };

    // Reject empty strings.
    if exception_class.is_empty() {
        return None;
    }

    // Extract up to 3 top stack frames after the exception line.
    // Stack frames start with "\t at " (or "    at " with spaces).
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
            // Non-frame, non-empty line after frames — stop collecting.
            break;
        }
    }

    Some(CrashFingerprint {
        exception_class,
        top_frames,
    })
}

// ---------------------------------------------------------------------------
// 4. SuspectScore
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// 5. SuggestedAction
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
// 6. InvestigationResult
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
// 7. record_crash_event
// ---------------------------------------------------------------------------

/// Record a crash event in the local state database and wire the co-crash
/// table for every mod pair in `mod_ids`.
pub fn record_crash_event<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
    fingerprint: &CrashFingerprint,
    mod_ids: &[String],
    signature_name: Option<&str>,
) -> LauncherResult<i64> {
    let conn = db::local_state_connection(app).map_err(|_| LauncherError::LocalStateFailed)?;

    let top_frames_json =
        serde_json::to_string(&fingerprint.top_frames).map_err(|_| LauncherError::Generic {
            code: "ERR_JSON_SERIALIZE".to_string(),
            message: "Failed to serialize top_frames.".to_string(),
        })?;

    db::insert_crash_event(
        &conn,
        instance_id,
        &fingerprint.fingerprint_str(),
        &fingerprint.exception_class,
        &top_frames_json,
        signature_name,
    )
    .map_err(|_| LauncherError::LocalStateFailed)?;

    // Wire the co-crash table for every pair.
    let mut pairs: Vec<(String, String)> = Vec::new();
    for a in mod_ids {
        for b in mod_ids {
            if a < b {
                pairs.push((a.clone(), b.clone()));
            }
        }
    }
    for (a, b) in pairs {
        let _ = db::record_co_crash(&conn, &a, &b);
    }

    Ok(conn.last_insert_rowid())
}

// ---------------------------------------------------------------------------
// 8. record_survival
// ---------------------------------------------------------------------------

/// Record that the instance survived a launch with the given mods installed.
pub fn record_survival<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
    mod_ids: &[String],
) -> LauncherResult<()> {
    let conn = db::local_state_connection(app).map_err(|_| LauncherError::LocalStateFailed)?;
    db::insert_survival(&conn, instance_id, mod_ids).map_err(|_| LauncherError::LocalStateFailed)
}

// ---------------------------------------------------------------------------
// 9. confirm_attribution
// ---------------------------------------------------------------------------

/// Increment the confirmation count for a mod_id matching a fingerprint.
pub fn confirm_attribution<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    fingerprint: &CrashFingerprint,
    mod_id: &str,
) -> LauncherResult<()> {
    let conn = db::local_state_connection(app).map_err(|_| LauncherError::LocalStateFailed)?;
    db::increment_confirmation(&conn, &fingerprint.fingerprint_str(), mod_id)
        .map_err(|_| LauncherError::LocalStateFailed)
}

// ---------------------------------------------------------------------------
// 10. rule_out
// ---------------------------------------------------------------------------

/// Mark a mod as ruled out for a given fingerprint.
pub fn rule_out<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    fingerprint: &CrashFingerprint,
    mod_id: &str,
) -> LauncherResult<()> {
    let conn = db::local_state_connection(app).map_err(|_| LauncherError::LocalStateFailed)?;
    db::add_ruled_out(&conn, &fingerprint.fingerprint_str(), mod_id)
        .map_err(|_| LauncherError::LocalStateFailed)
}

// ---------------------------------------------------------------------------
// 11. disable_mod
// ---------------------------------------------------------------------------

/// Rename `mods/<filename>` to `mods/<filename>.disabled` atomically.
///
/// If the `.disabled` file already exists, returns `Ok` without clobbering.
/// Updates the instance manifest but does NOT remove the InstalledMod entry
/// (MC ignores `.disabled` files).
pub fn disable_mod<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
    filename: &str,
) -> LauncherResult<()> {
    let sanitized = paths::sanitize_id(instance_id);

    let mods_dir = paths::instance_dir(app, &sanitized)
        .map_err(|_| LauncherError::InstanceCreateFailed)?
        .join("mods");

    let source = mods_dir.join(filename);
    let dest = mods_dir.join(format!("{}.disabled", filename));

    // If the .disabled file already exists, do not clobber.
    if dest.exists() {
        return Ok(());
    }

    // If source doesn't exist, nothing to do.
    if !source.exists() {
        return Ok(());
    }

    // Atomic rename.
    std::fs::rename(&source, &dest).map_err(|_| LauncherError::InstanceCreateFailed)?;

    // Update the manifest: the InstalledMod entry stays (MC ignores .disabled).
    let manifest_path = paths::instance_manifest_path(app, &sanitized)
        .map_err(|_| LauncherError::InstanceCreateFailed)?;

    if manifest_path.exists() {
        let text = std::fs::read_to_string(&manifest_path)
            .map_err(|_| LauncherError::InstanceCreateFailed)?;
        let mut manifest: InstanceManifest =
            serde_json::from_str(&text).map_err(|_| LauncherError::InstanceCreateFailed)?;

        // Find and update the matching mod entry (set a comment to note disabled).
        for mod_entry in &mut manifest.mods {
            if mod_entry.filename == filename {
                // Append a note to the version field to track disabled state.
                // We keep the original version and add a disabled marker.
                let original_version = mod_entry.version.clone().unwrap_or_else(|| String::new());
                mod_entry.version = Some(format!("{} [disabled]", original_version));
                break;
            }
        }

        // Atomic write: .tmp then rename.
        let tmp_path = manifest_path.with_extension("json.tmp");
        let write_text = serde_json::to_string_pretty(&manifest)
            .map_err(|_| LauncherError::InstanceCreateFailed)?;
        std::fs::write(&tmp_path, write_text).map_err(|_| LauncherError::InstanceCreateFailed)?;
        std::fs::rename(&tmp_path, &manifest_path)
            .map_err(|_| LauncherError::InstanceCreateFailed)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// 12. enable_mod
// ---------------------------------------------------------------------------

/// Reverse of `disable_mod`: rename `mods/<filename>.disabled` back to
/// `mods/<filename>` and update the manifest.
pub fn enable_mod<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
    filename: &str,
) -> LauncherResult<()> {
    let sanitized = paths::sanitize_id(instance_id);

    let mods_dir = paths::instance_dir(app, &sanitized)
        .map_err(|_| LauncherError::InstanceCreateFailed)?
        .join("mods");

    let disabled_path = mods_dir.join(format!("{}.disabled", filename));
    let source = mods_dir.join(filename);

    // If the file is already enabled (no .disabled), nothing to do.
    if !disabled_path.exists() {
        return Ok(());
    }

    // If the target already exists, do not clobber.
    if source.exists() {
        return Ok(());
    }

    // Atomic rename.
    std::fs::rename(&disabled_path, &source).map_err(|_| LauncherError::InstanceCreateFailed)?;

    // Update the manifest: remove the [disabled] marker from the version field.
    let manifest_path = paths::instance_manifest_path(app, &sanitized)
        .map_err(|_| LauncherError::InstanceCreateFailed)?;

    if manifest_path.exists() {
        let text = std::fs::read_to_string(&manifest_path)
            .map_err(|_| LauncherError::InstanceCreateFailed)?;
        let mut manifest: InstanceManifest =
            serde_json::from_str(&text).map_err(|_| LauncherError::InstanceCreateFailed)?;

        for mod_entry in &mut manifest.mods {
            if mod_entry.filename == filename {
                if let Some(ref v) = mod_entry.version {
                    if let Some(stripped) = v.strip_suffix(" [disabled]") {
                        if stripped.is_empty() {
                            mod_entry.version = None;
                        } else {
                            mod_entry.version = Some(stripped.to_string());
                        }
                    }
                }
                break;
            }
        }

        // Atomic write: .tmp then rename.
        let tmp_path = manifest_path.with_extension("json.tmp");
        let write_text = serde_json::to_string_pretty(&manifest)
            .map_err(|_| LauncherError::InstanceCreateFailed)?;
        std::fs::write(&tmp_path, write_text).map_err(|_| LauncherError::InstanceCreateFailed)?;
        std::fs::rename(&tmp_path, &manifest_path)
            .map_err(|_| LauncherError::InstanceCreateFailed)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// 13. compute_mod_score (pure)
// ---------------------------------------------------------------------------

/// Pure per-mod scoring computation — no `AppHandle`, no DB, no network.
///
/// Computes a `SuspectScore` for a single mod from pre-gathered inputs.
/// This is the behavior-preserving extraction of the per-mod loop body from
/// `score_suspects`.
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
    let ubiquity = (mod_survival_count as f64 / ubiquity_denom as f64)
        .min(1.0)
        .max(0.0);
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
// 14. score_suspects
// ---------------------------------------------------------------------------

/// Dynamic-weighted suspicion-scoring algorithm.
///
/// For each installed mod, computes a `total_score` from five signals (G, E,
/// A, B, C) with two confounder dampeners (ubiquity D and survival co-decay
/// on G). Returns one `SuspectScore` per installed mod sorted by score
/// descending.
pub fn score_suspects<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    _instance_id: &str,
    crash_text: &str,
    installed: &[InstalledMod],
) -> LauncherResult<Vec<SuspectScore>> {
    // -----------------------------------------------------------------------
    // Step 1 — parse the crash log
    // -----------------------------------------------------------------------
    let fingerprint = match parse_crash_log(crash_text) {
        Some(fp) => fp,
        None => {
            // Can't attribute — return 0 scores for all mods.
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
    // Stack frames look like: "    at com.example.Foo.bar(Foo.java:42)"
    let mut crashed_packages: HashSet<String> = HashSet::new();
    for line in crash_text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("at ") {
            let after_at = &trimmed[3..];
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
    // Step 2 — gather priors
    // -----------------------------------------------------------------------
    let conn = db::local_state_connection(app).map_err(|_| LauncherError::LocalStateFailed)?;

    let confirmed_map: HashMap<String, i64> =
        match db::get_confirmed_attribution(&conn, &fingerprint.fingerprint_str()) {
            Ok(rows) => rows
                .into_iter()
                .map(|r| (r.mod_id, r.confirm_count))
                .collect(),
            Err(_) => HashMap::new(),
        };

    let total_survivals: i64 = db::get_total_survival_count(&conn).unwrap_or(0);

    let known_conflicts: Vec<registry::KnownConflict> = match registry::get_known_conflicts(&conn) {
        Ok(v) => v,
        Err(_) => Vec::new(),
    };

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

        // Gather per-mod helpers before calling compute_mod_score

        // mod_survival_count
        let surv_count: i64 = db::get_mod_survival_count(&conn, &mod_id).unwrap_or(0);

        // pair_crash_counts: query local_crash_telemetry for each present other mod
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
            let (a, b) = db::normalize_pair(&mod_id, &other_id);
            let pair_crash: i64 = conn
                .query_row(
                    "SELECT crash_count FROM local_crash_telemetry \
                     WHERE mod_a_id = ?1 AND mod_b_id = ?2",
                    rusqlite::params![a, b],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            pair_crash_counts.push((other_id, pair_crash));
        }

        // pair_survival_counts: query crash_survival_mods for each G partner
        // (we only need survivals for mods involved in known conflicts)
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
            let pair_surv = db::get_pair_survival_count(&conn, &mod_id, other_id).unwrap_or(0);
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
    // Step 3b — indirect suspects: flag dependents of non-zero direct suspects.
    // -------------------------------------------------------------------

    // Build a reverse-dependency lookup: parent_mod_jar_id → Vec<dependent mod_id>.
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

    // Build a mod_id → InstalledMod lookup for filename resolution.
    let mut installed_by_id: HashMap<String, &InstalledMod> = HashMap::new();
    for mod_entry in installed {
        let mid = mod_entry
            .registry_id
            .clone()
            .unwrap_or_else(|| mod_entry.filename.clone());
        installed_by_id.insert(mid, mod_entry);
    }

    // Build a set of mod_ids already in suspects (direct candidates).
    let suspect_set: std::collections::HashSet<String> =
        suspects.iter().map(|s| s.mod_id.clone()).collect();

    // Collect indirect candidates separately to avoid borrowing `suspects` mutably
    // while iterating it immutably.
    let mut indirect_candidates: Vec<SuspectScore> = Vec::new();

    for s in &suspects {
        if s.total_score <= 0.0 {
            continue;
        }

        // Find the installed mod matching this suspect to get its mod_jar_id.
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
            // Skip if the dependent is already a direct candidate.
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
    // Step 4 — sort descending by total_score, tie-break mod_id ascending for determinism.
    // -------------------------------------------------------------------
    suspects.sort_by(|a, b| {
        let by_score = b
            .total_score
            .partial_cmp(&a.total_score)
            .unwrap_or(std::cmp::Ordering::Equal);
        if by_score != std::cmp::Ordering::Equal {
            return by_score;
        }
        // Tie-break: alphabetical mod_id ascending for determinism.
        a.mod_id.cmp(&b.mod_id)
    });

    Ok(suspects)
}

// ---------------------------------------------------------------------------
// 14. continue_investigation
// ---------------------------------------------------------------------------

/// Run the full investigation pipeline: score suspects, fetch ruled-out mods,
/// filter them out, pick the top remaining, and build an `InvestigationResult`.
///
/// If the top suspect's registry_id is `under_review` in the registry, the
/// suggested action is `ShowTriageBanner`.
pub fn continue_investigation<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
    fingerprint: &CrashFingerprint,
    installed: &[InstalledMod],
    crash_text: &str,
) -> LauncherResult<InvestigationResult> {
    let conn = db::local_state_connection(app).map_err(|_| LauncherError::LocalStateFailed)?;

    // Score suspects.
    let mut suspects = score_suspects(app, instance_id, crash_text, installed)?;

    // Fetch ruled-out mods for this fingerprint.
    let ruled_out = db::get_ruled_out_mods(&conn, &fingerprint.fingerprint_str())
        .map_err(|_| LauncherError::LocalStateFailed)?;

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
        // Check if the top suspect is under_review.
        let top = &suspects[0];
        let is_under_review = check_under_review(&conn, &top.mod_id);

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

/// Check whether a registry item has status `under_review`.
fn check_under_review(conn: &Connection, item_id: &str) -> bool {
    let mut stmt = match conn
        .prepare("SELECT 1 FROM registry_items WHERE id = ?1 AND status = 'under_review' LIMIT 1")
    {
        Ok(s) => s,
        Err(_) => return false,
    };
    match stmt.exists([item_id]) {
        Ok(true) => true,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// 15. Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::KnownConflict;
    use std::io::Write;
    use std::path::PathBuf;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Shared monotonic counter for all test-jar helpers so that concurrent
    /// test runs never collide on file names.
    static JAR_CTR: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
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

    /// Build a temporary .jar (zip) file with the given class entries and return its path.
    fn build_test_jar(entries: &[&str]) -> PathBuf {
        let id = JAR_CTR.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let jar_path =
            std::env::temp_dir().join(format!("agora-test-{}-{}.jar", std::process::id(), id));
        let file = std::fs::File::create(&jar_path).expect("create temp jar");
        {
            let mut zip = zip::ZipWriter::new(file);
            let opts: zip::write::FileOptions = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for entry in entries {
                zip.start_file(*entry, opts).expect("start zip entry");
                zip.write_all(&[]).expect("write zip entry");
            }
            zip.finish().expect("finish zip");
        }
        jar_path
    }

    /// Clean a temp jar if it exists.
    fn clean_jar(path: &PathBuf) {
        let _ = std::fs::remove_file(path);
    }

    /// Build a temporary .jar (zip) file with entries that have content and return its path.
    /// `entries` is a slice of `(path, content)` tuples.
    fn build_test_jar_with_content(entries: &[(&str, &str)]) -> PathBuf {
        let id = JAR_CTR.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let jar_path =
            std::env::temp_dir().join(format!("agora-test-{}-{}.jar", std::process::id(), id));
        let file = std::fs::File::create(&jar_path).expect("create temp jar");
        {
            let mut zip = zip::ZipWriter::new(file);
            let opts: zip::write::FileOptions = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (entry, content) in entries {
                zip.start_file(*entry, opts).expect("start zip entry");
                zip.write_all(content.as_bytes()).expect("write zip entry");
            }
            zip.finish().expect("finish zip");
        }
        jar_path
    }

    // -----------------------------------------------------------------------
    // A. CrashFingerprint + fingerprint_str
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // B. parse_crash_log
    // -----------------------------------------------------------------------

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
            &"\x00\x01\x02\x7f\x7e"[..],
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

    // -----------------------------------------------------------------------
    // C. parse_jar_packages
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_jar_packages_extracts_top_level_packages() {
        let jar_path = build_test_jar(&[
            "me/jellysquid/nautilus/Foo.class",
            "me/jellysquid/nautilus/Bar.class",
            "com/example/init/Baz.class",
            "assets/textures/foo.png",
            "META-INF/MANIFEST.MF",
        ]);
        let pkgs = parse_jar_packages(&jar_path);
        // Full directory path preserved, not truncated to 2 segments
        assert!(pkgs.contains(&"me.jellysquid.nautilus".to_string()));
        assert!(pkgs.contains(&"com.example.init".to_string()));
        // Old 2-segment truncation no longer occurs
        assert!(!pkgs.contains(&"me.jellysquid".to_string()));
        // Non-.class entries produce no packages
        assert!(!pkgs.contains(&"assets".to_string()));
        assert!(!pkgs.contains(&"META-INF".to_string()));
        clean_jar(&jar_path);
    }

    #[test]
    fn test_parse_jar_packages_distinguishes_same_developer_sibling_mods() {
        // Sodium and Lithium share the `me.jellysquid` prefix but differ at
        // the third segment. The old 2-segment code could not tell them apart.
        let sodium_jar = build_test_jar(&[
            "me/jellysquid/nautilus/SodiumCore.class",
            "me/jellysquid/nautilus/mixin/MixinRenderer.class",
        ]);
        let sodium_pkgs = parse_jar_packages(&sodium_jar);
        assert!(
            sodium_pkgs.contains(&"me.jellysquid.nautilus".to_string()),
            "sodium jar should contain me.jellysquid.nautilus, got {:?}",
            sodium_pkgs
        );
        assert!(
            sodium_pkgs.contains(&"me.jellysquid.nautilus.mixin".to_string()),
            "sodium jar should also contain nested package me.jellysquid.nautilus.mixin, got {:?}",
            sodium_pkgs
        );
        assert!(
            !sodium_pkgs.iter().any(|p| p.contains("lithium")),
            "sodium jar must not contain any lithium entries, got {:?}",
            sodium_pkgs
        );
        clean_jar(&sodium_jar);

        let lithium_jar = build_test_jar(&[
            "me/jellysquid/lithium/LithiumCore.class",
            "me/jellysquid/lithium/mixin/MixinTick.class",
        ]);
        let lithium_pkgs = parse_jar_packages(&lithium_jar);
        assert!(
            lithium_pkgs.contains(&"me.jellysquid.lithium".to_string()),
            "lithium jar should contain me.jellysquid.lithium, got {:?}",
            lithium_pkgs
        );
        assert!(
            lithium_pkgs.contains(&"me.jellysquid.lithium.mixin".to_string()),
            "lithium jar should also contain nested package me.jellysquid.lithium.mixin, got {:?}",
            lithium_pkgs
        );
        assert!(
            !lithium_pkgs.iter().any(|p| p.contains("nautilus")),
            "lithium jar must not contain any nautilus entries, got {:?}",
            lithium_pkgs
        );
        clean_jar(&lithium_jar);
    }

    #[test]
    fn test_parse_jar_packages_nonexistent_file_returns_empty() {
        let pkgs = parse_jar_packages(std::path::Path::new("/nonexistent/xyz.jar"));
        assert!(pkgs.is_empty());
    }

    #[test]
    fn test_parse_jar_packages_non_zip_returns_empty() {
        let txt_path =
            std::env::temp_dir().join(format!("agora-test-txt-{}.txt", std::process::id()));
        std::fs::write(&txt_path, "not a zip file").expect("write temp txt");
        let pkgs = parse_jar_packages(&txt_path);
        assert!(pkgs.is_empty());
        let _ = std::fs::remove_file(&txt_path);
    }

    // -----------------------------------------------------------------------
    // D. compute_mod_score
    // -----------------------------------------------------------------------

    fn make_conflict(a: &str, b: &str, severity: &str) -> KnownConflict {
        KnownConflict {
            mod_a_id: a.to_string(),
            mod_b_id: b.to_string(),
            severity: severity.to_string(),
            mitigated_by: vec![],
            notes: None,
        }
    }

    #[test]
    fn test_score_A_stack_frame_hit() {
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
    fn test_score_A_no_match_zero() {
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
    fn test_score_D_ubiquity_dampens_A() {
        // Ubiquitous mod: present in 80% of survivals
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
        // Rare mod: present in 0% of survivals
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
    fn test_score_G_hard_conflict() {
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
    fn test_score_G_weak_conflict_lower_than_hard() {
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
    fn test_score_G_mitigated_by_drops_to_zero() {
        let conflict = KnownConflict {
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
        // Conflict is mitigated by indium, so G = 0. No other signals.
        assert!(
            approx(s.total_score, 0.0),
            "mitigated hard conflict should yield ~0, got {}",
            s.total_score
        );
    }

    #[test]
    fn test_score_survival_co_decay_overrides_stale_G() {
        // Hard G conflict with 5 co-survivals → g_mod = max(0.1, 1.0 - 5*0.15) = max(0.1, 0.25) = 0.25
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
            &[("sodium".into(), 0)], // pair_crash_counts
            &[("sodium".into(), 5)], // 5 co-survivals
        );
        // G_final = 1.0 * 0.25 = 0.25
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
    fn test_score_E_confirmed_prior() {
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
        // E = min(3*0.5, 1.0) = 1.0, B = 0.2, total = 1.0 + 0.2 = 1.2
        // E alone is 1.0, but total includes B. Check E contribution.
        assert!(
            approx(s.total_score, 1.2),
            "expected total ~1.2 (E=1.0 + B=0.2), got {}",
            s.total_score
        );
    }

    #[test]
    fn test_score_B_proxy_when_confirmed() {
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
        // E = min(1*0.5, 1.0) = 0.5, B = 0.2, total = 0.7
        assert!(
            s.total_score > 0.0,
            "expected total > 0, got {}",
            s.total_score
        );
    }

    #[test]
    fn test_score_C_co_crash() {
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
            &[("other".into(), 10)], // 10 co-crashes → base_C = min(10*0.1, 0.5) = 0.5
            &[],
        );
        // D: total_survivals=0 → denom=1, mod_survival_count=0 → ubiquity=0 → dampener=1.0
        // C_final = 0.5 * 1.0 = 0.5
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

    // NOTE: The indirect-suspect augmentation logic (reverse-dependency flagging
    // inside `score_suspects`) requires an AppHandle and database access, so it
    // is not unit-testable here.  It is exercised through `continue_investigation`
    // integration paths.  The direct-candidate path via `compute_mod_score` is
    // covered by the tests above in Section D.
}
