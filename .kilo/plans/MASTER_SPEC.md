# MASTER_SPEC.md
# The Premium Curated Minecraft Mod Launcher — Complete Engineering Blueprint

> **This document is the single source of truth for the entire project. It is intended to be fed directly to an AI coding agent as unambiguous implementation context. Every architectural decision made across the full design session is captured here.**

> **IMPLEMENTATION STATUS (last consolidated 2026-07-05):** The original design captured below is preserved verbatim for its decision-rationale value. Several architectural pivots have since landed in code and are documented in **§19 — Architectural Evolution & Implementation Status** (appended at the end). Where §19 conflicts with the original prose in §0–§18, **§19 wins** for the purposes of new work. The pivots in brief:
> - **Pivot (E9):** The launcher now optionally performs **in-process Microsoft Account (MSA) authentication and direct JVM execution** (crates/agora-core/src/msa.rs, launch.rs). This is a deliberate expansion of the original *security by delegation* constraint in §0, motivated by the v1 launcher refactor — it enables in-launcher features (account sign-in, version manifest fetching) that the Mojang-launcher-delegation model cannot support. The Mojang-launcher-delegation path in §8 remains as a fallback; MSA + direct launch is the new primary path.
> - **Workspace restructure (D12/E13):** Business logic is migrating into a shared Rust crate crates/agora-core/ consumed by both the Tauri GUI (desktop/src-tauri/, package name **gora-desktop**) and a standalone CLI (crates/agora/, binary gora). See §1.1 and §19 for migration status.
> - **Modrinth unified into Browse (E4/D1):** The separate *Raw Modrinth tab / page* (originally described in §6.3) was removed — Modrinth search results are now merged directly into the Browse grid. The desktop/src/pages/ModrinthRaw.tsx file has been deleted.
> - **Telemetry removed (E5/§12):** The opt-in crash telemetry (§12) had no aggregation endpoint and was never wired to a real upload path. The opt-in prompt UI and the crash_telemetry_opt_in setting have been deleted. The spec text in §12 is kept for historical reference only.
> - **MCP server audit (A2/E2/E3):** The MCP server in desktop/src-tauri/src/mcp.rs is bound to 127.0.0.1:39741. Per the project pivot the per-session Bearer token from §10.0 #2 is **not yet implemented** (the localhost binding is the current security boundary). The tool set already implemented is the extended set (6 tools: list_instances, list_instance_mods, disable_mod, search_crash_signatures, suggest_mod_incompatibility, get_system_context) — the §10.1 set (
ead_latest_crash, 
ead_mod_manifest, enable_mod, search_knowledge_base) is planned to be added as a superset per user decision.
> - **Old plan files removed:** .kilo/plans/1782081355093-crash-investigator-plan.md, 1782611768583-agora-v1-launcher-refactor.md, and dependency-aware-mod-ops-plan.md have been deleted; their key decisions are folded into §19.

> **The original §0–§18 design spec follows below. For current code status, jump to §19.**

---


---

## Table of Contents

| § | Title | Purpose |
|---|---|---|
| 0 | Project Ethos & Philosophy | Mission, constraints, tech stack |
| 1 | Repository Structure | Flat-file "database" in GitHub |
| 2 | Source JSON Schemas | Mod, pack, crash signature, version_info manifests |
| 3 | The Nightly Compiler | GitHub Action that builds `registry.db` |
| 4 | Client-Side SQLite Schema | `registry.db` + `local_state.db`, versioning, security, errors |
| 5 | Community Governance | Code of Engagement, triage state machine, immune override |
| 6 | The Tauri Desktop App | Onboarding, browse, instances, packs, integration config |
| 7 | Modpack Architecture | Three-tier packs, override sanitization, OAuth token security |
| 8 | The Execution Engine | Mojang launcher wrapper, modloader injection, JVM args |
| 9 | Crash Diagnostics | Pre-launch interceptor, regex triage, GitHub issue search |
| 10 | Local AI / MCP Server | Tool definitions, approval state, system context injection |
| 11 | Dev Mode | Curator tool with sandboxed builds |
| 12 | Anonymous Crash Telemetry | Opt-in crash matrix |
| 13 | Web Directory | Static Next.js site |
| 14 | Build Execution Pipeline | AI agent module prompts (1–7) |
| 15 | Security Architecture | Threat model, security principles |
| 16 | Technical Decisions Log | All architectural decisions with rationale |
| 17 | Implementation Order & MVP Scope | Phased build guide |
| 18 | Known Limitations & Open Questions | Honest disclosure of gaps |

---

## 0. PROJECT ETHOS & PHILOSOPHY

### The "Agora" Mission
This is not a warehouse. This is a boutique.

The project is a decentralized, ad-free, open-source Minecraft mod launcher and discovery platform designed to capitalize on community pushback against corporate consolidation (e.g., Spark Universe acquiring Modrinth). The explicit goal is to return platform control to the community while bypassing all centralized commercial infrastructure entirely.

If CurseForge were a beer, this would be Agora.

### Core Constraints (Non-Negotiable)
- **$0.00/Month Server Footprint.** No backend servers. No proprietary databases. No hosted APIs. All infrastructure is offloaded to GitHub, GitHub Release Assets, and the official Mojang launcher.
- **Security by Delegation.** The application never touches Microsoft/Xbox authentication or JVM execution. These are fully delegated to the official Mojang launcher.
- **Decentralized by Design.** All social data (votes, reviews, governance) lives as structured GitHub interactions. All application data is compiled into a static file served via GitHub Release Assets.
- **AI Agent-Friendly.** The codebase must be broken into isolated, deterministic modules that AI tools can write, test, and debug independently.
- **Client-Side Scalability.** All GitHub API calls are made using the user's personal OAuth token, giving each user 5,000 requests/hour — meaning the platform scales infinitely at zero cost.
- **Modrinth Independence.** The primary download strategy is `github_release` — mods are sourced directly from developer GitHub repositories. `modrinth_id` is a supplementary fallback for mods that are not yet self-hosting. Users can disable all Modrinth API integration entirely and still use the full curated catalog, discovery, and instance management features.

### Tech Stack
| Layer | Technology |
|---|---|
| Desktop App Backend | Tauri (Rust) |
| Desktop App Frontend | React + Tailwind CSS |
| Web Directory | Next.js (static, hosted on Vercel or GitHub Pages) |
| Client-Side Database | SQLite (via `tauri-plugin-sql`): `registry.db` (read-only global state) + `local_state.db` (read-write user/instance state) |
| Data Compiler | Python (GitHub Actions, free runners) |
| Game Execution | Official Mojang Launcher (wrapper/delegation) |
| AI Integration | Local MCP Server (user-provided API keys or local Ollama) |
| Data Hosting | GitHub Release Assets (for `registry.db` + `registry.db.sig`) |

---

## 1. REPOSITORY STRUCTURE (THE GITHUB DATA ENGINE)

The central GitHub repository is the "database." All data is flat files committed via Pull Request.

```
/registry/
  mods/
    sodium.json
    iris.json
    ...
  packs/
    optimized-survival.json
    ...
  shaders/
    ...
  resourcepacks/
    ...
  servers/
    ...
  datapacks/
    ...
  worlds/
    ...
  governance/
    poll_blacklist.json     # Banned/bot GitHub usernames; zero-weight in polls
  pack-overrides/
    optimized-survival-configs.zip   # Config/resource overrides; NO executables
  archived/                  # Items removed by community triage (see §5.3)
    removed-mod.json
/crash-signatures/
  fabric-api-missing.json
  out-of-memory.json
  ...
/loader-manifests/
  fabric-1.21.json          # Pinned modloader URLs + SHA-256 hashes (see §8.2.1)
  neoforge-1.21.json
  ...
/.github/
  workflows/
    compile.yml             # Nightly compiler GitHub Action
  ISSUE_TEMPLATE/
    review-form.yml         # Structured review submission form
    mod-submission.yml      # New mod PR template
README.md                   # Code of Engagement (full text, see §5)
```

**Separate Repositories:**
- `launcher-media` — Custom promotional banners and images served via GitHub Pages (see §4 Image and Media Handling). Not part of the main registry repo to avoid binary bloat.

---

## 2. SOURCE JSON SCHEMAS

### 2.1 Mod Manifest (`/registry/mods/<id>.json`)

```json
{
  "id": "sodium",
  "name": "Sodium",
  "content_type": "mod",
  "author": "CaffeineMC",
  "license": "LGPL-3.0",
  "download_strategy": "github_release",
  "source_identifier": "CaffeineMC/sodium",
  "package_signatures": ["me.jellysquid.mods.sodium", "net.caffeine.sodium"],
  "base_categories": ["optimization", "rendering"],
  "community_categories": ["client-only", "performance-boost", "essentials"],
  "curator_note": "Essential rendering engine replacing legacy OpenGL pipelines. Significantly boosts framerates on nearly all hardware. Incompatible with OptiFine; use Iris for shader support instead.",
  "icon_url": "https://raw.githubusercontent.com/CaffeineMC/sodium/main/assets/icon.png",
  "gallery_urls": [
    "https://raw.githubusercontent.com/CaffeineMC/sodium/main/assets/screenshot1.png"
  ],
  "governance": {
    "immune": false,
    "override_justification": null,
    "allow_comments": true
  }
}
```

**Field Reference:**

| Field | Type | Description |
|---|---|---|
| `id` | string | Unique slug, lowercase, hyphenated |
| `name` | string | Display name |
| `content_type` | string | `mod`, `pack`, `shader`, `resourcepack`, `server`, `datapack`, `world` |
| `author` | string | Creator or organization name |
| `license` | string | SPDX license identifier (e.g., `MIT`, `LGPL-3.0`) |
| `download_strategy` | string | `github_release` (primary — direct from developer repo), `modrinth_id` (supplementary fallback — via Modrinth API), `direct_hash` (for closed-source/self-hosted) |
| `source_identifier` | string | GitHub `owner/repo`, Modrinth project ID, or direct URL |
| `sha256` | string | **Required for all strategies.** SHA-256 hash of the downloadable file. For `github_release`, the compiler populates from release asset metadata. For `modrinth_id`, the compiler populates from the Modrinth API. For `direct_hash`, manually provided by the developer. The launcher rejects any download where the computed hash does not match. |
| `package_signatures` | string[] | Java package prefixes for crash log cross-referencing. Note: multiple mods may transitively share package prefixes (e.g., `net.fabricmc`). The crash resolver uses these as an initial filter, then narrows down using class names from the stack trace and the instance's installed mod list. |
| `base_categories` | string[] | Official curated categories |
| `community_categories` | string[] | Freeform community tags (dynamic; auto-discovered by compiler) |
| `curator_note` | string | Human-written markdown write-up for display and AI semantic context |
| `icon_url` | string | CDN URL for the mod's icon image. For `github_release` strategy, curators provide this manually (e.g., pointing to the repo's `raw.githubusercontent.com` assets or the `launcher-media` repo). For `modrinth_id`, auto-populated from Modrinth API. For `direct_hash`, manually provided. |
| `gallery_urls` | string[] | Array of CDN URLs for gallery/screenshot images. Auto-populated from Modrinth API or manually provided |
| `governance.immune` | boolean | If `true`, bypasses all automated triage and vote penalties |
| `governance.override_justification` | string\|null | **Required when immune=true.** Displayed verbatim in UI. |
| `governance.allow_comments` | boolean | If `false`, review section is locked on this mod's page |

### 2.2 Closed-Source / Direct-Hash Mod

If a mod is self-hosted and closed-source, its manifest **must** include a direct URL and SHA-256 hash:

```json
{
  "id": "proprietary-mod",
  "download_strategy": "direct_hash",
  "source_identifier": "https://developer.com/releases/mod-v1.0.0.jar",
  "sha256": "a1b2c3d4e5f6..."
}
```

- The launcher **blocks download** if the hash of the fetched file does not match `sha256`.
- The developer **must** submit a new PR to update the hash for every new version. If they silently update the hosted file without a PR, every existing download is blocked for all users.
- This rule is explained in the repository README and enforced structurally by the launcher.

### 2.2b GitHub Release Version Metadata (`version_info.json`)

For mods distributed via `github_release`, developers may optionally attach a `version_info.json` file to their GitHub release assets. This provides structured compatibility data that the launcher can use instead of guessing from filenames and descriptions:

```json
{
  "minecraft_version": "1.21",
  "loader": "fabric",
  "loader_version": "0.15.11",
  "mod_version": "2.1.0",
  "release_date": "2024-06-15",
  "changelog_url": "https://github.com/owner/repo/releases/tag/v2.1.0"
}
```

If present, the launcher uses this data to populate `compatible_versions_json` in the registry and to filter versions in the mod detail page version picker. If absent, the launcher falls back to filename pattern matching (`*1.21*.jar`) and release description text parsing.

### 2.3 Modpack Manifest (`/registry/packs/<id>.json`)

```json
{
  "pack_id": "optimized-survival",
  "name": "Community Optimized Survival",
  "minecraft_version": "1.21",
  "loader": "fabric",
  "loader_version": "0.15.11",
  "mods": [
    { "id": "sodium", "source": "manifest", "status": "required" },
    { "id": "iris", "source": "github_release", "version": "v1.7.2", "status": "recommended", "description": "Enable this if you want to use shaders." },
    { "id": "xaeros-minimap", "source": "modrinth_id", "modrinth_id": "1bokaNcj", "version": "24.2.0", "status": "optional", "description": "Client-side minimap. Disable for a pure vanilla feel." }
  ],
  "override_url": "https://raw.githubusercontent.com/<org>/<repo>/main/registry/pack-overrides/optimized-survival-configs.zip",
  "curator_note": "A curated, performance-focused survival pack for 1.21. Vanilla+ aesthetic with dramatically improved framerates.",
  "governance": {
    "immune": false,
    "override_justification": null,
    "allow_comments": true
  }
}
```

**Pack Mod Entry Fields:**

| Field | Type | Description |
|---|---|---|
| `id` | string | Mod registry ID |
| `source` | string | `manifest` (lookup in registry.db), `modrinth_id` (query Modrinth API directly), `github_release` |
| `modrinth_id` | string | Modrinth project ID (required when `source = "modrinth_id"`) |
| `version` | string | **Exact version string** to install (e.g., `"1.7.2+mc1.21"`, `"v2.1.0"`). For `modrinth_id`, this is the version name from Modrinth. For `github_release`, this is the release tag. If omitted, the launcher defaults to the latest version compatible with the pack's `minecraft_version` and `loader` |
| `status` | string | `required`, `recommended`, `optional` |
| `description` | string | UI tooltip explaining why this mod is recommended/optional |

### 2.4 Community Crash Signature (`/crash-signatures/<id>.json`)

```json
{
  "id": "fabric-api-missing",
  "name": "Missing Fabric API",
  "regex_pattern": "net\\.fabricmc\\.loader\\.impl\\.discovery\\.ModResolutionException: Mod resolution encountered an incompatible mod set!.*requires \\{fabric @",
  "solution_markdown": "A mod you installed requires **Fabric API**, but it is missing from your mod folder. Click the button below to install it automatically.",
  "action_button": {
    "label": "Install Fabric API",
    "mod_id": "fabric-api"
  }
}
```

#### 2.4.1 Regex DoS Prevention

Community-submitted regex patterns create a ReDoS (Regular Expression Denial of Service) risk. Patterns like `(a+)+` can cause catastrophic backtracking when run against large crash logs, hanging the launcher.

**Countermeasures:**

1. **Rust `regex` Crate Only:** The Rust `regex` crate is *the only* regex engine permitted. It intentionally does not support unbounded backtracking — patterns that would cause exponential backtracking instead return an error at compile time or run in guaranteed linear time. This structurally prevents the most dangerous class of ReDoS attacks.

2. **Maximum Regex Length:** All submitted `regex_pattern` values are limited to **256 characters**. This prevents extremely complex patterns that could still cause performance issues even under the Rust regex engine's constraints.

3. **PR Review Enforcement:** Curators must test every new crash signature against a representative crash log (≥100KB) before merging. The compilation CI rejects any pattern that takes longer than **50ms** to evaluate against the test corpus.

4. **Compiled Regex Caching:** The Tauri client pre-compiles all `crash_signatures` patterns on startup and caches them in memory. Matching is then a simple linear scan with no per-match compilation overhead.

---

## 3. THE NIGHTLY COMPILER (GITHUB ACTION)

**File:** `.github/workflows/compile.yml`  
**Schedule:** Every 24 hours (cron: `0 2 * * *`)  
**Runtime:** Free GitHub Actions Ubuntu runner  
**Dependencies:** Python, `sqlite3` (stdlib), `requests`, `profanity-check`, `vaderSentiment`

### 3.1 Compiler Execution Steps

1. **Initialize in-memory SQLite** using Python's built-in `sqlite3` library.

2. **Parse all manifest JSON files** in `registry/mods/`, `registry/packs/`, etc. Items in `registry/archived/` are explicitly skipped and excluded from the compiled database.

3. **API Batch Fetch with Complexity Budgeting** — Query the GitHub API for all tracked Issues:
   - For reaction counts: Use the GitHub **REST API** `GET /repos/{owner}/{repo}/issues/{issue_number}/reactions` where more efficient (cheaper than GraphQL per-item queries).
   - For comments: Use GraphQL with `pageInfo` cursors. Budget approximately 100 items per query to stay well under the 5,000 complexity-point/hour limit.
   - Pull comment text for all `[REVIEW]`-tagged comments.
   - Pull commenter account metadata for trust scoring.
   - Cache issue metadata between nightly runs in the GitHub Action runner's workspace to avoid re-querying unchanged items.

4. **Trust Score Filtering** (applied to every reacting user):
    - **Account age:** Must be older than 30 days (`createdAt` check). *Note: public repository check was explicitly excluded to avoid disenfranchising non-developer users.*
    - **Activity threshold:** Must have at least 3 interactions (issues opened, comments, PRs, or reactions) across repositories owned by the agora-mc organization. This is queryable via the GitHub GraphQL API using `user.contributionsCollection` scoped to the org.
    - If account fails either check: reaction is left on GitHub but assigned a **weight of 0** in compilation. Bad actors waste time clicking buttons that have zero structural impact. No notification is sent.

   **Sybil Attack Resistance:** The base thresholds (30-day age + 3 comments) are cheap for bot farms to overcome at scale (create 1000 accounts, wait 30 days, leave 3 comments each). To mitigate this without disenfranchising legitimate users:
    - **Velocity weighting:** A single user's vote weight is capped relative to the historical daily average vote volume for that item. If an item typically receives 5 votes/day and suddenly receives 100 from new accounts in a 6-hour window, each of those votes is weighted down proportionally rather than counted at full weight.
    - **Account diversity bonus:** Vote weight receives a small multiplier if the account has a demonstrated history of participating in *different* issues/repositories (not just the targeted one). Accounts that only vote on a single item are treated with suspicion.
    - **Curator escalation:** If the compiler detects more than 50 new accounts voting on the same item within 24 hours—all passing the base trust check—it flags the item for curator review and applies the velocity anomaly detection (Step 5) even if the raw threshold hasn't been met.

5. **Velocity Anomaly Detection (Circuit Breaker):**
   ```
   if (recent_downvotes / historical_average) > 5.0 AND total_recent > 20:
       → set status = 'under_review'
       → freeze vote counts at pre-spike values
       → trigger GitHub Discussions Triage Poll (see §5.3)
   ```
   "Recent" is defined as the past 6-hour window. Historical average is the rolling 7-day mean.

6. **Programmatic Reaction Scrubbing** — If the circuit breaker fires on a burst window:
    - Use `DELETE /repos/{owner}/{repo}/issues/comments/{comment_id}/reactions/{reaction_id}` to remove the malicious downvotes from GitHub.
    - Log offending usernames to `registry/governance/poll_blacklist.json`.
    - For extreme cases: the compiler creates a high-priority confidential issue in the private admin repo with title `[ALERT] Coordinated Attack Detected` and lists offending usernames for manual curator review and org-level action.

