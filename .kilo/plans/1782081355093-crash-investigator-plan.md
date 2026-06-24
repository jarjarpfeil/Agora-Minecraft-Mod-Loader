# Plan: Local Crash Investigator (Guided Isolation + Dynamic Learning)

## Goal

Replace slow manual binary-search mod isolation with a guided, learning, focused-search Crash Investigator. The system ranks suspect mods from a crash log, offers "Disable & relaunch," confirms/rule-outs, and auto-advances. It gets faster the more you use it via persisted priors. Fully local; designed so curated community conflict data and (later) a shared server slot in.

## Non-goals / Out of scope

- Watching the JVM process for instant crash popups (`launch_instance` uses `.spawn()` → detached grandchild via Mojang; not achievable). Reactive detection only.
- Anonymous crash telemetry upload (no aggregation endpoint exists; `$0/month` footprint). The `crash_telemetry_opt_in` setting is preserved for future shared-data use but its one-time prompt UI is commented out.
- Crash-to-mod reaction on GitHub issues (single-reaction-per-user limit makes it unusable).

## Resolved decisions

- **Attribution model:** Guided isolation with auto-advance.
  1. Detect crash (reactive) → parse log → fingerprint → score all installed mods.
  2. Suggest top suspect → "Disable & relaunch" button.
  3. If relaunch produces NO new crash → prompt "Did that fix it?" → confirming attributes fingerprint→mod permanently (E prior).
  4. If relaunch STILL crashes (same signature) → mark #1 ruled_out for this fingerprint → re-rank → suggest #2 automatically. Loop until culprit found or exhausted.
- **Detection trigger:** Reactive — consume existing `check_for_crash` (compares `crash-reports/*.txt` mtime vs `last_launched_at`) when returning to the Instances tab or reopening the app. PLUS a manual "Troubleshoot Crash" button (for log-less crashes / re-investigation).
- **Jar→package attribution (Signal A):** Auto-parse `.jar` contents (top-level package dirs) at install time across ALL install paths. Zero curation burden; works for manual drops.
- **Algorithm = dynamic weighted scoring (NOT a flat trust list).** Each signal outputs a numeric suspicion score; confounders act as multipliers, not flat rankings. Weights are variable and adjust over time.

### Scoring model

Base suspicion scores (each 0.0–N, summed with weights):
- **G curated conflict** — severity-tiered: `hard=1.0`, `weak/likely-works=0.3`. Applies only if *both* mods of the conflict pair are present in the crashing instance. Community-vetted truth → highest base weight. Instantly reduced to ~0 if a curated `mitigated_by` patch mod is present.
- **E confirmed prior** — confirmation count for this exact fingerprint, recency-decayed (F), capped. Strong; user-verified.
- **A stack-frame attribution** — count of crash-log stack frames whose package matches an installed mod's `java_packages`, × package-match confidence.
- **B fingerprint recurrence** — how often this fingerprint (exception class + top-3 frames) has been seen before; lets E priors reuse.
- **C co-crash** — pairwise `crash_count` from `local_crash_telemetry`, normalized vs the pair's total co-occurrence (crash+survival) baseline.

Confounders (multipliers applied to the above):
- **D survival ubiquity** — mod's ubiquity coefficient = crash_presence / total_presence. Mods ubiquitous in *successful* launches (sodium/JEI/fabric-api/create) get multiplier < 1.0 on their A and C scores. Handles "mod on stack just because it's everywhere."
- **Local survival co-decay (new)** — if two mods have co-survived N successful launches without incident, their pairwise G/C scores decay. Empirically overrides stale curated conflicts and handles un-catalogued patch/mod relationships. Decays regardless of whether a known mitigator exists.
- **F recency decay** — time-weight on E/C/B history; old signal fades so the tracker adapts to mod updates that fix bugs.

Gates / display rules:
- **H confidence auto-disable** — once (G hard-confirmed OR E ≥2×) AND the mod is present, offer one-click "Disable known culprit & relaunch" skipping the full guided sequence.
- **I crash-to-poll banner** — after ranking, if the recommended culprit's `status='under_review'` in local registry.db, surface "This mod is under community review for similar issues → view in Triage Center."

