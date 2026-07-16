#!/usr/bin/env python3
"""Generate the runtime catalog of Adoptium Eclipse Temurin JRE/JDK releases.

Queries the official Adoptium v3 API for latest portable JRE/JDK packages for
a fixed matrix of Java major versions and platforms.  Writes
`runtime-catalog/runtime_catalog.json` with pinned SHA-256 hashes,
download URLs, and metadata.

Usage:
    python scripts/generate_runtime_catalog.py              # write catalog
    python scripts/generate_runtime_catalog.py --check      # validate without network
    python scripts/generate_runtime_catalog.py --refresh    # force network update
    python scripts/generate_runtime_catalog.py --verify     # verify archive bytes
    python scripts/generate_runtime_catalog.py --auto-discover  # auto-detect MC majors
    python scripts/generate_runtime_catalog.py --major 8 --major 21  # specific majors
"""

from __future__ import annotations

import argparse
import hashlib
import json
import logging
import os
import re
import sys
import urllib.error
import urllib.request
from collections.abc import Iterable
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parent.parent
CATALOG_DIR = REPO_ROOT / "runtime-catalog"
CATALOG_FILE = CATALOG_DIR / "runtime_catalog.json"

SCHEMA_VERSION = 1

# ---------------------------------------------------------------------------
# Fixed matrix: Adoptium Temurin JRE/JDK (Hotspot) for these majors/platforms.
# ---------------------------------------------------------------------------

REQUESTED_MAJORS = [8, 16, 17, 21, 25]

# Architecture is specified as "x64" or "aarch64" in the API.
# JRE availability: Adoptium provides JRE for all four majors on all three
# OS families for x64.  For aarch64 they provide JRE on linux and macos
# (mac aarch64 = Apple Silicon) but NOT on windows.
SUPPORTED_OS = ["windows", "linux", "mac"]
SUPPORTED_ARCH = ["x64", "aarch64"]

IMAGE_TYPE_PREFERENCE = ["jre", "jdk"]

# Unavailable combinations (deterministically known):
#   windows + aarch64 → no portable JRE published by Adoptium
UNAVAILABLE_COMBOS: list[tuple[int, str, str]] = []
for major in REQUESTED_MAJORS:
    for os_name in SUPPORTED_OS:
        for arch in SUPPORTED_ARCH:
            if os_name == "windows" and arch == "aarch64":
                UNAVAILABLE_COMBOS.append((major, os_name, arch))

# ---------------------------------------------------------------------------
# API helpers
# ---------------------------------------------------------------------------

API_BASE = "https://api.adoptium.net/v3"

USER_AGENT = (
    "AgoraRuntimeCatalogBot/1.0 "
    "(repository configured by AGORA_REGISTRY_REPO)"
)

_SHA256_LOWERCASE_RE = re.compile(r"^[0-9a-f]{64}$")
_SHA256_UPPERCASE_RE = re.compile(r"^[0-9A-F]{64}$")

ADOPTIUM_GITHUB_RELEASE_RE = re.compile(
    r"^https://github\.com/adoptium/"
    r"(?:temurin\d*-binaries|temurin-binaries)/releases/"
    r"download/.*"
)

LICENSE_SPDX = "GPL-2.0-only WITH Classpath-exception-2.0"
VENDOR = "eclipse-temurin"
JVM_IMPL = "hotspot"
IMAGE_TYPE = "jre"


