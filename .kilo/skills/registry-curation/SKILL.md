---
name: registry-curation
description: Rules for curating the Agora flat-file registry.
---
# Registry Curation

Use this skill when adding or reviewing entries under `registry/`, `crash-signatures/`, or `loader-manifests/`.

## JSON Schema Compliance

Every manifest must match the schemas in `.kilo/plans/MASTER_SPEC.md` §2. Required fields include `id`, `name`, `content_type`, `author`, `license`, `download_strategy`, `source_identifier`, and `sha256`.

## SPDX Licenses

The `license` field must be a valid SPDX license identifier (e.g., `MIT`, `LGPL-3.0`, `Apache-2.0`). Custom or non-open-source licenses must use `LicenseRef-*` where appropriate and include a `license_url` or explanation.

## Source Identifiers

- `github_release`: `source_identifier` is `owner/repo`; the compiler resolves the latest matching release asset.
- `modrinth_id`: `source_identifier` is the Modrinth project ID; the compiler calls the Modrinth API and pins hashes.
- `direct_hash`: `source_identifier` is a direct HTTPS URL and `sha256` is required and manually pinned.

## Package Signatures

`package_signatures` is a list of Java package prefixes that uniquely identify the mod in crash logs. Use specific prefixes (e.g., `me.jellysquid.mods.sodium`), not single top-level segments.

## Governance Fields

- `immune`: only `true` when there is a documented override justification.
- `override_justification`: required when `immune=true`; displayed verbatim in the UI.
- `allow_comments`: controls whether reviews are open on the mod page.

## Crash Signature Regex Safety

- Anchor regexes where possible and avoid nested quantifiers that can cause catastrophic backtracking.
- Prefer plain substring matches for known class names or error strings; use regex only when necessary.
- Test regexes against sample crash logs.

## Loader Hash Pinning

`loader-manifests/` entries must pin both the download URL and SHA-256 hash for each loader version. Update manifests only after downloading and hashing the published artifact locally.
