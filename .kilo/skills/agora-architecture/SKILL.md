---
name: agora-architecture
description: Architecture overview of the Agora Minecraft mod launcher monorepo.
---
# Agora Architecture

This skill describes the Agora project layout and data flow. Refer to `.kilo/plans/MASTER_SPEC.md` for the authoritative blueprint.

## Monorepo Layout

- `registry/` — Flat-file "database" of curated mods, packs, shaders, resource packs, servers, datapacks, worlds, and governance records. Every file is a JSON manifest committed via PR.
- `crash-signatures/` — Regex-based crash triage definitions used by the desktop client.
- `loader-manifests/` — Pinned modloader distribution metadata (download URLs + SHA-256).
- `compiler/` — Python compiler that ingests the flat files and emits `registry.db` plus signatures.
- `desktop/` — Tauri desktop app (Rust backend, React/Tailwind frontend).
- `web/` — Next.js static web directory for discovery and governance views.
- `scripts/` — Utility scripts (e.g., `verify_db.py`).

## Compiler Pipeline

1. Read every manifest under `registry/` and `crash-signatures/`.
2. Resolve release assets from GitHub or Modrinth, computing and pinning SHA-256 hashes.
3. Emit a versioned SQLite database (`registry.db`) consumed by the desktop app.
4. Sign `registry.db` in CI using `ED25519_PRIVATE_KEY`.

## Desktop App

The Tauri app uses `tauri-plugin-sql` for `registry.db` (read-only) and `local_state.db` (user/instance state). It delegates Microsoft/Xbox auth and JVM execution to the official Mojang launcher.

## Web Directory

The Next.js site provides public catalog browsing, governance polls, and docs. It is static exportable to Vercel or GitHub Pages.

## Security Principles

- Decentralized trust: pinned hashes over mutable URLs.
- Delegation: no direct Microsoft auth or JVM control in-app.
- Client-side scalability: GitHub API calls use the user's own OAuth token.
- Zero server footprint: data ships as a signed GitHub Release Asset and static site.

## Key MASTER_SPEC.md Sections

- §1 Repository Structure
- §2 Source JSON Schemas
- §3 The Nightly Compiler
- §4 Client-Side SQLite Schema
- §6 The Tauri Desktop App
- §8 The Execution Engine
- §15 Security Architecture