**Compiler API Permissions:** The nightly compiler requires `issues:write` (to delete malicious reactions and create curator alerts) and `discussions:write` (to create triage polls). These are granted via the `GITHUB_TOKEN` in the Action workflow with `permissions: issues: write, discussions: write` in the YAML.

7. **Sentiment & Spam Scrubbing** — Every comment that passes trust filtering is then evaluated:
   - **Regex filters (discard matching):**
     - Version begging: `(?i)(when\s+is|update\s+to|for|port|1\.\d+)\s*(release|\?|when)`
     - Empty praise: `^(good\s+mod|nice|cool|pog|great|wow)$`
   - **`profanity-check` (Python NLP):** Trained SVM model. Discards comments that fail toxicity threshold. Handles obfuscation (e.g., `b@d_w0rd`) better than wordlists.
   - **`vaderSentiment` (Python NLP):** Scores positivity/negativity. Discards comments with extreme aggression intensity scores even if profanity-check passes.
   - Comments surviving all filters are included in the compiled SQLite `curator_reviews` table.

8. **Under-Review State Resolution** — For items where a 7-day triage poll has expired:
   - Parse poll results using GitHub Discussions API.
   - Any vote cast by a user in `poll_blacklist.json` is counted at weight 0.
   - **If "Remove" wins:** Archive item. Remove from all future builds. JSON file is moved to `registry/archived/`.
   - **If "Keep" wins:** Restore `status = 'active'`. Set `immunity_cooldown` for 30 days, during which the automated triage logic is paused for this item.

9. **Immune Item Pass-Through:** If `governance.immune = true`, skip all score evaluation. Item is inserted into the database as `status = 'active'` regardless of vote counts.

10. **SQLite Compilation:** Build `registry.db` from the in-memory state.

11. **Image URL Hydration:** For mods with `download_strategy = "github_release"`, curators provide `icon_url` and `gallery_urls` directly in the manifest (e.g., pointing to `raw.githubusercontent.com` asset paths). For `modrinth_id` mods, the compiler queries the Modrinth API via the batch endpoint `GET /v2/projects?ids=[...]` (up to 500 IDs per request) to extract icon/gallery URLs. For non-Modrinth, non-GitHub items, curators provide image URLs manually. If custom promotional banners are needed, upload those assets to the dedicated `launcher-media` repository served via GitHub Pages. **The database never holds binary image data — only URL strings pointing to CDN-hosted images.**

12. **Database Signing:** Sign the compiled `registry.db` with the project's offline Ed25519 private key. Write the signature to `registry.db.sig`. This allows the Tauri client to verify database authenticity and detect a compromised GitHub account distributing a malicious build.

13. **Deployment as GitHub Release Asset:** Upload `registry.db` and `registry.db.sig` as assets on a tagged GitHub Release (e.g., `registry-2026-06-15`). **Do not commit `registry.db` to the source repository.** A binary file committed daily would bloat the repository's Git history irreversibly, making clones prohibitively slow. GitHub Release Assets support files up to 2GB (vs. 100MB for regular commits), and the database (storing text: names, descriptions, numbers) will remain well under 30MB for the foreseeable future. The Tauri client fetches the latest release asset URL from the GitHub Releases API on startup.

### 3.2 GitHub Interaction Limits ("Raid Shield")

If a major internet event causes a coordinated mass-attack:
- GitHub's native Interaction Limits can be programmatically enabled via API.
- Setting: "Limit to existing users" — only accounts with prior interactions can comment or react.
- This can be toggled by the compiler when a velocity anomaly is detected without requiring human intervention.

---

## 4. CLIENT-SIDE SQLITE SCHEMA

The launcher maintains **two separate SQLite databases**:

1. **`registry.db`** — Read-only global state downloaded from GitHub Release Assets. This database is treated as immutable by the launcher. It contains curated mod metadata, social metrics, categories, crash signatures, and curated reviews. The launcher never writes to this file.

2. **`local_state.db`** — Read-write local state stored in the user's app data directory (`%APPDATA%/agora-mc/local_state.db`, etc.). This contains user settings, instance metadata, launch history, crash telemetry, MCP approval grants, cached registry release tag, and any other mutable application state.

**Why the Split:**
- Prevents file-system locking bugs where a downloaded read-only database competes with runtime writes.
- Allows the launcher to replace `registry.db` atomically while `local_state.db` remains open.
- Makes offline mode simpler: `registry.db` can be stale or missing, but `local_state.db` still knows about installed instances.
- Enables clean backups: `local_state.db` + `instance_manifest.json` files are the only mutable state needed to reconstruct a user's setup.

### 4.0 Database Files & Paths

| Database | Location | Purpose | Mutability |
|---|---|---|---|
| `registry.db` | App data directory, downloaded from GitHub Release Assets | Curated catalog, crash signatures, social metrics | Read-only at runtime |
| `registry.db.sig` | Same directory as `registry.db` | Ed25519 signature for `registry.db` | Read-only at runtime |
| `local_state.db` | App data directory | User settings, instances, telemetry, approvals | Read-write |

### Table: `registry_items`
| Column | Type | Constraints | Description |
|---|---|---|---|
| `id` | TEXT | PRIMARY KEY | Unique slug (e.g., `"sodium"`) |
| `name` | TEXT | NOT NULL | Display name |
| `content_type` | TEXT | NOT NULL | `mod`, `pack`, `shader`, `resourcepack`, `server`, `datapack`, `world` |
| `download_strategy` | TEXT | NOT NULL | `github_release` (primary), `modrinth_id` (supplementary), `direct_hash` |
| `source_identifier` | TEXT | NOT NULL | GitHub `owner/repo`, Modrinth ID, or URL |
| `sha256` | TEXT | NOT NULL | Hash for verification. Required for all strategies; populated by compiler from GitHub release metadata or Modrinth API |
| `upvotes` | INTEGER | DEFAULT 0 | Trust-weighted thumbs-up count from GitHub |
| `downvotes` | INTEGER | DEFAULT 0 | Trust-weighted thumbs-down count |
| `net_score` | INTEGER | DEFAULT 0 | Pre-computed: `upvotes - downvotes` |
| `velocity` | REAL | DEFAULT 0.0 | Change in net score over last 7 days (trending metric) |
| `status` | TEXT | DEFAULT 'active' | `active`, `under_review`, `archived` |
| `is_immune` | BOOLEAN | DEFAULT 0 | If 1, triage is bypassed |
| `immunity_reason` | TEXT | | `override_justification` string displayed verbatim in UI |
| `allow_comments` | BOOLEAN | DEFAULT 1 | If 0, review section is locked |
| `immunity_cooldown_until` | TEXT | | ISO timestamp; triage paused until this date ("Keep" vote result) |
| `icon_url` | TEXT | | CDN URL for the mod's icon image (hotlinked; never stored as binary) |
| `gallery_urls_json` | TEXT | | JSON array of CDN URLs for gallery/screenshot images |
| `date_added` | TEXT | | ISO timestamp of first appearance in the registry |
| `compatible_versions_json` | TEXT | | JSON array of objects: `{ "mc_version": "1.21", "loader": "fabric", "mod_version": "1.7.2" }`. Populated by compiler from Modrinth API for `modrinth_id` strategy; extracted from GitHub release metadata for `github_release` strategy |

### Image and Media Handling

The database **never stores binary image data**. All images are hotlinked via URL strings:

1. **GitHub Release Assets:** For `github_release` mods, curators typically point `icon_url` to `raw.githubusercontent.com` paths within the developer's repo (e.g., `assets/icon.png` on the default branch), or to the `launcher-media` repo for custom promotional assets.
2. **Modrinth CDN:** For `modrinth_id` mods, the nightly compiler queries the Modrinth API and extracts project icon/gallery URLs. This is the **supplementary** path — used when a mod is not yet self-hosting on GitHub.
3. **Custom Assets:** For curated packs or items not on GitHub or Modrinth, curators provide image URLs directly in the manifest. Custom promotional banners are uploaded to the `launcher-media` repository served via GitHub Pages.
4. **Client-Side Caching:** The Tauri app may cache downloaded images locally for offline display, but the source of truth is always the URL in the database.

### Table: `categories`
| Column | Type | Constraints | Description |
|---|---|---|---|
| `id` | TEXT | PRIMARY KEY | Lowercase slug (e.g., `"create-addons"`) |
| `display_name` | TEXT | NOT NULL | Human-readable (e.g., `"Create Addons"`) |
| `is_community` | BOOLEAN | DEFAULT 0 | 0 = curated, 1 = community-submitted |

### Table: `item_categories` (Junction)
| Column | Type | Constraints |
|---|---|---|
| `item_id` | TEXT | FK → `registry_items(id)` |
| `category_id` | TEXT | FK → `categories(id)` |

### Table: `curator_reviews`
| Column | Type | Description |
|---|---|---|
| `item_id` | TEXT | FK → `registry_items(id)` |
| `curator_note` | TEXT | Markdown write-up from the curator team |
| `top_reviews_json` | TEXT | JSON array of top community comments (survived NLP filtering) |

### Table: `crash_signatures`
| Column | Type | Description |
|---|---|---|
| `id` | TEXT | PRIMARY KEY |
| `name` | TEXT | Human-readable crash name |
| `regex_pattern` | TEXT | Regex to match against crash log text |
| `solution_markdown` | TEXT | User-facing fix description |
| `action_button_json` | TEXT | JSON: `{ "label": "...", "mod_id": "..." }` |

**Note on Mutable State:** User instances, crash telemetry, settings, and MCP approval grants are **not** stored in `registry.db`. They live in the separate `local_state.db` database defined in §4.1b. This split ensures `registry.db` is read-only and replaceable at runtime without data loss.

#### Schema Versioning
| Column | Type | Description |
|---|---|---|
| `version` | INTEGER | Schema version number (monotonically increasing) |

The Tauri app checks `SELECT version FROM schema_version` on startup. If the cached DB's version is older than the app's expected version, it re-downloads `registry.db` from the latest Release Asset.

#### Database Update Detection

The Tauri app tracks the currently cached `registry.db` release tag (e.g., `registry-2026-06-15`) in its local settings store. The update check runs:
- On every app startup (if online).
- When the user manually clicks **"Check for Registry Update"** in Settings.
- No more than once per hour to avoid GitHub API quota waste.

**Update Check Flow:**
1. Query GitHub Releases API: `GET /repos/{owner}/{repo}/releases/latest`.
2. Compare the returned tag name with the locally cached tag.
3. If newer: download `registry.db` and `registry.db.sig` from the release assets.
4. Verify the Ed25519 signature.
5. Replace the old DB atomically (write to `registry.db.tmp`, then `rename` to `registry.db`).
6. Check `schema_version` and trigger any necessary app-side migrations.
7. Show a notification: *"Registry updated to <tag>. <N> new mods, <M> updated entries."*

If offline, the app skips the update check and uses the last cached DB. A "Limited Offline Mode" banner appears if the DB is older than 7 days.

### 4.1a Database Versioning Protocol

The launcher enforces a strict protocol whenever a new `registry.db` is available:

1. **Current Schema Check:** On startup, the launcher reads `SELECT version FROM schema_version` from the cached `registry.db`. It compares this value against the **app's supported schema version** (`APP_REGISTRY_SCHEMA_VERSION`), a compile-time constant in the Rust code.

2. **Backward Compatibility Only:** Schema versions are monotonically increasing integers. The launcher supports the cached DB version if `cached_version <= APP_REGISTRY_SCHEMA_VERSION` AND the launcher knows how to read and migrate that version's structure.

3. **Forward-Incompatible DB Detected:** If `cached_version > APP_REGISTRY_SCHEMA_VERSION` (e.g., the user downloaded a newer registry via a manual update, but the app binary is older):
   - Block the DB load.
   - Display: *"The registry database was created by a newer version of this launcher. Please update the app to continue."*
   - Offer a button to open the GitHub Releases page for the latest app version.
   - Do NOT fall back to the cached older DB in this case, because the launcher cannot safely interpret the newer schema.

4. **Backward-Incompatible DB Detected:** If `cached_version < APP_REGISTRY_SCHEMA_VERSION`:
   - If the launcher has a built-in migration for that version: apply it in-place on `registry.db` (read the old data, write a new temporary DB with the updated schema, then atomically replace).
   - If no migration exists: trigger a registry update download, which will provide a DB at the current schema version.

5. **Race Condition Prevention:** The launcher never replaces `registry.db` while an instance is launching or while the MCP server is actively reading from it. A simple readers-writer lock is used:
   - `registry.db` is opened in read-only mode via `tauri-plugin-sql`.
   - The update process acquires an exclusive write lock before replacing the file.
   - Any in-flight launches that started before the update continue using the old DB handle; new launches use the updated DB after the replacement completes.

### 4.1b `local_state.db` Schema (Mutable User State)

The `local_state.db` database is created on first run and is never replaced by the launcher. It stores all mutable user and instance state.

#### Table: `user_settings`
| Column | Type | Description |
|---|---|---|
| `key` | TEXT | PRIMARY KEY |
| `value_json` | TEXT | JSON-encoded value |

Stores: active sidebar tab, last-selected Minecraft version, JVM defaults, UI theme, notification preferences, telemetry opt-in, MCP server port, cached registry release tag, Modrinth integration toggle, AI/MCP integration toggle.

#### Table: `user_instances`
| Column | Type | Description |
|---|---|---|
| `instance_id` | TEXT | PRIMARY KEY (e.g., `"my-survival-pack"`) |
| `name` | TEXT | User-chosen display name |
| `minecraft_version` | TEXT | |
| `loader` | TEXT | `fabric`, `neoforge`, `quilt`, `forge` |
| `loader_version` | TEXT | |
| `is_modpack` | BOOLEAN | Built from a curated pack manifest? |
| `is_locked` | BOOLEAN | 1 = immutable manifest enforced |
| `last_launched_at` | TEXT | ISO timestamp; used for crash detection |
| `jvm_memory_mb` | INTEGER | User-selected RAM allocation in MB |
| `jvm_gc` | TEXT | `g1gc`, `zgc`, `shenandoah`, `custom` |
| `jvm_custom_args` | TEXT | Extra JVM flags from text box |

*Note: Detailed per-mod state lives in each instance's `instance_manifest.json` file rather than SQLite, for exportability and simplicity.*

#### Table: `local_crash_telemetry`
| Column | Type | Description |
|---|---|---|
| `mod_a_id` | TEXT | First mod identifier. For curated mods: `registry_items.id`. For raw Modrinth mods: `modrinth:{modrinth_id}`. For manual/unknown mods: `manual:{filename_hash}` |
| `mod_b_id` | TEXT | Second mod identifier (same format as `mod_a_id`) |
| `crash_count` | INTEGER | How often this pair co-crashes locally |

**Pair ID Generation:** For curated mods, use the `registry_id`. For raw Modrinth mods, prefix with `modrinth:`. For completely unknown mods (manual drag-drop, no metadata), generate a stable hash from the filename and prefix with `manual:`. This ensures every mod has a trackable identifier even outside the curated registry.

**Retention:** Records older than 90 days are purged. Pairs with `crash_count < 2` are also purged during weekly maintenance to prevent unbounded table growth.

#### Table: `mcp_approval_grants`
| Column | Type | Description |
|---|---|---|
| `tool_name` | TEXT | Name of the MCP tool |
| `instance_id` | TEXT | Instance scope (or `"*"` for all instances) |
| `state` | TEXT | `pending`, `approved_once`, `approved_always`, `denied` |
| `granted_at` | TEXT | ISO timestamp |
| `expires_at` | TEXT | ISO timestamp or NULL |

When a user selects "Always Allow for This Tool" in the approval dialog, the grant is stored here with `state = 'approved_always'`. This prevents repeated prompts across app restarts. Grants can be revoked by the user in Settings → Integrations → MCP Server → "Clear Tool Approvals."

#### Schema Versioning for `local_state.db`
| Column | Type | Description |
|---|---|---|
| `version` | INTEGER | Schema version number for `local_state.db` |

The launcher runs a Rust-based migration runner on startup. Migrations are applied sequentially from the stored version to `APP_LOCAL_STATE_SCHEMA_VERSION`. Because this database is truly local, migrations must be robust against data loss.

### 4.1c Output Security (XSS / Injection Prevention)

All data sourced from SQLite is **untrusted**. Curator notes, review text, category names, and crash signature markdown all originate from community input that survived NLP filtering but may still contain malicious markup. The following rendering rules are mandatory:

1. **React Frontend:** The Tauri React UI must **never** use `dangerouslySetInnerHTML` for any user/community-sourced content. All output is rendered via React's default JSX escaping, which HTML-encodes all special characters. Curator notes are rendered as plain text with a custom lightweight markdown parser that only supports bold, italic, code blocks, and links — no raw HTML passthrough.

2. **Next.js Web Directory:** The static website must render all SQLite-sourced text safely. Use `react-markdown` with a strict `allowedElements` list (`p`, `strong`, `em`, `code`, `a`, `pre`, `ul`, `ol`, `li`) and `disallowedElements={['html']}` to structurally prevent raw HTML generation. If raw HTML can never be emitted, there is nothing to sanitize. No `<script>`, `<iframe>`, `<object>`, `<embed>`, or event handler attributes are permitted.

3. **Markdown Rendering:** The custom markdown renderer supports only the following inline/block elements: `**bold**`, `*italic*`, `` `code` ``, `[links](url)` (with `rel="noopener noreferrer"` and URL scheme validation), and code fences. Everything else (HTML tags, image tags, etc.) is stripped.

### 4.2 Settings Persistence

General app settings are stored in `user_settings` table in `local_state.db`. Rapidly accessed UI state (active sidebar tab, last-selected filter, etc.) is also mirrored in `tauri-plugin-store` for reactive reads. Long-term authoritative settings live in `local_state.db` and are loaded into the store on startup. If a setting exists in both, `local_state.db` wins.

Settings include: active sidebar tab, last-selected Minecraft version, JVM defaults, UI theme, notification preferences, telemetry opt-in, MCP server port, cached registry release tag, cached Mojang launcher path, Modrinth integration toggle, AI/MCP integration toggle, and MCP approval grants.

**Settings are editable on the fly** — the Settings tab writes to `tauri-plugin-store` immediately, which asynchronously syncs to `local_state.db`. Components subscribing to store values update immediately.

### 4.3 Offline / Degraded Mode

If the launcher cannot reach GitHub Releases to fetch or update `registry.db`, it enters **Degraded Mode**. This is distinct from normal offline use (where a cached `registry.db` is simply stale).

**Degraded Mode Triggers:**
- GitHub Releases API returns 5xx errors or times out after retries.
- DNS resolution fails for `api.github.com`.
- The user's network is completely offline.
- GitHub is blocked by regional firewall or ISP.

**Degraded Mode Behavior:**
1. A persistent banner appears at the top of the app: *"⚠️ Registry Offline — Running in Degraded Mode. Curated catalog may be outdated."*
2. The launcher uses the last cached `registry.db` (even if older than 7 days).
3. The **My Instances** tab is fully functional. Users can launch existing instances because `local_state.db` and `instance_manifest.json` files are intact.
4. The **Browse** tab shows curated mods from the cached DB. Mods with `download_strategy = 'modrinth_id'` are filtered out if Modrinth integration is disabled; if Modrinth is enabled, they remain visible but show a warning that registry metadata may be stale.
5. The **Raw Modrinth Tab** (if enabled) switches to a **Manual Modrinth Input** mode:
   - Users can paste a Modrinth project URL or project ID.
   - The launcher queries the Modrinth API directly for that project and its versions.
   - Users can install mods manually, one at a time, bypassing the central registry entirely.
6. **Manual Download Mode:** A "Drag .jar Here" panel is always available. Users can install manually downloaded mod files directly into instances.
7. Users can still create **Custom Instances** from scratch and install modloaders, because `loader_manifests.json` is cached locally.
8. Governance features (Triage Center, voting, reviews) are disabled because they require GitHub API access.

