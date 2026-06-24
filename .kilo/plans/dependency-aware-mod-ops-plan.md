# Plan: Dependency-Aware Auto Install/Enable/Remove/Disable

## Goal

Auto-install/enable dependencies and auto-remove/disable dependents based on extracted + manifest-declared mod dependency data, with a multi-select UX that pre-checks required mods, labels optional ones, and surfaces jar-vs-manifest disagreements with a recommendation to use the jar.

## Resolved decisions

- **Data source:** extraction-first (jar-extracted `mod_jar_id` + `depends_on` already in `InstalledMod` from prior phase), with a new optional **registry manifest field** `mod_dependencies` as curator fallback.
- **Conflict resolution:** when jar-extracted deps and manifest-declared deps disagree (different set OR different required/optional classification), the multi-select UI shows BOTH lists as separate rows with a source badge ("from jar" vs "from manifest"), with jar rows visually recommended (pre-checked + a "recommended" tag). The user picks per-row.
- **Required vs optional capture:** extend extraction to distinguish — Fabric `depends` → required; Fabric `recommends`/`suggests` → optional; Forge `[[dependencies.X]]` with `type="required"` → required, `type="optional"` → optional (currently all treated as required — fix). Forge `type="incompatible"` is captured separately as ANTI-deps for future known-conflicts feed use; not used in install/remove flow for v1.
- **UX (Option 3 + multi-select from Option 2):** destructive ops (remove/disable a dependency) → multi-select prompt of dependents with required pre-checked, optional unchecked + labeled "optional", action button + cancel. Additive ops (enable a dependent whose required dep is disabled) → **automatic + toast** ("Enabled Y (required by X)"). Install (adding a mod whose required deps are missing) → multi-select prompt of missing deps, same pre-check logic.
- **Matching key:** `mod_jar_id ↔ mod_jar_id` (NOT registry_id; jar ids are authoritative). Cross-source id mismatches (e.g. Modrinth-raw `"fabric_api"` vs catalog `"fabric"`) are NOT resolved automatically for v1 — user can manually trigger the prompt via "Troubleshoot dependencies" if needed.

## Non-goals / out of scope

- Version-pinned dependencies (e.g. `requires fabric-api >= 0.100`). Extraction captures ids only; incompatibilities handled reactively by the crash investigator.
- Resolving cross-source id aliasing (catalog vs Modrinth-raw vs manual-jar naming differences). Flagged as a known v1 limitation; a future alias table could close this.
- Anti-dependencies (Forge `type="incompatible"`) integrated into the known_conflicts Signal G feed — captured in storage now, surfaced later.
- Fully automatic operation. Every destructive dep action prompts; additive enable is the only auto path.

## Data model changes

### `InstalledMod` (models.rs) — additive fields
- Keep existing `depends_on: Vec<String>` — now scoped to REQUIRED deps only (clarify in doc comment). Back-compat: existing manifests with the field get all entries treated as required (no behavior regression; matches current "all required" assumption).
- ADD `#[serde(default)] pub optional_deps: Vec<String>` — optional deps (Fabric recommends/suggests, Forge type=optional).
- ADD `#[serde(default)] pub incompatible_deps: Vec<String>` — anti-deps (Forge type=incompatible). Stored for future G-feed use; not used in install/remove flow.