def _fetch_json(url: str) -> Any:
    headers = {"User-Agent": USER_AGENT}
    req = urllib.request.Request(url, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            return json.loads(resp.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        if exc.code == 404:
            return None
        raise


def _download_streaming(url: str, destination: Path) -> None:
    """Stream a URL to a file with chunked reading."""
    headers = {"User-Agent": USER_AGENT}
    req = urllib.request.Request(url, headers=headers)
    destination.parent.mkdir(parents=True, exist_ok=True)
    with urllib.request.urlopen(req, timeout=300) as resp:
        with destination.open("wb") as fh:
            while True:
                chunk = resp.read(1024 * 1024)
                if not chunk:
                    break
                fh.write(chunk)


def _fetch_assets_latest(
    major: int, os_name: str, arch: str, image_type: str = "jre"
) -> dict[str, Any] | None:
    """Query the Adoptium v3 assets/latest endpoint.

    Returns the first binary's release info, or None if the combination
    is unavailable (404).
    """
    url = (
        f"{API_BASE}/assets/latest/{major}/hotspot"
        f"?architecture={arch}&image_type={image_type}&os={os_name}&vendor=eclipse"
    )
    data = _fetch_json(url)
    if data is None or not isinstance(data, list) or len(data) == 0:
        return None
    return data[0]


def _validate_and_extract(
    release: dict[str, Any], major: int, os_name: str, arch: str, image_type: str = "jre"
) -> dict[str, Any] | str:
    """Validate the API response and extract catalog entry fields.

    Returns the entry dict on success, or an error string on failure.
    """
    # Release-level fields: vendor and version
    vendor = release.get("vendor")
    if vendor != "eclipse":
        return f"unexpected vendor {vendor!r}, expected 'eclipse'"

    version_info = release.get("version")
    if not isinstance(version_info, dict):
        return "missing version data at release level"

    # Navigate to the binary entry
    binary = release.get("binary")
    if not isinstance(binary, dict):
        return "missing binary entry"

    # Validate image_type and jvm_impl
    if binary.get("image_type") != image_type:
        return (
            f"image_type mismatch: {binary.get('image_type')!r}"
            f" != {image_type!r}"
        )
    if binary.get("jvm_impl") != JVM_IMPL:
        return (
            f"jvm_impl mismatch: {binary.get('jvm_impl')!r} != {JVM_IMPL!r}"
        )

    # Validate os and arch
    if binary.get("os") != os_name:
        return f"os mismatch: {binary.get('os')!r} != {os_name!r}"
    if binary.get("architecture") != arch:
        return (
            f"architecture mismatch: {binary.get('architecture')!r}"
            f" != {arch!r}"
        )

    # Map Adoptium os name to canonical Agora platform
    os_map = {"linux": "linux", "windows": "windows", "mac": "macos"}
    canonical_os = os_map.get(os_name)
    if canonical_os is None:
        return f"unknown os {os_name!r}"

    # Map arch name
    arch_map = {"x64": "x64", "aarch64": "aarch64"}
    canonical_arch = arch_map.get(arch)
    if canonical_arch is None:
        return f"unknown arch {arch!r}"

    # Version fields
    semver = version_info.get("semver", "")
    openjdk_version = version_info.get("openjdk_version", "")
    version_major = version_info.get("major")
    version_minor = version_info.get("minor", 0)
    version_security = version_info.get("security", 0)

    if not openjdk_version:
        return "missing openjdk_version"

    # full_version: clean version without LTS/tag suffix
    full_version = semver or openjdk_version
    # Strip trailing .0.LTS (e.g. "21.0.11+10.0.LTS" → "21.0.11+10")
    full_version = re.sub(r'\.0\.LTS$', '', full_version)
    # Strip trailing -LTS (e.g. "21.0.11+10-LTS" → "21.0.11+10")
    full_version = re.sub(r'-LTS$', '', full_version)

    # Validate major matches
    if version_major != major:
        return (
            f"major version mismatch: {version_major} != requested {major}"
        )

    # Package info
    pkg = binary.get("package")
    if not isinstance(pkg, dict):
        return "missing package data"

    link = pkg.get("link")
    if not isinstance(link, str) or not link.startswith("https://"):
        return "missing or non-HTTPS package link"

    # Validate link is an official Adoptium GitHub release
    if not ADOPTIUM_GITHUB_RELEASE_RE.match(link):
        return f"package link not from official Adoptium GitHub: {link}"

    checksum_raw = pkg.get("checksum")
    if not isinstance(checksum_raw, str):
        return "missing checksum"

    # Adoptium returns uppercase hex; normalize to lowercase
    checksum = checksum_raw.strip()
    if _SHA256_UPPERCASE_RE.match(checksum):
        checksum = checksum.lower()
    elif not _SHA256_LOWERCASE_RE.match(checksum):
        return f"invalid SHA-256 checksum format: {checksum}"

    size = pkg.get("size")
    if not isinstance(size, int) or size <= 0:
        return f"invalid package size: {size}"

    # Package name → archive type
    name = pkg.get("name", "")
    if name.endswith(".zip"):
        archive_type = "zip"
    elif name.endswith(".tar.gz"):
        archive_type = "tar.gz"
    else:
        return f"unknown archive extension: {name}"

    # java_relative_path: path to java executable RELATIVE TO extraction root
    if os_name == "windows":
        java_relative_path = "bin/java.exe"
    elif os_name == "mac":
        java_relative_path = "Contents/Home/bin/java"
    else:
        java_relative_path = "bin/java"

    # Source API URL
    source_api_url = (
        f"{API_BASE}/assets/latest/{major}/hotspot"
        f"?architecture={arch}&image_type={image_type}&os={os_name}&vendor=eclipse"
    )

    entry: dict[str, Any] = {
        "vendor": VENDOR,
        "major": version_major,
        "full_version": full_version,
        "openjdk_version": openjdk_version,
        "os": canonical_os,
        "arch": canonical_arch,
        "image_type": image_type,
        "jvm_impl": JVM_IMPL,
        "archive_type": archive_type,
        "url": link,
        "sha256": checksum,
        "size": size,
        "java_relative_path": java_relative_path,
        "license": LICENSE_SPDX,
        "source_api_url": source_api_url,
    }

    # Lower-level version components for reference
    entry["version_major"] = version_major
    entry["version_minor"] = version_minor
    entry["version_security"] = version_security

    return entry


# ---------------------------------------------------------------------------
# Verification helpers
# ---------------------------------------------------------------------------


def verify_archive_bytes(entry: dict[str, Any], cache_dir: Path) -> None:
    """Download (if missing) and verify an archive's size and SHA-256."""
    destination = cache_dir / entry["sha256"]
    if not destination.exists():
        _download_streaming(entry["url"], destination)
    actual_size = destination.stat().st_size
    if actual_size != entry["size"]:
        raise RuntimeError(
            f"size mismatch for {entry['url']}: "
            f"expected {entry['size']}, got {actual_size}"
        )
    digest = hashlib.sha256()
    with destination.open("rb") as archive:
        for chunk in iter(lambda: archive.read(1024 * 1024), b""):
            digest.update(chunk)
    actual_hash = digest.hexdigest()
    if actual_hash != entry["sha256"]:
        raise RuntimeError(
            f"SHA-256 mismatch for {entry['url']}: "
            f"expected {entry['sha256']}, got {actual_hash}"
        )


# ---------------------------------------------------------------------------
# MC version auto-discovery
# ---------------------------------------------------------------------------

MOJANG_VERSION_MANIFEST_URL = (
    "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json"
)

BASELINE_MAJORS = {8, 16, 17, 21}


def discover_minecraft_java_majors(
    version_documents: list[dict[str, Any]],
) -> set[int]:
    majors = set(BASELINE_MAJORS)
    for version in version_documents:
        java_version = version.get("javaVersion")
        if not isinstance(java_version, dict):
            majors.add(8)
            continue
        major = java_version.get("majorVersion")
        if isinstance(major, int) and major > 0:
            majors.add(major)
    return majors


# ---------------------------------------------------------------------------
# Catalog generation
# ---------------------------------------------------------------------------


def generate_catalog(
    requested_majors: Iterable[int] | None = None,
) -> dict[str, Any]:
    """Query Adoptium API and build the catalog dict.

    Args:
        requested_majors: Java major versions to include. Defaults to
                          `REQUESTED_MAJORS`.
    """
    if requested_majors is None:
        requested_majors_list = list(REQUESTED_MAJORS)
    else:
        requested_majors_list = list(requested_majors)

    entries: list[dict[str, Any]] = []
    warnings: list[str] = []

    # Build unavailable combos for the requested majors
    unavailable_set: set[tuple[int, str, str]] = set()
    for m in requested_majors_list:
        for os_name in SUPPORTED_OS:
            for arch in SUPPORTED_ARCH:
                if os_name == "windows" and arch == "aarch64":
                    unavailable_set.add((m, os_name, arch))

    for major in requested_majors_list:
        for os_name in SUPPORTED_OS:
            for arch in SUPPORTED_ARCH:
                key = (major, os_name, arch)
                if key in unavailable_set:
                    warnings.append(
                        f"SKIP (unavailable): Java {major} "
                        f"{os_name}/{arch} — no portable JRE from Adoptium"
                    )
                    continue

                # Try image types in preference order (JRE first, then JDK)
                entry = None
                for img_type in IMAGE_TYPE_PREFERENCE:
                    logger.info(
                        "Querying Java %d %s/%s (%s) ...",
                        major, os_name, arch, img_type,
                    )
                    release = _fetch_assets_latest(
                        major, os_name, arch, image_type=img_type
                    )
                    if release is None:
                        continue
                    result = _validate_and_extract(
                        release, major, os_name, arch, image_type=img_type
                    )
                    if isinstance(result, str):
                        warnings.append(
                            f"SKIP (validation): Java {major} "
                            f"{os_name}/{arch} ({img_type}) — {result}"
                        )
                        continue
                    entry = result
                    break

                if entry is None:
                    warnings.append(
                        f"SKIP (404): Java {major} "
                        f"{os_name}/{arch} — no release found"
                    )
                    continue

                entries.append(entry)

    # Sort deterministically: major ASC, os ASC, arch ASC
    os_sort = {"linux": 0, "macos": 1, "windows": 2}
    entries.sort(
        key=lambda e: (
            e["major"],
            os_sort.get(e["os"], 99),
            e["arch"],
        )
    )

    generated_at = datetime.now(timezone.utc).isoformat()

    majors_str = ", ".join(str(m) for m in sorted(requested_majors_list))
    catalog: dict[str, Any] = {
        "schema_version": SCHEMA_VERSION,
        "generated_at": generated_at,
        "source": f"Adoptium API v3 — Eclipse Temurin JRE/JDK Hotspot (Java {majors_str})",
        "entries": entries,
        "warnings": warnings,
    }

    return catalog


# ---------------------------------------------------------------------------
# Validation (--check mode, no network)
# ---------------------------------------------------------------------------


def _is_valid_sha256(s: str) -> bool:
    return bool(_SHA256_LOWERCASE_RE.match(s))


def check_catalog(catalog: dict[str, Any]) -> list[str]:
    """Validate an existing catalog dict without network access.

    Returns a list of error strings (empty = valid).
    """
    errors: list[str] = []

    # Schema version
    sv = catalog.get("schema_version")
    if sv != SCHEMA_VERSION:
        errors.append(
            f"schema_version mismatch: {sv} != expected {SCHEMA_VERSION}"
        )

    # Source metadata
    if not catalog.get("generated_at"):
        errors.append("missing generated_at")
    if not catalog.get("source"):
        errors.append("missing source")

    entries = catalog.get("entries")
    if not isinstance(entries, list):
        errors.append("entries is not a list")
        return errors

    seen_keys: set[tuple[int, str, str]] = set()

    for i, entry in enumerate(entries):
        if not isinstance(entry, dict):
            errors.append(f"entry[{i}] is not a dict")
            continue

        idx = f"entry[{i}]"

        # Required string fields
        for field in (
            "vendor", "full_version", "openjdk_version",
            "os", "arch", "image_type", "jvm_impl",
            "archive_type", "url", "sha256", "java_relative_path",
            "license", "source_api_url",
        ):
            val = entry.get(field)
            if not isinstance(val, str) or not val:
                errors.append(f"{idx}: missing or empty '{field}'")

        # Required int fields
        for field in ("major", "size"):
            val = entry.get(field)
            if not isinstance(val, int) or val <= 0:
                errors.append(f"{idx}: missing or invalid '{field}'")

        # Validate vendor
        if entry.get("vendor") != VENDOR:
            errors.append(
                f"{idx}: vendor '{entry.get('vendor')}' != '{VENDOR}'"
            )

        # Validate license
        if entry.get("license") != LICENSE_SPDX:
            errors.append(
                f"{idx}: license '{entry.get('license')}' != '{LICENSE_SPDX}'"
            )

        # Validate image_type (accept JRE or JDK)
        if entry.get("image_type") not in {"jre", "jdk"}:
            errors.append(
                f"{idx}: image_type '{entry.get('image_type')}'"
                f" not in {{'jre', 'jdk'}}"
            )

        # Validate jvm_impl
        if entry.get("jvm_impl") != JVM_IMPL:
            errors.append(
                f"{idx}: jvm_impl '{entry.get('jvm_impl')}' != '{JVM_IMPL}'"
            )

        # SHA-256
        sha = entry.get("sha256", "")
        if not _is_valid_sha256(sha):
            errors.append(f"{idx}: invalid sha256 format '{sha}'")

        # URL must be HTTPS and official Adoptium GitHub
        url = entry.get("url", "")
        if not url.startswith("https://"):
            errors.append(f"{idx}: URL not HTTPS: {url}")
        elif not ADOPTIUM_GITHUB_RELEASE_RE.match(url):
            errors.append(
                f"{idx}: URL not official Adoptium GitHub release: {url}"
            )

        # OS compatibility checks
        os_name = entry.get("os", "")
        arch = entry.get("arch", "")
        archive_type = entry.get("archive_type", "")
        if os_name == "windows" and archive_type != "zip":
            errors.append(f"{idx}: Windows entries must use zip, got {archive_type}")
        if os_name in ("linux", "macos") and archive_type != "tar.gz":
            errors.append(
                f"{idx}: {os_name} entries must use tar.gz, got {archive_type}"
            )

        # java_relative_path check
        jrp = entry.get("java_relative_path", "")
        if os_name == "windows" and jrp != "bin/java.exe":
            errors.append(f"{idx}: Windows JRP should be 'bin/java.exe', got '{jrp}'")
        elif os_name == "macos" and jrp != "Contents/Home/bin/java":
            errors.append(
                f"{idx}: macOS JRP should be 'Contents/Home/bin/java', got '{jrp}'"
            )
        elif os_name == "linux" and jrp != "bin/java":
            errors.append(f"{idx}: Linux JRP should be 'bin/java', got '{jrp}'")

        # Duplicate detection
        major = entry.get("major")
        entry_os = entry.get("os")
        entry_arch = entry.get("arch")
        if major is not None and entry_os and entry_arch:
            key = (major, entry_os, entry_arch)
            if key in seen_keys:
                errors.append(
                    f"{idx}: duplicate tuple "
                    f"major={major} os={entry_os} arch={entry_arch}"
                )
            seen_keys.add(key)

        # Check that size is reasonable (at least 10MB for a JRE)
        size = entry.get("size", 0)
        if size < 10_000_000:
            errors.append(
                f"{idx}: suspiciously small size {size} bytes"
            )

        # Check that major is in REQUESTED_MAJORS
        if major not in REQUESTED_MAJORS:
            errors.append(
                f"{idx}: major {major} not in requested set {REQUESTED_MAJORS}"
            )

    return errors


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(message)s",
)
logger = logging.getLogger("generate_runtime_catalog")


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Generate or validate the runtime catalog of"
                    " Adoptium Eclipse Temurin JRE/JDK releases."
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="Validate the existing catalog file without network access.",
    )
    parser.add_argument(
        "--refresh",
        action="store_true",
        help="Force a network update even if the catalog exists.",
    )
    parser.add_argument(
        "--major",
        type=int,
        action="append",
        dest="majors",
        help="Java major to include; may be specified multiple times.",
    )
    parser.add_argument(
        "--verify",
        action="store_true",
        help="Download all archives (cached by SHA-256) and verify their size and hash.",
    )
    parser.add_argument(
        "--verify-cache",
        type=Path,
        default=REPO_ROOT / ".archive_cache",
        help="Cache directory for --verify (default: .archive_cache).",
    )
    parser.add_argument(
        "--auto-discover",
        action="store_true",
        help="Auto-discover Java majors from Mojang version manifest.",
    )
    args = parser.parse_args()

    if args.check:
        # No-network validation mode
        if not CATALOG_FILE.exists():
            logger.error("Catalog file not found: %s", CATALOG_FILE)
            return 1

        with CATALOG_FILE.open("r", encoding="utf-8") as fh:
            catalog = json.load(fh)

        errors = check_catalog(catalog)
        if errors:
            logger.error("Catalog validation FAILED (%d errors):", len(errors))
            for err in errors:
                logger.error("  - %s", err)
            return 1
        else:
            logger.info(
                "Catalog validation PASSED (%d entries, %d warnings).",
                len(catalog.get("entries", [])),
                len(catalog.get("warnings", [])),
            )
            return 0

    # Verify mode (download and verify archive bytes)
    if args.verify:
        if not CATALOG_FILE.exists():
            logger.error("Catalog file not found: %s", CATALOG_FILE)
            return 1

        with CATALOG_FILE.open("r", encoding="utf-8") as fh:
            catalog = json.load(fh)

        entries = catalog.get("entries", [])
        cache_dir = args.verify_cache
        cache_dir.mkdir(parents=True, exist_ok=True)

        failures = 0
        for i, entry in enumerate(entries):
            entry_id = f"entry[{i}] (Java {entry.get('major')} {entry.get('os')}/{entry.get('arch')})"
            logger.info("Verifying %s ...", entry_id)
            try:
                verify_archive_bytes(entry, cache_dir)
                logger.info("  PASS: %s", entry.get("sha256", "?"))
            except RuntimeError as exc:
                logger.error("  FAIL: %s", exc)
                failures += 1

        if failures:
            logger.error("Verification FAILED: %d of %d entries failed.", failures, len(entries))
            return 1
        else:
            logger.info("Verification PASSED: all %d entries verified.", len(entries))
            return 0

    # Auto-discover mode
    if args.auto_discover:
        logger.info("Fetching Mojang version manifest ...")
        manifest = _fetch_json(MOJANG_VERSION_MANIFEST_URL)
        if not isinstance(manifest, dict):
            logger.error("Failed to parse version manifest.")
            return 1

        versions = manifest.get("versions", [])
        # Pick up to 10 recent release/snapshot versions
        recent_versions = [v for v in versions if v.get("type") in ("release", "snapshot")][:10]
        version_docs: list[dict[str, Any]] = []
        for v in recent_versions:
            url = v.get("url")
            vid = v.get("id", "?")
            if not url:
                continue
            logger.info("  Fetching version JSON for %s ...", vid)
            doc = _fetch_json(url)
            if isinstance(doc, dict):
                version_docs.append(doc)

        discovered = discover_minecraft_java_majors(version_docs)
        logger.info("Discovered Java majors: %s", sorted(discovered))
        majors = sorted(discovered)
    else:
        majors = args.majors if args.majors is not None else REQUESTED_MAJORS

    # Generate mode (unless --check or --verify already returned)
    if CATALOG_FILE.exists() and not args.refresh:
        logger.info(
            "Catalog already exists at %s (use --refresh to overwrite).",
            CATALOG_FILE,
        )
        return 0

    catalog = generate_catalog(requested_majors=majors)

    # Validate what we just generated
    errors = check_catalog(catalog)
    if errors:
        logger.error("Generated catalog validation FAILED (%d errors):", len(errors))
        for err in errors:
            logger.error("  - %s", err)
        return 1

    # Write catalog
    CATALOG_DIR.mkdir(parents=True, exist_ok=True)
    with CATALOG_FILE.open("w", encoding="utf-8", newline="\n") as fh:
        json.dump(catalog, fh, indent=2, ensure_ascii=False)
        fh.write("\n")

    entries = catalog["entries"]
    warnings = catalog.get("warnings", [])
    logger.info("Wrote %s with %d entries.", CATALOG_FILE, len(entries))
    if warnings:
        logger.info("Warnings (%d):", len(warnings))
        for w in warnings:
            logger.info("  - %s", w)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