**Exit Condition:** Degraded Mode exits automatically when the launcher successfully completes a registry update check against GitHub.

### 4.4 Upstream Verification Policy

The launcher does not blindly trust official modloader domains. To prevent compromise of upstream CDNs from translating directly into compromise of users, the launcher maintains a **hardcoded `known_good_hashes.json`** map embedded in the Rust binary at compile time.

```json
{
  "loader_hashes": {
    "fabric": {
      "fabric-loader-0.15.11-1.21.json": "sha256:a1b2c3...",
      "fabric-loader-0.15.10-1.21.json": "sha256:d4e5f6..."
    },
    "neoforge": {
      "neoforge-21.0.0-beta.json": "sha256:..."
    },
    "quilt": {
      "quilt-loader-0.26.0-1.21.json": "sha256:..."
    },
    "forge": {
      "forge-1.21-51.0.0.json": "sha256:..."
    }
  },
  "domain_allowlist": [
    "meta.fabricmc.net",
    "maven.fabricmc.net",
    "neoforged.net",
    "maven.neoforged.net",
    "meta.quiltmc.org",
    "maven.quiltmc.org",
    "minecraftforge.net",
    "files.minecraftforge.net"
  ]
}
```

**Policy Rules:**
1. The `known_good_hashes.json` file is maintained by curators in the registry repository at `/loader-manifests/known_good_hashes.json`.
2. The nightly compiler includes this file in `registry.db` as a `system_config_json` row.
3. The Rust binary also embeds a **compile-time copy** of the map. If the embedded copy conflicts with the one in the downloaded `registry.db`, the embedded copy wins. This prevents a compromised `registry.db` from weakening modloader verification.
4. Before downloading any modloader version JSON, the launcher verifies its SHA-256 hash against the map. If the hash does not match exactly, the download is rejected.
5. Domain pinning is still enforced. A download is rejected if it redirects to any domain not in `domain_allowlist`.
6. Curators update the hash map only via Pull Request, and every change is reviewed by at least one other curator.
7. If a new modloader version is released and the map hasn't been updated yet, the launcher shows: *"This modloader version has not been verified by the curation team yet. Please wait for the next registry update or check the project's official channels."*

### 4.5 Human-Centric Error Taxonomy

The Rust backend communicates errors to the React UI using standardized error codes. Each error code maps to a user-facing message, a log level, and (where applicable) a recommended action. This keeps the UI code clean and ensures consistent messaging.

| Error Code | Severity | User-Facing Message | Recommended Action |
|---|---|---|---|
| `ERR_NETWORK_OFFLINE` | Info | *"You're offline. Using cached data."* | Continue with cached DB; show Degraded Mode banner |
| `ERR_REGISTRY_DOWNLOAD_FAILED` | Warning | *"Could not download the latest registry. Using cached version."* | Retry later; check network |
| `ERR_REGISTRY_SIGNATURE_INVALID` | Critical | *"Registry signature check failed. The database may be compromised."* | Block DB load; use last known-good cached DB; notify curators |
| `ERR_SCHEMA_TOO_NEW` | Critical | *"This registry requires a newer launcher version. Please update the app."* | Open GitHub Releases page for app update |
| `ERR_ZIP_BOMB` | Critical | *"Installation aborted: this archive exceeds safety limits."* | Delete archive; log security incident |
| `ERR_OVERRIDE_SECURITY_VIOLATION` | Critical | *"Installation aborted: pack override contains forbidden files."* | Abort; delete partial files; report pack to curators |
| `ERR_HASH_MISMATCH` | Critical | *"Downloaded file does not match its expected hash. It may be corrupted or tampered with."* | Re-download once; if persists, skip mod and report |
| `ERR_UNTRUSTED_SOURCE` | Critical | *"Download rejected: URL is not from an allowed source."* | Block download; log incident |
| `ERR_DISKFULL` | Warning | *"Not enough disk space to complete this operation."* | Show required vs available space; abort before writing |
| `ERR_AUTH_EXPIRED` | Warning | *"Your GitHub session has expired. Sign in again to continue."* | Prompt OAuth Device Flow again |
| `ERR_AUTH_REQUIRED` | Info | *"This feature requires GitHub sign-in."* | Show sign-in button |
| `ERR_MODRINTH_DISABLED` | Info | *"Modrinth integration is disabled. Enable it in Settings or install this mod manually."* | Open Settings → Integrations or drag-and-drop panel |
| `ERR_INSTANCE_LOCKED` | Info | *"This instance is locked. Unlock it to add or remove mods."* | Show unlock confirmation dialog |
| `ERR_SANDBOX_UNAVAILABLE` | Error | *"Dev Mode builds require Docker, Podman, or Firecracker."* | Link to sandbox setup guide |
| `ERR_MOJANG_NOT_FOUND` | Error | *"Minecraft Launcher not found. Please install it or set its path in Settings."* | Open Settings → Launcher Path |
| `ERR_LAUNCH_FAILED` | Error | *"Could not start Minecraft. Check the logs for details."* | Open Diagnostics / Logs tab |
| `ERR_UNSUPPORTED_LOADER` | Error | *"This modloader version is not yet verified by the curation team."* | Wait for registry update |
| `ERR_VERSION_NOT_FOUND` | Warning | *"Requested mod version not found. Install the closest compatible version?"* | Show fallback version picker |
| `ERR_DEPENDENCY_MISSING` | Warning | *"A mod requires [dependency]. Try installing it?"* | Offer auto-install from crash signature or manual search |
| `ERR_MCP_TOO_MANY_REQUESTS` | Warning | *"AI client sent too many requests. Approve or deny pending requests first."* | Show MCP approvals queue |
| `ERR_MCP_DENIED` | Info | *"AI tool request denied based on your saved approval preferences."* | No action — call was explicitly blocked |
| `ERR_MCP_UNAUTHORIZED` | Error | *"AI client connection rejected: invalid or missing token."* | Show current token in Settings |

**Error Response Shape (JSON-RPC and Tauri commands):**
```json
{
  "success": false,
  "error": {
    "code": "ERR_HASH_MISMATCH",
    "message": "Downloaded file does not match its expected hash.",
    "details": {
      "filename": "sodium.jar",
      "expected": "a1b2...",
      "actual": "c3d4..."
    },
    "suggested_action": "retry" 
  }
}
```

`message` is always safe to display directly to the user. `details` may contain technical data and is shown only when the user clicks "Show Details." `suggested_action` is one of: `retry`, `skip`, `abort`, `open_settings`, `sign_in`, `manual_install`.

### 4.6 Audit Log (Transparency "Black Box")

The compiler generates an append-only **`/governance/audit_log.json`** file in the central repository. Every automated or curator-initiated governance action is recorded with a timestamp, actor, target, and reason.

```json
{
  "log_format_version": 1,
  "entries": [
    {
      "timestamp": "2026-06-16T19:00:00Z",
      "action": "AUTO_FLAG",
      "actor": "compiler-bot",
      "target_type": "mod",
      "target_id": "tech-mod",
      "reason": "velocity_anomaly",
      "details": {
        "recent_downvotes": 150,
        "historical_average": 5.2,
        "poll_id": "D_kwDO123ABC"
      }
    },
    {
      "timestamp": "2026-06-16T20:30:00Z",
      "action": "POLL_CREATED",
      "actor": "compiler-bot",
      "target_type": "mod",
      "target_id": "tech-mod",
      "reason": "community_triage",
      "details": {
        "discussion_id": 12345,
        "duration_days": 7
      }
    },
    {
      "timestamp": "2026-06-23T20:30:00Z",
      "action": "IMMUNITY_APPLIED",
      "actor": "curator-alice",
      "target_type": "mod",
      "target_id": "tech-mod",
      "reason": "vote_brigading_outside_scope",
      "details": {
        "override_justification": "Administrative Lock..."
      }
    },
    {
      "timestamp": "2026-06-23T21:00:00Z",
      "action": "ARCHIVED",
      "actor": "compiler-bot",
      "target_type": "mod",
      "target_id": "tech-mod",
      "reason": "poll_result_remove",
      "details": {
        "keep_votes": 45,
        "remove_votes": 312
      }
    }
  ]
}
```

**Action Types:**
- `AUTO_FLAG` — Circuit breaker fired, item marked under_review
- `POLL_CREATED` — GitHub Discussion triage poll created
- `POLL_CLOSED` — Poll expired, results recorded
- `IMMUNITY_APPLIED` — Curator applied immune override
- `IMMUNITY_REMOVED` — Curator removed immune override
- `ARCHIVED` — Item removed from registry by community vote
- `RESTORED` — Item restored after successful "Keep" vote
- `BLACKLIST_UPDATED` — Username added to `poll_blacklist.json`
- `REACTION_SCRUBBED` — Malicious reaction deleted by compiler
- `SIGNATURE_REJECTED` — Registry signature failed verification (logged by client, submitted via telemetry)

**Retention:** The audit log is appended to nightly. When it exceeds 10,000 entries, the oldest 2,000 entries are moved to `/governance/audit_log_archive.{YYYYMMDD}.json`. The current log and the last 3 archived logs are included in `registry.db` as `audit_log_json` for in-app display in the Triage Center "Transparency Log" panel.

---

## 5. COMMUNITY GOVERNANCE

### 5.1 The Code of Engagement

The following text appears verbatim in three places:
1. The central repository's `README.md`
2. The GitHub Issue Form header (`review-form.yml`)
3. The Tauri "Write a Review" modal — the user must check a consent box before the Submit button becomes active

The canonical text lives in `CODE_OF_ENGAGEMENT.md` at the repository root. A CI workflow copies this file into the three required locations during the nightly build. This ensures a single source of truth.

---

> **📜 Platform Code of Engagement**
>
> This platform is a curated asset repository, not a general discussion forum or social media feed. We built this ecosystem to keep modding open, high-quality, and hyper-focused.
>
> **Rules of Engagement (Zero Tolerance):**
> - Comments must strictly address the technical performance, stability, features, or usability of the mod or asset in question.
> - No memes, no off-topic banter, no update-begging ("1.21 when?"), no philosophical discussions.
> - No cultural, political, or social drama. Leave it at the door.
> - No aggression, entitlement, or personal attacks against mod creators or curators.
> - Violations result in immediate and permanent removal from the registry's review system.
>
> If you want to socialize, share memes, or debate off-topic things, visit our community spaces instead:
> 🔗 [Project Discord] | 🔗 [Project Matrix/Lemmy]

---

### 5.2 GitHub Issue Form (`review-form.yml`)

```yaml
# .github/ISSUE_TEMPLATE/review-form.yml
name: "Submit a Mod Review"
description: "Leave a professional technical review for a curated asset."
body:
  - type: markdown
    attributes:
      value: |
        ### 🚨 STOP: Read the Code of Engagement above before continuing.
        Reviews must be strictly focused on technical stability, performance, and features.
        Zero tolerance for version begging, memes, politics, or low-effort filler.
  - type: input
    id: mod_id
    attributes:
      label: "Mod Registry ID"
      placeholder: "e.g., sodium"
    validations:
      required: true
  - type: textarea
    id: review_text
    attributes:
      label: "Your Technical Review (50 character minimum)"
      placeholder: "Describe your experience with this asset's performance, stability, or features..."
    validations:
      required: true
  - type: checkboxes
    id: guidelines_check
    attributes:
      label: "Community Affirmation"
      options:
        - label: "I confirm this review contains no toxic language, version begging, memes, or off-topic commentary."
          required: true
```

### 5.3 The Under-Review State Machine

```
[Active Status]
      │
      ▼ (Net score drops below -10 organically, OR velocity spike detected)
[Flagged / Under Review]
      │ 
      │ → Mod stays downloadable with warning banner
      │ → GitHub Action auto-creates a 7-day Discussion poll:
      │     Title: "[Community Triage] Should '<Mod Name>' be removed from the registry?"
      │     Options: [Keep] / [Remove]
      │ → Votes from poll_blacklist.json users are counted at weight 0
      │
      ▼ (7 days pass)
      ├── [REMOVE wins] → Item archived. JSON moved to /registry/archived/. Removed from all future builds.
      │
      └── [KEEP wins] → status = 'active'. immunity_cooldown set to +30 days. Triage paused.
```

### 5.4 Curator Immune Override

Applied by adding to the JSON manifest:
```json
{
  "governance": {
    "immune": true,
    "override_justification": "Administrative Lock: The triage system was being weaponized by outside political drama. We are a modding platform, not a battleground. Locked to protect ecosystem stability.",
    "allow_comments": false
  }
}
```

When `immune = true`:
- The nightly compiler skips all score evaluation for this item.
- The client UI renders a permanent non-dismissible **Curator Shield** banner (steel blue/gray, not warning red/yellow) at the very top of the mod's profile page, above the download button:

```
┌─────────────────────────────────────────────────────────────────┐
│  🛡️ CURATOR PROTECTED ASSET                                     │
├─────────────────────────────────────────────────────────────────┤
│  This mod is permanently exempt from automated community triage │
│  and negative review penalties.                                 │
│                                                                 │
│  Reasoning from the Curators:                                   │
│  "[override_justification text rendered verbatim here]"         │
└─────────────────────────────────────────────────────────────────┘
```

- The "Upvote / Downvote" buttons are **removed** from the UI (not just greyed out — the user should not be screaming into a void).
- If `allow_comments = false`, the review section is hidden with: *"Review section locked by administration."*
- This banner is **conditional** — it only renders if `is_immune = 1`. No visual clutter for non-immune items.

### 5.5 In-App Comment Reporting

The launcher's UI includes a "🚩 Flag Review" button on every comment. This must be rate-limited to prevent abuse.

**Rate Limiting:** Each user is limited to **5 flag submissions per hour** and **20 per day**. If the limit is exceeded, the flag button is greyed out with a tooltip showing when the limit resets. This prevents a single user from generating thousands of admin tickets.

```
[User clicks 🚩 on a comment in the launcher]
        │
        ▼
[Rate limit check — if exceeded, block and show cooldown timer]
        │
        ▼
[Tauri app creates a GitHub Issue directly via the GitHub REST API]
  Target: Private admin repo (e.g., `agora-mc/admin-alerts`)
  Using: The user's OAuth token with `public_repo` scope
  Title: "[REPORT] Low-effort/Toxic comment on Mod: <mod_name>"
  Body: Direct link to the comment ID + quoted text + reporter's username
  Labels: `triage`, `comment-report`
        │
        ▼
[Curator reviews the flag, clicks Delete on GitHub]
        │
        ▼
[Comment is deleted. It disappears from the next morning's database build.]
```

**Note:** The private admin repo must be created during project initialization. The Tauri app pre-configures its `owner/repo` path.

### 5.6 Triage Center UI (In-App Tab)

A dedicated sidebar tab labeled **"Community Governance"** or **"Triage Center"** shows:

- **Active Triage Polls:** Live cards for every mod currently under review. Each card shows the mod name, reason for triage, live Keep/Remove percentage bars (fetched from GitHub Discussions API on page load), and a **"Cast Your Vote"** button that deep-links directly to the GitHub Discussion.
- **Recent Resolutions:** A historical feed showing recently resolved votes (kept or removed), with the final percentages displayed. Maintains absolute platform transparency.
- **Crash-to-Poll Connection:** If a user experiences a crash, and the crash log's `package_signatures` cross-reference reveals the crashing mod is currently under community review, the crash popup will dynamically say: *"This mod is currently being voted on by the community due to similar issues. Click here to read the report and view the active poll."*

---

## 6. THE TAURI DESKTOP APPLICATION

### 6.1 App Structure Overview

```
Sidebar Tabs:
  ├── 🏠 Home (Featured & Trending)
  ├── 🔍 Browse (Curated Mod/Pack/Shader/etc. discovery)
  ├── 📦 My Instances
  ├── 🗳️ Community Governance (Triage Center)
  └── ⚙️ Settings

Main Content Area: Dynamic based on sidebar selection

### 6.1a First-Run / Onboarding Flow

On the app's first launch (or if `registry.db` has never been downloaded):

1. **Welcome Screen:** Displays the "Agora" mission statement and project ethos. A "Get Started" button proceeds.

2. **Integration Configuration (Optional — Default Disabled):**
   Before any other setup, the user is presented with a clean, minimal screen titled **"Connect External Services"** with the subtitle *"These are completely optional. You can change your mind at any time in Settings."*

   | Integration | Default | Description |
   |---|---|---|
   | **Modrinth Access** | `OFF` | Enable live Modrinth API queries for the raw Modrinth tab and version fallback for Modrinth-sourced mods. No Modrinth API calls are made when disabled. |
   | **AI / MCP Server** | `OFF` | Enable the local MCP server so external AI tools (Claude, Cursor, Ollama) can connect with a per-session token. No AI features run when disabled. |

   - Each integration has a large toggle switch with a short one-sentence description.
   - **Both toggles default to OFF.** The user must explicitly enable them.
   - A "Continue" button proceeds regardless of toggle state.
   - These preferences are persisted to `tauri-plugin-store` immediately.
   - **Rationale:** The platform's core philosophy is sovereignty and zero-mandatory-external-deps. Requiring opt-in for every non-essential service respects user agency and prevents accidental data leakage to third parties.

3. **GitHub OAuth Setup:** The GitHub Device Flow is initiated. A code and verification URL are displayed.
   - The user can click **"I'll do this later"** to skip OAuth. The app enters **Browse-Only Mode**.
   - In Browse-Only Mode: the user can browse the curated catalog and assemble modpacks, but cannot vote, submit reviews, report crashes to GitHub, or use the Triage Center.
   - A profile icon in the top-right corner shows a "Sign in with GitHub" badge. Clicking it resumes the Device Flow.

4. **Database Download:** If online, the app queries the GitHub Releases API for the latest `registry-*` tag, downloads the attached `registry.db` and `registry.db.sig`, verifies the Ed25519 signature, and loads it into `tauri-plugin-sql`. If offline, the app uses the last cached DB or shows "Limited Offline Mode."

5. **Optional Tutorial:** A brief interactive overlay highlights the sidebar tabs (Home, Browse, My Instances, Community Governance, Settings).

6. **Home Screen:** The user lands on the Home tab showing "Featured Platform Packs."

### 6.1b Integration Configuration UI (Settings → Integrations)

After onboarding, users can revisit and modify their integration preferences at any time via **Settings → Integrations**. This panel mirrors the onboarding screen exactly:

```
┌─────────────────────────────────────────────────────────────────┐
│  ⚙️ Settings  >  Integrations                                    │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  External Services                                               │
│  ─────────────────────────────────────────────────────────────  │
│                                                                  │
│  █  Modrinth Access                                              │
│     ┌────────────────────────────────────────────────────────┐  │
│     │ Allow live Modrinth API queries. Enables the Raw      │  │
│     │ Modrinth tab and version fallback for Modrinth-sourced │  │
│     │ curated mods. When off, Modrinth-sourced curated mods │  │
│     │ are hidden from Browse and search results entirely.   │  │
│     │ No Modrinth calls are made when off.                  │  │
│     └────────────────────────────────────────────────────────┘  │
│     [Toggle:  OFF  |  ON]                                       │
│                                                                  │
│  █  AI / MCP Server                                              │
│     ┌────────────────────────────────────────────────────────┐  │
│     │ Enable local MCP server for external AI tools to      │  │
│     │ diagnose crashes and search mods. Generates a         │  │
│     │ per-session token. No AI features run when off.       │  │
│     └────────────────────────────────────────────────────────┘  │
│     [Toggle:  OFF  |  ON]                                       │
│                                                                  │
│  Current token: mcp://localhost:39741?token=...                 │
│  [Regenerate Token]  [Copy to Clipboard]                        │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

