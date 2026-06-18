# Agora — Backlog

> Single source of truth for remaining work. Organized by spec phase, then by priority within each phase.
> Each item has a **short** summary (one line) and a **detailed** description (what, why, spec ref, acceptance criteria).
> Status: `[x]` done · `[~]` in progress · `[ ]` not started

---

## Phase 0 — Repository & Data Plumbing ✅

- [x] **Monorepo structure** — Create `registry/`, `compiler/`, `desktop/`, `web/`, `scripts/`, `.github/`, `.kilo/` per §1.
- [x] **Seed 5–10 example mods** — Sodium, Iris, Lithium, Fabric API, Starlight, Xaero's, + 1 pack.
- [x] **Pinned loader hashes** — `loader-manifests/loader_manifests.json` with Fabric/Quilt/NeoForge/Forge entries.
- [x] **Loader auto-refresh pipeline** — `scripts/refresh_loader_manifests.py` discovers stable MC versions, fetches/hashes/verifies.
- [x] **Rebrand** — All "Fine Wine" / "Curated Launcher" references replaced with "Agora".
- [x] **`.env` loading** — Compiler loads `.env` automatically; `ED25519_PRIVATE_KEY` works locally.

---

## Phase 1 — Compiler (Nightly GitHub Action)

### P1 · High Priority

- [x] **Flat-file ingestion → SQLite** (`compile.py`)
  - Parse `registry/mods/`, `registry/packs/`, `crash-signatures/`, `loader-manifests/`.
  - Build `registry_items`, `categories`, `item_categories`, `pack_mods`, `curator_reviews`, `crash_signatures`, `system_config` tables.

- [x] **Ed25519 signing** (`compile.py`)
  - Sign `registry.db` with `ED25519_PRIVATE_KEY` from env or `.env`.
  - Accepts 32- or 64-byte seeds. Fails loudly on missing key. `--skip-sign` for local dev.

- [x] **sha256 validation** (`compile.py`)
  - `validate_sha256` rejects `None`/`""` and exits non-zero. All manifests must have a real 64-hex hash.

- [x] **date_added via git log** (`compile.py`)
  - `manifest_date_added()` uses `git log --reverse --format=%aI` for deterministic first-commit date. Falls back to mtime for untracked files.

- [x] **DB indexes** — Added `idx_registry_items_*`, `idx_item_categories_*`, `idx_pack_mods_*` for query performance.

- [x] **Dotenv loading + error taxonomy improvement** — Compile loads `.env`; error codes aligned with spec §4.5.

### P2 · Medium Priority

- [ ] **Parse all content types**
  - **Short:** Ingest `shaders/`, `resourcepacks/`, `servers/`, `datapacks/`, `worlds/` directories.
  - **Detail:** Currently only `registry/mods/` and `registry/packs/` are iterated. §1 enumerates 7 content types; all must be discoverable in Browse and the web directory.
  - **Spec:** §1, §2.1
  - **Acceptance:** Compiler ingests manifests from all 7 directories; `verify_db.py` shows non-zero counts for each type.

- [x] **Release-asset upload in CI** (`compile.yml`)
  - **Short:** Wire the GitHub Release Asset upload step; create `scripts/deploy_release_assets.py`.
  - **Detail:** `compile.yml` uploads an ephemeral Actions artifact instead of a tagged release asset. Without this, the desktop client and web directory cannot fetch `registry.db` from GitHub Releases. Create the deploy script (tags with `registry-<date>`, uploads `registry.db` + `registry.db.sig`, cleans old assets), uncomment the upload step, and verify the release appears on GitHub.
  - **Spec:** §3.1 step 13
  - **Acceptance:** A nightly run produces a visible GitHub Release with `registry.db` + `.sig` attached.

