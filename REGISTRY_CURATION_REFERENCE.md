# Agora Registry Curation Reference

A self-contained reference for an agent who does **not** have access to the Agora repository. With this document alone, you can author valid mod / pack / shader / resource pack / datapack / server / world manifests for the curated registry.

---

## 1. Repository layout (flat-file manifest)

Everything lives under `registry/` in the Agora monorepo. Each content type has its own subdirectory, and one JSON file = one registry entry. The filename (minus `.json`) should match the manifest's `id` / `pack_id`.

```
registry/
├── mods/              ← .jar mods
│   ├── sodium.json
│   ├── fabric-api.json
│   └── ...
├── packs/             ← Curated modpacks
│   └── optimized-survival.json
├── shaders/           ← Shader packs
├── resourcepacks/     ← Resource packs
├── servers/           ← Server configs
├── datapacks/         ← Datapacks
├── worlds/            ← Pre-built worlds
├── pack-overrides/    ← (Optional) zip bundles of configs for a pack
├── governance/        ← Cross-cutting policy files (see §5)
│   ├── known_conflicts.json
│   ├── poll_blacklist.json
│   └── audit_log.json
└── archived/          ← Disabled entries (compiler SKIPS this dir entirely)
```

A nightly compiler (`.github/workflows/compile.yml`) walks the 7 content dirs (`mods`, `packs`, `shaders`, `resourcepacks`, `servers`, `datapacks`, `worlds`) and compiles every `*.json` into a signed SQLite database (`registry.db`) that the desktop + web apps consume. Anything in `registry/archived/` is ignored entirely — that's how you retire an entry without deleting history.

---

## 2. Mod manifest schema (`registry/mods/<id>.json`)

Required fields: `id`, `name`, `content_type`, `author`, `license`, `download_strategy`, `source_identifier`, `sha256`. Other fields are optional or auto-populated.

### Full example (GitHub-release strategy)

