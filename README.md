# Agora Minecraft Mod Launcher

> This is not a warehouse. This is a boutique.

A decentralized, ad-free, open-source Minecraft mod launcher and discovery platform built to return platform control to the community. Curated mods, packs, shaders, and more are delivered as a signed SQLite database compiled nightly from flat JSON manifests stored directly in this repository.

## Mission

The **"Agora"** mission is simple: bypass centralized commercial infrastructure and serve a high-quality, community-governed catalog directly from developer-controlled sources. If traditional mod platforms are a beer, this is Agora.

Core principles:

- **$0/month server footprint** — GitHub, GitHub Release Assets, and the official Mojang launcher handle everything.
- **Security by delegation** — We never touch Microsoft/Xbox auth or JVM execution.
- **Decentralized governance** — Votes, reviews, and triage live as structured GitHub interactions.
- **Modrinth independence** — Primary source is `github_release`; Modrinth is an optional fallback.

## Tech Stack

| Layer | Technology |
| --- | --- |
| Desktop backend | Tauri (Rust) |
| Desktop frontend | React + Tailwind CSS |
| Web directory | Next.js (static) |
| Client DB | SQLite (`tauri-plugin-sql`) |
| Compiler | Python (GitHub Actions) |
| Game execution | Official Mojang Launcher |
| AI integration | Local MCP server |
| Data hosting | GitHub Release Assets |

## Monorepo Layout

```
/registry/          Curated data store (the "GitHub database")
  mods/            Curated mod manifests
  packs/           Curated modpack manifests
  shaders/         Shader pack entries
  resourcepacks/   Resource pack entries
  servers/         Listed server entries
  datapacks/       Datapack entries
  worlds/          World download entries
  governance/      Community governance data
  pack-overrides/  Config/resource override zips
  archived/        Removed items
/crash-signatures/ Regex-based crash triage signatures
/loader-manifests/ Pinned modloader hashes and domain allowlists
/.github/
  workflows/       CI/CD (nightly compiler)
  ISSUE_TEMPLATE/  Structured community forms
/compiler/         Python nightly compiler
/desktop/          Tauri desktop application (React + Tailwind + Rust)
/web/              Static Next.js public directory
/scripts/          Development helper utilities
CODE_OF_ENGAGEMENT.md  Canonical review conduct rules
```

## Quick Start

### 1. Compile the registry database

```bash
cd compiler
python -m venv .venv
# .venv\Scripts\activate on Windows; source .venv/bin/activate on macOS/Linux
pip install -r requirements.txt
python compile.py --out ../registry.db
python ../scripts/verify_db.py
```

The signed database is normally published as a GitHub Release Asset by `.github/workflows/compile.yml`.

### 2. Desktop app

```bash
cd desktop
npm install
npm run build      # builds the Vite frontend under dist/
# Rust toolchain required:
npm run tauri:dev  # or cargo tauri dev from src-tauri/
```

### 3. Web directory

```bash
cd web
npm install
npm run build      # static export to web/dist/
```

## Code of Engagement

All contributors and reviewers are bound by the canonical rules in [`CODE_OF_ENGAGEMENT.md`](./CODE_OF_ENGAGEMENT.md).

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

## Environment Setup

Copy the example environment file and fill in any values you need locally:

```bash
cp .env.example .env
```

See `.env.example` for the list of supported variables.

## Agent Tooling

This repository includes Kilo agent configuration under `.kilo/`:

- `.kilo/kilo.json` — project-level model, permissions, MCP, and skill settings.
- `.kilo/agent/*.md` — agent profiles (`code`, `security`, `registry-curator`, `reviewer`).
- `.kilo/command/*.md` — slash commands: `/registry`, `/desktop`, `/web`, `/review`.
- `.kilo/skills/*/` — project-specific skills: `agora-architecture`, `tauri-security`, `registry-curation`.

See [`AGENTS.md`](./AGENTS.md) for the canonical guide to agent interactions and standards.