### Mitigator handling (G + local, Option 3)
- Curated conflict entries may include `mitigated_by: ["<mod_id>"]`. If any listed mitigator is present in the instance, the G conflict score drops to ~0 immediately (curated truth).
- Absent a known mitigator, local survival co-decay still applies as empirical override for stale entries and un-catalogued patches.

## Constraints / security (AGENTS.md)

- All SQL parameterized (`?`-bound); never concatenate/fmt-interpolate SQL.
- Crash log parsed locally; never uploaded.
- `dangerouslySetInnerHTML` forbidden on any community/curated content (render as plain React children).
- `.jar` parsing uses the existing `zip` crate (read-only directory listing); honor the Override Sanitization Engine's directory whitelist + Zip-Slip protections are not in play here (reading installed jars, not extracting overrides) — but never write outside the instance dir.
- Disable/enable mod = rename `mods/<name>.jar` ↔ `mods/<name>.jar.disabled` (standard MC convention); reversible; sync `instance_manifest.json` atomically.
- Resources: lightweight (regex over one log + scoring ~100 mods + small SQLite writes, ms-scale). No threading/perf-mode/prompt-to-run. Runs automatically on crash detection.

## Key findings (from code inspection)

- `desktop/src-tauri/src/db.rs:71` `local_crash_telemetry` table + `record_co_crash` (db.rs:272) **exist but are never called** → produce zero data today. Wire it at every crash event.
- Crash commands `check_for_crash`, `triage_crash`, `list_crash_reports`, `read_crash_log` are registered (`commands.rs`) but have **no frontend consumer**. The Investigator becomes that consumer.
- `instances.rs:308` `launch_instance` uses `.spawn()` (detached JVM via Mojang launcher) → can't track game exit; reactive detection is the only viable trigger (Options 2/3 infeasible).
- `models.rs:57` `InstalledMod` has `registry_id`/`source`/`sha256` but **no `java_packages` field** → add it (serde `#[serde(default)]` for back-compat with existing manifests).
- `local_state.db` is at schema v2 (after Phase 5). Crash tables will be migration v3.
- `registry.db` schema is at v2 (compiler `SCHEMA_VERSION`/desktop `APP_REGISTRY_SCHEMA_VERSION`). Adding a `known_conflicts` table bumps it to v3 and requires matched bumps in `compile.py` + desktop `registry.rs`.

## Tasks

### Task 1 — `models.rs`: add `java_packages` field
- Add `#[serde(default)] pub java_packages: Vec<String>` to `InstalledMod` (models.rs:57).
- Back-compat: existing `instance_manifest.json` files without the field deserialize to empty Vec.
- Verify: `cargo check`.

### Task 2 — `crash_investigator.rs` (new module): core engine
Create `desktop/src-tauri/src/crash_investigator.rs` with:
- `parse_jar_packages(jar_path: &Path) -> Vec<String>` — open `.jar` via existing `zip` crate, enumerate top-level package dirs (e.g. `me/jellysquid/nautilus` → `me.jellysquid.nautilus`), return sorted unique prefixes. Read-only; never writes.
- `CrashFingerprint { exception_class: String, top_frames: Vec<String> }` + `parse_crash_log(text: &str) -> Option<CrashFingerprint>` — extract exception class + top-3 stack frames (handle `Caused by:` chains).
- `score_suspects(...) -> Vec<SuspectScore>` — applies G/E/A/B/C base scores + D/survival-co-decay/F confounders. Reads `known_conflicts` (task 5), `crash_attribution`/`crash_ruled_out`/`local_crash_telemetry` (task 3). Returns ranked mod list with per-signal breakdown (for UI transparency).
- `record_crash_event(conn, fingerprint, installed_mod_ids)` — persists the crash event + wires `record_co_crash` for all present pairs (currently-dormant).
- `record_survival(conn, installed_mod_ids)` — logs successful launch mod-sets (Signal D baseline + survival co-decay).
- `confirm_attribution(conn, fingerprint, mod_id)` / `rule_out(conn, fingerprint, mod_id)` — E priors + ruled-out lists.
- `disable_mod(app, instance_id, filename)` / `enable_mod(app, instance_id, filename)` — rename `.jar`↔`.jar.disabled`, sync `instance_manifest.json` atomically (reuse the existing atomic `.tmp`+rename writer pattern in `mod_install.rs`).
- `continue_investigation(...) -> SuspectScore` — given a fingerprint + already-ruled-out set, returns the next suspect (auto-advance).
- `SuggestedAction` enum for UI: `GuidedDisable(next) | ConfidenceAutoDisable(mod) | ShowTriageBanner(mod)`.
- Do NOT register commands here (Task 4 wires them). Add `pub mod crash_investigator;` to `lib.rs` so it compiles.

