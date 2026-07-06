---
name: agora-mcp
description: Guide for using the Agora launcher MCP server to diagnose Minecraft mod crashes and manage instances.
---

# Agora MCP Server Guide

## Overview

The Agora launcher runs a local MCP server on `127.0.0.1:39741` that exposes 10 tools for managing Minecraft mod instances and diagnosing crashes. The server is built into the desktop app and runs when the user has "AI / MCP Server" enabled in Settings (disabled by default in the shipped app). **All connections require a Bearer token** -- the token is generated automatically on first MCP-enable and displayed in Settings -> Integrations -> MCP Server.

The server speaks JSON-RPC 2.0 over HTTP with SSE (Server-Sent Events) for response delivery. Tools are called via the `tools/call` method with `name` and `arguments` in the params. All tool responses are wrapped in a JSON object with `content` (array of `{type: "text", text: "..."}`) and `isError` fields.

## The 6 Tools

### 1. `list_instances`

- **Params:** None
- **Returns:** `{"instances": [{"instance_id", "name", "minecraft_version", "loader", "loader_version"}, ...]}`
- **Use when:** You need to know what instances exist before any instance-specific operation. Always call this first when a user mentions an instance by name so you can match it to an `instance_id`.

### 2. `list_instance_mods`

- **Params:** `instance_id` (string) — the instance ID to list mods for.
- **Returns:** `{"mods": [{"filename", "version", "source", "mod_jar_id", "depends_on", "optional_deps", "java_packages"}, ...]}`
- **Use when:** You need to see what mods are installed, check dependency chains, or find which mod a Java package belongs to. Call after `list_instances` to inspect a specific instance's mod list.

### 3. `disable_mod`

- **Params:** `instance_id` (string), `filename` (string) — the mod filename to disable.
- **Returns:** On success: `{"content": [{"type": "text", "text": "Mod <filename> disabled in instance <id>. Restart the game to apply."}], "isError": false}`. On failure: `{"content": [{"type": "text", "text": "<error message>"}], "isError": true}`.
- **Destructive — requires user approval.** The user must grant `disable_mod` permission for each instance in Agora Settings → MCP → Approvals. If not granted, returns `ERR_MCP_DENIED` with an error message directing the user to grant permission.
- **Use when:** You've identified a suspect mod and want to disable it for testing. The user must restart the game for the change to take effect.

### 4. `search_crash_signatures`

- **Params:** `crash_text` (string) — the full crash log text.
- **Returns:** `{"matches": [{"id", "name", "fix_hint"}, ...]}` — curated crash signature patterns that match the crash text, with their fix hints.
- **Use when:** A user shares a crash log — search for known curated patterns first before deeper analysis. This is the fastest path to a solution when a curated signature exists.

### 5. `suggest_mod_incompatibility`