### `JarMetadata` (crash_investigator.rs) — mirror the above
- `depends_on: Vec<String>` → required-only.
- ADD `optional_deps: Vec<String>`.
- ADD `incompatible_deps: Vec<String>`.
- `parse_jar_metadata` parsing extended:
  - Fabric `fabric.mod.json`: `depends` keys → `depends_on`; `recommends` AND `suggests` keys → `optional_deps`; (no incompatible concept in Fabric).
  - Forge `META-INF/mods.toml`: each `[[dependencies.<id>]]` block — inspect the `type=` line inside the block (already walked line-by-line): `required`→`depends_on`, `optional`→`optional_deps`, `incompatible`→`incompatible_deps`. (Currently the inline `type=` isn't read — extend the parser to read it.)
  - Apply `DEPENDENCY_IGNORE_LIST` filter to ALL three lists (`minecraft`, `fabricloader`, `quilt_loader`, `java`).

### Registry manifest field (new, optional)
Add to registry manifest JSON files (e.g. `registry/mods/fabric-api.json`):
```json
"mod_dependencies": {
  "required": ["fabricloader"],
  "optional": ["sodium"],
  "incompatible": ["optifine"]
}
```
All three lists optional; field itself optional.

### Compiler (compile.py)
- Ingest `mod_dependencies` from each manifest.
- New table `mod_manual_dependencies ( item_id TEXT PRIMARY KEY, required_json TEXT, optional_json TEXT, incompatible_json TEXT, FOREIGN KEY (item_id) REFERENCES registry_items(id) )`.
- Bump `SCHEMA_VERSION` 3 → 4 (and desktop `APP_REGISTRY_SCHEMA_VERSION` 3 → 4 to match — coordinated bump per the existing pattern).
- Seed `fabric-api.json` with a realistic example (`required: ["fabricloader"]`).

### Desktop reader (registry.rs)
- New struct `ManifestDeps { required: Vec<String>, optional: Vec<String>, incompatible: Vec<String> }`.
- New query `get_manifest_dependencies(item_id: String) -> LauncherResult<Option<ManifestDeps>>` reading the new table; defensive `None` if table absent in older dbs.

## Tasks

### T1 — Extend `JarMetadata` + `InstalledMod` with optional/incompatible dep fields
- models.rs: add `optional_deps` + `incompatible_deps` to `InstalledMod` (`#[serde(default)]`).
- crash_investigator.rs `JarMetadata`: add the same two fields; `parse_jar_metadata` populates them (Fabric recommends/suggests; Forge type=optional/incompatible).
- Verify: cargo check (construction-site missing-field errors at install paths are EXPECTED; T5 fixes them).

### T2 — Compiler: ingest `mod_dependencies` + new table + schema bump
- compile.py: read `mod_dependencies` from manifests; insert into new `mod_manual_dependencies` table; bump SCHEMA_VERSION → 4.
- Seed `registry/mods/fabric-api.json` with `mod_dependencies: {required: ["fabricloader"]}`.
- Verify: `python compiler/compile.py --skip-sign` clean; `SELECT COUNT(*) FROM mod_manual_dependencies` ≥ 1.

### T3 — Desktop reader + APP_REGISTRY_SCHEMA_VERSION bump
- registry.rs: `ManifestDeps` struct + `get_manifest_dependencies(item_id)`. Bump `APP_REGISTRY_SCHEMA_VERSION` → 4 (in registry_sync.rs).
- Verify: cargo check.

### T4 — New `dependency_ops.rs` module (pure, testable)
Create `desktop/src-tauri/src/dependency_ops.rs`:
- `pub struct DependentInfo { pub mod_id: String, pub filename: String, pub requirement: Requirement, pub source: DepSource }` where `enum Requirement { Required, Optional }` and `enum DepSource { Jar, Manifest }`.
- `pub fn find_dependents(installed: &[InstalledMod], target_mod_jar_id: &str) -> Vec<DependentInfo>` — scans installed mods; for each, if `depends_on` contains target → Required+Jar; if `optional_deps` contains target → Optional+Jar.
- `pub fn resolve_install_deps(target_manifest_deps: Option<ManifestDeps>, target_jar_deps: &JarMetadata, installed: &[InstalledMod]) -> ResolvedInstallDeps` — merges jar deps + manifest deps into a unified list with conflict detection ( disagreement = a dep appears in one source but not the other, OR with different requirement classification). Returns:
  - `pub struct ResolvedInstallDeps { pub missing_required: Vec<DepCandidate>, pub missing_optional: Vec<DepCandidate>, pub conflicts: Vec<DepConflict> }` where `DepCandidate { mod_id, requirement, source }` and `DepConflict { mod_id, jar_requirement: Option<Requirement>, manifest_requirement: Option<Requirement> }`.
- `pub fn detect_source_disagreement(jar: &JarMetadata, manifest: Option<&ManifestDeps>) -> Vec<DepConflict>` — pure comparison.
- All pure functions (no AppHandle, no DB).

### T5 — Install hooks (mod_install.rs + modrinth_raw.rs)
- At all 4 install construction sites, switch from `parse_jar_packages` → `parse_jar_metadata` (already done for java_packages/mod_jar_id/depends_on — now also populate `optional_deps` + `incompatible_deps` from the same metadata struct). Fixes the construction-site errors from T1.

### T6 — Commands: dependency-aware install/remove/enable/disable
Files: commands.rs + lib.rs + tauri.ts.
Each existing entrypoint gains an optional "needs prompt" signal path:
- `install_mod_version` — BEFORE the actual install, call `dependency_ops::resolve_install_deps(...)`; return a `InstallPlan` containing `missing_required` + `missing_optional` + `conflicts`. The frontend prompts; on user confirm, the command continues installing the mod + selected deps (calling `install_mod_version` recursively for each selected dep).
- `remove_mod_from_instance` — BEFORE removing, call `find_dependents(installed, target.mod_jar_id)`; return a `RemovalPlan` with the dependents list. Frontend prompts (multi-select, required pre-checked); on confirm, remove the user-selected dependents AND the original mod.
- `enable_mod_for_test` — AUTO: if the enabled mod has required deps that are disabled, enable them automatically; emit a list of auto-enabled mod filenames (frontend shows a toast per). No prompt.
- `disable_mod_for_test` — BEFORE disabling, `find_dependents(...)`; return `DisablePlan`. Frontend prompts (multi-select, required pre-checked); on confirm, disable selected dependents AND the original mod.
- New command `get_install_plan(item_id, candidate) -> InstallPlan` (let frontend preview before confirming) and `get_removal_plan(instance_id, filename) -> RemovalPlan` and `get_disable_plan(instance_id, filename) -> DisablePlan`. The existing commands keep their signatures but return enriched result types (or new preview commands are added alongside — prefer preview commands to avoid breaking the existing happy-path returns).
- TS types + bindings for `InstallPlan` / `RemovalPlan` / `DisablePlan` / `DependentInfo` / `Requirement` / `DepSource`.

### T7 — Frontend multi-select dependency prompt component
- New `desktop/src/components/DependencyPrompt.tsx` — reusable modal: title, list of candidate mods as checkboxes with:
  - Required → pre-checked, badge "Required", disabled-checkbox (can't uncheck a hard-required dep without explicit override — or allow uncheck with a warning).
  - Optional → unchecked, badge "Optional".
  - Conflict rows rendered with source badge ("from jar" vs "from manifest") and a "recommended" tag on jar-sourced rows.
  - Action button ("Install selected" / "Disable selected" / "Remove selected") + Cancel.
  - No `dangerouslySetInnerHTML`; all labels plain text.
- Wire into ModDetail install flow (call `getInstallPlan` first → if plan has missing/conflicts → show `DependencyPrompt` → on confirm, continue install with selected deps).
- Wire into InstanceEditor remove flow (call `getRemovalPlan` → if dependents → `DependencyPrompt` → on confirm, remove selected).
- Wire into CrashInvestigator disable flow (call `getDisablePlan` → if dependents → `DependencyPrompt` → on confirm, disable selected).
- Enable auto-resolution: silent + toast (no prompt) when enabling a mod auto-enables a required dep.

### T8 — Rust unit tests for `dependency_ops.rs`
Pure functions are fully testable:
- `find_dependents` — required vs optional classification.
- `resolve_install_deps` — missing required/optional detection, conflict (disagreement) detection across jar+manifest sources.
- `detect_source_disagreement` — jar says optional, manifest says required → conflict flagged.

## Constraints / security (AGENTS.md)

- All SQL parameterized (`?`); never concatenate.
- No `dangerouslySetInnerHTML`; all dependency labels in the prompt render as plain React text.
- `.jar` parsing read-only; never writes outside instance dirs.
- Enable auto-resolution is the ONLY non-prompted destructive-ish path (enabling is additive + reversible: a disabled `.jar.disabled` re-enables; never destructive). Remove/disable always prompt.
- Schema bump coordination: registry.db v3→v4 must be matched in `compile.py` (`SCHEMA_VERSION`) AND `registry_sync.rs` (`APP_REGISTRY_SCHEMA_VERSION`); older clients reject with the existing schema-mismatch error.

## Key findings (from code inspection)

- `InstalledMod` (models.rs:57) already has `mod_jar_id` + `depends_on` from prior phase; additive `optional_deps`/`incompatible_deps` slots cleanly alongside.
- `parse_jar_metadata` already handles Fabric `depends` object + Forge `[[dependencies.X]]` headers; only the per-block `type=` inspection is new.
- No registry manifest has a dependency field today (`fabric-api.json` confirmed — has `package_signatures`, `base_categories`, etc., no deps). New `mod_dependencies` is truly additive.
- Operations exist as async commands: `install_mod_version` (mod_install.rs:404, commands.rs:436), `remove_mod_from_instance` (mod_install.rs:506, commands.rs:448), `disable_mod_for_test`/`enable_mod_for_test` (commands.rs:784/799 — these wrap crash_investigator::disable_mod/enable_mod). The crash investigator's disable/enable already do the `.jar.disabled` rename + manifest sync; we're layering dependency-awareness on top.
- Matching key is `mod_jar_id` (the mod's self-declared id), not `registry_id`. Both installed mods have it populated post-T5 (from prior phase + this plan's optional/incompatible extension).

## Risks / open questions for implementer

- **Recursion depth:** `install_mod_version` recursing for each missing required dep could stack deep if a dep itself has missing deps. Cap recursion at e.g. depth 5 and surface a "couldn't auto-resolve transitive deps for X" prompt. Recommended: iterate (worklist) rather than recurse, to bound easily.
- **Circular deps:** a mod A deps B, B deps A (shouldn't happen in practice but defensively guard). Track visited set in install planning.
- **Manifest field adoption:** curators won't backfill `mod_dependencies` for all mods immediately. The jar-extracted path is the default; manifest is fallback. UI degrades cleanly when manifest deps are absent (no conflict rows).
- **Cross-source id mismatches** (catalog `fabric-api` vs Modrinth-raw `fabric_api` vs jar `fabric`) — known v1 limitation; documented in the user-facing prompt as a possible troubleshooting step.
- **Forge `type=` parsing edge cases:** the type line can be `type = "required"` (with spaces) or `type='required'` (single quotes). The ad-hoc parser must be tolerant.
