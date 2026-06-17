---
name: tauri-security
description: Security hardening rules for the Tauri desktop application.
---
# Tauri Security

Security rules for the Agora desktop app. Apply these whenever modifying `desktop/` configuration or code.

## Capability Scoping

- Use explicit, narrow capability patterns. Prefer path literals over globs for privileged APIs.
- Never grant `sql:allow-execute` or arbitrary SQL execution. Use `tauri-plugin-sql` with prepared statements only.
- Shell and HTTP permissions require explicit allow-lists; use `deny` as the default and add scopes one-by-one.

## Shell / URL Scopes

- Open external URLs only through the system browser API with target `_blank`.
- Restrict shell commands to a fixed allow-list (e.g., the official Mojang launcher binary path), never a wildcard.

## Content Security Policy

- Keep the CSP tight: restrict `script-src`, `style-src`, and `connect-src`.
- Treat all mod metadata, reviews, curator notes, and crash logs as untrusted user content; render them as plain text or escaped JSX. Never use `dangerouslySetInnerHTML`.

## Database Security

- All SQL queries through `tauri-plugin-sql` must use parameter binding.
- Treat `registry.db` as read-only and `local_state.db` as read-write. Version both schemas.

## Secrets & OAuth

- Never write `GITHUB_TOKEN`, OAuth tokens, or signing keys to files or logs.
- Store OAuth tokens in the platform keychain via Tauri secure APIs; load at runtime into memory.
- The Ed25519 signing key (`ED25519_PRIVATE_KEY`) is for CI only and must never be bundled with the app.

## Modpack Sandboxing

- Validate override archives before extraction; reject files with executable extensions or absolute paths.
- Run game instances using the official Mojang launcher rather than a custom JVM invocation.
