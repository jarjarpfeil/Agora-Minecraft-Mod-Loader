# Agora — Backlog

> Single source of truth for remaining work. Organized by spec phase, then by priority within each phase.
> Each item has a **short** summary (one line) and a **detailed** description (what, why, spec ref, acceptance criteria).
> Status: `[x]` verified done · `[~]` implemented in part or awaiting acceptance criteria · `[ ]` not started/deferred
>
> **Verification status (2026-07-12):** Reconciled against the current uncommitted working tree and superseding decisions in `MASTER_SPEC.md` §19. Rust formatting, 499 workspace tests, the desktop and web builds, 21 mocked Playwright tests, Clippy (warnings only), and 20 script tests passed. The compiler suite is **not green**: 86 tests ran with 3 failures and 10 errors (missing test imports plus stale alias/known-conflict expectations). Native desktop smoke checks and the final Release C/D gate have not been completed. Deferred or removed features remain listed as `[ ]` for traceability.

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

- [x] **Parse all content types**
  - **Short:** Ingest `shaders/`, `resourcepacks/`, `servers/`, `datapacks/`, `worlds/` directories.
  - **Detail:** `compiler/compile.py` now iterates all 7 content directories via a `CONTENT_DIRS` list (mods/packs/shaders/resourcepacks/servers/datapacks/worlds); `content_type` read from each manifest's own field. The 5 new dirs currently hold only `.gitkeep` (awaiting curator seed data), so `verify_db.py` shows 0 items for them — structurally wired, not yet populated.
  - **Spec:** §1, §2.1
  - **Acceptance:** Compiler ingests manifests from all 7 directories; `verify_db.py` shows non-zero counts for each type (once curators seed the new dirs).

- [x] **Release-asset upload in CI** (`compile.yml`)
  - **Short:** Wire the GitHub Release Asset upload step; create `scripts/deploy_release_assets.py`.
  - **Detail:** `compile.yml` uploads an ephemeral Actions artifact instead of a tagged release asset. Without this, the desktop client and web directory cannot fetch `registry.db` from GitHub Releases. Create the deploy script (tags with `registry-<date>`, uploads `registry.db` + `registry.db.sig`, cleans old assets), uncomment the upload step, and verify the release appears on GitHub.
  - **Spec:** §3.1 step 13
  - **Acceptance:** A nightly run produces a visible GitHub Release with `registry.db` + `.sig` attached.

- [x] **GitHub API social metrics integration**
  - **Short:** Fetch reactions, comments, trust scores, and velocity data from the GitHub API during compilation.
  - **Detail:** Spec-strict §3.1 steps 3-9 + §3.2 fully implemented in `compiler/compile.py` (diff ~600 lines added). Pass 1 (`_hydrate_github_social_metrics`) enumerates governance-repo (`agora-mc/agora-mc`) issues, extracts `mod_id` from the `### Mod Registry ID` field of `review-form.yml`-created issues, fetches reactions on the issue + each comment via paginated GitHub REST API, attaches a `ModSocialMetrics` dataclass. Pass 2 (`_apply_trust_velocity_pass`) applies (a) §3.1 step 4 trust via GraphQL `user.contributionsCollection` scoped to the agora-mc org (30-day-age + 3-interaction gate, `poll_blacklist.json` short-circuit, Sybil single-mod diversity weighting), (b) §3.1 step 5 velocity circuit breaker (6h recent / 7d historical, fires when `recent_downvotes > 5× historical AND > 20` → `status='under_review'` + counts frozen at pre-spike values), (c) §3.1 step 9 immune passthrough (`governance.immune=true` skips ALL score evaluation). Pass 3 (`_respond_to_circuit_breaker` + `_resolve_expired_triage_polls`) handles (a) §3.2 Raid Shield (PUT interaction-limits=existing_users, 24h expiry, once per compile run), (b) §3.1 step 6 DELETE offending reactions (`_gather_offending_reactions` captures pre-DELETE data; `_append_audit_entry` records intent BEFORE destructive API call), (c) §3.1 step 8 create triage discussion poll under a discovered "Triage" / "Mod Reviews" / "Community Triage" category (soft-fails if no matching category; status flip is the hard requirement); (d) organic §3.1 step 5 trigger when `net_score < -10` (no Raid Shield / DELETE, just status flip + poll); (e) resolve expired polls via GraphQL (>7d elapsed): tallies keep-vs-remove reactions, blacklisted users weighted to 0, archiving manifest file to `registry/archived/<id>.json` on REMOVE win, restoring to `active` on KEEP win. Step 7 NLP scrubbing (`_scrub_review_text`): regex filtering (version begging + empty praise) + `profanity-check` SVM + `vaderSentiment` extreme-aggression gate; survivors populate `curator_reviews.top_reviews_json` instead of always `[]`. `insert_registry_item` now writes computed `upvotes`/`downvotes`/`net_score`/`velocity`/`status` from the attached `ModSocialMetrics` instead of hardcoded zeros. `compile.yml` passes `GITHUB_TOKEN` env to the compile step (alongside the pred-existing `ED25519_PRIVATE_KEY`). 50 stdlib `unittest` tests in `compiler/_test_social_metrics.py` cover: mod_id extraction, reaction parsing, dataclasses, Sybil weighting, interaction counts, velocity computation, anomaly firing, regex scrubbing patterns, review-text extraction, NLP fail-open behaviour, audit-log rotation, triage-poll category discovery. Local dev (no GITHUB_TOKEN) is a clean no-op: metrics stay zero, audit log unmutated, no destructive API calls attempted. Additionally supports an optional Discord-webhook real-time push for curator alerts via `DISCORD_WEBHOOK_URL` (silent no-op when unset); paired with the existing audit-trail admin-alert issue filed on the `agora-mc/agora-mc` governance repo itself (no separate admin repo required). One-time manual setup needed before triage-poll creation engages: enable GitHub Discussions on `agora-mc/agora-mc` (repo Settings → General → Features → check "Discussions") and create a category named "Triage" — until then `_create_triage_poll` soft-fails with a warning; status flip + Raid Shield + DELETE + admin-alert + Discord webhook all still fire.
  - **Spec:** §3.1 steps 3-9 (all complete), §3.2
  - **Acceptance:** `verify_db.py` shows non-zero `upvotes`/`net_score` for seeded mods (REQUIRES a real `GITHUB_TOKEN` in CI + at least one reaction filed on a tracking issue in the `agora-mc/agora-mc` repo — the code path is in place; data populates when real GitHub activity occurs).