- [ ] **GitHub API social metrics integration**
  - **Short:** Fetch reactions, comments, trust scores, and velocity data from the GitHub API during compilation.
  - **Detail:** Steps 3–9 of §3.1 are entirely absent. `upvotes`/`downvotes`/`net_score`/`velocity` are hardcoded to `0`/`0`/`0`/`0.0`. Implement: (a) emoji reaction counting from issue comments, (b) trust filtering via `user.contributionsCollection` scoped to the org, (c) Sybil resistance, (d) velocity circuit breaker (7-day rolling window with decay), (e) reaction scrubbing/NLP filtering (profanity-check, vaderSentiment), (f) Discussions poll resolution.
  - **Spec:** §3.1 steps 3–9, §5
  - **Acceptance:** `verify_db.py` shows non-zero `upvotes`/`net_score` for seeded mods.

- [ ] **Modrinth batch image hydration**
  - **Short:** Call `GET /v2/projects?ids=[...]` to populate `icon_url` and `gallery_urls` for Modrinth-sourced mods.
  - **Detail:** Currently `icon_url`/`gallery_urls` are simply propagated from the manifest. For `modrinth_id` strategy mods, the compiler should batch-query the Modrinth API (up to 500 IDs per request) and populate image URLs from the API response.
  - **Spec:** §3.1 step 11
  - **Acceptance:** Modrinth-sourced mods have populated `icon_url` after a compile run.

- [ ] **Audit log generation**
  - **Short:** Generate `registry/governance/audit_log.json` during compilation with rotation.
  - **Detail:** No audit log is currently produced. §4.6 requires an append-only transparency log of governance actions (immune grants, velocity overrides, reaction scrubs, trust filter exclusions). Must enforce rotation (e.g., keep last 1000 entries per file, archive old ones). Also add `audit_log_json` row to `registry.db`.
  - **Spec:** §4.6
  - **Acceptance:** `audit_log.json` exists after compile; Transparency Log UI has data to surface.

- [ ] **Raid Shield (Interaction Limits toggle)**
  - **Short:** Programmatically enable GitHub Interaction Limits on velocity anomalies during compilation.
  - **Detail:** When the velocity circuit breaker fires for an item (rapid reaction spike indicating a coordinated raid), the compiler should call the GitHub API to enable interaction limits on the affected issue/repo.
  - **Spec:** §3.2
  - **Acceptance:** A simulated velocity spike triggers interaction limits on the test repo.

### P3 · Low Priority

- [ ] **Regex DoS protections** (§2.4.1)
  - **Short:** Add 256-char pattern length validator on crash signatures; add compiler-side 100KB corpus / 50ms timeout test; add Rust `regex` crate with startup precompilation cache.
  - **Detail:** Current crash signature patterns are benign but un-gated. §2.4.1 mandates: (a) reject patterns longer than 256 chars, (b) test each pattern against a 100KB corpus with a 50ms timeout, (c) precompile all patterns at startup in the Rust client and cache the compiled `RegexSet`.
  - **Spec:** §2.4.1
  - **Acceptance:** A pathological regex pattern is rejected at compile time; the client precompiles all signatures without measurable startup delay.

- [ ] **CODE_OF_ENGAGEMENT.md in 3 locations** (§5.1)
  - **Short:** Ensure the Code of Engagement exists in README, review-form.yml, and the desktop "Write a Review" consent modal.
  - **Detail:** Only 2 of 3 required locations exist. Add a CI workflow step that copies `CODE_OF_ENGAGEMENT.md` into all three locations to prevent drift. The third location is the in-app review consent modal (desktop UI).
  - **Spec:** §5.1 (3rd location), §5 CI copy step
  - **Acceptance:** `grep -r "Code of Engagement"` finds the text in all three required locations; CI enforces this.

---

## Phase 2 — Tauri Desktop App & Instance Engine

### P1 · High Priority

