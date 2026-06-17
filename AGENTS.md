# Agent Guide: Agora

## Mission & Ethos

Agora is a decentralized, ad-free, open-source Minecraft mod launcher and discovery platform. It returns platform control to the community by treating the GitHub repository itself as the database: flat-file manifests are compiled into a signed SQLite registry, and the app delegates authentication and game execution to the official Mojang launcher.

> "If CurseForge were a beer, this would be Agora."

Core values:
- **$0.00/month server footprint.** No backend services; data ships via GitHub Release Assets and static sites.
- **Security by delegation.** No Microsoft/Xbox auth or custom JVM execution inside the app.
- **Curated, not warehoused.** Boutique quality over infinite inventory; every entry is community reviewed.

## Directory Map

| Path | Purpose |
|---|---|
| `registry/` | Curated flat-file manifests (mods, packs, shaders, resource packs, servers, datapacks, worlds, governance) |
| `crash-signatures/` | Crash triage regex definitions |
| `loader-manifests/` | Pinned modloader URLs + SHA-256 hashes |
| `compiler/` | Python compiler that builds `registry.db` from the flat files |
| `desktop/` | Tauri desktop app (Rust backend, React frontend) |
| `web/` | Next.js static web directory |
| `scripts/` | Sanity-check and utility scripts |
| `.github/` | Workflows, issue templates, and governance forms |
| `.kilo/` | Kilo AI tooling configuration, agent profiles, commands, and skills |
| `.kilo/plans/MASTER_SPEC.md` | Authoritative engineering blueprint (read-only for agents) |

## Agent Roles

| Agent | Use for |
|---|---|
| `code` | Primary implementation in Rust, TypeScript/React, and Python |
| `security` | Security audits, threat-model reviews, hardening guidance |
| `registry-curator` | Adding or reviewing registry entries and loader manifests |
| `reviewer` | Focused code review across security, logic, and deploy safety |

## Conventions

- Treat `AGENTS.md` and `.kilo/plans/MASTER_SPEC.md` as the source of truth. Do **not** edit `MASTER_SPEC.md`.
- Prefer the smallest change that satisfies the request; avoid drive-by refactoring.
- Edit files via Kilo tools. Do not manually stage or edit files outside the project directory.
- After registry/loader/crash-signature changes, run `/registry`.
- After desktop changes, run `/desktop`.
- After web changes, run `/web`.
- Never commit or push from an agent session unless explicitly requested.
- Do not modify `.lock` files or existing data history in `registry/archived/`.
- Security defaults:
  - **Whitelist over denylist** for capabilities, shell scopes, and network access.
  - Verify every download with SHA-256 and package signatures.
  - Use `tauri-plugin-sql` with parameterized queries only.
  - Never render community content with `dangerouslySetInnerHTML`.
  - Never store secrets, tokens, or private keys in source files or manifests.

## MCP Server

When the optional Agora launcher MCP server is running locally, agents can interact with it through Kilo's MCP panel. It is bound to `127.0.0.1:39741`, is disabled by default in this project's configuration, and requires a Bearer token via the `LAUNCHER_MCP_TOKEN` environment variable. Keep MCP calls stateless and avoid privileged operations without explicit user approval.

## Environment Variables

- `ED25519_PRIVATE_KEY` — CI-only Ed25519 key used to sign `registry.db`. Never expose or bundle it.
- `LAUNCHER_MCP_TOKEN` — Optional Bearer token for the local Agora launcher MCP server.
- `GITHUB_TOKEN` — Standard GitHub token for compiler and CI operations.