- **Params:** `instance_id` (string), `crash_text` (string) — the crash log text to analyze.
- **Returns:** `{"suspects": [{"mod_id", "filename", "total_score", "is_dependent_of", "breakdown"}, ...]}` — ranked suspect mods from a dynamic weighted scoring algorithm.
  - `total_score`: higher means more likely to be the cause.
  - `is_dependent_of`: set to another mod's filename if this mod indirectly depends on a direct suspect (reduced score).
  - `breakdown`: per-signal score contribution. Signal letters:
    - **A** — stack-frame attribution (the mod's `java_packages` appear in the crash stack trace).
    - **B** — fingerprint recurrence (this mod has been attributed in past crashes).
    - **C** — co-crash signal (this mod co-occurs in crashes with other suspects).
    - **D** — dampened by survival ubiquity (penalty for mods installed everywhere).
    - **E** — confirmed prior (the mod was previously confirmed as a crash cause).
    - **G** — curated conflict (a curated conflict entry flags these two mods).
    - **F** — recency factor (recent attributions weigh more).
- **Use when:** A crash log doesn't match a curated signature, or you need deeper analysis of which mod is causing the crash. The scoring algorithm gets smarter over time — confirmed attributions create priors that speed up future diagnosis.

### 6. `get_system_context`

- **Params:** None
- **Returns:** A markdown summary with sections for Instances, Installed Mods, and Recent Crashes.
- **Use when:** You need an overview of the user's system state before diagnosing an issue, or when the user asks "what do you know about my setup?".

### 7. `read_latest_crash`

- **Params:** `instance_id` (string) -- the instance ID.
- **Returns:** The last 200 lines of the newest crash report file (e.g. `crash-2026-07-06_00.23.24-client.txt`), plus the filename and total line count.
- **Use when:** A user reports a crash for a known instance and you need to quickly read the most recent crash log. Always call this before `suggest_mod_incompatibility`.

### 8. `read_mod_manifest`

- **Params:** `mod_id` (string) -- the curated registry ID (e.g. `"sodium"`, `"fabric-api"`).
- **Returns:** The full registry item record: name, content_type, download_strategy, license, description, body_markdown, page_url, icon_url, upvotes/downvotes/net_score/velocity, status, immunity fields, compatible_versions, and Modrinth ID.
- **Use when:** You need curator-authored context about a mod -- its description, known behaviors, compatibility notes, or community vote status. This is local SQLite data; no network call required.

### 9. `enable_mod`

- **Params:** `instance_id` (string), `filename` (string) -- the mod filename to re-enable (must end in `.jar.disabled`).
- **Returns:** On success: re-enabled notification. On failure: error message.
- **Destructive -- requires user approval.** Mirrors the `disable_mod` approval flow. Reverses a previous `disable_mod` call by renaming `.jar.disabled` back to `.jar`.
- **Use when:** The user confirms that disabling a mod didn't solve the crash, or they want to restore a previously disabled mod.

### 10. `search_knowledge_base`

- **Params:** `query` (string) -- a natural-language search string.
- **Returns:** `{"results": [{"id", "name", "content_type", "description"}, ...], "query": "<query>"}` -- up to 5 matching registry items whose name or description contains the query text (parameterized SQLite LIKE binding).
- **Use when:** A user asks a "vibe-based" question like "find me a mod that makes caves feel eerie" or "what performance mods are available?" The LIKE query searches the curated catalog locally with zero network calls.

## Crash Diagnosis Workflow

Follow this decision tree when a user reports a crash:

1. **Get context.** If you don't know the user's setup, call `get_system_context` or `list_instances` to understand what instances and mods they have.

2. **Quick check.** If the user shares a crash log, call `search_crash_signatures` first. This is the fastest path — if a curated signature matches, you can provide the fix hint immediately.

3. **Deep analysis.** If no curated match: call `suggest_mod_incompatibility` with the `instance_id` and crash text. This runs the weighted scoring algorithm across all installed mods.

4. **Review suspects.** Examine the ranked suspects — the top suspect has the highest `total_score`. Check the `breakdown` to understand why:
   - `"A: 2.0"` means the mod's Java packages appear in the crash stack trace (strongest signal).
   - `"E: 1.5"` means this mod was previously confirmed as a crash cause.
   - `"B: 1.0"` means the crash fingerprint has appeared with this mod before.
   - Look at the total score to gauge confidence.

5. **Confirm and disable.** If the top suspect looks correct:
   - Call `list_instance_mods` to confirm the mod is installed and verify its details.
   - Call `disable_mod` to disable it (requires user approval). Explain to the user that they need to restart the game for the change to take effect.

6. **Follow up.** If the crash persists after disabling the suspect:
   - Check `is_dependent_of` in the suspect list for mods that depend on the disabled one — they may be the indirect cause.
   - Consider other suspects in the ranked list.

7. **No suspects above zero.** If no suspects score above zero, the crash is likely not mod-related (game engine issue, world corruption, shaders, etc.). Advise the user accordingly.

## Important Notes

- **`disable_mod` and `enable_mod` are the only destructive tools.** It requires explicit per-instance approval in Agora Settings → MCP → Approvals. If you receive `ERR_MCP_DENIED`, tell the user to grant permission for that instance.

- **The scoring algorithm learns.** When you disable a mod and the crash stops, that attribution is recorded as a prior (signal E). This speeds up future diagnosis for similar crashes.

- **`java_packages` may be empty for older mods.** `suggest_mod_incompatibility` uses `java_packages` extracted from `.jar` files at install time to match stack frames to mods (signal A). Mods installed before this feature existed may have empty `java_packages` and won't match on signal A.

- **Indirect suspects.** Mods with `is_dependent_of` set are mods that depend on a direct suspect. They have reduced scores and are worth testing if the direct suspect is ruled out — disabling the direct mod may have disabled a dependency that another mod relied on.

- **Rate limiting.** The server enforces a rate limit of 100 requests per 60 seconds per session. If you hit `ERR_MCP_TOO_MANY_REQUESTS`, slow down your tool calls.

- **Authentication.** Every request requires a Bearer token. The token + a ready-to-paste AI client config snippet are displayed in **Agora Settings -> Integrations -> MCP Server**. Send it either as an `Authorization: Bearer <token>` header or as a `?token=<token>` query parameter on the SSE URL. The token is persistent across app restarts (generated once on first MCP-enable; you can regenerate it from Settings if needed). The server also validates `127.0.0.1`-only binding as defense-in-depth.

  **Configuring your AI client** -- copy the config snippet from Agora Settings and paste it into your AI client's MCP server configuration. For Claude Desktop, add it to `claude_desktop_config.json` under `mcpServers`:
  ```json
  {"mcpServers": {"agora": {"url": "http://127.0.0.1:39741/sse", "headers": {"Authorization": "Bearer <YOUR_TOKEN>"}}}}
  ```
  For Cursor, add the equivalent HTTP header configuration. The token is the same across all AI clients you connect.

- **Connection.** The server only accepts `127.0.0.1` (loopback) connections with a valid Bearer token. See the Settings panel for your token.
- **Tool superset.** All 10 tools (the original 6 + the 4 from MASTER_SPEC section 10.1) are now implemented per E2 (2026-07-06). The combined set is: `list_instances`, `list_instance_mods`, `disable_mod`, `search_crash_signatures`, `suggest_mod_incompatibility`, `get_system_context`, `read_latest_crash`, `read_mod_manifest`, `enable_mod`, `search_knowledge_base`.