```json
{
  "id": "sodium",
  "name": "Sodium",
  "content_type": "mod",
  "author": "CaffeineMC",
  "license": "LGPL-3.0",
  "download_strategy": "github_release",
  "source_identifier": "CaffeineMC/sodium",
  "sha256": "ee9d62778c8b664aa8501af83ec4738e01d20f2cdca133208c7bf66cbcaa37b8",
  "package_signatures": [
    "me.jellysquid.mods.sodium",
    "net.caffeine.sodium"
  ],
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

### Minimal example (Modrinth-ID strategy)

The shortest valid manifest. The compiler queries Modrinth's API to hydrate icon, gallery, description, body markdown, page URL, license, and `compatible_versions` automatically.

```json
{
  "id": "xaeros-minimap",
  "name": "Xaero's Minimap",
  "content_type": "mod",
  "author": "Xaero96",
  "license": "LicenseRef-ARR",
  "download_strategy": "modrinth_id",
  "source_identifier": "1bokaNcj",
  "modrinth_id": "1bokaNcj",
  "sha256": "acba53bff782903d64ed8d92fc1f21116830f389c003eb37bc49579980f333bf",
  "package_signatures": ["xaero.minimap"],
  "base_categories": ["utility", "navigation"],
  "community_categories": ["client-only", "minimap", "vanilla-plus"],
  "curator_note": "Lightweight client-side minimap with waypoints. A solid vanilla-plus navigation aid; disable for a purist experience.",
  "icon_url": "https://cdn.modrinth.com/data/1bokaNcj/354080f65407e49f486fcf9c4580e82c45ae63b8_96.webp",
  "gallery_urls": [],
  "governance": {
    "immune": false,
    "override_justification": null,
    "allow_comments": true
  }
}
```

### Closed-source / direct-hash example

For mods that are self-hosted and not on GitHub or Modrinth. The hash is **manually pinned** by the developer; if they silently change the file without a PR, every existing download is blocked for all users.

```json
{
  "id": "proprietary-mod",
  "name": "Proprietary Mod",
  "content_type": "mod",
  "author": "Developer Name",
  "license": "LicenseRef-Proprietary",
  "download_strategy": "direct_hash",
  "source_identifier": "https://developer.com/releases/mod-v1.0.0.jar",
  "sha256": "a1b2c3d4e5f6...(64 hex chars)",
  "package_signatures": ["com.developer.mod"],
  "base_categories": ["content"],
  "community_categories": [],
  "curator_note": "",
  "governance": {
    "immune": false,
    "override_justification": null,
    "allow_comments": true
  }
}
```

### Field reference

| Field | Type | Required? | Description |
|---|---|---|---|
| `id` | string | Yes | Unique slug, lowercase, hyphenated. Must match the filename (minus `.json`). |
| `name` | string | Yes | Display name shown to users. |
| `content_type` | string | Yes | Always `"mod"` for this directory. Other valid values: `pack`, `shader`, `resourcepack`, `server`, `datapack`, `world`. |
| `author` | string | Yes | Creator or organization name. |
| `license` | string | Yes | SPDX license identifier (see §3 below). |
| `download_strategy` | string | Yes | One of: `github_release` (primary), `modrinth_id` (supplementary fallback), `direct_hash` (closed-source / self-hosted). |
| `source_identifier` | string | Yes | Depends on strategy: `github_release` → GitHub `"owner/repo"`; `modrinth_id` → Modrinth project ID; `direct_hash` → direct HTTPS URL to the file. |
| `sha256` | string | Yes | SHA-256 hash of the downloadable file (64 lowercase hex chars). For `github_release` and `modrinth_id`, the compiler populates this from API metadata. For `direct_hash`, it MUST be manually provided. The launcher **blocks download** if the computed hash doesn't match. |
| `package_signatures` | string[] | Recommended | Java package prefixes used to attribute crash-log stack frames to this mod (e.g. `me.jellysquid.mods.sodium`). Use 2+ segments; single top-level like `net` is too broad. |
| `base_categories` | string[] | Recommended | Official curated category tags. Free-form lowercase strings. |
| `community_categories` | string[] | Optional | Freeform community tags. Auto-discovered by the compiler if absent. |
| `curator_note` | string | Recommended | Human-written markdown writeup shown in the UI and used as AI semantic context. |
| `icon_url` | string | Optional | CDN URL for the mod's icon. For `github_release`, provide manually (e.g. point to `raw.githubusercontent.com`). For `modrinth_id`, auto-populated. |
| `gallery_urls` | string[] | Optional | Array of CDN URLs for screenshots. Auto-populated from Modrinth or manually provided. |
| `compatible_versions` | array | Optional | Array of `{mc_version, loader, mod_version}` objects. If absent, the compiler queries Modrinth for real version data (when a `modrinth_id` is resolvable). Otherwise falls back to `[{mc_version: "1.21", loader: "fabric", mod_version: "latest"}]`. **Do not set this manually unless you need to override** — the hydrator does it for you. |
| `mod_dependencies` | object | Optional | `{required: [...], optional: [...], incompatible: [...]}`. Mod IDs this mod depends on / is incompatible with. If absent, the compiler extracts from the jar's `fabric.mod.json` / `mods.toml` at install time (desktop-side), so this is a curated override only. |
| `mod_jar_aliases` | string[] | Optional | Alternate jar-declared IDs for cross-source matching (e.g. catalog `fabric-api` ↔ jar `fabric` ↔ Modrinth `fabric_api`). Lets the dependency-aware install system match across sources. Only needed when the jar-declared ID differs from the manifest `id`. |
| `governance.immune` | boolean | Optional (default `false`) | If `true`, bypasses all automated triage, vote penalties, and velocity circuit breakers. |
| `governance.override_justification` | string\|null | Required if `immune=true` | Displayed verbatim in the UI. |
| `governance.allow_comments` | boolean | Optional (default `true`) | If `false`, the review section is locked on this mod's page. |

---

## 3. SPDX license identifiers

The `license` field must be a valid SPDX identifier. Common examples:

- `MIT` — permissive
- `Apache-2.0` — permissive with patent grant
- `LGPL-3.0` — weak copyleft (Sodium, Iris, Lithium)
- `GPL-3.0` — strong copyleft
- `MPL-2.0` — weak file-level copyleft
- `LicenseRef-ARR` — All Rights Reserved (closed-source / proprietary; used by Xaero's Minimap)
- `LicenseRef-Proprietary` — for closed-source self-hosted mods
- `LicenseRef-<CustomName>` — any custom license; include a `license_url` or explanation in `curator_note`

Custom or non-open-source licenses MUST use the `LicenseRef-*` prefix. Do NOT invent SPDX-like strings (e.g. `"ARR"` without the prefix is invalid).

---

## 4. Modpack manifest schema (`registry/packs/<id>.json`)

Packs reference mods by ID and declare which loader + MC version the pack targets. A pack can mix mods from the curated registry, Modrinth (referenced by ID), and GitHub releases.

### Full example

```json
{
  "pack_id": "optimized-survival",
  "name": "Community Optimized Survival",
  "minecraft_version": "1.21",
  "loader": "fabric",
  "loader_version": "0.15.11",
  "mods": [
    { "id": "sodium", "source": "manifest", "status": "required" },
    { "id": "lithium", "source": "manifest", "status": "required" },
    { "id": "starlight", "source": "manifest", "status": "required" },
    { "id": "fabric-api", "source": "manifest", "status": "required" },
    {
      "id": "iris",
      "source": "manifest",
      "status": "recommended",
      "description": "Enable this if you want to use shader packs."
    },
    {
      "id": "xaeros-minimap",
      "source": "modrinth_id",
      "modrinth_id": "1bokaNcj",
      "version": "24.2.0",
      "status": "optional",
      "description": "Client-side minimap. Disable for a pure vanilla feel."
    }
  ],
  "override_url": null,
  "curator_note": "A curated, performance-focused survival pack for 1.21. Vanilla+ aesthetic with dramatically improved framerates.",
  "governance": {
    "immune": false,
    "override_justification": null,
    "allow_comments": true
  },
  "sha256": "de1d1fc288c327a2980c11dfbb370976f66f309a7dfcd72a746d82bc9623f51b"
}
```

### Pack-specific fields

| Field | Type | Required? | Description |
|---|---|---|---|
| `pack_id` | string | Yes | Unique slug for the pack (instead of `id`). |
| `minecraft_version` | string | Yes | Target Minecraft version. |
| `loader` | string | Yes | Target loader: `fabric`, `quilt`, `forge`, `neoforge`. |
| `loader_version` | string | Yes | Pinned loader version. |
| `mods` | array | Yes | List of mod entries (see below). |
| `override_url` | string\|null | Optional | URL to a zip of configs / resourcepacks / shaderpacks to apply as overrides when installing the pack. Set to `null` if the pack has no overrides. |
| `sha256` | string | Optional | Hash of the override zip (if `override_url` is set). |

### Pack mod-entry fields

Each entry in `mods[]`:

| Field | Type | Required? | Description |
|---|---|---|---|
| `id` | string | Yes | Mod registry ID (if `source: "manifest"`) or display ID. |
| `source` | string | Yes | `manifest` (lookup in registry.db), `modrinth_id` (query Modrinth API directly), or `github_release`. |
| `modrinth_id` | string | Required when `source: "modrinth_id"` | The Modrinth project ID. |
| `version` | string | Optional | Exact version string. If omitted, the launcher defaults to the latest version compatible with the pack's `minecraft_version` + `loader`. |
| `status` | string | Yes | `required`, `recommended`, or `optional`. Drives the UI badge and whether the pack install flow aborts on failure. |
| `description` | string | Optional | Tooltip shown next to the mod in the pack install UI. |

---

## 5. Other content types

For shaders, resource packs, datapacks, servers, and worlds: the schema is the same as the mod manifest (§2), differing only in `content_type` and the directory:

| Content type | Directory | `content_type` value | Notes |
|---|---|---|---|
| Mod | `registry/mods/` | `"mod"` | See §2. |
| Pack | `registry/packs/` | `"pack"` | Uses `pack_id` instead of `id`; see §4. |
| Shader | `registry/shaders/` | `"shader"` | The downloader writes to `<instance>/shaderpacks/`. |
| Resource pack | `registry/resourcepacks/` | `"resourcepack"` | Writes to `<instance>/resourcepacks/`. |
| Server | `registry/servers/` | `"server"` | Server configuration / mod set. |
| Datapack | `registry/datapacks/` | `"datapack"` | Writes to `<instance>/datapacks/`. |
| World | `registry/worlds/` | `"world"` | Pre-built world download. |

**Shaders / resource packs / datapacks** typically use `modrinth_id` or `direct_hash` strategy. They are NOT `.jar` files (usually `.zip`), so:
- `package_signatures` is irrelevant (no Java packages) — leave as `[]` or omit.
- The desktop app routes the download to the correct instance subdirectory based on `content_type` (`shaderpacks/`, `resourcepacks/`, `datapacks/`).
- SHA-256 verification still applies — the hash must match the downloaded file.

---

## 6. Governance files (`registry/governance/`)

These are cross-cutting policy files that affect the whole registry, not a single entry.

### 6.1 Known conflicts (`known_conflicts.json`)

A JSON array of mod-pair conflicts. Used by the crash investigator (signal G in the dynamic scoring algorithm) and by the dependency-aware install system.

```json
[
  {
    "a": "optifine",
    "b": "sodium",
    "severity": "hard",
    "mitigated_by": [],
    "notes": "OptiFine and Sodium both replace the vanilla renderer; running both causes startup/crash conflicts."
  },
  {
    "a": "optifine",
    "b": "rubidium",
    "severity": "hard",
    "mitigated_by": [],
    "notes": "Rubidium (Forge Sodium port) conflicts with OptiFine for the same renderer-replacement reason."
  }
]
```

| Field | Type | Description |
|---|---|---|
| `a` | string | First mod ID (lexicographically smaller). |
| `b` | string | Second mod ID (lexicographically larger). |
| `severity` | string | `"hard"` (will crash) or `"weak"` (may work but not recommended). |
| `mitigated_by` | string[] | Mod IDs that, when present, neutralize the conflict (e.g. `["indium"]` for Sodium+OptiFine). |
| `notes` | string | Free-text explanation. |

### 6.2 Poll blacklist (`poll_blacklist.json`)

A list of GitHub usernames excluded from triage poll vote tallies (bots, known bad actors). Empty by default.

```json
{"usernames": []}
```

### 6.3 Audit log (`audit_log.json`)

Compiler-generated; do not edit manually. Appended to on every compile.

---

## 7. Crash signatures (`crash-signatures/<id>.json`)

Not under `registry/` — these live at the repo root in `crash-signatures/`. Each file defines a regex pattern that matches a known crash type + a human-readable fix hint + an optional action button.

### Example

```json
{
  "id": "fabric-api-missing",
  "name": "Missing Fabric API",
  "regex_pattern": "requires \\{fabric @",
  "solution_markdown": "A mod you installed requires **Fabric API**, but it is missing from your mod folder. Click the button below to install it automatically.",
  "action_button": {
    "label": "Install Fabric API",
    "mod_id": "fabric-api"
  }
}
```

| Field | Type | Description |
|---|---|---|
| `id` | string | Unique slug matching the filename. |
| `name` | string | Display name shown in the crash diagnostic UI. |
| `regex_pattern` | string | Rust regex (no backreferences, no unbounded backtracking). Max 256 chars. Test against a ≥100KB crash log before merging. |
| `solution_markdown` | string | Markdown shown to the user explaining the fix. |
| `action_button` | object\|null | Optional `{label, mod_id}` — renders a button that installs the named mod. |
| `action_button.label` | string | Button text. |
| `action_button.mod_id` | string | Registry mod ID to install when clicked. |

### Regex DoS safety rules

1. The Rust `regex` crate is the only engine used — it structurally prevents catastrophic backtracking.
2. Maximum pattern length: 256 characters.
3. Anchor patterns where possible.
4. Prefer plain substring matches for known class names / error strings over regex.
5. Test every new pattern against a 100KB+ crash log before submitting the PR.

---

## 8. SHA-256 hash requirements

The `sha256` field (and `sha256` for packs) must be:
- A string (not a number).
- Exactly 64 lowercase hexadecimal characters.
- The actual SHA-256 of the downloadable file the user will receive.

How to compute it locally before submitting a PR:

**PowerShell (Windows)**
```powershell
(Get-FileHash .\mod-file.jar -Algorithm SHA256).Hash.ToLower()
```

**Python**
```python
import hashlib
print(hashlib.sha256(open("mod-file.jar", "rb").read()).hexdigest())
```

**Bash (Linux / macOS)**
```bash
sha256sum mod-file.jar | cut -d' ' -f1
```

For `github_release` and `modrinth_id` strategies, the compiler populates the hash automatically from API metadata — you don't need to compute it yourself, but the field must still be present (it will be overwritten on compile).

---

## 9. Submitting a new entry (PR workflow)

1. **Create the manifest file** in the appropriate `registry/<type>/` directory. The filename must match the `id` (e.g. `registry/mods/my-cool-mod.json` → `"id": "my-cool-mod"`).
2. **Compute the SHA-256** of the downloadable file (§8) and put it in the `sha256` field.
3. **For mods**: populate `package_signatures` with the Java package prefixes found inside the `.jar` (open it as a zip and look at the top-level directories). Use 2+ segments.
4. **Test locally** (if you have the repo checked out):
   ```bash
   python compiler/compile.py --skip-sign
   ```
   This compiles `registry.db` from the flat files. Verify your entry appears:
   ```bash
   python -c "import sqlite3; print(sqlite3.connect('registry.db').execute('SELECT id, name FROM registry_items').fetchall())"
   ```
5. **Submit a PR** with the new manifest file. The nightly CI compile runs `compile.py` and ships a new signed `registry.db` to GitHub Releases. Your entry appears in the Browse tab after the next nightly compile.

### Curation principles

- **Boutique, not warehoused.** Every entry is community-reviewed. Quality over quantity.
- **`github_release` is the preferred strategy** — it points directly to the developer's own GitHub release, with no intermediary. Use `modrinth_id` when the mod isn't on GitHub or as a supplementary metadata source. Use `direct_hash` only for closed-source / self-hosted mods.
- **Curator notes matter.** The `curator_note` field is shown in the UI and used as semantic context for the AI crash investigator. Write a clear, 1-3 sentence summary of what the mod does and why a user would (or wouldn't) want it.
- **Don't set `compatible_versions` manually** unless you have a specific reason to override. The compiler fetches real version data from Modrinth's API for any mod with a resolvable `modrinth_id` (or whose manifest `id` matches a Modrinth slug). Manual overrides should be rare.
- **Immunity is rare.** `governance.immune: true` should only be set for mods that are foundational and shouldn't be subject to community vote triage (e.g. a core API). Always include `override_justification` when doing this.
- **Archiving, not deleting.** To retire an entry, move its JSON file to `registry/archived/`. The compiler skips that directory entirely, so the entry disappears from the compiled database without losing git history.

---

## 10. Validation checklist

Before submitting a PR, verify:

- [ ] Filename matches `id` (or `pack_id` for packs).
- [ ] `content_type` matches the directory (mods → `"mod"`, packs → `"pack"`, etc.).
- [ ] `license` is a valid SPDX identifier (or `LicenseRef-*` for custom).
- [ ] `download_strategy` is one of `github_release`, `modrinth_id`, `direct_hash`.
- [ ] `source_identifier` matches the strategy format (GitHub `owner/repo`, Modrinth ID, or HTTPS URL).
- [ ] `sha256` is 64 lowercase hex chars (compute via §8).
- [ ] `package_signatures` uses 2+ segment prefixes (for mods).
- [ ] `governance.immune` is `false` unless you have an `override_justification`.
- [ ] No file in `registry/archived/` has the same `id`.
- [ ] If adding a pack: every mod in `mods[]` either exists in `registry/mods/` (when `source: "manifest"`) or has a valid `modrinth_id` (when `source: "modrinth_id"`).
- [ ] If adding a known conflict: `a` is lexicographically smaller than `b`.
- [ ] If adding a crash signature: test the regex against a 100KB+ crash log; max 256 chars.

---

## 11. Autopopulated fields (don't set these manually unless overriding)

The nightly compiler hydrates these from the Modrinth API for any mod with a resolvable Modrinth presence (explicit `modrinth_id` OR a manifest `id` that matches a Modrinth slug):

- `icon_url` — from Modrinth project data.
- `gallery_urls` — from Modrinth project's gallery array.
- `description` — short description from Modrinth.
- `body_markdown` — full README-style body from Modrinth.
- `page_url` — constructed from the Modrinth slug.
- `license_id` — from Modrinth's license object.
- `source_updated_at` — from Modrinth's `updated` timestamp.
- `compatible_versions` — fetched from `/v2/project/{id}/version`, deduplicated by `(mc_version, loader)` pair with the latest mod_version per pair.
- `_hydrated_categories` — categories from Modrinth, filtered to remove loader-name noise.

**Manifest values always take precedence** over API-hydrated values. If you set `icon_url` in the manifest, the compiler keeps yours. If you leave it empty, the hydrator fills it from Modrinth. This is the "curator override" principle.

For mods with no Modrinth presence (pure GitHub-release mods whose slug doesn't match a Modrinth project), these fields stay empty unless you provide them manually. `compatible_versions` falls back to `[{mc_version: "1.21", loader: "fabric", mod_version: "latest"}]` — so set it manually if the mod targets a different version.

---

## 12. Common mistakes to avoid

1. **Using `"ARR"` as the license** — it's invalid. Use `"LicenseRef-ARR"` for All Rights Reserved.
2. **Single-segment `package_signatures`** like `["net"]` or `["com"]` — too broad; they match thousands of mods. Use at least 2 segments: `["net.fabricmc.fabric"]`.
3. **Setting `compatible_versions` to a hardcoded list** when you don't need to — the compiler fetches real data from Modrinth. Only override if the hydrator is wrong or the mod isn't on Modrinth.
4. **Uppercase `sha256`** — must be lowercase hex.
5. **Putting a pack manifest in `registry/mods/`** — packs go in `registry/packs/` and use `pack_id` instead of `id`.
6. **Deleting a manifest to retire it** — move to `registry/archived/` instead to preserve git history.
7. **Setting `governance.immune: true` without `override_justification`** — the compiler rejects it.
8. **Inventing a `download_strategy`** like `"curseforge"` — only `github_release`, `modrinth_id`, and `direct_hash` are supported.
9. **Using a URL as `source_identifier` for `github_release`** — it must be `owner/repo` format (e.g. `CaffeineMC/sodium`), not a full URL.
10. **Forgetting `sha256` on a `direct_hash` mod** — it's required for all strategies; for `direct_hash` it's the only integrity guarantee and must be manually provided.