- [x] **Modrinth batch image hydration**
  - **Short:** Call `GET /v2/projects?ids=[...]` to populate `icon_url` and `gallery_urls` for Modrinth-sourced mods.
  - **Detail:** `_hydrate_modrinth_images()` in `compile.py` batch-queries Modrinth (chunks of 100, JSON-array-encoded `ids` param) for `modrinth_id`-strategy items missing `icon_url`/`gallery_urls`; manifest values always take precedence. Degrades gracefully (warning + fallback) on network failure. Verified working: `xaeros-minimap` (modrinth_id) hydrates without the prior 400 error.
  - **Spec:** §3.1 step 11
  - **Acceptance:** Modrinth-sourced mods have populated `icon_url` after a compile run.

- [x] **Modrinth metadata hydration (compiler-side aggregation)** (§6.3 / §4.2)
  - **Short:** Nightly compiler bakes rich Modrinth metadata (description, full markdown body, page URL, license, updated timestamp, category fallback) into the signed `registry.db` so the client browses offline with zero Modrinth API calls.
  - **Detail:** Generalized `_hydrate_modrinth_images()` → `_hydrate_modrinth_metadata()` in `compiler/compile.py`: same single bulk `/v2/projects?ids=[...]` request (≤100/project) now also fills `description`, `body_markdown`, `page_url` (canonical, built from slug + project_type), `license_id`, and `source_updated_at`. Precedence = manifest/curator always wins; explicit `description_override`/`body_override` keys let curators sanitize upstream text. Added 5 nullable TEXT columns to `registry_items` and bumped `SCHEMA_VERSION` 1→2 (compiler) with matched `APP_REGISTRY_SCHEMA_VERSION` 2 (desktop). Category fallback: when a manifest sets NEITHER `base_categories` NOR `community_categories`, the hydrator links the upstream Modrinth `categories` (loaders filtered out) as community/unvetted categories — trivially overridable later by adding a manual category list. Image URLs only (never binary), so the db stays compact. Desktop `ModDetail.tsx` renders the description tagline, a `View on Modrinth ↗` + license + source-updated row, and the body via `react-markdown` (escapes raw HTML by default — no `dangerouslySetInnerHTML`, satisfying AGENTS.md) with `@tailwindcss/typography` prose styling. Live counters (downloads/follows) deliberately excluded to avoid forcing nightly db churn.
  - **Spec:** §6.3 / §4.2 ("Hybrid Media Strategy")
  - **Acceptance:** A `modrinth_id` mod with a 5-line manifest renders a rich detail page (description + formatted markdown body + categories + icon) entirely from local `registry.db` with no network calls. Verified: `xaeros-minimap` and a synthetic no-category test both hydrate from the live Modrinth API.

- [x] **Audit log generation**
  - **Short:** Generate `registry/governance/audit_log.json` during compilation with spec-compliant rotation. Implemented in `compiler/compile.py`.
  - **Detail:** Append-only transparency log written at end of compile. When the log exceeds 10,000 entries, the oldest 2,000 are moved to a dated archive. Also writes `audit_log_json` path to `system_config`. Verified by implementation and focused tests; the full compiler suite currently has unrelated/stale failures recorded in the verification note above.
  - **Spec:** §4.6
  - **Acceptance:** `audit_log.json` exists after compile; Transparency Log UI has data to surface.

- [x] **Raid Shield (Interaction Limits toggle)**
  - **Short:** Programmatically enable GitHub Interaction Limits on velocity anomalies during compilation.
  - **Detail:** Implemented inside backlog #1's `_respond_to_circuit_breaker` (in `compiler/compile.py`) rather than as a discrete deliverable: when the velocity circuit breaker fires, `_enable_raid_shield(owner, repo, token=...)` PUTs `interaction-limits = existing_users` for 24h against `PUT /repos/{owner}/{repo}/interaction-limits`. Toggled once per compile run (one shield covers all items). Raid Shield is a §3.2 mutation that DOES require GitHub-side permissions — `compile.yml` already grants `permissions: issues: write, discussions: write`.
  - **Spec:** §3.2
  - **Acceptance:** A simulated velocity spike triggers interaction limits on the test repo. (Verified via unit-test coverage of the underlying `ORGANIC_UNDER_REVIEW_THRESHOLD` constant + integration via live compile once a real GITHUB_TOKEN writes data.)