**Toggle Behavior:**
- **Modrinth Access `OFF → ON`:** The Raw Modrinth sidebar tab immediately appears. Modrinth-sourced curated mods reappear in Browse and search results.
- **Modrinth Access `ON → OFF`:** The Raw Modrinth tab disappears immediately. **All curated mods with `download_strategy = 'modrinth_id'` are filtered out of Browse, search, and category queries via SQL:**
  ```sql
  -- When Modrinth is disabled, append to every catalog query:
  WHERE download_strategy != 'modrinth_id'
  ```
  In-flight Modrinth downloads are allowed to finish but no new Modrinth API calls are initiated. Installed Modrinth-sourced mods in existing instances continue to work — they are already on disk.
- **AI / MCP `OFF → ON`:** The MCP server starts on an ephemeral port. The token and URL are displayed. A system tray / OS notification says: *"MCP Server running on port XXXX. Share the token with your AI client."*
- **AI / MCP `ON → OFF`:** The MCP server shuts down immediately. All active AI client connections are terminated. The token is invalidated. The UI shows: *"MCP Server stopped. Previous token is no longer valid."*

**Browse-Only Mode Limitations (Updated):**

| Feature | With OAuth | Browse-Only |
|---|---|---|
| Browse curated catalog | ✅ | ✅ |
| Install curated packs (GitHub-sourced) | ✅ | ✅ |
| Install curated packs (Modrinth-sourced, if Modrinth ON) | ✅ | ✅ |
| Download raw Modrinth mods (if Modrinth ON) | ✅ | ✅ |
| Create custom instances | ✅ | ✅ |
| Use MCP AI tools (if AI ON) | ✅ | ❌ |
| Vote on mods | ✅ | ❌ |
| Submit reviews | ✅ | ❌ |
| Access Triage Center | ✅ | ❌ |
| Report crashes to GitHub | ✅ | ❌ |
| Flag comments | ✅ | ❌ |

All features work offline once `registry.db` is cached, except those requiring live API calls. Integration toggles are independent of OAuth state — a user can have Modrinth ON and AI OFF, or vice versa, in Browse-Only Mode.
```

### 6.2 Discovery & Sorting System

All sorting and filtering is executed locally against the downloaded SQLite database. No API call is needed.

**Sort Options (with Modrinth filter when disabled):**
- Net Score (default): `SELECT * FROM registry_items WHERE download_strategy != 'modrinth_id' ORDER BY net_score DESC`
- Trending (7-day velocity): `SELECT * FROM registry_items WHERE download_strategy != 'modrinth_id' ORDER BY velocity DESC`
- Most Downvoted: `SELECT * FROM registry_items WHERE download_strategy != 'modrinth_id' ORDER BY downvotes DESC`
- Newest additions: `SELECT * FROM registry_items WHERE download_strategy != 'modrinth_id' ORDER BY date_added DESC`
- Most upvoted: `SELECT * FROM registry_items WHERE download_strategy != 'modrinth_id' ORDER BY upvotes DESC`

When Modrinth integration is enabled, the `WHERE download_strategy != 'modrinth_id'` clause is omitted and all mods are shown.

**Category Filter (with Modrinth filter when disabled):**
```sql
SELECT ri.*
FROM registry_items ri
JOIN item_categories ic ON ri.id = ic.item_id
WHERE ic.category_id = 'tech'
  AND ri.download_strategy != 'modrinth_id'
ORDER BY ri.velocity DESC;
```

**Minecraft Version / Loader Filter:**
When a user has selected a Minecraft version and loader (either in an active instance or via filter dropdown), the Browse tab shows only mods whose `compatible_versions_json` includes that combination. For Modrinth-sourced mods, the compiler populates this field from the Modrinth API. For GitHub-sourced mods, it is extracted from release filenames, descriptions, or an optional `version_info.json` attachment.

Categories in the filter menu are dynamically generated from the `categories` table, including all freeform community tags. Users can filter by `"tech"`, `"create-addons"`, `"cozy-rpg"`, `"client-only"`, or any other tag that curators have used in `community_categories`.

**"For You" Algorithm:**
The Tauri app tracks which content_types and categories the user installs locally. If a user installs three mods tagged `"magic"`, the local UI boosts uninstalled mods sharing the `"magic"` category or with matching linguistic profiles in `curator_note`. This runs entirely on the user's machine; zero data leaves the device.

#### Mod Detail Page: Version Picker

When a user opens a mod's detail page from the Browse tab:
- The mod's versions are queried from the source API (Modrinth or GitHub) **live** if the user is online, or from cached version metadata in `registry.db`.
- Versions are **filtered by the user's currently selected Minecraft version and loader** (from their active instance or global filter).
- The default selected version is the latest compatible one.
- The user can pick a different compatible version from the dropdown.
- For GitHub releases, versions are sorted by release date. The launcher's best-guess compatibility (from filename/description/`version_info.json`) is shown next to each version.
- A "Version Mismatch Warning" appears if the user selects a version that the launcher cannot confirm is compatible with the active instance's MC version/loader.

### 6.3 The "Boutique vs. Warehouse" UX Split

- **Default View ("The Boutique"):** Only items from the curated `registry.db` are shown. No MCreator slop, no abandoned weekend projects. If a user searches for something and it isn't in the curated list, the UI shows a subtle message: *"Not finding what you need? Search all of Modrinth →"*
- **Raw Modrinth Tab:** Clicking the above button (or selecting the Modrinth tab in the sidebar) flips to a live Modrinth API search. The launcher downloads mod files directly from Modrinth's CDN and **verifies each file against the SHA-1 hash published in Modrinth's API** before writing it to the instance. An uncurated mod is structurally unvetted by this community, but the file itself is integrity-checked against Modrinth's own records. This section displays a persistent warning banner: *"⚠️ These mods are uncurated by the community. Download at your own discretion."*

**Modrinth Integration Toggle:** In Settings → Integrations, users can disable all Modrinth API access entirely. When disabled:
- The "Search all of Modrinth →" link is hidden.
- The Raw Modrinth sidebar tab is removed.
- Curated mods with `download_strategy = "modrinth_id"` show a warning: *"This mod is hosted on Modrinth. Modrinth integration is disabled. Enable it in Settings to install, or download the file manually and drag it into the instance."*
- All other features (GitHub-sourced curated mods, pack management, instance creation, governance) continue to work normally.

### 6.4 Extended Content Types

The same manifest system, data pipeline, and discovery algorithms support:
- `mod` — Standard Minecraft modifications
- `pack` — Curated modpacks (see §7)
- `shader` — Iris/OptiFine-compatible shader packs
- `resourcepack` — Texture/resource replacements
- `server` — Listed public servers running curated packs
- `datapack` — Vanilla-compatible data modifications
- `world` — World downloads/saves

### 6.5 Instance Management

The app supports unlimited independent instances, each completely isolated from the user's default `.minecraft` directory.

**Instance Directory Structure:**
```
~/your-app/instances/
  optimized-survival/
    mods/
    config/
    crash-reports/
    logs/
    saves/
    screenshots/
    instance_manifest.json   # Lightweight JSON manifest tracking all installed mods
  creative-builds/
    ...
```

**`instance_manifest.json` Schema:**
Each instance directory contains a lightweight JSON file tracking every mod installed in the `mods/` folder:

```json
{
  "instance_id": "optimized-survival",
  "name": "Optimized Survival",
  "created_from_pack": "optimized-survival",
  "minecraft_version": "1.21",
  "loader": "fabric",
  "loader_version": "0.15.11",
  "is_locked": true,
  "mods": [
    {
      "filename": "sodium-fabric-0.5.8+mc1.21.jar",
      "registry_id": "sodium",
      "source": "github_release",
      "version": "v0.5.8",
      "sha256": "a1b2c3...",
      "installed_at": "2026-06-15T14:30:00Z"
    },
    {
      "filename": "xaeros-minimap-24.2.0-fabric-1.21.jar",
      "registry_id": null,
      "modrinth_id": "1bokaNcj",
      "source": "modrinth_raw",
      "version": "24.2.0",
      "sha256": "d4e5f6...",
      "installed_at": "2026-06-15T14:31:00Z"
    },
    {
      "filename": "random-mod.jar",
      "registry_id": null,
      "modrinth_id": null,
      "source": "manual_drag_drop",
      "version": null,
      "sha256": "g7h8i9...",
      "installed_at": "2026-06-16T09:00:00Z"
    }
  ],
  "user_preferences": {
    "recommended_mods_enabled": ["iris"],
    "optional_mods_enabled": ["xaeros-minimap"]
  }
}
```

**Design Rationale:**
- **JSON over SQLite for instance metadata:** Avoids SQLite merge complexity when updating `registry.db`. The instance manifest is a standalone file that can be read, written, and exported without database joins.
- **Reconstructible from scratch:** Given only the `instance_manifest.json`, the launcher can rebuild the entire instance (re-download all mods, re-inject loader, re-apply configs) on any machine.
- **Lightweight backups:** Backing up an instance means copying `instance_manifest.json` + any custom configs. The `mods/` directory itself can be regenerated.
- **Cross-tool compatibility:** The `.mrpack` export format is generated by transforming `instance_manifest.json` into `modrinth.index.json`.

**`source` field values:** `github_release`, `modrinth_id`, `modrinth_raw`, `direct_hash`, `manual_drag_drop`, `curated_pack`.

**Modpack Lock State Machine:**
```
[LOCKED (Default for curated packs)]
  - UI hides "Add Mod" button
  - Manifest is immutable; shows only installed mod list
  - "Unlock" button is visible with a warning tooltip
        │
        ▼ (User clicks Unlock → confirms warning)
[UNLOCKED]
  - Rust copies instances/<pack>/ to instances/<pack>_backup/ recursively, then verifies that the file count and total size match the source. If verification fails, display: *'Backup creation failed. Unlock operation cancelled to prevent data loss.'*
  - Note: directory copies are not atomic; this is a best-effort safeguard.
  - UI exposes raw Modrinth search and manual .jar drag-and-drop
  - "Revert to Original" button appears
        │
        ▼ (User clicks Revert)
[REVERT]
  - Rust deletes instances/<pack>/ and renames instances/<pack>_backup/ back
  - Instance is perfectly restored to the curator's original state