### Task 3 — `db.rs` v3 migration: crash learning tables
Add migration block guarded by `current < 3` (schema_version row = 3). Tables (all `CREATE TABLE IF NOT EXISTS`):
- `crash_events (id INTEGER PRIMARY KEY AUTOINCREMENT, instance_id TEXT, fingerprint TEXT, exception_class TEXT, top_frames_json TEXT, signature_name TEXT NULL, occurred_at TEXT)` — per-crash log.
- `crash_survivals (id INTEGER PRIMARY KEY AUTOINCREMENT, instance_id TEXT, mod_set_json TEXT, occurred_at TEXT)` — Signal D baseline + survival co-decay source.
- `crash_attribution (fingerprint TEXT, mod_id TEXT, confirm_count INTEGER DEFAULT 0, last_confirmed_at TEXT, PRIMARY KEY(fingerprint, mod_id))` — Signal E priors.
- `crash_ruled_out (fingerprint TEXT, mod_id TEXT, ruled_out_at TEXT, PRIMARY KEY(fingerprint, mod_id))` — ruled-out lists.
- Reuse existing `local_crash_telemetry` (Signal C) — now populated by `record_co_crash` calls.
- Helper fns: `insert_crash_event`, `insert_survival`, `get_crash_history_for_fingerprint`, `get_confirmed_attribution`, `increment_confirmation`, `add_ruled_out`, `is_ruled_out`, `get_survival_counts`. All parameterized.

### Task 4 — `commands.rs` + `lib.rs` + `tauri.ts`: register commands
Tauri commands (thin wrappers around Task 2 fns):
- `investigate_crash(instance_id, filename?) -> InvestigationResult` — parses newest crash (or the named file), returns ranked suspects + suggested action.
- `investigate_manual(instance_id, log_text) -> InvestigationResult` — for the Troubleshoot button's pasted/typed log path.
- `confirm_crash_fix(instance_id, fingerprint, mod_id) -> ()` — E confirmation.
- `report_still_crashing(instance_id, fingerprint, mod_id) -> NextSuspect` — rule out + advance.
- `disable_mod_for_test(instance_id, filename) -> ()` / `enable_mod_for_test(instance_id, filename) -> ()`.
- `get_crash_history(instance_id) -> Vec<CrashEventSummary>` — UI history view.
- Register all in `generate_handler![]`. TS bindings + types in `tauri.ts`.

### Task 5 — Install-time jar package extraction (Signal A source)
Hook `parse_jar_packages` into the 4 install paths, writing `java_packages` onto the `InstalledMod`:
- `mod_install.rs::install_mod_version` (catalog)
- `mod_install.rs::add_manual_mod` (drag-drop)
- `modrinth_raw.rs::install_raw_modrinth` (Modrinth-raw)
- `mod_install.rs::import_instance_pack` bundled jars
Use existing `zip` crate (already a dependency for overrides). Defensive: extraction failure → empty Vec (never blocks install).
ALSO: call `record_survival` (Task 2) at the end of a successful `launch_instance` to feed Signal D.