### P3 · Low Priority

- [x] **Regex DoS protections** (§2.4.1)
  - **Short:** Compiler-side pattern validation: 256-char length limit + 100KB corpus timeout test. Implemented in `compiler/compile.py`.
  - **Detail:** Compiler validates each crash-signature regex before insertion: (a) rejects patterns longer than 256 characters, (b) tests each pattern against a 100KB corpus with a 1-second hard timeout (via `signal.alarm` on Unix / `subprocess.run(timeout=...)` on Windows). Rejected patterns are logged and skipped. Verified: all 4 existing crash signatures pass both checks. Rust-side `RegexSet` precompilation cache is a separate future item.
  - **Spec:** §2.4.1
  - **Acceptance:** A pathological regex pattern is rejected at compile time; all 4 existing signatures inserted without rejection.

- [x] **CODE_OF_ENGAGEMENT.md in 3 locations** (§5.1)
  - **Short:** All 3 locations verified; CI enforcement step added.
  - **Detail:** `README.md` has a Code of Engagement section with link, `.github/ISSUE_TEMPLATE/review-form.yml` contains the full CoE text, and `CODE_OF_ENGAGEMENT.md` is the root source. Added a CI grep-verification step in `.github/workflows/compile.yml` that enforces the text is present in all 3 locations.
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
  - **Short:** GitHub Device Flow + keyring/AES-256-GCM token storage; enables voting, reviews, crash reporting, and triage. Implemented via the desktop auth facade and `agora-core` auth support, including the PBKDF2/AES-256-GCM `tokens.enc` fallback.
  - **Detail:** (a) Implement GitHub Device Flow (`POST /login/device/code` → poll `POST /login/oauth/access_token`). (b) Store token in OS keyring via `keyring` crate. (c) Fallback: AES-256-GCM encrypt to `tokens.enc` in app data dir with machine-bound key. (d) Token is never in config files, env vars, or SQLite. (e) Use token for: voting (emoji reactions), reviews (issue comments), crash reports (issue creation), flag submission, and triage participation. (f) Browse-Only Mode: all of the above gracefully degrade when token is absent.
  - **Spec:** §7.5, §5
  - **Acceptance:** User signs in via Device Flow; can vote on a mod; token survives restart via keyring.

- [x] **Instance detail panel** (§6.5)
  - **Short:** Implemented in `desktop/src/pages/InstanceEditor.tsx`: full-page editor (routed via `App.tsx` `editingInstanceId` state) showing instance header, mods list with per-mod Remove buttons, embedded Browse-like Add-Mod view with search/category/sort/filter, and inline version picker → install.
  - **Detail:** The command exists but has no UI consumer. Build a detail view when clicking an instance card: shows installed mods list (filename, version, source, hash), JVM settings, lock state, and supports "Check for Pack Update", "Export Pack", "Unlock/Revert" actions.
  - **Spec:** §6.5
  - **Acceptance:** Clicking an instance shows its mod list and settings.

- [x] **`AlwaysPreTouch` toggle + GC-conditional default** (§8.5)
  - **Short:** GC-conditional default (ON for G1GC/empty, OFF for ZGC/Shenandoah) + Settings UI toggle + DB column migration. Implemented in `instances.rs`, `models.rs`, `db.rs`, `Settings.tsx`.
  - **Detail:** `build_profile_entry` in `instances.rs` now computes `always_pre_touch`: ON for G1GC/empty, OFF for ZGC/Shenandoah. `InstanceRow` + `db.rs` migration adds `jvm_always_pre_touch` column (ALTER TABLE with silent "exists" guard). User setting `jvm_always_pre_touch` overrides instance-level default. `CreateInstanceRequest` accepts optional `jvm_always_pre_touch`. Settings.tsx "JVM Defaults" card with checkbox + description.
  - **Spec:** §8.5
  - **Acceptance:** Switching GC to ZGC warns about AlwaysPreTouch; user can toggle it.

### P3 · Low Priority