- [x] **Tauri project initialized** — React + Tailwind + Vite, 5 sidebar tabs (Home, Browse, Instances, Governance, Settings).
- [x] **`local_state.db` schema + migrations** — `user_settings`, `user_instances`, `local_crash_telemetry`, `mcp_approval_grants`, `schema_version`.
- [x] **Instance creation + loader injection** — Fabric/Quilt profile JSON injection with domain pinning, SHA-256 verification, stable-hash canonicalization, three-stage rollback on failure.
- [x] **`launcher_profiles.json` atomic mutation + recovery** — `.tmp` → `rename()` with `.bak` backup; corrupt live file never poisons backup; minimal regeneration fallback.
- [x] **Mojang launcher discovery + delegation** — OS-specific path resolution (Windows/macOS/Linux); `Command::new(launcher).arg("--profile")`.
- [x] **JVM argument builder** — Memory + GC + custom args + AlwaysPreTouch assembly (§8.5).
- [x] **Typed registry queries** — Replaced raw-SQL `queryRegistry` with parameterized `browse_items`, `get_registry_item`, `list_categories`.
- [x] **Browse page wired** — Categories fetched dynamically, sort/filter/content-type working.
- [x] **Settings persistence** — Modrinth/AI toggles + launcher path persist to `local_state.db` via `get_setting`/`set_setting`.
- [x] **Crash telemetry pair normalization + retention** — `normalize_pair()`, `record_co_crash()`, `purge_stale_crash_telemetry()` (90-day + count < 2 purge).
- [x] **Error taxonomy improved** — Added `LocalStateFailed`, `InstanceCreateFailed`, `ProfileWriteFailed`, `RegistryMissing`; removed incorrect `ERR_LAUNCH_FAILED` mappings.

### P2 · Medium Priority

- [x] **Registry.db download + Ed25519 verify + atomic replace** (§4, §4.1a)
  - **Short:** Client-side flow that fetches `registry.db` + `.sig` from GitHub Releases, verifies the Ed25519 signature, checks schema version, and atomically replaces the cached copy. Implemented in `desktop/src-tauri/src/registry_sync.rs`.
  - **Detail:** This is the #1 blocker for the app reaching its primary data source. The Rust backend currently only opens `registry.db` read-only if it already exists; there is no download, no signature verification, and no atomic replace. Implement: (a) query GitHub Releases API for latest `registry-*` tag, (b) download `registry.db` + `registry.db.sig`, (c) verify Ed25519 signature using a hardcoded public key, (d) check `schema_version` against `APP_REGISTRY_SCHEMA_VERSION`, (e) write to `.tmp`, rename atomically, (f) implement degraded/offline mode fallback to cached DB, (g) readers-writer lock to prevent replacement during active launches.
  - **Spec:** §4, §4.1a, §4.3
  - **Acceptance:** On first run, the app downloads `registry.db`, verifies its signature, and Browse shows real curated items. Offline launch works with cached DB.

