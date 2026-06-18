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

See `.env.example` for the list of supported variables. Note: `.env` is loaded
at runtime by the Python compiler only; the Tauri desktop app does **not** read
`.env`.

### Environment variables for the Tauri build

The Tauri desktop app reads two values **at compile time** via Rust's
`option_env!` macro — they are embedded directly into the compiled binary.
This means they must be set as **real shell environment variables** in the
session that runs `npm run tauri:dev` (or the production build step). They
are **not** read from `.env` (which is loaded at runtime by the Python
compiler only, not by the Rust build).

For production GitHub Actions builds, set both as repository **Variables**
(not Secrets — neither value is sensitive) in
repo Settings → Secrets and variables → Actions → **Variables** tab:

| Variable | Purpose | Sensitive? | Example |
|---|---|---|---|
| `AGORA_OAUTH_CLIENT_ID` | GitHub OAuth App client ID for in-app sign-in (Device Flow) | ❌ Public | `Iv1.xxxxxxxxxxxxxxxx` |
| `AGORA_REGISTRY_PUBKEY` | Ed25519 public key (hex) for verifying downloaded `registry.db` signatures | ❌ Public | `a7f07f88d56cb93c84010ed82c253a305b8528810113e51fc6920fd70a42e6e0` |

Without these, the desktop app fails fast with clear errors at the affected
feature (`ERR_AUTH_NOT_CONFIGURED` for OAuth, `ERR_REGISTRY_PUBKEY_NOT_CONFIGURED`
for signature verification) rather than silently misbehaving.

#### `AGORA_OAUTH_CLIENT_ID` — GitHub OAuth (in-app sign-in)

The desktop app's "Sign in with GitHub" button uses the OAuth Device Flow.
Register a GitHub App at <https://github.com/settings/developers> (Authorization
type: **GitHub App**, enable **Device Flow**), then grant these permissions on
the app's **Permissions** tab:

**Repository permissions:**
- **Contents** — Read-only (`GET /repos/{owner}/{repo}/releases` for mod
  install version resolution + registry release fetch)
- **Issues** — Read and write (covers issue reactions for voting, issue
  comments for reviews, and issue creation for crash reports / flag
  submission — Phase 5 governance)
  *(Metadata: Read-only is mandatory and always granted.)*

**Organization permissions:**
- **Members** — Read-only (org membership read for the Sybil/trust check,
  §3.1)

> **Note on scopes:** GitHub Apps ignore the `scope` parameter in the
> device-code request — permissions are determined solely by the app's
> settings in the GitHub UI. The Rust build does **not** send an OAuth-App
> scope string; configure everything via the app's Permissions tab above.

Then expose the app's Client ID (shown on the GitHub App's General tab —
the `Iv1.xxxxx` string; **this is public, not a secret**) in your shell:

```powershell
# PowerShell (one session)
$env:AGORA_OAUTH_CLIENT_ID = "Iv1.xxxxxxxxxxxxxxxx"
npm run tauri:dev
```

```bash
# bash/zsh (one session)
export AGORA_OAUTH_CLIENT_ID="Iv1.xxxxxxxxxxxxxxxx"
npm run tauri:dev
```

A Client Secret is **not** needed — Device Flow is specifically designed for
native apps that can't safely store a secret. If GitHub prompts for one,
generate-and-discard; never place it in this codebase.

#### `AGORA_REGISTRY_PUBKEY` — registry.db signature verification

Before trusting a downloaded `registry.db`, the desktop app verifies its
Ed25519 signature against a public key compiled into the binary. The matching
private key (`ED25519_PRIVATE_KEY`, a real secret) is held by the CI compiler
workflow only; the public key is needed on the desktop side.

If you don't yet have a keypair, generate one once (e.g. via `openssl` or the
`cryptography` Python package), store the private key in GitHub Actions
Secrets as `ED25519_PRIVATE_KEY`, and derive the public key:

```bash
# Derive the 32-byte Ed25519 public key (hex) from a 32-byte seed:
python -c "from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey; \
  from cryptography.hazmat.primitives import serialization; \
  seed = bytes.fromhex('YOUR_32_BYTE_PRIVATE_SEED_HEX'); \
  pub = Ed25519PrivateKey.from_private_bytes(seed).public_key(); \
  print('AGORA_REGISTRY_PUBKEY=' + pub.public_bytes(\
    encoding=serialization.Encoding.Raw, \
    format=serialization.PublicFormat.Raw).hex())"
```

Then set the resulting public key (without the `AGORA_REGISTRY_PUBKEY=`
prefix) in your shell before building:

```powershell
$env:AGORA_REGISTRY_PUBKEY = "a7f07f88d56cb93c84010ed82c253a305b8528810113e51fc6920fd70a42e6e0"
npm run tauri:dev
```

In debug builds (`npm run tauri:dev`), an unset `AGORA_REGISTRY_PUBKEY` is
non-fatal: signature verification is skipped with a console warning, to
keep the local-dev loop smooth. In release builds (`npm run tauri:build`),
the app refuses to verify any registry without the key compiled in.

## Agent Tooling

This repository includes Kilo agent configuration under `.kilo/`:

- `.kilo/kilo.json` — project-level model, permissions, MCP, and skill settings.
- `.kilo/agent/*.md` — agent profiles (`code`, `security`, `registry-curator`, `reviewer`).
- `.kilo/command/*.md` — slash commands: `/registry`, `/desktop`, `/web`, `/review`.
- `.kilo/skills/*/` — project-specific skills: `agora-architecture`, `tauri-security`, `registry-curation`.

See [`AGENTS.md`](./AGENTS.md) for the canonical guide to agent interactions and standards.