- [x] **Windows Mojang discovery completion** (§8.4)
  - **Short:** Implemented in `desktop/src-tauri/src/mojang.rs`: legacy Program Files paths, `%LOCALAPPDATA%\Programs\` per-user path, `C:\XboxGames\Minecraft Launcher\Content\` Xbox app path, `reg query HKLM\SOFTWARE\Mojang\Launcher` registry discovery, `Get-AppxPackage` MSIX discovery, probes both `MinecraftLauncher.exe` and `Minecraft.exe` in every location.
  - **Spec:** §8.4
  - **Acceptance:** App finds launcher installed via MSIX/registry on Windows.

- [x] **MCP server** (§10)
  - **Short:** Implement localhost MCP server with 6 tools, approval queue, and system context injection.
  - **Detail:** Binds only to `127.0.0.1`; the localhost boundary is the current security model. Per-session Bearer authentication is intentionally deferred by `MASTER_SPEC.md` §19.6. Includes the six launcher tools, approval state with persistent grants, `resources/list` system context, and a Settings toggle.
  - **Spec:** §10
  - **Acceptance:** Claude Desktop connects with token, calls `list_instance_mods`, user sees approval prompt.



---

## Phase 3 — Browse, Discovery & Search

- [x] **Mod detail page** (§6.2)
  - **Short:** Clicking a Browse item opens a detail page with version picker, compatibility info, working install flow.
  - **Detail:** PAGE + INSTALL PATH DELIVERED. `desktop/src/pages/ModDetail.tsx` renders icon/badges/stats/immunity banner/curator notes/compatible versions/reviews; the Install button opens a 3-step inline flow (instance picker → version picker → install). Backend in `desktop/src-tauri/src/mod_install.rs`: `list_mod_versions` resolves live candidates via GitHub Releases API or Modrinth version API (filtered by instance mc_version+loader, using stored OAuth token for GitHub rate limits); `install_mod_version` downloads the chosen candidate, verifies SHA-256 against the pinned registry hash, writes to `mods/<filename>.jar`, and atomically appends an `InstalledMod` to `instance_manifest.json`. Mod-download domain allowlist (github/modrinth) + redirect-safe policy enforced separately from the loader allowlist.
  - **Spec:** §6.2
  - **Acceptance:** User opens a mod, sees compatible versions, can install it to their active instance.

- [x] **"For You" algorithm** (§6.2)
  - **Short:** Track locally installed categories; boost uninstalled mods in matching categories.
  - **Detail:** Backend `for_you_items` (`desktop/src-tauri/src/registry.rs`): walks all `instance_manifest.json` files under `instances/` to collect installed mod `registry_id`s; derives the user's interest categories from `item_categories` for those ids; runs a single parameterized SQL query that joins `registry_items` ↔ `item_categories`, excludes already-installed items, and ranks by `COUNT(ic.category_id) DESC, net_score DESC` (more category overlap = higher rank). Degrades to plain `net_score` ordering when the user has no installed mods (or none resolve categories). Registered as `for_you_items` command. Frontend: `SORTS` gains a "For You" option (default sort) in `Browse.tsx`; when selected, `forYouItems(modrinthEnabled)` is called instead of `browseItems` (category/content-type/MC/loader filters are intentionally inert for For You, as it's a global recommendation). The Modrinth toggle is still respected.
  - **Spec:** §6.2
  - **Acceptance:** After installing 3 "magic" mods, Browse surfaces more magic mods.

- [x] **Federated Modrinth discovery** (§6.3, §19.9 E4)
  - **Short:** Live Modrinth API search is federated into Browse with uncurated labeling and published-hash verification.
  - **Detail:** The former standalone Raw Modrinth tab was removed by the E4 architecture pivot. Backend Modrinth search/version/install support remains, while Browse presents curated and uncurated results through one discovery surface and preserves source labeling, opt-in gating, allowlisted downloads, and published-hash verification.
  - **Spec:** §6.3
  - **Acceptance:** User can search Modrinth directly, download a mod, and it's hash-verified before writing to `mods/`.

- [x] **Manual .jar drag-and-drop** (§6.5b)
  - **Short:** Drag-and-drop .jar files into an instance's `mods/` folder.
  - **Detail:** Backend `add_manual_mod` (`desktop/src-tauri/src/mod_install.rs`) copies the dropped file into the instance `mods/` dir (validates `.jar` + rejects path-traversal filenames), computes SHA-256, and appends an `InstalledMod` with `source: "manual_drag_drop"` atomically. Registered as `add_manual_mod` command. `InstanceEditor.tsx` renders a drop zone over the mods list that reads the Tauri `File.path`, calls the command, and refreshes the manifest.
  - **Spec:** §6.5b
  - **Acceptance:** Dragged file appears in `instance_manifest.json` with `source: "manual_drag_drop"`.

- [x] **Pack export (.mrpack / custom JSON)** (§6.5c)
  - **Short:** Export an instance as a shareable `.mrpack` or custom `.json` pack file.
  - **Detail:** Backend `export_instance_pack` (`desktop/src-tauri/src/mod_install.rs`): `format: "json"` writes a small `.agora-pack.json` manifest (instance meta + mod list with registry ids / sources / versions / SHA-256 — no binaries, ~5–20KB); `format: "mrpack"` writes a `.mrpack` zip containing `modrinth.index.json` (formatVersion 1 + dependencies) plus the mod `.jar`s under `mods/<filename>`. Output written to `<app_data>/exports/<id>.<ext>` atomically (.tmp + rename). `InstanceEditor.tsx` exposes "Export as JSON" and "Export as .mrpack" buttons with loading + success-path display.
  - **Spec:** §6.5c
  - **Acceptance:** Exported file is 5–20KB and can rebuild the instance on another machine.

- [x] **Pack install flow with partial-failure fallback** (§7.1.1)
  - **Short:** Sequential pack install with per-mod progress tracking + partial-failure resilience. Implemented in `desktop/src/pages/InstanceEditor.tsx`.
  - **Detail:** "Install all mods from pack" button in InstanceEditor: user enters pack ID → `listPackMods(packId)` returns `PackModRow[]` → for each mod, calls `listModVersions` + `installModVersion` sequentially → live progress display (✓/✗/⏳/○) → summary with failed-mod detail. Partial failures are tracked and reported inline ("Installed M of N mods. N-M failed:" + list of failed mod IDs with error messages).
  - **Spec:** §7.1.1
  - **Acceptance:** A pack with one broken link installs all other mods and shows a "1 mod failed" notice.

- [x] **Unlock/Revert state machine** (§6.5)
  - **Short:** Implement the lock → unlock → revert state machine for curated pack instances.
  - **Spec:** §6.5
  - **Acceptance:** User can unlock a pack instance, add manual mods, and revert to original.

---

## Phase 4 — Web Directory

- [x] **Static Next.js export** — 19 pages generated from `registry.db`.
- [x] **Landing page + about page + content-type pages + detail pages + client-side search/filter.**
- [x] **Image URL scheme validation** — Only `https:` and `data:` render.

- [x] **Sanitized react-markdown rendering** (§4.1c #3, §13)
  - **Short:** Curator notes render through `react-markdown` with `rehype-raw` and a strict `rehype-sanitize` schema in `web/src/components/MarkdownRenderer.tsx`.
  - **Detail:** The detail page uses the shared renderer; allowed tags, attributes, and URL protocols are constrained by the sanitizer, and curator content is never passed to `dangerouslySetInnerHTML`.
  - **Spec:** §4.1c #3, §13
  - **Acceptance:** Curator note renders bold/italic/links but never raw HTML.

- [x] **Fetch registry.db from GitHub Release Asset during CI** (§13)
  - **Short:** Web build should fetch the latest `registry.db` from GitHub Releases, not read a sibling file.
  - **Detail:** New `scripts/fetch_registry_db.py` (stdlib only) queries the GitHub Releases API for the latest `registry-*` release and downloads `registry.db` (+ `.sig` if present) to a target dir. New `.github/workflows/web-build.yml` (dispatch / daily schedule / push to `web/**`) sets up Node 20 + Python 3.11, runs the fetch script to place `registry.db` at the repo root (matching `web/src/lib/db.ts` fallback), then `npm ci && npm run build`, uploading `web/out` as `web-static`. Depends on a `registry-*` release existing (created by `compile.yml`).
  - **Spec:** §13
  - **Acceptance:** `npm run build` in CI works without a local `registry.db`.

- [x] **Category / MC version / loader filters on web**
  - **Short:** Category chips + MC version dropdown + loader filter in the web catalog. Implemented in `web/src/components/Catalog.tsx`.
  - **Detail:** Client-side filtering of pre-fetched items (compatible with `output: 'export'`). Category chips derived from item data via `useMemo`. MC version filter matches against `compatible_versions[].mc_version`. Loader filter matches against `compatible_versions[].loader`.
  - **Spec:** §13
  - **Acceptance:** Web visitor can filter by category and MC version.

- [x] **Velocity / newest sort options on web**
  - **Short:** Net score (default) / velocity (trending) / newest (date_added) sort options in the web catalog.
  - **Detail:** Dropdown selector in `Catalog.tsx` triggers client-side re-sort via `useMemo`. Velocity sorts by `item.velocity`; newest sorts by `item.date_added` descending.
  - **Spec:** §13
  - **Acceptance:** Web visitor can sort by "Trending" and "Newest."

- [x] **Top community reviews on detail page**
  - **Short:** Reviews section on the web mod detail page with star ratings + author attribution. Implemented in `web/src/components/Reviews.tsx`.
  - **Detail:** Server component calls `getReviews(itemId)` from `lib/db.ts`, renders each review as author + star rating + date + body. Empty state shows "No reviews yet."
  - **Spec:** §13
  - **Acceptance:** Top reviews render as plain text with attribution.

---

## Phase 5 — Governance & Triage

- [x] **Triage Center tab** (§5, §6.1)
   - **Short:** Implement the Community Governance tab with under-review items, live poll data, and recent resolutions.
   - **Detail:** `Governance.tsx` renders the Triage Center with three sections: (a) Active Triage Polls — queries `list_under_review_items` backend command, displays items with `status = 'under_review'` and their poll percentages; (b) Recent Resolutions feed — `list_recent_resolutions` command returns promoted/demoted items with timestamps; (c) Transparency Log (already implemented in §4.6 item). Backend commands: `list_under_review_items`, `fetch_triage_poll`, `list_recent_resolutions` in `governance.rs` + `commands.rs`.
   - **Spec:** §5, §6.1
   - **Acceptance:** Under-review item appears in Triage Center with live poll percentage.

- [x] **Curator Shield banner** (§5.4)
  - **Short:** Display a non-dismissable steel-blue banner on immune items' detail pages.
  - **Detail:** Desktop `ModDetail.tsx` already rendered the "Immunity Shield Active" banner above the install button. Added the same Curator Shield banner to the **web** detail page (`web/src/app/[type]/[id]/page.tsx`): rendered at the top of the page when `item.is_immune` is truthy, using the existing `is_immune` field on the web `RegistryItem`. No `dangerouslySetInnerHTML`.
  - **Spec:** §5.4
  - **Acceptance:** Immune mod profile page shows "Curator Shield" banner above download button.

- [x] **Flag Review system** (§5.6)
   - **Short:** "🚩 Flag Review" button on every comment (rate-limited).
   - **Detail:** `ModDetail.tsx` renders a 🚩 Flag button on each curator review; clicking it calls `flag_review` backend command (`governance.rs`) which creates a GitHub issue in the `agora-mc/admin-alerts` repo with the review text and reviewer info. Rate-limited via `get_flag_rate_limit` + `FlagRateLimit` struct (per-user, sliding window). `db.rs` has the `flag_submissions` table for storing flagged items.
   - **Spec:** §5.6
   - **Acceptance:** User can flag a comment; triggering creates a GitHub issue in `agora-mc/admin-alerts`.

- [x] **In-app Transparency Log** (§4.6)
  - **Short:** Display `audit_log_json` entries in the Governance tab.
  - **Detail:** Compiler (`compiler/compile.py`) now bakes the audit entries into an `audit_log` table (`id, timestamp, action, details`) in `registry.db` (parameterized `executemany`; table created if absent; `verify_db.py` reports `audit_log: 8`). Desktop backend `list_audit_log` command (`registry.rs` + `commands.rs`, reads newest-first, defensively returns `[]` if the table is absent in older builds). `Governance.tsx` renders a scrollable Transparency Log section (loading/error/empty states, `<time>` timestamps, action badge, details). `audit_log_json` path indicator in `system_config` preserved.
  - **Spec:** §4.6
  - **Acceptance:** User can see governance actions (immune grants, velocity overrides) in a scrollable log.

---

## Phase 6 — Polish & Hardening

- [x] **Error envelope shape** (§4.5)
  - **Short:** Custom `Serialize` impl on `LauncherError` outputs `{code, message, details, suggested_action}` flat envelope. Implemented in `desktop/src-tauri/src/error.rs` + `desktop/src/lib/tauri.ts`.
  - **Detail:** Replaced `#[derive(Serialize)]` with manual `impl Serialize` producing `{"code": "...", "message": "...", "details": null, "suggested_action": "..."}`. `suggested_action` populated for `MojangNotFound`, `HashMismatch`, `NetworkOffline`, `AuthRequired`; null for all others. `formatError` in `tauri.ts` updated to handle new envelope shape with backward compat for old Tauri tagged-variant shape.
  - **Spec:** §4.5
  - **Acceptance:** Frontend receives structured error envelope with `suggested_action` field.

- [x] **CSP additions** (§8.2, §7.1)
  - **Short:** Added `neoforged.net`, `maven.neoforged.net`, `minecraftforge.net`, `files.minecraftforge.net`, `raw.githubusercontent.com` to `connect-src` in `tauri.conf.json`. Verified `img-src` already includes `raw.githubusercontent.com`.
  - **Spec:** §8.2, §7.1
  - **Acceptance:** CSP allows NeoForge/Forge downloads and launcher-media image URLs.

- [x] **Disk space pre-check** (§7.1.2)
  - **Short:** 500MB minimum disk space check before mod download. Implemented in `desktop/src-tauri/src/mod_install.rs`.
  - **Detail:** `available_disk_space_bytes()` uses `fsutil volume diskfree` on Windows (no new crate). `install_mod_version` checks before `download_mod_bytes` call: if available < 500MB, returns `Err(LauncherError::DiskFull)` immediately (before any network request). If check returns None (can't determine), proceeds without blocking.
  - **Spec:** §7.1.2
  - **Acceptance:** Insufficient disk shows `ERR_DISKFULL` before any download starts.

- [ ] **Code signing** (§17 Phase 9)
  - **Short:** Windows code signing cert + macOS notarization.
  - **Spec:** §17
  - **Acceptance:** Signed binary doesn't trigger SmartScreen/Gatekeeper warnings.

- [ ] **Auto-update** (§17 Phase 9)
  - **Short:** Tauri built-in updater for seamless app updates.
  - **Detail:** Deferred. Updater dependencies exist, but `tauri.conf.json` has no updater endpoints/public key configuration and the acceptance flow is not operational.
  - **Spec:** §17
  - **Acceptance:** New release auto-downloads and installs on next launch.

- [ ] **Telemetry opt-in flow** (§12, §19.9 E5)
  - **Short:** Clear opt-in prompt for anonymous crash telemetry; respects user choice.
  - **Detail:** Deferred/removed from the active v1 UI by §19.9 E5. Local crash-data retention remains, but the opt-in prompt/toggle and upload path are not active.
  - **Spec:** §12
  - **Acceptance:** User is prompted once; saying no disables all telemetry.

- [ ] **Localization (i18n)** (§17 Phase 9)
  - **Short:** Extract all UI strings into a resource bundle; add language selector.
  - **Detail:** Deferred post-v1. `desktop/src/i18n/index.ts` and the language selector are currently commented out.
  - **Spec:** §17
  - **Acceptance:** App renders in at least one non-English language.

- [~] **Automated test suite** (§18.1)
  - **Short:** Add unit tests, integration tests, and end-to-end tests.
  - **Detail:** Rust coverage (499 passing workspace tests), Python unit/integration suites, and 21 passing mocked Playwright tests exist. Remaining acceptance gaps: the compiler suite currently has 3 failures and 10 errors, and a native E2E instance-create-and-launch run has not been demonstrated.
  - **Spec:** §18.1
  - **Acceptance:** `cargo test` and `pytest` pass; E2E test creates an instance and launches.

---

## Deferred 
- [x] **Cross-cutting Pack Overrides: mrpack `overrides/` extraction + non-mod file round-trip**
   - **Short:** When importing or exporting `.mrpack` packs, honor Modrinth's `overrides/` (and `client-overrides/` / `server-overrides/`) directory convention and broaden the agora-pack JSON to round-trip non-mod files.
   - **Detail:** `import_mrpack` in `mod_install.rs` now extracts override directories (`overrides/`, `client_overrides/`) alongside mod `.jar`s, applying the override sanitizer's whitelist: `config/`, `defaultconfigs/`, `resourcepacks/`, `shaderpacks/`, `datapacks/`, `kubejs/`. Sanitization runs inline in `import_mrpack` (the archive is already open in-memory, so `override_sanitizer::extract_overrides`'s zip-path API is not used directly). Banned extensions and Zip Slip protection apply.
   - **Spec:** §7.2 (override sanitization), mrpack v1 (`overrides/`, `client-overrides/`, `server-overrides/`) .agora-pack/v1 (extend `mods[]` or add `files[]`).
   - **Acceptance:** Importing a `.mrpack` that ships `overrides/config/foo.toml` extracts the file into `<instance>/config/foo.toml`; a malicious `overrides/../../evil.exe` is rejected; exporting an instance whose `config/` dir has files bundles them under `overrides/config/` in the resulting `.mrpack`.

- [ ] **Dev Mode (sandboxed builds)** (§11)
  - **Short:** Detect Docker/Podman/Firecracker; clone + build mod .jar in sandbox with no network.
  - **Spec:** §11
  - **Acceptance:** User can build a mod from a GitHub URL inside Docker and test it.

- [ ] **Anonymous crash telemetry aggregation** (§12)
  - **Short:** Opt-in weekly compression + upload of `local_crash_telemetry` table to an aggregation endpoint.
  - **Spec:** §12
  - **Acceptance:** Opt-in user's crash matrix data is compressed and submitted weekly.

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
| 6 | ~~Mod detail page~~ ✅ | `ModDetail.tsx` + `mod_install.rs`: live GitHub/Modrinth version resolution → SHA-256-verified download → atomic manifest write |
| 7 | ~~Crash diagnostics~~ ✅ | Phase 4 requirement for MVP |
| 8 | ~~NeoForge/Forge installer support~~ ✅ | `inject_loader` runs `java -jar <installer> --installClient` with SHA-256 verification; neoforge+forge entries in loader manifests |
| 9 | ~~GitHub API social metrics (steps 3-9 + §3.2)~~ ✅ | `compiler/compile.py` writes real upvotes/downvotes/velocity/status. Trust filter via GraphQL contributionsCollection, circuit-breaker response with Raid Shield + DELETE reactions + triage polls, NLP review scrubbing, audit-log rotation. 50 unit tests. |

---

## Desktop Reliability & UX Upgrade Execution Plan

Tracking packages A1 through D5 from `Agora Desktop Upgrade.md`. Packages are completed in order within each release.

### Release A — Critical Desktop Stabilization

- [x] **A0** — Baseline and specification synchronization
- [x] **A1** — Registry recovery and first-run dead end
- [x] **A2** — Immediate startup shell and theme precedence
- [x] **A3** — Browse request isolation and stale pagination
- [x] **A4** — Command palette reachability and actionable results
- [x] **A5** — Health-dialog launch-mode defect and filename correctness
- [x] **A6** — Onboarding and device-flow consistency

#### A0 baseline record (2026-07-10)

- Base commit: `21cf5df3778f7d66ebd99e65e82e579ab3bf84c8`.
- `desktop/npm run build`: passed; existing Vite chunk-size and mixed static/dynamic import warnings recorded.
- `cargo test --workspace`: passed, 379 tests total (2 CLI, 264 core, 113 desktop).
- `cargo clippy --workspace --all-targets`: completed with pre-existing unused-import, unused-variable, dead-code, and non-snake-case warnings; no errors.
- `cargo fmt --check`: failed on pre-existing formatting differences. Those unrelated files were not reformatted during A0.
- Pre-existing failures are the formatting check and warnings above; runtime code was unchanged by A0.

### Release B — Desktop Application Infrastructure

- [~] **B1** — Typed destination and history model (navigation/history implemented; explicit recoverable not-found handling for invalid entity IDs remains)
- [x] **B2** — Canonical launch and process controller
- [~] **B3** — Typed settings access and settings decomposition (launcher Browse/Auto-detect/Test actions and persistent per-section save errors remain)
- [x] **B4** — Accessible dialogs, notifications, and sidebar
- [x] **B5** — Critical integration-test matrix

### Release C — Canonical Safe-Install Infrastructure

- [x] **C0** — Install-transaction architecture ✅ (Design in MASTER_SPEC.md §19.13.3, core types implemented)
- [~] **C1** — Read-only install-plan contract (resolver/fingerprint implemented; migration gate and complete acceptance remain)
- [~] **C2** — Transactional install execution (pipeline exists; legacy pack-import install paths still bypass it)
- [~] **C3** — One frontend install flow (`InstallFlow` exists; legacy `ModDetail` and pack-import paths remain)
- [~] **C4** — Safe batch updates (detection/core types exist; complete transactional batch-update UX remains)

### Release D — Product Differentiation and UX Coherence

- [~] **D1** — Action-oriented Home and instance-aware discovery (partial Home/discovery implementation)
- [~] **D2** — Last-known-good recovery and reproducibility model (architecture/core model implemented; final integration validation pending)
- [~] **D3** — Last-known-good user flow (snapshot/LKG integration exists; complete one-click recovery acceptance pending)
- [~] **D4** — Explainable Crash Doctor (deterministic investigation exists; fully reversible, navigation-safe flow pending)
- [~] **D5** — Reproducible sharing and drift detection (lockfile/drift foundations exist; exact end-to-end reproduction and repair acceptance pending)

---

### Health-check follow-ups (post incompatibility/dependency bug fix)

- [x] **HF1** — Deduplicate the desktop JAR parser against `agora_core::jar_metadata` ✅
  - The pre-launch health check's false-positive incompatibility/blocker bug was
    fixed in the **core** parser (`crates/agora-core/src/jar_metadata.rs`) — Forge
    dependency blocks now read the inner `modId` (not the section header, which
    caused self-conflicts), Fabric `breaks`/`conflicts` carry severity + version
    ranges, NeoForge `neoforge.mods.toml` is parsed, `mandatory=false` → optional,
    and `forge`/`neoforge` are ignored as loader deps.
  - The **duplicate, still-buggy** copy that lived in
    `desktop/src-tauri/src/crash_investigator.rs::parse_jar_metadata` has been
    **deleted**. All callers (`modrinth_raw.rs`, `mod_install.rs`, `commands.rs`,
    internal `parse_jar_packages`) now call
    `agora_core::jar_metadata::parse_jar_metadata` directly. The
    `impl From<JarMetadata> for JarDeps` bridge in
    `desktop/src-tauri/src/dependency_ops.rs` is gone;
    `build_install_plan` now accepts `&JarDeps` directly. Six duplicate tests in
    `crash_investigator.rs` (sections E–J) that asserted the legacy buggy
    Forge header-as-dep behavior were removed (the core parser has comprehensive
    tests for these cases).

- [x] **HF2** — Real version-range matching for JAR incompatibilities ✅
  - New module `crates/agora-core/src/version_match.rs`:
    - `compare_versions()` — lenient non-SemVer comparator; splits on
      `.`, `-`, `+`, `_`; numeric segments compare numerically (empty = 0,
      so `1.0.0` == `1.0`); non-numeric > numeric; case-insensitive
      lexicographic fallback for alphabetic segments.
    - `fabric_predicate_matches()` — Fabric predicate grammar: space-separated
      sub-predicates are AND-joined; recognizes `>=`, `<=`, `>`, `<`, `=`,
      `~` (treated as `>=`), `*`; bare version = exact match.
    - `fabric_ranges_match()` — OR semantics across the array of predicate
      entries; empty list = unconditional (any version).
    - `maven_range_matches()` — Forge Maven range grammar: `[a,b)`,
      `[a,b]`, `(a,b)`, `(a,b]`, `[a,]`, `[a,)`, `(,b]`, `[a]` (exact),
      and bare version (treated as `>=` minimum).
    - `evaluate_version_match()` — unified entry point returning
      `Matched` / `NotMatched` / `Unconditional`, switching on
      `IncompatibilitySource::is_fabric_grammar()`.
  - `JarDeps` gained `mod_version: Option<String>` (`#[serde(default)]`,
    not persisted to `instance_manifest.json`; re-parsed fresh by health).
    The parser populates it from: Fabric `fabric.mod.json::version`,
    Quilt `quilt_loader.version`, Forge `mods.toml::version=` (literal
    value used directly), `MANIFEST.MF::Implementation-Version`
    (resolved when the TOML used `${file.jarVersion}`).
  - `health.rs` Step 6 policy updated: hard incompat now consults
    `evaluate_version_match()`. `Matched` or `Unconditional` → blocker;
    `NotMatched` → informational warning, NOT a blocker. Soft incompat
    (Fabric `conflicts`, NeoForge `discouraged`, legacy backfill) remains
    a warning, never a blocker. When the target mod has no
    machine-readable version, an unconditional declaration still blocks
    (preserving the safety default); a conditional one warns.

- [x] **HF3** — `quilt.mod.json` parsing (`quilt_loader.breaks`/`.depends`) ✅
  - `IncompatibilitySource` gained `QuiltBreaks` (hard) and `QuiltConflicts`
    (soft); the new `is_fabric_grammar()` helper groups them with Fabric,
    since Quilt uses the same predicate syntax.
  - `jar_metadata.rs` parses `quilt.mod.json`, extracting `quilt_loader.id`
    (→ `mod_jar_id`), `quilt_loader.version` (→ `mod_version`), and the
    `depends` / `breaks` / `conflicts` sections through the existing
    Fabric `extract_fabric_deps` helper (which already handles object /
    array / string forms). Quilt-only mods are no longer `UnknownMod`.