### Task 6 — G curated known-conflict feed (community-curation advantage)
- **Registry format:** `registry/governance/known_conflicts.json` — array of `{ a: "<mod_id>", b: "<mod_id>", severity: "hard"|"weak", mitigated_by: ["<mod_id>"], notes: "..." }`.
- **Compiler:** `compile.py` ingests → `known_conflicts` table `(mod_a_id, mod_b_id, severity, mitigated_by_json)`; bump `SCHEMA_VERSION` 1→...→next (currently 2 → 3).
- **Desktop:** `registry.rs` read query `get_known_conflicts() -> Vec<KnownConflict>`; `APP_REGISTRY_SCHEMA_VERSION` bumped to match. Scorer (Task 2) consumes it (pluggable → degrades to empty when no data curated).
- **Validation:** seed 1-2 example entries so the path is exercised.

### Task 7 — Frontend: Crash Investigator UI + manual button + comment out telemetry prompt
- `desktop/src/components/CrashInvestigator.tsx` (new) — modal/panel opened on crash detection (Instances tab focus → calls `checkInstanceCrash`; if present → opens investigator). Shows ranked suspect cards with per-signal breakdown, "Disable & relaunch" button (calls `disable_mod_for_test` + `launch_instance`), post-relaunch "Did that fix it?" confirm, auto-advance on "Still crashing." Confidence one-click (H). Crash-to-poll banner (I) when culprit `under_review`.
- `desktop/src/pages/Instances.tsx` — add manual "Troubleshoot Crash" button → opens investigator (manual log paste path); wire reactive `checkInstanceCrash` on tab focus.
- `desktop/src/App.tsx:70-92` (state + handler) + `:124+` (the "Help improve Agora" prompt box) — COMMENT OUT (preserve `crash_telemetry_opt_in` set-setting write for future; just disable the prompt display). Add a code comment explaining why.
- `desktop/src/pages/Settings.tsx` — keep the explicit Crash Telemetry toggle (it still controls future opt-in); add a helper note that local crash learning runs regardless of this toggle.

### Task 8 — Validation
- Rust unit tests (`crash_investigator.rs` `#[cfg(test)]`): jar-package parsing (use a tiny test fixture zip if feasible, else synthetic), fingerprint extraction from sample logs, scoring with seeded priors (G hard vs weak, E decayed, A damped-by-D for ubiquitous mod, survival co-decay overriding a stale G).
- `cargo check` + frontend `npm run build` (`/desktop` sanity).
- Verify `record_co_crash` now populates `local_crash_telemetry` after a simulated crash event.

## Risks / open questions for implementer

- **Manifest back-compat:** existing `instance_manifest.json` files predate `java_packages`. The serde default handles read, but those mods have no packages until reinstalled/re-extracted. Investigator will still work (A signal just absent for them) — acceptable, no migration jiggering needed.
- **Schema bump coordination:** registry.db v2→v3 (known_conflicts) MUST be matched in both `compile.py` (`SCHEMA_VERSION`) and desktop `registry.rs` (`APP_REGISTRY_SCHEMA_VERSION`), or signature verification + schema check will reject the db.
- **Disable/enable mod reliability:** if the instance is mid-launch or the manifest is open elsewhere, the rename must be atomic. Reuse the existing `.tmp`+`rename` writer; if a `.jar.disabled` already exists, don't clobber.
- **Fingerprint stability:** top-3 frames can shift across MC versions; E priors may not transfer. Acceptable — F recency decay + new fingerprints just require re-learning. Document this.
- **Performance:** scoring ~100 mods with C/D requiring survival baseline queries — keep SQL indexed on `(mod_a_id, mod_b_id)` (already PK) and `(fingerprint)` columns.

## Rollout / migration

- local_state.db v2→v3 migration auto-runs on next app start (idempotent `CREATE TABLE IF NOT EXISTS`).
- registry.db v3 ships via next nightly compile; older clients reject with a clear schema-mismatch error (existing `registry_sync.rs` behavior).
- No data backfill required; learning accrues from this point forward.