- [x] **Override sanitization engine** (§7.2)
  - **Short:** Implement zip extraction with directory whitelist, zip-bomb limits, banned extensions, and Zip Slip protection.
  - **Detail:** This is a critical security control (§15 threats #4/#5) that must land before any pack-install feature. Add the `zip` crate. Implement: (a) max 500MB uncompressed / 2GB total / 5000 files, (b) directory whitelist: `config/`, `defaultconfigs/`, `resourcepacks/`, `kubejs/` — **NOT `mods/`**, (c) banned executable extensions (`.exe`, `.dll`, `.so`, `.dylib`, `.sh`, `.bat`, `.cmd`, `.ps1`), (d) Zip Slip protection (reject paths with `..` or absolute paths), (e) per-file logging of skipped/extracted items.
  - **Spec:** §7.2, §15
  - **Acceptance:** A malicious zip with `mods/evil.jar`, `../../evil.exe`, and a 10GB padding file is rejected; a valid config-only override extracts successfully.

- [x] **NeoForge/Forge installer support** (§8.2)
  - **Short:** Installer-jar execution for NeoForge and Forge loaders. Implemented in `desktop/src-tauri/src/instances.rs` `inject_loader` (`installer_jar` branch: stages verified jar → `java -jar <installer> --installClient` → cleanup → `ERR_INSTALLER_FAILED`). Loader manifests pinned with neoforge + forge installer_jar entries.
  - **Detail:** The installer jar is downloaded via `download::download_verified` (SHA-256 verified against pinned hash), staged in the app data dir, run with `java -jar --installClient` on a blocking thread, and cleaned up regardless of outcome. `loader_version_id` derives `neoforge-{v}` and `forge-{mc}-{v}` IDs. Errors map to `ERR_INSTALLER_FAILED`.
  - **Spec:** §8.2 (MVP scope lists all 4 loaders)
  - **Acceptance:** User can create a NeoForge or Forge instance and launch successfully.

- [x] **Onboarding flow** (§6.1a)
  - **Short:** First-run welcome screen, integration configuration, and OAuth prompt. Implemented in `desktop/src/pages/Onboarding.tsx` (4-step flow: welcome → services → github → registry), gated by `onboarding_complete` setting in `App.tsx`.
  - **Detail:** (a)–(e) implemented: Welcome screen with Agora mission + "Get Started", "Connect External Services" panel with Modrinth + AI/MCP toggles (both default OFF), GitHub Device Flow with "I'll do this later" → Browse-Only Mode, registry.db download on first run. (d) profile icon badge and (f) tutorial overlay are optional polish not in the acceptance criteria; deferred.
  - **Spec:** §6.1a
  - **Acceptance:** New user sees welcome → toggles → can skip OAuth → lands on Home with registry loaded.

- [x] **Crash diagnostics** (§9, Phase 4)
  - **Short:** Pre-launch interceptor, regex signature matching, GitHub issue duplicate search, preview-before-submit, manual log viewer. Implemented in `desktop/src-tauri/src/crash_diagnostics.rs` (`check_for_crash`, `triage_crash`, `list_crash_reports`, `read_crash_log`).
  - **Detail:** (a) Pre-launch interceptor reads `last_launched_at` (already fixed to update before spawn); if the previous launch crashed, show crash prompt. (b) Add Rust `regex` crate; read `crash_signatures` table; match against latest crash log. (c) Search GitHub issues for known duplicate patterns. (d) Show preview of what will be submitted before creating a GitHub issue. (e) Manual log viewer panel for browsing `crash-reports/`. (f) Local crash telemetry already has `record_co_crash()` + retention purge; wire it into the crash detection flow.
  - **Spec:** §9, Phase 4
  - **Acceptance:** A simulated crash matches a regex signature, shows the fix hint, and the user can preview + submit a GitHub issue.

- [x] **OAuth + token storage** (§7.5, §5.1)
  - **Short:** GitHub Device Flow + keyring/AES-256-GCM token storage; enables voting, reviews, crash reporting, and triage. Implemented in `desktop/src-tauri/src/auth.rs` (Device Flow + OS keyring store/read/delete). AES-256-GCM encrypted-file fallback not yet implemented.
  - **Detail:** (a) Implement GitHub Device Flow (`POST /login/device/code` → poll `POST /login/oauth/access_token`). (b) Store token in OS keyring via `keyring` crate. (c) Fallback: AES-256-GCM encrypt to `tokens.enc` in app data dir with machine-bound key. (d) Token is never in config files, env vars, or SQLite. (e) Use token for: voting (emoji reactions), reviews (issue comments), crash reports (issue creation), flag submission, and triage participation. (f) Browse-Only Mode: all of the above gracefully degrade when token is absent.
  - **Spec:** §7.5, §5
  - **Acceptance:** User signs in via Device Flow; can vote on a mod; token survives restart via keyring.

- [ ] **Instance detail panel** (§6.5)
  - **Short:** Wire `get_instance_detail` to an instance detail view showing installed mods from `instance_manifest.json`.
  - **Detail:** The command exists but has no UI consumer. Build a detail view when clicking an instance card: shows installed mods list (filename, version, source, hash), JVM settings, lock state, and supports "Check for Pack Update", "Export Pack", "Unlock/Revert" actions.
  - **Spec:** §6.5
  - **Acceptance:** Clicking an instance shows its mod list and settings.

- [ ] **`AlwaysPreTouch` toggle + GC-conditional default** (§8.5)
  - **Short:** Add a Settings UI toggle for AlwaysPreTouch; default ON for G1GC, OFF (with warning) for ZGC/Shenandoah.
  - **Detail:** Currently hardcoded `always_pre_touch: true`. Spec says: ON for G1GC (recommended), OFF for ZGC/Shenandoah ("may cause issues with ZGC"). Add a checkbox in instance JVM settings + conditional logic in `JvmConfig`.
  - **Spec:** §8.5
  - **Acceptance:** Switching GC to ZGC warns about AlwaysPreTouch; user can toggle it.

### P3 · Low Priority

- [ ] **Windows Mojang discovery completion** (§8.4)
  - **Short:** Add registry-key query (`HKLM\SOFTWARE\Mojang\Launcher\InstallPath`) and `Get-AppxPackage` Microsoft Store check. Linux: $PATH priority search.
  - **Spec:** §8.4
  - **Acceptance:** App finds launcher installed via MSIX/registry on Windows.

- [ ] **MCP server** (§10)
  - **Short:** Implement localhost MCP server with ephemeral port, per-session token, 6 tools, approval queue, and system context injection.
  - **Detail:** (a) Bind to `127.0.0.1` on an ephemeral port, (b) Bearer token auth via `LAUNCHER_MCP_TOKEN`, (c) 6 tools: `list_instances`, `list_instance_mods`, `disable_mod`, `search_crash_signatures`, `suggest_mod_incompatibility`, `get_system_context`, (d) approval state machine with persistent grants in `local_state.db`, (e) `resources/list` exposing `system_context.md`, (f) toggle on/off from Settings.
  - **Spec:** §10
  - **Acceptance:** Claude Desktop connects with token, calls `list_instance_mods`, user sees approval prompt.

- [ ] **Dev Mode (sandboxed builds)** (§11)
  - **Short:** Detect Docker/Podman/Firecracker; clone + build mod .jar in sandbox with no network.
  - **Spec:** §11
  - **Acceptance:** User can build a mod from a GitHub URL inside Docker and test it.

- [ ] **Anonymous crash telemetry aggregation** (§12)
  - **Short:** Opt-in weekly compression + upload of `local_crash_telemetry` table to an aggregation endpoint.
  - **Spec:** §12
  - **Acceptance:** Opt-in user's crash matrix data is compressed and submitted weekly.

---

## Phase 3 — Browse, Discovery & Search

- [~] **Mod detail page** (§6.2)
  - **Short:** Clicking a Browse item opens a detail page with version picker, compatibility info, download button.
  - **Status:** PAGE DELIVERED + WIRED. `desktop/src/pages/ModDetail.tsx` fetches the item (`get_registry_item`), renders icon/badges/upvotes·downvotes·net·velocity/immunity shield/curator notes/compatible-versions list/reviews section; `App.tsx` wires `Browse.onSelectMod` → `<ModDetail>`. The **Install button is a stub** and is the only remaining gap.
  - **Blocker:** The `registry_items` table (compiler `compile.py` schema) and the mod manifests pin only `source_identifier` + `download_strategy` + a single `sha256` — there is **no `download_url` column**. The install action therefore needs one of: (a) adding `download_url` to manifests + a compiler column + struct field, with each URL verified against its pinned `sha256` (needs network fetch of the curated artifacts); or (b) Phase 3 live version resolution (GitHub Releases / Modrinth version API at install time, matching a candidate `.jar` against the pinned hash). Until one lands, the install button stays a stub. Tracked together with "Pack install flow" (§7.1.1).
  - **Spec:** §6.2
  - **Acceptance:** User opens a mod, sees compatible versions, can install it to their active instance. (First two clauses met; install pending the download-URL decision above.)

- [ ] **"For You" algorithm** (§6.2)
  - **Short:** Track locally installed categories; boost uninstalled mods in matching categories.
  - **Spec:** §6.2
  - **Acceptance:** After installing 3 "magic" mods, Browse surfaces more magic mods.

- [ ] **Raw Modrinth tab** (§6.3)
  - **Short:** Live Modrinth API search with uncurated warning banner and SHA-1 hash verification.
  - **Spec:** §6.3
  - **Acceptance:** User can search Modrinth directly, download a mod, and it's hash-verified before writing to `mods/`.

- [ ] **Manual .jar drag-and-drop** (§6.5b)
  - **Short:** Drag-and-drop .jar files into an instance's `mods/` folder.
  - **Spec:** §6.5b
  - **Acceptance:** Dragged file appears in `instance_manifest.json` with `source: "manual_drag_drop"`.

- [ ] **Pack export (.mrpack / custom JSON)** (§6.5c)
  - **Short:** Export an instance as a shareable `.mrpack` or custom `.json` pack file.
  - **Spec:** §6.5c
  - **Acceptance:** Exported file is 5–20KB and can rebuild the instance on another machine.

- [ ] **Pack install flow with partial-failure fallback** (§7.1.1)
  - **Short:** Download all mods in a pack concurrently (6 at a time, 3 retries); on partial failure, install what succeeded and report missing mods.
  - **Spec:** §7.1.1
  - **Acceptance:** A pack with one broken link installs all other mods and shows a "1 mod failed" notice.

- [ ] **Unlock/Revert state machine** (§6.5)
  - **Short:** Implement the lock → unlock → revert state machine for curated pack instances.
  - **Spec:** §6.5
  - **Acceptance:** User can unlock a pack instance, add manual mods, and revert to original.

---

## Phase 4 — Web Directory

- [x] **Static Next.js export** — 19 pages generated from `registry.db`.
- [x] **Landing page + about page + content-type pages + detail pages + client-side search/filter.**
- [x] **Image URL scheme validation** — Only `https:` and `data:` render.

- [ ] **react-markdown strict allow-list** (§4.1c #3, §13)
  - **Short:** Replace plain-text curator notes with strict markdown rendering allowing only `p`, `strong`, `em`, `code`, `a`, `pre`, `ul`, `ol`, `li`.
  - **Spec:** §4.1c #3, §13
  - **Acceptance:** Curator note renders bold/italic/links but never raw HTML.

- [ ] **Fetch registry.db from GitHub Release Asset during CI** (§13)
  - **Short:** Web build should fetch the latest `registry.db` from GitHub Releases, not read a sibling file.
  - **Detail:** `web/src/lib/db.ts` currently reads `../registry.db` from the filesystem. Spec says the web build should fetch the latest release asset during CI. This requires a CI step that downloads `registry.db` before `next build`.
  - **Spec:** §13
  - **Acceptance:** `npm run build` in CI works without a local `registry.db`.

- [ ] **Category / MC version / loader filters on web**
  - **Short:** Add category chips, MC version dropdown, and loader filter to the web catalog.
  - **Spec:** §13
  - **Acceptance:** Web visitor can filter by category and MC version.

- [ ] **Velocity / newest sort options on web**
  - **Short:** Add velocity and date_added sort options alongside net_score.
  - **Spec:** §13
  - **Acceptance:** Web visitor can sort by "Trending" and "Newest."

- [ ] **Top community reviews on detail page**
  - **Short:** Display `top_reviews_json` on the web mod detail page.
  - **Spec:** §13
  - **Acceptance:** Top reviews render as plain text with attribution.

---

## Phase 5 — Governance & Triage

- [ ] **Triage Center tab** (§5, §6.1)
  - **Short:** Implement the Community Governance tab with under-review items, live poll data, and recent resolutions.
  - **Detail:** (a) Query `registry_items WHERE status = 'under_review'`, (b) integrate GitHub Discussions API for poll percentages, (c) "Recent Resolutions" feed showing recently promoted/demoted items, (d) flag review creation (GitHub issue direct from app).
  - **Spec:** §5, §6.1
  - **Acceptance:** Under-review item appears in Triage Center with live poll percentage.

- [ ] **Curator Shield banner** (§5.4)
  - **Short:** Display a non-dismissable steel-blue banner on immune items' detail pages.
  - **Spec:** §5.4
  - **Acceptance:** Immune mod profile page shows "Curator Shield" banner above download button.

- [ ] **Flag Review system** (§5.6)
  - **Short:** "🚩 Flag Review" button on every comment (rate-limited).
  - **Spec:** §5.6
  - **Acceptance:** User can flag a comment; triggering creates a GitHub issue in `agora-mc/admin-alerts`.

- [ ] **In-app Transparency Log** (§4.6)
  - **Short:** Display `audit_log_json` entries in the Governance tab.
  - **Spec:** §4.6
  - **Acceptance:** User can see governance actions (immune grants, velocity overrides) in a scrollable log.

---

## Phase 6 — Polish & Hardening

- [ ] **Error envelope shape** (§4.5)
  - **Short:** Serialize errors as `{success, error: {code, message, details, suggested_action}}` instead of bare tagged enums.
  - **Spec:** §4.5
  - **Acceptance:** Frontend receives structured error envelope with `suggested_action` field.

- [ ] **CSP additions** (§8.2, §7.1)
  - **Short:** Add `neoforged.net`, `maven.neoforged.net`, `minecraftforge.net`, `files.minecraftforge.net`, `raw.githubusercontent.com` to `connect-src`.
  - **Spec:** §8.2, §7.1
  - **Acceptance:** CSP allows NeoForge/Forge downloads and launcher-media image URLs.

- [ ] **Disk space pre-check** (§7.1.2)
  - **Short:** Check available disk space before downloading a pack.
  - **Spec:** §7.1.2
  - **Acceptance:** Insufficient disk shows `ERR_DISKFULL` before any download starts.

- [ ] **Code signing** (§17 Phase 9)
  - **Short:** Windows code signing cert + macOS notarization.
  - **Spec:** §17
  - **Acceptance:** Signed binary doesn't trigger SmartScreen/Gatekeeper warnings.

- [ ] **Auto-update** (§17 Phase 9)
  - **Short:** Tauri built-in updater for seamless app updates.
  - **Spec:** §17
  - **Acceptance:** New release auto-downloads and installs on next launch.

- [ ] **Telemetry opt-in flow** (§12)
  - **Short:** Clear opt-in prompt for anonymous crash telemetry; respects user choice.
  - **Spec:** §12
  - **Acceptance:** User is prompted once; saying no disables all telemetry.

- [ ] **Localization (i18n)** (§17 Phase 9)
  - **Short:** Extract all UI strings into a resource bundle; add language selector.
  - **Spec:** §17
  - **Acceptance:** App renders in at least one non-English language.

- [ ] **Automated test suite** (§18.1)
  - **Short:** Add unit tests, integration tests, and end-to-end tests.
  - **Detail:** Spec explicitly notes "No automated tests are defined." Add: (a) Rust unit tests for hash verification, profile mutation, pair normalization (2 tests exist), (b) Python tests for compiler validation, (c) Playwright or Cypress E2E for browse/launch flows.
  - **Spec:** §18.1
  - **Acceptance:** `cargo test` and `pytest` pass; E2E test creates an instance and launches.

---

## Quick Reference: Most Critical Next Steps

> Reconciled against `desktop/src-tauri/src` at `HEAD`. Rows marked open (no strikethrough) are the current top-priority targets; do **not** re-mark them done without verifying code exists.

| # | Task | Why it's blocking |
|---|------|-------------------|
| 1 | ~~registry.db download + Ed25519 verify~~ ✅ | App can't reach its primary data source without this |
| 2 | ~~Release-asset upload in CI~~ ✅ | No production pipeline for `registry.db` distribution |
| 3 | ~~Override sanitization engine~~ ✅ | Must exist before any pack-install feature lands |
| 4 | ~~OAuth + token storage~~ ✅ | Blocks all governance, voting, reviews, crash reporting |
| 5 | ~~Onboarding flow~~ ✅ | First-run 4-step flow (welcome → services → GitHub → registry) wired in `App.tsx` + `Onboarding.tsx` |
| 6 | [~] Mod detail page | Page delivered+wired (`ModDetail.tsx`); Install button is a stub. Blocked: `registry_items` pins only `sha256`+`source_identifier`, no `download_url` — install needs curated download URLs or Phase 3 live version resolution |
| 7 | ~~Crash diagnostics~~ ✅ | Phase 4 requirement for MVP |
| 8 | ~~NeoForge/Forge installer support~~ ✅ | `inject_loader` runs `java -jar <installer> --installClient` with SHA-256 verification; neoforge+forge entries in loader manifests |