```

### 6.5a Pack Updates

When a curated pack manifest is updated in the registry (new mod versions added, config changes, dependency fixes), existing instances created from that pack can be updated:

1. The instance settings shows a **"Check for Pack Update"** button.
2. The launcher reads `instances/<pack>/instance_manifest.json` to determine the current state of installed mods (filename, version, hash).
3. The launcher compares the manifest against the current pack definition in `registry.db` to compute a diff.
4. If differences exist, a diff dialog appears:
   - *"3 mods updated (Sodium 1.7.1 → 1.7.2, Iris 1.6.0 → 1.6.1, ...)*
   - *"1 mod added (NEWMOD)*
   - *"2 mods removed (OLDMOD1, OLDDMOD2)*
5. The launcher respects the user's previous optional/recommended choices stored in `instance_manifest.json` under `user_preferences`.
6. **If the instance was unlocked and has manual mods added:** the diff also shows these as *"3 manual mods not in pack (will be preserved)"*. They are not removed during the update.
7. Before applying the update, the launcher automatically creates a backup: copies `instances/<pack>/instance_manifest.json` to `instances/<pack>_backup_update_YYYYMMDD/instance_manifest.json` plus any custom config files.
8. The user can review and approve the update, or dismiss it.
9. If approved: download new/changed mods, remove deleted mods, update `instance_manifest.json`, re-extract any changed override config files.
10. After update, if the instance was originally LOCKED, it remains LOCKED. If it was UNLOCKED before the update, it remains UNLOCKED.

### 6.5b Custom Instance Creation

Users can create instances from scratch (not just from curated packs or `.mrpack` files):

1. **Create Instance** button opens a dialog.
2. User selects: Minecraft version (dropdown), loader (Fabric/NeoForge/Quilt/Forge), loader version (auto-populated from `loader_manifests.json`).
3. Instance is created empty with the correct modloader injected.
4. User can browse the curated registry and add mods individually.
5. For each mod, the version picker shows versions compatible with the instance's MC version/loader.
6. The instance is treated as an unlocked pack — mods can be freely added/removed.

### 6.5c Pack Export

Users can export any existing instance as a shareable pack:

1. **"Export Pack"** button in instance settings.
2. User chooses export format:
   - **`.mrpack`**: Standard Modrinth format. Generates `modrinth.index.json` with all installed mod hashes and URLs. Compatible with any Modrinth launcher.
   - **Custom `.json`**: Platform-native pack manifest compatible with this launcher. Includes mod IDs, versions, and optional/recommended status derived from the instance's current state.
3. User can optionally include config overrides (generates a zip of `config/`, `kubejs/`, etc. that's been modified from defaults).
4. The exported file is typically 5-20KB and can be shared over Discord or any platform.

### 6.5d Instance Deletion

1. Right-click on instance card → "Delete Instance" or settings menu → "Delete Instance".
2. Confirmation modal: *"This will permanently delete the instance directory including all mods, configs, and saves. This cannot be undone."*
3. On confirmation, the instance directory is moved to the OS trash/recycle bin (using platform-specific APIs like Windows `SHFileOperation` or macOS `NSWorkspace`) rather than immediately deleted.
4. User can recover from trash if accidental.

---

## 7. MODPACK ARCHITECTURE (THREE-TIER)

### Tier 1: Curated Platform Packs (Central Repo)
Community curators submit `registry/packs/*.json` manifests via GitHub PR. Because mods are resolved via Modrinth ID or the existing mod manifest database, the pack itself is a tiny JSON file — no files are hosted by the platform. These appear on the launcher's home screen as "Featured Platform Packs."

### Tier 2: Native Modrinth .mrpack Support
1. User searches in the Modrinth tab (e.g., "Fabulously Optimized").
2. App hits the Modrinth API, downloads the lightweight `.mrpack` zip file. Rust verifies the `.mrpack` file integrity against the SHA-1 hash provided by the Modrinth API (if available) or the download response.
3. Rust unzips the `modrinth.index.json` and parses the file list. For each file entry, it extracts the SHA-1 hash from the index and verifies the downloaded `.jar` against that hash before writing it to the instance directory.
4. Rust concurrently downloads all `.jar` files via Modrinth CDN.
5. No platform hosting required.

### Tier 3: Local Offline Sharing
- A "Build Pack" UI lets users select their own installed mods and export a standard `.mrpack` or the platform's custom `.json` format.
- The resulting file is typically 5-15KB and can be shared over Discord or any other platform.
- Friends drag-and-drop the file into the launcher, and it builds the instance entirely locally.

### 7.1 Pack Installation Flow (Full)

1. **Parse Manifest:** Read the pack's `mods` array.
2. **Present Configuration Screen:** Show required mods (locked + checked), recommended mods (pre-enabled toggle), and optional mods (pre-disabled toggle). Remember user preferences across pack updates.
3. **Resolve Mod Versions:** For each selected mod, resolve the exact file to download:
   - **Primary path (`github_release`):** Query the GitHub Releases API for `owner/repo` and match the exact release tag specified in `version`. If no `version` is specified, use the latest release. Verify the release asset SHA-256 hash against the pinned hash in `registry.db`.
   - **Supplementary path (`modrinth_id`):** Query the Modrinth API for the exact version. If the exact version is not found, show a dialog offering the closest compatible version or a skip option. If no `version` is specified, query `GET /v2/project/{id}/version?loaders=["fabric"]&game_versions=["1.21"]` for the latest compatible.
   - **Direct path (`direct_hash`):** Download from the provided URL and verify SHA-256.
   - For GitHub releases without a `version_info.json`, the launcher searches release assets by filename pattern (e.g., `*1.21*.jar`) and release description as a best-effort compatibility guess.
   - The resolved version ID, file URL, and hash are cached in the instance metadata for future reference (updates, crash reporting).

4. **Fetch Mods:** Concurrently download all resolved mod files. See §7.1.1 for download failure handling.
5. **Fetch Overrides:** Download `override_url` zip from GitHub raw CDN.
6. **Sanitize Overrides (CRITICAL — see §7.2).**
7. **Extract Config Files:** Write `.toml`, `.json`, `.properties`, `.png`, `.mcmeta`, `.js` (for KubeJS scripts) to the instance directory.
8. **Inject Modloader (see §8.2).**
9. **Mutate `launcher_profiles.json` (see §8.3).**
10. **Launch.**

#### 7.1.1 Download Failure Handling (Partial Pack Load)

If one or more mod downloads fail during pack installation (network error, file not found, hash mismatch, Modrinth API error), the launcher **does not abort the entire installation**. Instead:

1. Continue downloading and installing all other mods that succeed.
2. After all downloads complete, present a summary dialog: *"<N> mods installed successfully. <M> mods failed to download:"* followed by a list of failed mods with their error reasons.
3. For each failed mod, the user has three options:
   - **Search Modrinth**: Query the Modrinth API directly for the mod by name/ID and show available versions filtered by the instance's `minecraft_version` and `loader`.
   - **Check Registry**: Search the local `registry.db` for a matching mod entry with an alternative download strategy.
   - **Install Manually**: Open a file picker to let the user drag-and-drop a `.jar` file they downloaded themselves.
4. If the user dismisses the dialog without resolving all failures, the instance is created but flagged as *"Incomplete Installation"* in the UI. A persistent banner appears in the instance settings: *"This instance is missing <N> mods. Some features may not work correctly."*

**Concurrency & Retry Settings:**
- Maximum 6 concurrent download streams.
- Per-file retry: 3 attempts with exponential backoff (1s, 2s, 4s delays).
- If a file fails all retries, it moves to the failure summary.
- Total download timeout per file: 60 seconds.

#### 7.1.2 Disk Space Pre-Check

Before starting any download, extraction, or instance creation:
- Sum the expected download sizes (from API metadata or user-provided `.mrpack` index).
- Add a 20% headroom buffer for extraction overhead.
- Check available disk space on the target drive.
- If insufficient: *"This installation requires ~X GB of free space. You have Y GB available. Please free up space or reduce your selection."*

### 7.2 Override Sanitization Engine (Security Critical)

The Rust backend acts as a strict gatekeeper when extracting any zip file from a pack's `override_url`.

#### 7.2.1 Zip Bomb Mitigation (Pre-Extraction Checks)

Before any file is extracted, the Rust backend enforces hard limits on the zip archive:

```rust
const MAX_ZIP_SIZE: u64 = 500 * 1024 * 1024;       // 500MB compressed
const MAX_EXTRACTED_SIZE: u64 = 2 * 1024 * 1024 * 1024; // 2GB total extracted
const MAX_FILE_COUNT: usize = 5000;                  // 5000 files max

// Before extraction:
// 1. Check compressed size of the zip against MAX_ZIP_SIZE
// 2. Iterate all entries and sum the declared uncompressed sizes against MAX_EXTRACTED_SIZE
// 3. Count entries against MAX_FILE_COUNT
// 4. If any limit is exceeded: abort, delete zip, display fatal error
```

Zip bombs (e.g., a 10KB file expanding to 500GB) are structurally defeated by checking declared entry sizes before writing a single byte to disk. If the zip header is lying, the extraction loop also tracks actual bytes written and aborts mid-stream if the real total exceeds the limit.

#### 7.2.2 Directory Whitelist (Replacing the Denylist)

The previous design used a denylist of dangerous extensions (`.jar`, `.exe`, etc.). This is insufficient — dangerous content can exist in many forms beyond denylisted extensions (e.g., KubeJS scripts that are effectively code, malicious JSON payloads, OpenLoader resources that load additional content).

The launcher instead uses a **directory whitelist**. Only files whose paths start with an allowed prefix are extracted; everything else is silently skipped and logged:

| Allowed Prefix | Purpose |
|---|---|
| `config/` | Mod configuration files |
| `defaultconfigs/` | Default server-side configs |
| `resourcepacks/` | Embedded resource packs |
| `kubejs/` | KubeJS scripts (sandboxed inside Java; no OS access) |
| `mods/` | Not allowed via overrides — see below |

**The `mods/` directory is explicitly excluded from the whitelist.** This structurally enforces the "Manifest Only" guarantee: all `.jar` files must enter the `/mods/` folder through the JSON manifest array and Modrinth API, never through pack overrides.

Instead of a denylist (where anything not explicitly banned passes through), the whitelist ensures that only known-safe directory patterns are permitted. Unknown or unexpected paths are rejected by default.

Additionally, within whitelisted directories, certain file types remain hard-banned regardless:
```
.jar, .class, .exe, .bat, .cmd, .sh, .ps1, .dll, .so, .dylib, .msi, .dmg
```

If any file with a banned extension is found inside the zip — even within a whitelisted directory — the installation is aborted, partially-extracted files are deleted, and the user sees:

*"Installation Aborted: Security Violation. Pack overrides cannot contain executable files or mods. All mods must be routed through the platform manifest. This is a platform security requirement."*

#### 7.2.3 Path Traversal Protection (Zip Slip)

Before writing any extracted file to disk:
- Strip all absolute path prefixes and `../` parent directory operators from the file's stored path.
- Verify that the resolved canonical path is inside the designated instance directory sandbox.
- If any path escapes the sandbox: abort the entire installation.

#### 7.2.4 The "Manifest Only" Guarantee

By enforcing the directory whitelist plus the executable extension ban, the platform structurally guarantees that the only way a `.jar` file enters the user's `/mods/` folder is if it was defined in the JSON manifest array, passed through Modrinth's API (respecting developer permissions and download statistics), and verified against its SHA-256 hash.

#### 7.2.5 Why `.sh` and `.bat` Are Banned

- Shell scripts can download and execute remote payloads at runtime. A curator might audit a "safe" script today, but a bad actor can change the remote URL's content after approval.
- `.sh` files break Windows users; `.bat` files break macOS/Linux users. Cross-platform packs must not rely on OS-level scripts.
- The correct solution for in-game scripting is **KubeJS** or **CraftTweaker** — mods that run `.js` scripts strictly inside the Java sandbox, with no OS-level access.

---

### 7.5 GITHUB OAUTH TOKEN SECURITY

The platform relies heavily on user GitHub OAuth tokens for API calls, voting, crash reporting, and governance. These tokens are a high-value target — if stolen, they grant the attacker the ability to impersonate the user on GitHub, create issues, vote, and potentially access private repositories depending on granted scopes.

#### 7.5.1 Authentication Flow

The launcher uses the **GitHub Device Flow** (`POST https://github.com/login/device/code`) for authentication. This flow is designed for desktop applications that cannot securely embed a client secret:

1. The launcher requests a device code and verification URL from GitHub.
2. The user opens the URL in their browser and enters the code.
3. The launcher polls for authorization. Once the user approves in the browser, the launcher receives an access token.

**Token Scopes:** Only the minimum required scopes are requested:
- `public_repo` — to create issues (crash reports) and interact with public repositories
- `read:org` — for organization membership checks (if applicable for governance)

No `repo` (full private access), `user`, `admin`, or other broad scopes are ever requested.

#### 7.5.2 Token Storage

The OAuth token is **never stored in plaintext** on the filesystem. Instead, it is secured using the operating system's native credential store:

| OS | Credential Store |
|---|---|
| Windows | Windows Credential Manager (via `keyring` crate) |
| macOS | Keychain (via `keyring` crate) |
| Linux | Secret Service (via `keyring` crate) |

The `keyring` Rust crate provides a cross-platform abstraction over these stores. The token is stored as a single entry (e.g., `io.agora-mc.github-token`) and is never written to configuration files, environment variables, or local SQLite databases.

**Degraded Security Fallback:** If the keyring is unavailable (common on headless Linux, WSL, or minimal distributions without D-Bus/Secret Service), the launcher falls back to local encryption:
- Derive a key from the OS username + machine ID (e.g., Windows SID, Linux machine-id, macOS hardware UUID) using PBKDF2.
- Encrypt the token with AES-256-GCM and store the ciphertext in the app's local data directory (`%APPDATA%/agora-mc/tokens.enc` on Windows, `~/.config/agora-mc/tokens.enc` on Linux, `~/Library/Application Support/agora-mc/tokens.enc` on macOS).
- Display a persistent warning in Settings: *"Credential store unavailable. Token encrypted with a machine-bound key. This is less secure than OS keychain storage."*
- If the machine ID changes (e.g., OS reinstall, VM migration), the encrypted token becomes unreadable and the user must re-authenticate.

#### 7.5.3 Token Usage Principles

- **User's own rate limit:** Each user's API calls count against their personal 5,000 requests/hour quota. The launcher never makes API calls that count against a shared rate limit.
- **No server-side token storage:** The platform has no backend. Tokens never leave the user's machine.
- **Token revocation:** Users can revoke the launcher's access at any time via GitHub Settings → Applications. A "Disconnect GitHub" button in the launcher's Settings tab also deletes the token from the credential store.

---

## 8. THE EXECUTION ENGINE (MOJANG WRAPPER)

The Rust backend **never touches Microsoft credentials, Xbox Live, XSTS tokens, or JVM execution**. All of this is delegated to the official Mojang Launcher. This eliminates an entire class of security liability, auth-chain complexity, and future API breakage risk.

### 8.1 OS-Specific Path Resolution

Rust must resolve the Minecraft directory dynamically at runtime:

| OS | Path |
|---|---|
| Windows | `%APPDATA%\.minecraft` |
| macOS | `~/Library/Application Support/minecraft` |
| Linux | `~/.minecraft` |

### 8.2 Modloader Injection Flow (Fabric, NeoForge, Quilt, Forge)

To make the Mojang launcher understand a modded version:

1. Rust downloads the modloader version JSON from the official CDN (e.g., Fabric Meta API: `https://meta.fabricmc.net/v2/versions/loader/<mc_version>/<loader_version>/profile/json`).
2. Rust writes this file to `~/.minecraft/versions/<loader_name>/<loader_name>.json`.
3. Rust downloads all required modloader library JARs and writes them to `~/.minecraft/libraries/`.

Supported loaders (MVP scope):
- **Fabric** (via Fabric Meta API)
- **NeoForge** (via NeoForge Maven installer)
- **Quilt** (via Quilt Meta API)
- **Forge** (via MinecraftForge installer)

#### 8.2.1 Supply Chain Verification (Modloader Downloads)

Downloading modloader metadata and libraries from internet sources introduces a supply chain risk — if an upstream CDN is compromised, malicious code reaches every user.

**Countermeasures:**

1. **SHA-256 Hash Verification (Version JSON Only):** The nightly compiler fetches and pins the SHA-256 hash of the modloader **version JSON file only** (e.g., `fabric-loader-0.15.11-1.21.json`). The Rust client verifies this JSON file against the pinned hash before trusting its contents. The library list inside the JSON is trusted as the official modloader build output. The pinned hashes are stored in `loader_manifests.json` (see point 3 below). If verification fails, the download is rejected and the user is shown: *"Modloader download failed integrity check. This may indicate a network error or a supply chain compromise. Please try again or report this issue."*

2. **Domain Pinning:** The Rust client only downloads modloader files from the official, hard-coded CDN domains:
   - Fabric: `meta.fabricmc.net`, `maven.fabricmc.net`
   - NeoForge: `neoforged.net`, `maven.neoforged.net`
   - Quilt: `meta.quiltmc.org`, `maven.quiltmc.org`
   - Forge: `minecraftforge.net`, `files.minecraftforge.net`
   
   Any redirect to an off-domain URL is blocked. This prevents DNS hijacking or MITM attacks from redirecting downloads to attacker-controlled servers.

3. **Pinned Source Lists:** The compiler maintains a `loader_manifests.json` file in the registry mapping each supported `(loader, mc_version, loader_version)` tuple to its official source URL and pinned hash. This file is updated only via curator PR — the same governance process as mod manifests.

### 8.3 `launcher_profiles.json` Mutation

Rust reads the official `~/.minecraft/launcher_profiles.json`, then injects a new profile entry:

```json
"curated-optimized-survival": {
  "name": "Optimized Survival (Agora)",
  "type": "custom",
  "created": "<ISO timestamp>",
  "lastVersionId": "fabric-loader-0.15.11-1.21",
  "icon": "Furnace",
  "gameDir": "/home/user/your-app/instances/optimized-survival",
  "javaArgs": "-Xmx8G -Xms8G -XX:+UseG1GC -XX:+UnlockExperimentalVMOptions -XX:+AlwaysPreTouch"
}
```

Note: `javaArgs` is dynamically assembled from the JVM Argument Builder (§8.4). Note that `gameDir` points to the **isolated instance folder**, not the default `.minecraft` directory.

#### 8.3.1 Atomic Write with Backup

Directly mutating `launcher_profiles.json` is risky — a crash, power loss, or write interruption during mutation could corrupt the file, potentially destroying all of the user's launcher profiles.

**Atomic Write Procedure:**

1. Read the current `launcher_profiles.json` into memory.
2. Parse the JSON and inject/update the profile entry.
3. Serialize the mutated JSON back to a string.
4. Write the serialized string to a **temporary file**: `launcher_profiles.json.tmp`.
5. Create (or overwrite) a **backup file**: `launcher_profiles.json.bak` from the current on-disk file.
6. Use `std::fs::rename()` to atomically replace `launcher_profiles.json` with the `.tmp` file. On most filesystems, `rename()` is atomic — the file is either the old version or the new version, never a partial write.
7. If any step fails, the `.bak` file is used to restore the original state.

**Recovery:** If the launcher detects that `launcher_profiles.json` is invalid JSON on startup:
1. Attempt to restore from `launcher_profiles.json.bak`.
2. If `.bak` is also invalid or missing, **regenerate a minimal valid `launcher_profiles.json`** containing only the curated launcher profiles managed by this app. Display a warning: *"`launcher_profiles.json` was corrupted and has been regenerated with your curated profiles. You may need to re-add any manually-created Mojang launcher profiles."*
3. The regenerated file is a valid JSON object with empty `profiles` and `settings` objects plus any curated profiles stored in the app's local database.

### 8.4 Process Execution

**OS-Specific Launcher Discovery:**
Rust resolves the Mojang launcher executable path at runtime using the following priority:

1. **User override:** If set in Settings, use the user-provided path.
2. **Windows:** Query registry key `HKEY_LOCAL_MACHINE\SOFTWARE\Mojang\Launcher\InstallPath` (legacy) or search `C:\Program Files (x86)\Minecraft Launcher\MinecraftLauncher.exe` and `C:\Program Files\Minecraft Launcher\MinecraftLauncher.exe`. Also check the Microsoft Store app package location via `Get-AppxPackage`.
3. **macOS:** Check `/Applications/Minecraft.app/Contents/MacOS/launcher`.
4. **Linux:** Check `$PATH` for `minecraft-launcher`, then common locations (`/usr/bin/minecraft-launcher`, `/opt/minecraft-launcher/minecraft-launcher`, `~/.local/bin/minecraft-launcher`).
5. **Fallback:** If no executable is found, display an error: *"Minecraft Launcher not found. Please install the official Mojang Launcher or set its path in Settings."*

The resolved path is cached in the app's local configuration for subsequent launches.

```rust
// Rust backend — uses cached path if available, otherwise discovers
let launcher_path = config.get("mojang_launcher_path")
    .unwrap_or_else(|| discover_mojang_launcher());
std::process::Command::new(&launcher_path)
    .arg("--profile")
    .arg("curated-optimized-survival")
    .spawn()?;
```

The Mojang launcher opens, sees the user is already logged in (we never touched auth), reads the modloader `lastVersionId`, downloads any missing vanilla assets it needs, points its read directory at the isolated instance folder, and boots the game.

### 8.5 JVM Argument Builder (UI → Args)

**Memory Slider:**
- User drags a slider (e.g., 1GB – 32GB based on detected system RAM).
- Rust translates: a selection of `8GB` → `-Xmx8G -Xms8G` (setting min and max to the same value is best practice for modded Minecraft; prevents GC pressure from heap resizing).

**Garbage Collector Dropdown:**
| UI Label | JVM Flag | Notes |
|---|---|---|
| Default (G1GC) | `-XX:+UseG1GC` | Standard for all modern Minecraft; recommended for most users |
| ZGC (Low Latency) | `-XX:+UseZGC` | Eliminates lag spikes on high-end machines; higher memory overhead |
| Shenandoah | `-XX:+UseShenandoahGC` | Alternative low-pause collector; good for midrange hardware |
| Custom | *(empty)* | User manually enters GC flag in the text box below |

**Custom JVM Arguments Text Box:**
- A plain text input at the bottom of the JVM settings panel.
- Appended to the memory and GC args in the final `javaArgs` string.
- Useful for Aikar's flags, server-mode tweaks, or advanced optimization presets.

**Final Assembly:**
```
[Memory Args] + " " + [GC Args] + " " + [Custom Args] + " " + [AlwaysPreTouch if enabled]
```

`-XX:+AlwaysPreTouch` is included by default for G1GC, but is a toggle in Settings (default: ON for G1GC, OFF for ZGC/Shenandoah). Tooltip: *'Pre-touches all heap pages at startup, increasing launch time but reducing in-game stutter. Recommended for G1GC; may cause issues with ZGC on memory-overcommitted systems.'*

---

## 9. CRASH DIAGNOSTICS SYSTEM

### 9.1 Pre-Launch Crash Interceptor

When the user clicks "Play":

1. Rust checks `instances/<pack>/crash-reports/` for any files with a modification timestamp newer than `last_launched_at` (stored in `user_instances` table).
2. **If a new crash report exists:**
   - Halt the launch.
   - Display a modal: *"⚠️ This instance crashed during its last session. Do you want to review the crash report before launching, or proceed anyway?"*
   - Options: **[Review Crash]** | **[Launch Anyway]**
3. If user clicks "Review Crash": open the Crash Diagnostic View (§9.2).
4. After any crash review, update `last_launched_at` to now.

**Timing Rule:** `last_launched_at` is updated to the current timestamp **immediately before launching the Mojang launcher process**, regardless of whether crash review was performed. This prevents infinite crash prompt loops when the user clicks "Launch Anyway."

### 9.2 Community Regex Instant Triage (Layer 1)

The crash log text is streamed through the local `crash_signatures` table in order of specificity:

```rust
for signature in db.query("SELECT * FROM crash_signatures") {
    if Regex::new(&signature.regex_pattern)?.is_match(&crash_text) {
        // Display solution_markdown to user
        // Render action_button if present (e.g., "Install Fabric API" auto-installs the mod)
        return Triage::Resolved(signature);
    }
}
// If no match: escalate to AI (Layer 2) or manual review
```

If a match is found, the user sees a plain-English popup with a one-click "Fix It" button. Zero latency. Zero AI tokens consumed.

The `crash-signatures/` folder is a community-maintained repository. Anyone can submit a new crash pattern via PR. Curators verify the regex works before merging.

### 9.3 Manual Log Viewer

Every instance has a **"Diagnostics / Logs" tab** in its settings screen:
- Lists all crash reports and `latest.log` files with human-readable timestamps.
- Rust reads and streams the file content to the React frontend.
- **Syntax highlighting:** `[ERROR]` lines → red, `[WARN]` lines → amber/yellow, `[INFO]` → default, `[DEBUG]` → gray.
- User can select any crash report at any time (not just on pre-launch intercept) and send it to the AI for analysis.

### 9.4 Automated GitHub Issue Submission (Layer 0 — Duplicate Check)

Before either the regex engine or AI runs, Rust checks if the crash is already a known issue:

1. Extract the `Caused by:` block from the crash log using regex.
2. Identify the offending Java package name (cross-referenced against `package_signatures` in the manifest DB).
3. Use the user's GitHub OAuth token to call: `GET /search/issues?q=<offending_package_signature>+repo:<mod_owner>/<mod_repo>`
4. **If existing issue found:** Display a link: *"This crash appears to be a known issue. See [GitHub Issue #123] for the current status and fix."*
5. **If no existing issue found:** Offer the user a "Report This Crash" button.
   - Clicking it **first displays a full preview** of the issue that will be submitted, containing: Java version, OS and architecture, modloader + version, full sorted mod list, and formatted stack trace. The user must explicitly review and approve the content before it is submitted. **The report is never auto-submitted.**
   - This preview step is critical: it shows the user exactly what personal/identifying information (username in file paths, OS details, mod list, hardware hints) will be posted publicly. The user can redact any information they consider sensitive before approving.
   - After user approval, Rust creates a GitHub Issue on the **mod developer's repository** (not the platform repo) using the user's token.

---

## 10. LOCAL AI / MCP SERVER

The Tauri app exposes a local JSON-RPC MCP (Model Context Protocol) server on `localhost`. This is **entirely opt-in.** Users connect their own AI tools (Claude Desktop, Cursor, etc.) or local models (Ollama). No API key is stored by the platform.

### 10.0 MCP Security (Authentication & Authorization)

Without authentication, any local process could connect to the MCP server and execute privileged operations (disable all mods, read crash logs, manipulate installations). This is a **critical** attack surface.

**Mandatory Security Controls:**

1. **Localhost Binding Only:** The MCP server binds to `127.0.0.1` on an OS-assigned ephemeral port. The port is displayed in Settings → MCP Server. The user copies the full URL with token into their AI client.

2. **Per-Session Authentication Token:** When the Tauri app starts the MCP server, it generates a cryptographically random 256-bit token. This token is displayed to the user once in the Settings panel (e.g., `mcp://localhost:PORT?token=...`). The user manually copies this token into their AI client's MCP configuration. Any connection without the correct token is immediately rejected.

3. **User Approval Flow (Capability Permissions):** Destructive operations require explicit user approval. Each request is tracked in an internal `approval_state` enum in the Rust backend:
   - `pending` — Waiting for user decision
   - `approved_once` — Approved for this specific call only; deleted immediately after execution
   - `approved_always` — User selected "Always Allow"; persisted to `local_state.db` in the `mcp_approval_grants` table
   - `denied` — User denied the request; may be persisted if the user selects "Don't ask again for this tool/instance"

   When an AI client calls a destructive tool like `disable_mod` or `enable_mod`, the Rust backend first checks `mcp_approval_grants`:
   - If a matching `approved_always` grant exists for `(tool_name, instance_id)` or `(tool_name, "*")`, the call proceeds without UI interruption.
   - If a matching `denied` grant exists, the call is rejected with `ERR_MCP_DENIED`.
   - Otherwise, the Tauri app displays a native OS notification/modal:
     ```
     [AI Tool Name] requests to: disable_mod("sodium.jar")
     Instance: my-survival-pack
     
     [Allow Once]  [Always Allow]  [Deny]  [Deny & Don't Ask Again]
     ```

   Read-only tools (`read_latest_crash`, `list_instance_mods`, `read_mod_manifest`, `search_knowledge_base`) are auto-approved without a prompt since they expose no destructive capability.

4. **Persistent Approval Grants:** `approved_always` and remembered `denied` grants are stored in `local_state.db.mcp_approval_grants`. This table survives app restarts, preventing the user from being pestered by the same request every time they launch a mod. Each grant can be scoped to:
   - A specific `(tool_name, instance_id)` pair, or
   - A global `(tool_name, "*")` allowing the tool across all instances.

   Users can review and revoke all grants in Settings → Integrations → MCP Server → "Clear Tool Approvals." Grants do not have a forced expiration, but the user can set one via the approval dialog (e.g., "Allow for 7 days").

5. **Pending Approvals Queue:** Destructive tool calls (`disable_mod`, `enable_mod`) awaiting user response are placed in a queue. The Tauri UI displays a single "Pending MCP Approvals" panel showing all queued requests with Batch Approve/Deny options. If the queue exceeds 10 pending requests, the MCP server returns `JSON-RPC error -32002: Too many pending approvals. Please approve or deny existing requests first.` to the AI client.

### 10.1 MCP Tool Definitions

| Tool Name | Signature | Behavior |
|---|---|---|
| `read_latest_crash` | `(instance_id: string)` | Returns the last 200 lines of the newest crash report or `latest.log` for the given instance |
| `list_instance_mods` | `(instance_id: string)` | Reads `instances/<id>/instance_manifest.json` and returns a JSON array of all installed mods with their `filename`, `registry_id` (null if raw/untracked), `source`, `version`, and `sha256`. Curated mods include extra metadata from `registry.db` (curator notes, categories). Raw/untracked mods show basic file info only. |
| `read_mod_manifest` | `(mod_id: string)` | Fetches the community data for a specific mod from the local SQLite DB: curator notes, known dependencies, categories |
| `disable_mod` | `(instance_id: string, mod_filename: string)` | Rust physically renames `mod.jar` → `mod.jar.disabled`. Provides immediate mechanical relief without deleting the file. Reversible. |
| `enable_mod` | `(instance_id: string, mod_filename: string)` | Reverses a `disable_mod` call |
| `search_knowledge_base` | `(query: string)` | Executes a parameterized `LIKE %query%` search against the `curator_note` column in the SQLite DB. The query string is bound as a parameter — never concatenated into the SQL string. Returns the top 3-5 matching items with their curator notes. Powers "vibe-based" semantic mod discovery. |

### 10.2 System Context Injection

When an AI connects to the MCP server, the launcher automatically injects a `system_context.md` file into the AI's context as a hidden system prompt. This turns any generic LLM into a Minecraft modding specialist:

```markdown
You are a Minecraft Modding Triage Expert connected to a curated mod launcher.

CRASH ANALYSIS RULES:
- Always locate and read the `Caused by:` block first. This is the root cause.
- If a Mixin injection failure appears, identify the target class and cross-reference which other mods inject into the same class. Mixin conflicts are the most common crash cause in heavily modded instances.
- Fabric loader errors appear near the top of the log. Forge/NeoForge errors often appear near the bottom.
- If you see `OutOfMemoryError`, the first recommendation is always increasing heap allocation (`-Xmx`), not disabling mods.

TOOL USAGE RULES:
- Always use `list_instance_mods` to see the full environment before diagnosing.
- If you identify an offending mod, use `disable_mod` to provide immediate relief. Never ask the user to manually navigate their file system.
- If you need more context about a mod's known behavior or conflicts, use `read_mod_manifest`.
- If you want to find a mod based on a user's natural language description (e.g., "something that makes caves feel eerie"), use `search_knowledge_base`.

PROHIBITED ACTIONS:
- Never recommend deleting the `.minecraft` folder.
- Never recommend a full reinstall as a first step.
- Never guess mod IDs from memory; always verify against `list_instance_mods`.
```

### 10.3 Token Efficiency Architecture

**Do not** dump the entire `crash_signatures` database into the AI's context. Doing so causes context dilution, inflated token costs, and reduced accuracy.

The efficient approach:
1. The community Regex Engine (Layer 1) runs first. If it matches, AI is never invoked.
2. If no regex match: AI receives **only** three things:
   - The raw crash log filtered to the last 200 lines.
   - The mod list environment from `list_instance_mods`.
   - The 1-page system context prompt (injected by the server automatically).
3. If the AI determines it needs community knowledge, it **calls the tool** `search_knowledge_base(keywords)`. The server returns only the 2-3 most relevant knowledge base entries, not the entire database.

This keeps per-triage token consumption minimal while maintaining expert-level diagnostic accuracy.

### 10.4 Semantic "Vibe" Discovery

`search_knowledge_base` enables natural language mod discovery. A user can ask their connected AI: *"Find me a mod that makes the world feel incredibly lonely and eerie while keeping performance high."*

The AI calls `search_knowledge_base("lonely eerie atmosphere performance")`. The Rust server searches the `curator_note` field using TF-IDF scoring and returns matching mods that a standard keyword tag search would completely miss (since no tag is literally called "eerie"). The AI synthesizes the results into a personalized recommendation.

---

## 11. DEV MODE (CURATOR TOOL)

A dedicated **"Dev Mode"** panel is accessible to users who opt into curator workflows. This is the mechanism by which cutting-edge mods not yet on Modrinth can be compiled, tested, and submitted to the platform.

**Workflow:**
1. Curator enters a GitHub repository URL (e.g., `CaffeineMC/sodium`).
2. Rust clones the repo locally and executes `./gradlew build` (configurable build command with directory offset support).
3. The compiled `.jar` is installed into a test instance.
4. The curator plays/tests the mod.
5. A "Submit to Registry" button pre-populates a PR template with the manifest JSON for the curator to review and finalize before pushing.

### 11.1 Build Sandbox (Critical Security Requirement)

Running `./gradlew build` or any repository build system directly on the host OS is the single highest-risk operation in the entire platform. Build scripts (Gradle, Maven, etc.) execute arbitrary code — a malicious `build.gradle` can download and execute remote payloads, steal files, or install persistent malware. This is not theoretical; it is a common supply chain attack vector.

**Mandatory Build Isolation:**

All Dev Mode builds **must** execute inside an isolated sandbox. The launcher supports the following sandbox backends (selected automatically based on availability, or manually in Settings):

| Sandbox | Platform | Isolation Level | Notes |
|---|---|---|---|
| Docker | All | Container-level | Requires Docker Desktop installed; most convenient |
| Podman | Linux, macOS | Container-level | Rootless; no daemon required |
| Firecracker microVM | Linux | VM-level | Highest isolation; requires KVM |
| WSL2 + Docker | Windows | VM-level | Uses Windows Subsystem for Linux 2 |

**Sandbox Enforcement:**

1. The build command is executed **only** inside the sandbox — never on the host OS.
2. The sandbox has **no network access** by default during build (prevents build scripts from downloading remote payloads). If a build legitimately requires network access (e.g., to fetch Maven dependencies), the curator must explicitly enable network for that build session via a toggle.
3. The sandbox's filesystem is **ephemeral** — it is destroyed after the build completes. Only the output `.jar` file(s) are copied out to the test instance's `mods/` directory.
4. The sandbox has **no access** to the host's home directory, SSH keys, browser profiles, or any other sensitive data.

**Failure Mode:** If no sandbox backend is available, Dev Mode refuses to build and displays: *"Dev Mode builds require a sandbox environment (Docker, Podman, or Firecracker) for security. Please install one to continue."*

---

## 12. ANONYMOUS CRASH TELEMETRY (OPT-IN)

Users who enable telemetry contribute to a global **Crash Matrix** that benefits the entire community.

**Local collection:** Every time the app detects a crash, it records the pair of mods that co-occurred in `local_crash_telemetry`. Pairs are always stored alphabetically (e.g., always `(iris, sodium)`, never `(sodium, iris)`) to prevent duplicates.

**Aggregation:** Once a week, the launcher compresses the local telemetry table and posts it as an anonymous line item to a public aggregation endpoint (e.g., a serverless form endpoint or a public GitHub Gist). The nightly compiler ingests these contributions to produce a global Crash Matrix JSON file.

**User-facing warning:** When a user attempts to install a mod combination that appears in the Crash Matrix with a co-crash rate above a threshold (e.g., 30%+), the launcher shows: *"⚠️ Community data indicates these mods have a high co-crash rate. Proceed with caution."*

**Privacy:** No intentional personal data is collected. Only mod ID pairs and counts are included in telemetry submissions. However, if using GitHub Gists or serverless endpoints as the aggregation point, the receiving service may inherently observe IP addresses, timing metadata, and user agent strings as part of standard HTTP request processing. Users are informed of this nuance in the telemetry opt-in dialog.

---

## 13. WEB DIRECTORY (NEXT.JS)

A completely static Next.js website deployed for free on Vercel or GitHub Pages. It serves as the public face of the platform — a searchable, browsable directory that doesn't require the desktop app.

**Data Source:** Fetches the latest `registry.db` from the GitHub Release asset URL (see §3.1 Step 13). The static Next.js build queries the GitHub Releases API for the latest `registry-*` tag and downloads the attached `registry.db` asset during its own CI build process.

**Features:**
- Full-text search across mod names and curator notes
- Filter by `content_type`, `base_categories`, `community_categories`, Minecraft version, modloader
- Sort by net score, velocity (trending), newest
- Mod detail pages showing: curator note, top community reviews, download strategy, categories, vote counts
- **No user login required.** Read-only public interface.

---

## 14. BUILD EXECUTION PIPELINE (AI AGENT PROMPTS)

Feed these module prompts sequentially to your AI coding agent. Each is self-contained.

### Module 1: The Nightly Compiler (Python)
```
Build a Python script for a GitHub Action that:
1. Reads all JSON files in /registry/mods/, /registry/packs/, and other subdirectories.
2. For github_release mods: queries the GitHub Releases API to fetch release asset metadata including SHA-256 hashes.
3. For modrinth_id mods: queries the Modrinth API (batch endpoint for efficiency) to fetch version metadata, SHA-256 hashes, and icon/gallery URLs.
4. Makes GraphQL/REST calls to the GitHub API to fetch +1/-1 reaction counts on Issues and comment text.
5. Filters reactions by account trust score (age > 30 days, minimum org-scoped interactions).
6. Runs comments through the profanity-check and vaderSentiment Python libraries.
7. Applies velocity anomaly detection: if (recent_downvotes / historical_average) > 5.0 AND total_recent > 20, set status to 'under_review'.
8. Compiles the data into a SQLite database using the schema defined in the architecture spec.
9. Signs the database with an offline Ed25519 key.
10. Deploys the database as a GitHub Release Asset (not a repository commit).
```

### Module 2: The React/Tauri Desktop UI
```
Build a React frontend for a Tauri desktop app. Create a tabbed sidebar with: Home (featured/trending), Browse (curated mods), My Instances, Community Governance (Triage Center), and Settings. The Browse tab fetches and queries a local SQLite database via tauri-plugin-sql. Implement sort controls for net score, velocity, upvotes, downvotes, and newest. Implement dynamic category filter chips generated from the categories table. Include a "For You" algorithmic feed based on locally tracked install preferences.
```

### Module 3: The Rust Instance Engine
```
Write a Rust Tauri backend module that:
1. Manages isolated modpack instances in a local /instances/ directory.
2. Maintains an instance_manifest.json per instance tracking all installed mods (source, version, hash, filename).
3. Downloads mod .jar files concurrently via GitHub Releases API (primary) or Modrinth API (supplementary fallback) with SHA-256 hash verification for all strategies.
4. Implements a zip override extractor with a directory whitelist (only config/, defaultconfigs/, resourcepacks/, kubejs/ — no mods/), hard-banned executable extensions (.jar, .exe, .bat, .sh, etc.), Zip Slip path traversal protection, and zip bomb limits (500MB compressed, 2GB extracted, 5000 files max).
5. Injects modloader version JSON and library files into the official ~/.minecraft directory with domain pinning and JSON hash verification.
6. Reads and mutates the official launcher_profiles.json atomically with backup and corruption recovery.
7. Constructs javaArgs strings from user-selected memory (slider), GC type (dropdown), custom args (text box), and AlwaysPreTouch toggle.
8. Discovers and executes the official Mojang Launcher via OS-specific paths (Windows Registry/Store, macOS Applications, Linux $PATH) with cached path.
```

### Module 4: The Crash Diagnostics Engine
```
Write a Rust module that:
1. On "Play" button click, compares crash-reports/ directory mtime against last_launched_at from the local SQLite DB.
2. If a new crash exists, halts launch and presents a review/skip modal.
3. Streams the crash log through all regex patterns from the crash_signatures SQLite table.
4. If a match is found, renders a human-readable solution with an optional 1-click fix button (e.g., auto-install a missing mod).
5. If no match, uses the user's GitHub OAuth token to search the offending mod's GitHub repository for matching open issues.
6. Provides a "Report Crash" flow that generates a standardized Markdown issue body, shows a full preview with redaction support, and submits it to the mod developer's GitHub repository after explicit user approval.
7. Provides a manual log viewer with [ERROR]/[WARN]/[INFO] line-level syntax highlighting.
8. Tracks crash telemetry in local_crash_telemetry using normalized pair IDs (registry_id, modrinth: prefix, or manual: prefix for untracked mods).
```

### Module 5: The Local MCP Server
```
Write a Rust module that exposes a local JSON-RPC MCP server on localhost with the following security requirements:
- Bind exclusively to 127.0.0.1 (no external connections).
- Generate a cryptographically random 256-bit per-session token on MCP server startup. Display it in Settings for the user to copy into their AI client.
- Reject any connection without the correct token.
- For destructive tools (disable_mod, enable_mod), display a native OS approval modal with [Allow Once], [Always Allow for This Tool], [Deny]. Read-only tools are auto-approved.
- Maintain a pending approvals queue with a maximum of 10 queued requests; reject excess with JSON-RPC error -32002.
- Clear all "Always Allow" grants when the Tauri app restarts.
Implement the following tools:
- read_latest_crash(instance_id): Returns the last 200 lines of the newest crash file.
- list_instance_mods(instance_id): Returns a JSON array of installed .jar files with versions.
- read_mod_manifest(mod_id): Fetches curator data for a mod from the local SQLite DB.
- disable_mod(instance_id, mod_filename): Renames mod.jar to mod.jar.disabled.
- enable_mod(instance_id, mod_filename): Reverses a disable operation.
- search_knowledge_base(query): TF-IDF search against the curator_note column.
On connection, expose the system_context.md content via the MCP resources/list capability so AI clients can fetch it as a resource.
```

### Module 6: The Static Next.js Web Directory
```
Build a static Next.js site hosted on GitHub Pages or Vercel. It fetches registry.db (or a compiled index.json) from a GitHub Pages CDN URL and renders a searchable, filterable mod directory. Implement filter controls for content_type, categories (dynamically generated), and sort order. Each mod card shows the name, curator_note excerpt, content_type badge, net_score, and a download link. The site requires no backend and no user accounts.
```

### Module 7: The Community Governance UI
```
Build a React "Community Governance" tab in the Tauri app that:
1. Fetches active 'under_review' items from the local SQLite DB.
2. For each item, makes a live GitHub Discussions API call to fetch current poll percentages.
3. Renders a card per item with: mod name, reason for review, live Keep/Remove percentage bars, and a "Cast Your Vote" button deep-linking to the GitHub Discussion.
4. Shows a "Recent Resolutions" history feed below.
5. On each mod's profile page, checks is_immune and conditionally renders the non-dismissible steel-blue Curator Shield banner with the immunity_reason text.
6. Disables vote UI elements on immune items.
```

---

## 15. SECURITY ARCHITECTURE

This section consolidates all cross-cutting security decisions, threat models, and hardening requirements. Individual sections throughout this document reference back to these principles.

### 15.1 Threat Model Summary

| # | Threat | Severity | Mitigation Location | Status |
|---|---|---|---|---|
| 1 | Dev Mode executing arbitrary Gradle/Maven builds | Critical | §11.1 — Build sandbox | Addressed |
| 2 | Unauthenticated MCP tools allowing local process takeover | Critical | §10.0 — MCP Security | Addressed |
| 3 | GitHub OAuth token theft from compromised app | Critical | §7.5 — OAuth Token Security | Addressed |
| 4 | Zip bomb attacks via pack override downloads | High | §7.2.1 — Zip Bomb Mitigation | Addressed |
| 5 | Override extraction relying on denylist instead of whitelist | High | §7.2.2 — Directory Whitelist | Addressed |
| 6 | Supply-chain attacks on modloader downloads | High | §8.2.1 — Supply Chain Verification | Addressed |
| 7 | Regex DoS via community crash signatures | High | §2.4.1 — Regex DoS Prevention | Addressed |
| 8 | `launcher_profiles.json` corruption during mutation | High | §8.3.1 — Atomic Write with Backup | Addressed |
| 9 | GitHub as a strategic dependency (not a vulnerability per se) | Medium | §15.2 below | Acknowledged |
| 10 | Sybil attacks on governance voting | Medium | §3.1 Step 4 — Sybil Resistance | Partially addressed |
| 11 | Privacy leakage in crash report submissions | Medium | §9.4 — Preview before submit | Addressed |
| 12 | Telemetry not fully anonymous at transport level | Medium | §12 — Updated wording | Acknowledged |

### 15.2 Strategic Dependency: GitHub as Implicit Backend

This project's architecture describes itself as "serverless" and "$0/month." While technically true — no servers are rented, no databases are hosted — **GitHub functions as the de facto backend** for:

- Database (Issues, Discussions, Reactions)
- Authentication (OAuth provider)
- Voting system (Reactions)
- Governance (Discussions polls)
- Telemetry aggregation (Gists or webhooks)
- Content delivery (Release Assets / Pages CDN)

This is a **strategic dependency risk**, not a security vulnerability. If GitHub:
- Changes its API rate limits or pricing
- Modifies or deprecates Discussions, Reactions, or Issue features
- Introduces breaking changes to the GraphQL API
- Experiences extended outages

...portions of the platform break. This risk is accepted because:
1. GitHub's free tier is extremely generous and has been stable for years.
2. All data is stored as flat JSON files in the repository — migration to a different platform is structurally possible (if labor-intensive).
3. The client-side SQLite model means the launcher remains functional offline even if GitHub is temporarily unreachable.

**Offline Capability:** The downloaded `registry.db` is cached locally. If the user is offline, they can still browse the curated catalog, read curator notes, and assemble modpack configurations. Mod file downloads are deferred until an internet connection is available. The app displays a "Limited Offline Mode" banner when it cannot reach GitHub Releases or Modrinth.

**Mitigation:** The nightly compiler logs all API assumptions and endpoint versions. If a GitHub API change breaks the pipeline, the compiler fails loudly (not silently) and curators are notified. The `registry.db` artifact from the last successful build continues to be served until the compiler is fixed.

### 15.3 Security Principles (Cross-Cutting)

1. **Whitelist over denylist.** When controlling what enters the filesystem (override extraction), only allow known-safe paths. Banning "known bad" is insufficient because unknown bad always exists.
2. **Verify everything from the network.** Every downloaded file (mods, modloader libraries, override zips, registry.db itself) must be verified via SHA-256 hash or Ed25519 signature before being written to disk or executed.
3. **Never store secrets in plaintext.** OAuth tokens live in the OS keychain, not in files, environment variables, or databases.
4. **Treat all community data as untrusted.** Curator notes, reviews, category names, and crash signature markdown are all potentially malicious. Escape everything before rendering. Never use raw HTML passthrough.
5. **Sandbox arbitrary code execution.** Any operation that compiles, builds, or executes code from an external source (Dev Mode, MCP tool invocation) must run in an isolated environment with explicit user approval.
6. **Atomic writes for critical files.** When mutating user-owned files (launcher_profiles.json, instance configs), always write to a temporary file first, then rename atomically. Always maintain a backup.
7. **Fail closed, not open.** When verification fails (hash mismatch, signature invalid, sandbox unavailable), the operation is blocked — not logged and allowed to proceed.
8. **Accepted Ecosystem Limitations:** KubeJS scripts run within Minecraft's JVM without OS-level sandboxing. This is a limitation of the Minecraft modding ecosystem, not this launcher. The directory whitelist prevents OS-level scripting attacks (`.sh`, `.bat`), but in-game scripts with network access remain a broad Minecraft security concern outside this platform's scope.
9. **Parameterized Queries Only.** All user input that reaches SQLite must use parameterized queries or prepared statements. Never concatenate user input into SQL strings. The `tauri-plugin-sql` API supports parameter binding natively. This applies to: browse search queries, category filters, instance IDs passed to crash diagnostics, and MCP `search_knowledge_base` tool inputs.

---

## 16. TECHNICAL DECISIONS LOG

This section documents key architectural decisions made during design so future agents and contributors understand the "why."

| Decision | Rationale |
|---|---|
| Delegate auth and JVM to the official Mojang launcher | Eliminates entire attack surface of Microsoft OAuth chains, token management, vanilla asset downloading, and JVM execution. Zero future maintenance burden when Microsoft changes auth APIs. |
| GitHub Issues + Reactions for voting | $0 cost, built-in spam protections, open API, auditable public ledger. Scales infinitely with user base. |
| SQLite for client-side database | Enables complex SQL sorting/filtering entirely locally. No API calls needed for discovery. No privacy leakage. |
| Weight-0 for untrusted voters (not hard rejection) | Silent ignoring is more effective against bot farms than visible blocking; bots see success but have zero impact. |
| No public repository check for voter trust | Would disenfranchise 95%+ of the player base who are gamers, not software engineers. Account age + comment history is sufficient. |
| Ban .sh and .bat in pack overrides | Runtime payload injection attack vector. Platform dependency scripts break cross-platform compatibility. KubeJS/CraftTweaker are the correct in-game scripting tools. |
| Do NOT dump crash signatures into AI context | Context dilution, token waste, hallucination risk. Use `search_knowledge_base` tool on demand instead. |
| Separate curator_reviews table from registry_items | Allows compiler to update social metrics without overwriting curator-authored content. Enables complex joins. |
| immunity_cooldown after "Keep" vote | Prevents immediate re-review-bombing after a successful defense. 30-day cooling period restores fairness. |
| modrinth_id as supplementary (not primary) download strategy | Modrinth provides convenient hosting, but the project's ethos prioritizes developer sovereignty via GitHub releases. Modrinth is a fallback for mods that haven't self-hosted yet, not the default path. This keeps the platform independent of Modrinth's infrastructure and supports users who disable Modrinth entirely. |
| Direct SHA-256 hash for closed-source mods | Prevents silent malicious updates. If a developer updates their file without submitting a new PR with an updated hash, every download is blocked for all users automatically. |
| Crash Matrix is opt-in only | Privacy-first. Opt-in telemetry with no PII produces community-trusted data while respecting users who don't want to share. |
| Deploy registry.db as GitHub Release Asset (not repo commit) | Committing a binary `.db` file daily would bloat the Git history irreversibly, making clones prohibitively slow. Release Assets support files up to 2GB and don't pollute repository history. |
| Sign registry.db with Ed25519 offline key | Prevents a compromised GitHub account from distributing a malicious database. The Tauri client verifies the signature before trusting the data. |
| Store image URLs, not binary images, in database | Hosting raw images in the repository would exhaust storage limits. Modrinth CDN already hosts mod icons and gallery images; custom assets use a dedicated `launcher-media` repo with GitHub Pages. |
| Directory whitelist for override extraction (not denylist) | Denylists are fundamentally insufficient — dangerous content exists in many forms beyond banned extensions (KubeJS scripts, malicious JSON, OpenLoader resources). A whitelist of allowed paths structurally prevents unknown threats. |
| Sandbox Dev Mode builds (Docker/Podman/Firecracker) | Build scripts execute arbitrary code. Running them on the host OS is the single highest-risk operation. Sandboxing with ephemeral filesystems and no host access prevents supply chain compromise. |
| MCP server authentication with per-session tokens and user approval flow | Without auth, any local process could disable all mods, read crash logs, or manipulate installations. Per-session tokens + capability-level approval dialogs treat MCP tools as the privileged operations they are. |
| Store OAuth tokens in OS keychain, never plaintext | Storing tokens in config files, environment variables, or local databases makes them trivially stealable. OS credential managers provide hardware-backed encryption and access auditing. |
| Atomic writes with backup for launcher_profiles.json | Direct mutation of critical files risks corruption from crashes or power loss during writes. The `.tmp` → `rename()` pattern is atomic on most filesystems, and `.bak` files enable automatic recovery. |
| Rust `regex` crate only for crash signature matching | The Rust regex engine avoids catastrophic backtracking by construction, structurally preventing ReDoS. Maximum pattern length and CI performance gates add defense-in-depth. |
| SHA-256 hash verification for all download strategies (not just direct_hash) | Supply chain attacks can compromise any upstream source. Verifying hashes for Modrinth downloads, GitHub releases, and modloader libraries ensures integrity regardless of the source. |
| GitHub as implicit backend — strategic dependency acknowledged | GitHub provides database, auth, voting, governance, and CDN at zero cost. This dependency is accepted because: data is portable (flat JSON files), the client works offline, and the compiler fails loudly on breaking API changes. |
| Preview-before-submit for crash reports | Auto-submitting crash reports to developer repos leaks username, OS, mod list, and hardware hints without user awareness. Mandatory preview allows redaction of sensitive information before public posting. |
| Realistic telemetry anonymity wording | Claiming "no personal data is ever collected" is inaccurate when HTTP transport inherently exposes IP, timing, and user agent. Honest wording ("no intentional personal data") sets correct user expectations. |
| Trust score based on org interactions only | "3 comments anywhere on GitHub" is not queryable via any API. Org-scoped interactions are the only practical and verifiable metric. |
| Remove org-level user ban from compiler | `PUT /orgs/{org}/blocks/{username}` requires `admin:org` scope; GitHub Action tokens have repo-level permissions only. Manual curator action is the only viable path. |
| last_launched_at update before launch | Updating only after crash review causes infinite crash prompt loops for "Launch Anyway" users. Updating before launching ensures the current session's crash is detected on the next launch. |
| Modloader hash pin: version JSON only | Pinning every transitive library JAR is a manual effort nightmare (50+ libs per version, frequent updates). Pinning the JSON root file + domain pinning is the same model the official launcher uses. |
| OAuth keyring fallback with machine-bound encryption | Linux Secret Service requires D-Bus; headless/WSL setups fail. A degraded mode with PBKDF2+AES-256-GCM and explicit user warning is better than failing entirely. |
| Modrinth batch endpoint for image URL hydration | Querying Modrinth individually per mod hits rate limits. The `/v2/projects?ids=[...]` endpoint accepts 500 IDs per request, dramatically reducing API call volume. |
| Atomic write: regenerate on total corruption | Double-backup is overkill. Detecting invalid JSON and regenerating a minimal valid file from the app's local database is simpler and sufficient. |
| MCP approval queue cap at 10 | Without a cap, rapid-fire tool calls flood the user with modals. A queue with batch approve/deny and an error response to the AI client provides a clean UX. |
| SHA-256 mandatory for all download strategies | Marking it "optional" for Modrinth/GitHub leaves a supply-chain gap. The compiler auto-populates it from Modrinth API and GitHub release metadata, so there is no valid reason to skip verification. |
| Raw Modrinth tab still verifies hashes | Even uncurated mods should be integrity-checked against Modrinth's published hashes. Curation is about quality, not file integrity. |
| Flag review via direct GitHub issue creation | A "webhook" implies a backend, violating the $0 constraint. The Tauri app creates the issue directly in a private admin repo using the user's token. |
| Regenerate launcher_profiles.json on corruption | Only way the file gets corrupted is manual user tampering or disk failure. Regenerating from the app's local database is simpler than maintaining multiple backup rotations. |
| Pack manifests specify exact mod versions | Curators control what version users get. If the exact version disappears, the user is prompted to accept a fallback rather than silently installing something different. |
| Version picker filtered by MC version/loader in mod detail page | Users need to see only compatible versions. Defaulting to the latest compatible avoids manual compatibility checking. |
| GitHub release version_info.json (optional) | Filename pattern matching and description parsing are unreliable. An optional structured metadata file lets developers provide precise compatibility data without forcing it. |
| Dependency resolution: crash-triggered, not auto-resolve | Modrinth's dependency graph is reliable but incorporating it into the primary pipeline couples the platform to Modrinth's system. Crash logs already reveal missing dependencies; offering to install found deps keeps the pipeline simple. |
| Partial pack load on download failure | Aborting the entire installation because one mod failed is user-hostile. Installing what succeeded and offering remediation options for the rest is a better UX. |
| Disk space pre-check with 20% headroom | Users shouldn't discover they're out of disk space halfway through a 200-mod download. A pre-check prevents wasted bandwidth and partial installations. |
| Pack update with diff preview and backup | Users need to see exactly what changed before updating a pack. Automatic backups before update allow rollback if something breaks. |
| Custom instance creation from scratch | Not all users want curated packs. Custom instances with manual mod assembly are a standard launcher feature and easy to support given the existing architecture. |
| Pack export to .mrpack or custom JSON | Sharing modpacks between friends is a core use case. Supporting both standard Modrinth format and native format maximizes compatibility. |
| Instance deletion to OS trash | Immediate deletion is unforgiving. Moving to trash gives users a recovery window without requiring us to build an undo system. |
| Browse-Only Mode without GitHub OAuth | Some users don't want to create a GitHub account or share OAuth access. Allowing browse/install without auth broadens the user base while clearly communicating what requires auth. |
| DB update detection via GitHub Releases API | The app needs to know when a new registry build is available without downloading the entire DB. Comparing release tags via the API is a single lightweight request. |
| Settings persistence via tauri-plugin-store | A JSON-based key-value store is simple, reactive, and doesn't require a full database for user preferences. |
| Full-text search via SQLite FTS5 or LIKE | Users expect to type "sodium" and find it. FTS5 is fast and native to SQLite; LIKE fallback ensures it works even if FTS5 is unavailable in the build. |
| Original filenames preserved for mod JARs | Renaming causes confusion when users browse their mods folder manually and breaks some mods that check their own filename. Original names are zero-cost. |
| github_release as primary download strategy | The project's ethos is developer independence and platform sovereignty. Sourcing directly from developer GitHub repos eliminates dependency on Modrinth's infrastructure and aligns with the anti-corporate-consolidation mission. Modrinth is supplementary, not essential. |
| Modrinth integration fully disable-able | Users who object to Modrinth on principle (or are in regions where it's blocked) must still have full access to curated packs, discovery, and instance management. Only Modrinth-sourced mods and the raw Modrinth tab are affected by the toggle. |
| instance_manifest.json per instance (not SQLite) | Avoids SQLite merge complexity when updating registry.db. JSON is human-readable, directly exportable to .mrpack, and reconstructible from scratch on any machine. Backups are lightweight (manifest + configs, mods are re-downloadable). |
| Parameterized queries for all user input | SQL injection is a classic vulnerability that SQLite is not immune to. tauri-plugin-sql supports parameter binding natively. Explicitly mandating it prevents an entire class of attacks with zero performance cost. |
| Crash telemetry uses prefixed identifiers | Curated mods use registry_id, raw Modrinth mods use modrinth: prefix, unknown mods use manual: prefix. This allows the telemetry system to track all mods uniformly without requiring every mod to be in the curated registry. |
| Split `registry.db` and `local_state.db` | Treating the downloaded registry as read-only while mutable user state lives in a separate DB eliminates file-locking races, simplifies DB updates, and makes offline mode more robust. |
| Database versioning protocol | Without a schema version check, a newer `registry.db` could crash an older launcher. Blocking forward-incompatible DBs and forcing a client update is safer than guessing. |
| Offline / Degraded Mode | If GitHub is unreachable (outage, block, or user offline), the launcher must not brick. It falls back to cached data, existing instances, Modrinth direct input, and manual .jar installs. |
| Upstream verification policy with known_good_hashes | Trusting official domains alone is insufficient if the domain is compromised. Embedding a hardcoded hash map and requiring curator PR review creates a real trust anchor. |
| Human-centric error taxonomy | Standardized error codes let the React UI handle failures consistently and provide actionable user-facing messages instead of raw Rust error strings. |
| Audit log / transparency black box | Automated governance must be auditable. An append-only log builds public trust and provides evidence in disputes about bias or unfair moderation. |
| Persistent MCP approval grants | Asking the user to approve the same AI tool every app session is hostile. Persisting `approved_always` in `local_state.db` respects user intent while keeping revocation easy. |

---

## 17. IMPLEMENTATION ORDER & MVP SCOPE

This section gives a concrete build order so a coding agent (or human developer) knows what to ship first.

### Phase 0: Repository & Data Plumbing (Required Before Anything Else)

1. **Create the central GitHub repository** with the directory structure from §1 (`registry/mods/`, `registry/packs/`, `crash-signatures/`, `loader-manifests/`, `.github/workflows/`, `CODE_OF_ENGAGEMENT.md`).
2. **Seed registry with 5–10 example mods** (Sodium, Iris, Lithium, Fabric API, etc.) using `github_release` strategy.
3. **Create `loader-manifests/known_good_hashes.json`** with pinned hashes for Fabric, NeoForge, Quilt, Forge for the current Minecraft version.
4. **Create the separate `launcher-media` repository** for custom banners.
5. **Create the private `agora-mc/admin-alerts` repository** for flag reports and triage alerts.

### Phase 1: Compiler (Python GitHub Action) — Module 1

1. Implement manifest JSON parser.
2. Implement GitHub Releases API fetcher with SHA-256 hash extraction for `github_release` mods.
3. Implement GraphQL/REST trust score fetcher (org interactions only).
4. Implement NLP filtering (profanity-check, vaderSentiment).
5. Implement velocity anomaly detection.
6. Build `registry.db` with all schemas from §4 (registry tables only).
7. Sign with Ed25519 offline key.
8. Upload as GitHub Release Asset.

**Acceptance test:** Compiler runs nightly, produces a `registry.db` with 5–10 mods, a valid signature, and uploads it to a tagged release.

### Phase 2: Rust Tauri Skeleton & Instance Engine — Module 3

1. Initialize Tauri project with React + Tailwind.
2. Implement §4 dual-DB setup (`registry.db` + `local_state.db`).
3. Implement first-run onboarding flow (§6.1a) with integration toggles.
4. Implement downloader with SHA-256 verification, 6 concurrent, 3 retries, partial-pack fallback (§7.1.1).
5. Implement override sanitization with whitelist + zip bomb limits (§7.2).
6. Implement modloader injection with domain pinning + JSON hash verification (§8.2).
7. Implement `launcher_profiles.json` atomic write + corruption recovery (§8.3).
8. Implement Mojang launcher discovery (§8.4).
9. Implement JVM argument builder with `AlwaysPreTouch` toggle (§8.5).
10. Implement `instance_manifest.json` for each instance.

**Acceptance test:** User can create a custom instance, install 3 mods, set JVM args, and successfully launch Minecraft via the Mojang launcher.

### Phase 3: Browse, Discovery & Search — Module 2

1. Implement React sidebar with 5 tabs.
2. Implement Browse tab with sort, filter, search (FTS5 or LIKE).
3. Implement category chips dynamically generated from `categories` table.
4. Implement Modrinth SQL filter (`WHERE download_strategy != 'modrinth_id'` when disabled).
5. Implement "For You" algorithm with local install tracking.
6. Implement "Boutique vs. Warehouse" split.
7. Implement "Search all of Modrinth →" link to Raw Modrinth tab.

**Acceptance test:** User can browse curated mods, sort by net score, filter by category, and search by name.

### Phase 4: Crash Diagnostics — Module 4

1. Pre-launch interceptor with `last_launched_at` timing fix.
2. Regex signature engine using Rust `regex` crate.
3. Manual log viewer with syntax highlighting.
4. GitHub issue duplicate check via search.
5. Preview-before-submit crash reporting.
6. Local crash telemetry in `local_state.db` with prefixed identifiers.

**Acceptance test:** A simulated crash report matches a regex signature, displays the fix, and the user can optionally submit to GitHub after preview.

### Phase 5: Governance & Triage — Module 7

1. Triage Center tab with under-review items from `registry.db`.
2. Live GitHub Discussions API integration for poll percentages.
3. "Recent Resolutions" history feed.
4. Curator Shield banner for immune items.
5. Flag review system (GitHub issue direct creation).
6. In-app Transparency Log display from `audit_log.json`.

**Acceptance test:** An item marked under_review appears in the Triage Center with live poll data.

### Phase 6: MCP Server — Module 5

1. MCP server with localhost binding, ephemeral port, per-session auth token.
2. Approval queue with persistent grants in `local_state.db`.
3. All 6 MCP tools implemented.
4. `system_context.md` via MCP `resources/list` capability.

**Acceptance test:** Claude Desktop connects with the token, calls `list_instance_mods` and `disable_mod`, and the user sees the approval prompt.

### Phase 7: Dev Mode — Optional

1. Sandbox detection (Docker, Podman, Firecracker).
2. Repo clone + build in sandbox with no network.
3. `.jar` extraction to test instance.

**Acceptance test:** User can build a mod from a GitHub URL inside a Docker container and test it.

### Phase 8: Web Directory — Module 6

1. Static Next.js site that fetches `registry.db` from GitHub Release Asset.
2. Server-rendered mod cards.
3. Search, filter, sort.
4. React Markdown strict mode for safety.

**Acceptance test:** Site loads, shows curated mods, and search works.

### Phase 9: Polish & Hardening

1. Code signing certificate for Windows / macOS notarization.
2. Auto-update mechanism via Tauri's built-in updater.
3. Disk space pre-check (§7.1.2).
4. Instance deletion to OS trash.
5. Pack export to `.mrpack` / custom JSON.
6. Telemetry opt-in flow.
7. Localization (i18n) framework.

### MVP Definition (End of Phase 5)

The **minimum viable product** is reached when:
- Users can install curated packs and play them.
- Browse, search, sort, and filter work.
- Crash reports work with preview-before-submit.
- Triage Center shows live poll data.
- MCP server works with approvals.
- All security controls from §15 are in place.
- Offline Mode works with cached DB.
- Modrinth integration can be fully disabled.
- All GitHub OAuth features work or gracefully degrade.

Dev Mode (Phase 7) and the Web Directory (Phase 8) can ship later. Phases 1–5 are the core product.

---

## 18. KNOWN LIMITATIONS & OPEN QUESTIONS

This section is an honest disclosure of what the spec does not yet cover, what assumptions may need to be revisited, and what could be improved in future iterations.

### 18.1 Known Limitations

- **GitHub as a single point of failure for the central registry.** If GitHub is down, the curated catalog and updates are inaccessible. The "Bootstrap Bootstrap" emergency URL is a future concern (user deferred this). Current mitigation: Degraded Mode (§4.3) allows continuing with cached data.

- **Modrinth is still reachable for its batch and version APIs even with the toggle off** in some code paths. The `modrinth_id` curated mods are filtered from Browse, but if a developer URL points to a Modrinth CDN (e.g., direct file hosting), those downloads would still work. The toggle is a UI/feature gate, not a network-level block.

- **No code signing requirements are defined in the spec.** Windows SmartScreen and macOS Gatekeeper will block unsigned binaries with scary warnings. The spec mentions this in the decisions log but does not specify the signing infrastructure (HSM? cert provider? cost budget?).

- **No i18n/localization framework.** All UI text is English. Adding localization would require restructuring all strings into a resource bundle.

- **No accessibility (a11y) requirements.** WCAG compliance, screen reader support, and keyboard navigation are not specified.

- **No conflict resolution for versions of transitive dependencies.** If two mods in a pack require different versions of the same library, the spec doesn't say what happens. The pack curator is responsible for ensuring compatibility.

- **Concurrent instance launches are not specified.** What happens if a user clicks "Play" on two instances simultaneously? The atomic write to `launcher_profiles.json` is handled, but the Mojang launcher can only run one instance at a time.

- **The `system_context.md` injection depends on the AI client's MCP implementation.** Not all MCP clients (e.g., older versions of Claude Desktop, custom clients) support `resources/list`. If unsupported, the AI will not have the system context.

- **The Crash Matrix telemetry is opt-in but never actually aggregated.** The spec says "once a week, the launcher compresses the local telemetry table and posts it" but does not specify the aggregation endpoint. A serverless form or a GitHub Gist is mentioned but not implemented.

- **No automated tests are defined.** The spec describes the system but does not include a testing strategy (unit tests, integration tests, end-to-end tests).

- **No migration scripts for `registry.db` schema changes.** When the compiler adds a new column, existing client apps will fail. The versioning protocol (§4.1a) blocks forward-incompatible DBs but doesn't define what migrations look like.

- **`local_state.db` migrations are specified as "robust against data loss" but no rollback strategy is defined.** If a migration fails halfway, what state is the user left in?

### 18.2 Open Questions for Future Iterations

- **Should the MCP server be exposed to remote (LAN) connections?** Currently localhost-only. Some users may want to run the launcher on a NAS and connect from their gaming PC.

- **Should the audit log be append-only or should curators be able to edit it?** Append-only is more trustworthy but means a typo in a justification is permanent.

- **How are mods removed from `registry.db` handled in active instances?** If a mod is archived by community triage, does the launcher warn users? Auto-remove from instances?

- **What's the multi-user story?** If two users share a machine, each gets their own `local_state.db`? Or shared?

- **What's the relationship between `local_crash_telemetry` and the global Crash Matrix?** The spec says "community data indicates these mods have a high co-crash rate" but doesn't define the aggregation math or threshold.

- **How does the launcher handle modpack drift over time?** If a pack's manifest changes the MC version, what happens to users running the old version?

- **What about server packs (the `server` content type)?** The spec mentions them but doesn't describe installation or hosting.

- **Should there be a "Verified Curator" badge system?** Currently, all curators have equal power. Some form of reputation system might help with spam-resistant governance.

- **What happens when a mod is deleted from GitHub but still appears in packs?** The download fails. The user sees an error. But should the pack be auto-archived? Or flagged for curator review?

- **Mobile companion app?** The spec is desktop-only. A Tauri mobile build might be feasible but is out of scope.

### 18.3 What This Spec Does Well

- **Clear separation of concerns:** Data layer (registry.db), execution layer (Tauri/Rust), presentation layer (React), governance layer (GitHub Issues).
- **Security by delegation:** Microsoft auth and JVM execution are fully delegated to the Mojang launcher.
- **Zero server cost:** No backend, no database, no CDN bills.
- **User sovereignty:** Modrinth integration is opt-in. OAuth is opt-in. Telemetry is opt-in. Dev Mode is opt-in.
- **Transparency:** Audit log, curator immune override justification, visible vote weights.
- **Extensibility:** New content types (shaders, resourcepacks, datapacks, worlds, servers) plug into the same pipeline.

### 18.4 What an Implementer Should Watch Out For

1. **Tauri's `tauri-plugin-sql` API surface.** Check the version compatibility with the SQLite version used by the compiler. Schema migration tools differ.

2. **Ed25519 key management.** The offline key must be generated once, stored securely (not in a developer's home directory), and used only by the GitHub Action via a `secrets.ED25519_PRIVATE_KEY` secret. Rotation is a future problem.

3. **GitHub API rate limits.** Even with user OAuth, hitting 5,000 requests/hour is possible if the launcher makes many small calls. Batch where possible.

4. **Cross-platform testing.** The spec defines behavior on Windows, macOS, and Linux but most testing will be on one platform initially. All OS-specific paths need explicit test coverage.

5. **The `loader_manifests.json` maintenance burden.** Every new modloader version requires a curator PR to add the hash. If the curator team is small, this becomes a bottleneck.

6. **The audit log grows unboundedly.** Even with rotation, this is a 10,000-entry file. Performance is fine for `append` operations but reading the whole file for the Triage Center Transparency Log could be slow.

7. **Race conditions in the readers-writer lock on `registry.db`.** A simple lock works but has edge cases. Consider using a SQLite-native approach or a proper RWLock with timeout.

8. **The `known_good_hashes.json` is embedded at compile time.** This means a security fix requires a new app release. The downside of a hardcoded trust anchor.

---

## Final Readiness Assessment

**Status: READY FOR EXECUTION**

The spec is comprehensive, internally consistent, and covers all major architectural concerns. An AI coding agent or human developer can use this document as a self-contained implementation guide.

**Confidence levels by area:**

| Area | Confidence | Notes |
|---|---|---|
| Core architecture (Tauri + React + SQLite) | High | Standard stack, well-documented |
| Data pipeline (GitHub + flat JSON + compiled DB) | High | Proven pattern |
| Security controls | High | Multiple layers, no obvious gaps |
| Mojang launcher integration | Medium | OS-specific behavior may need runtime testing |
| MCP server | High | Standard protocol, well-specified |
| Offline / Degraded Mode | High | Clear fallback paths |
| Crash diagnostics | Medium | Regex patterns need community curation over time |
| Dev Mode sandboxing | High | Docker/Podman/Firecracker are well-understood |
| Governance (GitHub Issues as voting) | High | Creative use of existing infrastructure |
| Audit log | High | Append-only, well-defined format |

**Recommended next steps:**
1. Begin Phase 0 (repository setup) and Phase 1 (compiler) immediately — these are prerequisites for everything else.
2. Build a working prototype of the Rust instance engine (Phase 2) before investing heavily in the React UI.
3. Get 2–3 curators to seed the registry with real mods and test the pack installation flow end-to-end.
4. Set up CI for the compiler with sample test data.
5. Establish a security review process before the public launch — all Critical/High items in §15.1 should be verified.

**This document is the source of truth.** When implementation questions arise that are not answered here, prefer the simplest interpretation that is consistent with the existing decisions. Update this document as those questions are resolved.
```


## 19. ARCHITECTURAL EVOLUTION & IMPLEMENTATION STATUS

> This section supersedes conflicting statements above. Last updated 2026-07-05.

### 19.1 Workspace Layout (supersedes section 1)

`
D:/Agora/
+-- registry/                 # Curated flat-file manifests (the GitHub database)
|   +-- mods/ packs/ shaders/ resourcepacks/ servers/ datapacks/ worlds/
|   +-- governance/            # audit_log.json, known_conflicts.json, poll_blacklist.json
|   +-- pack-overrides/        # Config/resource override zips
|   +-- archived/              # Retired entries (compiler skips)
+-- crash-signatures/         # Regex triage definitions
+-- loader-manifests/         # loader_manifests.json + known_good_hashes.json + minecraft_versions.json
+-- compiler/                  # Python nightly compiler -> registry.db (+ .sig)
+-- crates/
|   +-- agora-core/           # Shared business-logic library (no tauri/clap types) -- Phase 1 of v1 refactor
|   +-- agora/                # Standalone gora CLI binary (Phase 9 of v1 refactor)
+-- desktop/                   # Tauri GUI app (crate package name: agora-desktop)
|   +-- src/                   # React + Tailwind + Vite frontend
|   +-- src-tauri/             # Rust backend -- thin facades delegating to agora-core
|   +-- e2e/                   # Playwright end-to-end tests
+-- web/                       # Static Next.js public directory (static export)
+-- scripts/                   # verify_db.py, deploy_release_assets.py, fetch_registry_db.py, refresh_loader_manifests.py
+-- .github/                   # workflows (compile, release-desktop, web-build, e2e), ISSUE_TEMPLATE
+-- .kilo/                     # Kilo agent config, commands, agent profiles, skills, MASTER_SPEC.md
+-- AGENTS.md                  # Canonical agent guide
+-- README.md                  # Project overview + setup + release cutting
+-- BACKLOG.md                 # Phase-by-phase task tracker
+-- CODE_OF_ENGAGEMENT.md      # Canonical review-conduct rules
+-- REGISTRY_CURATION_REFERENCE.md   # Self-contained manifest-authoring guide
+-- Cargo.toml                 # Workspace root (members: crates/agora-core, desktop/src-tauri, crates/agora)
+-- .env.example               # Compiler env vars (GITHUB_TOKEN, ED25519_PRIVATE_KEY, DISCORD_WEBHOOK_URL)
`

### 19.2 Migration Status: desktop to agora-core (v1 refactor)

Goal (per the deleted v1-launcher-refactor plan): move all business logic into crates/agora-core/, leaving desktop/src-tauri/src/ as thin facades, and expose a standalone gora CLI binary from crates/agora/.

**Fully migrated to agora-core (desktop delegates):** error, models, paths, download, loader_manifests, registry, registry_sync, crash_diagnostics, dependency_ops, launcher_profiles (with section 8.3 atomic write + .bak recovery), override_sanitizer, mod_cache, snapshot, import, clone, loadout, server_export, github_ratelimit, ai_assistant, msa (full 9-step account chain), launch (direct Java spawn + Forge/NeoForge install), gc, java (JRE detection), health (pre-launch scanner), modrinth (search/versions/install), jar_metadata, browse_cache, pack_install. Desktop db.rs::run_migrations now delegates to gora_core::db::run_migrations.

**Still thick in desktop (not yet migrated):** crash_investigator.rs (~1968 lines -- contains scoring algorithm), mod_install.rs (~2275 lines), instances.rs, mojang.rs, mcp.rs (plan: move into core with gora serve CLI), ersion_cache.rs. The desktop governance.rs duplicates etch_triage_poll / lag_review network logic not yet in core. Future migration work should target these modules.

**Dead code removed during 2026-07-05 audit:** dead REGISTRY_SCHEMA_VERSION = 1 constants, commands::greet template leftover, the duplicate compiler/_test_social_metrics.py (merged into 	est_compile.py), compiler/analyze_sha256.py, desktop/src-tauri/neoforge-installer.jar.log, the stray root rowse search response.md debug dump, and the entire desktop/src/pages/ModrinthRaw.tsx (separate Modrinth UI merged into Browse).

### 19.3 MSA Authentication & Direct Launch (supersedes section 0, section 8.1)

Per the v1 refactor (decision E9): the launcher now optionally performs **Microsoft Account (MSA) authentication and direct JVM execution in-process** -- crates/agora-core/src/msa.rs implements the full: device code, then MSA token, then XSTS, then Minecraft services token, then profile + Xbox profile, then DRM header token flow. crates/agora-core/src/launch.rs then constructs the classpath + args + natives and spawns java directly.

This deliberately relaxes the original *security by delegation* constraint because in-launcher features (one-click version selection, accurate launch errors, native version manifest caching via piston-meta.mojang.com, OAuth token refresh) cannot be implemented on top of the Mojang-launcher-delegation model. The Mojang-launcher-path (section 8.4 -- discover official launcher binary, mutate launcher_profiles.json) is retained as a fallback for users who prefer not to use the in-process MSA flow.

MSA tokens use the same storage backend as GitHub OAuth tokens: OS keyring first, with a PBKDF2 + AES-256-GCM encrypted-file fallback (	okens.enc) per section 7.5.2 -- implemented during the 2026-07-05 audit (was previously a hard error).

### 19.4 Crash Investigator (signal-based dynamic scoring)

From the deleted 1782081355093-crash-investigator-plan.md: when a curated regex signature (section 9.2) does not match a crash log, the launcher runs a **dynamic weighted scoring algorithm** in desktop/src-tauri/src/crash_investigator.rs to rank suspect mods. Score contributions per mod:

- **A -- stack-frame attribution:** the mod Java packages (extracted from .jar manifests at install time) appear in the crash Caused by: / stack trace. The strongest single signal.
- **B -- fingerprint recurrence:** the crash fingerprint (normalized Caused by block hash) has co-occurred with this mod in past local crashes.
- **C -- co-crash signal:** this mod co-occurs in historical crashes with another direct suspect (local_crash_telemetry table).
- **D -- survival ubiquity dampener:** penalty for mods installed on >50 percent of instances (they appear everywhere so their presence is weak evidence).
- **E -- confirmed prior:** the user previously confirmed this mod caused a similar crash (attribution recorded as a prior).
- **F -- recency factor:** recent attributions weighted higher than stale ones.
- **G -- curated conflict:** the mod appears in a curated known_conflicts.json entry with another suspect.

Indirect suspects (mods that depend on a direct suspect via the mod_dependencies manifest field) are listed separately with reduced scores. The algorithm writes attributions back into local_crash_telemetry and the priors store via 
ecord_crash_event (now wired into investigate_crash per A5 of the 2026-07-05 audit).

### 19.5 Dependency-Aware Mod Operations

From the deleted dependency-aware-mod-ops-plan.md: manifests may declare mod_dependencies: {required: [...], optional: [...], incompatible: [...]} and mod_jar_aliases: [...] to let the install/dependency system match across sources (curated registry ID, Modrinth project ID, jar-declared ID). crates/agora-core/src/dependency_ops.rs and desktop/src-tauri/src/dependency_ops.rs implement a unified dependency graph; crates/agora-core/src/jar_metadata.rs parses abric.mod.json / mods.toml from jars at install time. abric-api.json is the canonical example of a manifest with mod_dependencies.

### 19.6 MCP Server -- Implemented Tool Set & Roadmap

Per the E2 user decision (combine spec section 10.1 with the implemented v1 set), the MCP server is being expanded to a **superset**. Currently implemented (in desktop/src-tauri/src/mcp.rs):

1. list_instances -- list all instance IDs + metadata.
2. list_instance_mods(instance_id) -- read instance_manifest.json, return mod jar info + java packages.
3. disable_mod(instance_id, filename) -- destructive; requires per-instance approval grant (ERR_MCP_DENIED if not granted).
4. search_crash_signatures(crash_text) -- local regex triage against crash_signatures table.
5. suggest_mod_incompatibility(instance_id, crash_text) -- runs the section 19.4 dynamic scoring algorithm.
6. get_system_context -- markdown overview returnable to AI clients.

**Planned additions** (from the section 10.1 superset, not yet implemented):
- 
ead_latest_crash(instance_id) -- return last 200 lines of newest crash report.
- 
ead_mod_manifest(mod_id) -- fetch curator data from local SQLite.
- enable_mod(instance_id, filename) -- reverse disable_mod.
- search_knowledge_base(query) -- TF-IDF LIKE search over curator_note.

Per B2 (user decision): the per-session Bearer token from section 10.0 number 2 is **not yet implemented** -- the localhost binding is the current sole security boundary. Spec text in section 10.0 is preserved for future hardening. The gora serve CLI stub (Phase 9 of v1 refactor) returns *MCP server is not yet implemented* from crates/agora/src/main.rs.

### 19.7 Audit Log Schema (replaces section 4.6 compile-only entries)

Per the E8 user decision (expand to match section 4.6), the compiler (compiler/compile.py) now writes audit entries with the full section 4.6 schema -- every entry includes 	imestamp, ction, ctor, 	arget_type, 	arget_id, 
eason, details. Action types cover: compile (every nightly run), AUTO_FLAG (velocity circuit breaker), POLL_CREATED, POLL_CLOSED, IMMUNITY_APPLIED, IMMUNITY_REMOVED, ARCHIVED, RESTORED, BLACKLIST_UPDATED, REACTION_SCRUBBED, SIGNATURE_REJECTED (client-side). The root audit_log object carries log_format_version: 1. Rotation per section 4.6: at 10,000 entries, the oldest 2,000 move to 
egistry/governance/audit_log_archive.{YYYYMMDD}.json (new archive file per day; existing archive appended to).

### 19.8 Triage Poll Resolution Bugs (fixed 2026-07-05)

Per the A4 fixes during the 2026-07-05 audit:

- **Tied polls** (keep_votes == remove_votes with 	otal_votes > 0): now treated as KEEP-win, with an audit entry noting the tie. Items with **zero total votes** remain under_review (no resolution) and an audit entry is written explaining the no-vote situation.
- **Organic nomaly_window_start is now preserved** across nightly runs -- it is only set when the item first transitions to under_review. Previously every nightly build overwrote it with 
ow, so the 7-day poll timer never elapsed.
- **KEEP-win now writes immunity_cooldown_until** -- a 30-day ISO timestamp populated on the 
egistry_items row. (Previously the column was always NULL.)

### 19.9 Deferred / Removed Features

- **Anonymous Crash Telemetry (section 12) -- removed (E5):** the opt-in prompt UI and the crash_telemetry_opt_in setting have been deleted. The local_crash_telemetry table is still populated locally (for section 19.4 signal B/C), but no aggregation endpoint exists and nothing leaves the device. The spec text in section 12 is kept for historical reference only.
- **Raw Modrinth Tab (section 6.3 / desktop/src/pages/ModrinthRaw.tsx) -- removed (E4):** instead, Modrinth search results are federated directly into the Browse grid via the rowse_search command (desktop/src-tauri/src/commands.rs), with the Modrinth-on setting still gated by the modrinth_enabled toggle in Settings.
- **Strict allowedElements markdown rendering (section 4.1c number 2):** the original spec mandated a strict allow-list (p, strong, em, code, a, pre, ul, ol, li). In practice this broke Modrinth about-page rendering (which legitimately uses tables, headings, images, <center> elements). The current implementation keeps 
ehypeRaw + 
ehypeSanitize (with a tightened schema that strips <script>/<iframe>/<input>/<video>/<form>/etc. but allows structural + image tags). See web/src/components/MarkdownRenderer.tsx and desktop/src/pages/ModDetail.tsx SANITIZE_SCHEMA.

### 19.10 CLI (crates/agora/) -- Current Capability Set

The standalone gora CLI binary (Phase 9 of v1 refactor) implements: instances list/details, mods install/remove, health <instance>, 
egistry status/sync, snapshots list/create/restore, import <path> (mrpack/zip/directory), launch <instance> (MSA check + health gate + direct Java spawn), uth login/status/logout.

Not yet implemented: gora serve (MCP server mode -- returns stub), the --yes health-confirm skip flag (currently always gates on health). The CLI mod-download path enforces a host allowlist + redirect policy (MOD_ALLOWED_HOSTS in crates/agora/src/main.rs) parallel to the desktop MOD_DOWNLOAD_ALLOWLIST.

### 19.11 Implementation Phase Status

Cross-referenced against section 17 phase definitions:

| Phase | Status | Notes |
|---|---|---|
| 0 (Repo & Data plumbing) | Done | registry/, loader-manifests/, crash-signatures/, .github/workflows/. |
| 1 (Compiler) | Done | All of section 3 incl. trust filtering, velocity breaker, Raid Shield, NLP scrubbing, audit log (post-E8 expansion), Modrinth hydration, Ed25519 signing, GitHub Release Asset deploy. |
| 2 (Tauri skeleton + instance engine) | Done (with v1 crate migration in progress) | agora-core holds most logic; desktop still has thick mod_install/instances/crash_investigator/mcp modules pending migration. |
| 3 (Browse, discovery, search) | Done | Browse + Modrinth federated into single page; For-You algorithm; debounce + removed per-item log spam during 2026-07-05 audit. |
| 4 (Crash diagnostics) | Done | Pre-launch interceptor, regex triage, GitHub issue search, preview-before-submit, manual log viewer, section 19.4 dynamic investigator wired (A5). |
| 5 (Governance & Triage) | Done | Triage Center tab, Curator Shield, Flag Review, Transparency Log. |
| 6 (MCP Server) | Done | All 6 currently-implemented tools functional; per-session auth deferred (B2 approval pending user re-decision). |
| 7 (Dev Mode sandboxed builds) | Not started | Spec section 11. |
| 8 (Web directory) | Done | Static Next.js, registry.db fetched at CI build via scripts/fetch_registry_db.py, react-markdown with tightened sanitize schema, CSP header in 
ext.config.js. |
| 9 (Polish & Hardening) | Partial | Auto-update done, i18n engine present but ~99 percent of UI strings still hard-coded English (separate cleanup session), code signing not started, telemetry removed (E5), disk pre-check fixed (cross-platform now), atomic writes fixed for instance manifest (A8). |

### 19.12 Future Cleanup Backlog (low-priority)

- Migrate crash_investigator.rs, mod_install.rs, instances.rs, mojang.rs, mcp.rs, ersion_cache.rs from desktop to agora-core (finish v1 refactor Phase 1).
- Implement the 4 missing MCP tools (
ead_latest_crash, 
ead_mod_manifest, enable_mod, search_knowledge_base) per E2 superset.
- Add the MCP Bearer token (section 10.0 number 2) per user re-decision.
- Delete the unused crates/agora-core/src/catalog/ trait + ModrinthSource impl (zero callers) -- was speculative design from a prior planning iteration.
- Delete the unused crates/agora-core/src/ctx.rs Ctx struct + state.rs AppState re-export (zero callers) -- replaced by ad-hoc per-module DB connection patterns; a proper Ctx struct may be reintroduced when finishing the v1 refactor.

---

**This MASTER_SPEC.md is the single authoritative spec. The previously-separate plan files (1782081355093-crash-investigator-plan.md, 1782611768583-agora-v1-launcher-refactor.md, dependency-aware-mod-ops-plan.md) have been deleted; their key decisions are captured in section 19 above. BACKLOG.md remains the canonical per-phase task tracker.**
