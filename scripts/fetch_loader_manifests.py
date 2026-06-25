#!/usr/bin/env python3
"""Generate loader manifests and pinned hashes from official upstream APIs.

Usage:
    python scripts/fetch_loader_manifests.py [--mc-versions 1.21 1.20.1]
"""

from __future__ import annotations

import argparse
import hashlib
import json
import logging
import re
import urllib.error
import urllib.request
import zipfile
from pathlib import Path
from typing import Any
from xml.etree import ElementTree as ET

DEFAULT_MC_VERSIONS = ["1.21"]

REPO_ROOT = Path(__file__).resolve().parent.parent
LOADER_MANIFESTS_DIR = REPO_ROOT / "loader-manifests"
CACHE_DIR = REPO_ROOT / ".cache" / "loader-manifests"

DOMAIN_ALLOWLIST = [
    "meta.fabricmc.net",
    "piston-meta.mojang.com",
    "maven.fabricmc.net",
    "neoforged.net",
    "maven.neoforged.net",
    "meta.quiltmc.org",
    "maven.quiltmc.org",
    "minecraftforge.net",
    "files.minecraftforge.net",
]

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(message)s",
)
logger = logging.getLogger("fetch_loader_manifests")


# ---------------------------------------------------------------------------
# Utility helpers
# ---------------------------------------------------------------------------


def _version_key(v: str):
    """Return a sortable key for a dotted version string."""
    parts = re.split(r"[.\-+]", v)
    out: list[Any] = []
    for part in parts:
        try:
            out.append(int(part))
        except ValueError:
            out.append(part.lower())
    return out


def _sha256_hex(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _stable_json_sha256(data: bytes, drop: set[str] | None = None) -> str:
    """Return a deterministic SHA-256 of a JSON payload after stripping volatile keys.

    Fabric dynamically rewrites `time`/`releaseTime` on every request, so pinning
    the raw response is unstable. This normalizes the payload for verification.
    """
    drop = drop or {"time", "releaseTime"}
    obj = json.loads(data.decode("utf-8"))
    if isinstance(obj, dict):
        obj = {k: v for k, v in obj.items() if k not in drop}
    canonical = json.dumps(
        obj, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    )
    return _sha256_hex(canonical.encode("utf-8"))


def _fetch_bytes(url: str) -> bytes:
    headers = {
        "User-Agent": (
            "AgoraLoaderManifestBot/1.0 "
            "(https://github.com/agora-mc/agora-mc)"
        ),
    }
    req = urllib.request.Request(url, headers=headers)
    with urllib.request.urlopen(req, timeout=60) as resp:
        return resp.read()


def _fetch_json(url: str) -> Any:
    return json.loads(_fetch_bytes(url).decode("utf-8"))


def _download_to_cache(url: str, cache_name: str) -> Path:
    CACHE_DIR.mkdir(parents=True, exist_ok=True)
    cache_path = CACHE_DIR / cache_name
    if cache_path.exists():
        logger.debug("Using cached %s", cache_name)
        return cache_path

    logger.info("Downloading %s", url)
    data = _fetch_bytes(url)
    cache_path.write_bytes(data)
    return cache_path


def _extract_version_json_sha256(jar_path: Path) -> str | None:
    """Extract version.json from an installer jar and return its stable SHA-256."""
    try:
        with zipfile.ZipFile(jar_path, "r") as zf:
            if "version.json" in zf.namelist():
                return _stable_json_sha256(zf.read("version.json"))
    except (zipfile.BadZipFile, OSError) as exc:
        logger.warning("Could not read %s: %s", jar_path.name, exc)
    return None


def _neoforge_version_to_mc(version: str) -> str | None:
    """Map NeoForge version to Minecraft version heuristically."""
    parts = version.split(".")
    if not parts or not parts[0].isdigit():
        return None
    major = parts[0]
    minor = parts[1] if len(parts) > 1 else "0"
    if minor == "0":
        return f"1.{major}"
    return f"1.{major}.{minor}"


# ---------------------------------------------------------------------------
# Fetchers per loader
# ---------------------------------------------------------------------------


def _fetch_fabric(mc_version: str, per_mc_limit: int | None = None) -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []
    url = f"https://meta.fabricmc.net/v2/versions/loader/{mc_version}"
    try:
        versions = _fetch_json(url)
    except (urllib.error.URLError, json.JSONDecodeError) as exc:
        logger.error("Failed to fetch Fabric loader list for %s: %s", mc_version, exc)
        return entries

    versions = sorted(
        versions,
        key=lambda info: _version_key(info.get("loader", {}).get("version", "")),
        reverse=True,
    )
    if per_mc_limit:
        versions = versions[:per_mc_limit]

    for info in versions:
        loader_info = info.get("loader") if isinstance(info, dict) else None
        loader_version = loader_info.get("version") if isinstance(loader_info, dict) else None
        if not loader_version:
            continue

        profile_url = (
            f"https://meta.fabricmc.net/v2/versions/loader/{mc_version}"
            f"/{loader_version}/profile/json"
        )
        try:
            data = _fetch_bytes(profile_url)
        except urllib.error.URLError as exc:
            logger.error(
                "Failed to fetch Fabric profile %s/%s: %s",
                mc_version,
                loader_version,
                exc,
            )
            continue

        sha = _stable_json_sha256(data)
        entries.append({
            "mc_version": mc_version,
            "loader_version": loader_version,
            "source_url": profile_url,
            "sha256": sha,
            "file_name": f"fabric-loader-{loader_version}-{mc_version}.json",
            "file_type": "profile_json",
        })
        logger.info(
            "Added Fabric loader %s for MC %s (stable sha256=%s...)",
            loader_version,
            mc_version,
            sha[:16],
        )

    return entries


def _fetch_quilt(mc_version: str, per_mc_limit: int | None = None) -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []
    url = f"https://meta.quiltmc.org/v3/versions/loader/{mc_version}"
    try:
        versions = _fetch_json(url)
    except (urllib.error.URLError, json.JSONDecodeError) as exc:
        logger.error("Failed to fetch Quilt loader list for %s: %s", mc_version, exc)
        return entries

    versions = sorted(
        versions,
        key=lambda info: _version_key(info.get("loader", {}).get("version", "")),
        reverse=True,
    )
    if per_mc_limit:
        versions = versions[:per_mc_limit]

    for info in versions:
        loader_info = info.get("loader") if isinstance(info, dict) else None
        loader_version = loader_info.get("version") if isinstance(loader_info, dict) else None
        if not loader_version:
            continue

        # Quilt's profile URL order matches Fabric: mc_version then loader_version.
        # If that 404s, fall back to the swapped order before giving up.
        profile_url_mc_first = (
            f"https://meta.quiltmc.org/v3/versions/loader/{mc_version}"
            f"/{loader_version}/profile/json"
        )
        profile_url_loader_first = (
            f"https://meta.quiltmc.org/v3/versions/loader/{loader_version}"
            f"/{mc_version}/profile/json"
        )
        data: bytes | None = None
        profile_url = profile_url_mc_first
        try:
            data = _fetch_bytes(profile_url_mc_first)
        except urllib.error.HTTPError as exc:
            if exc.code == 404:
                logger.info(
                    "Quilt profile path %s 404, trying swapped order",
                    profile_url_mc_first,
                )
                profile_url = profile_url_loader_first
                try:
                    data = _fetch_bytes(profile_url_loader_first)
                except urllib.error.URLError as exc2:
                    logger.error(
                        "Failed to fetch Quilt profile %s/%s: %s",
                        mc_version,
                        loader_version,
                        exc2,
                    )
                    continue
            else:
                logger.error(
                    "Failed to fetch Quilt profile %s/%s: %s",
                    mc_version,
                    loader_version,
                    exc,
                )
                continue
        except urllib.error.URLError as exc:
            logger.error(
                "Failed to fetch Quilt profile %s/%s: %s",
                mc_version,
                loader_version,
                exc,
            )
            continue

        if data is None:
            continue

        sha = _stable_json_sha256(data)
        entries.append({
            "mc_version": mc_version,
            "loader_version": loader_version,
            "source_url": profile_url,
            "sha256": sha,
            "file_name": f"quilt-loader-{loader_version}-{mc_version}.json",
            "file_type": "profile_json",
        })
        logger.info(
            "Added Quilt loader %s for MC %s (stable sha256=%s...)",
            loader_version,
            mc_version,
            sha[:16],
        )

    return entries


def _fetch_neoforge(
    mc_versions: list[str], per_mc_limit: int | None = None
) -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []
    metadata_url = (
        "https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml"
    )
    try:
        xml = _fetch_bytes(metadata_url)
        root = ET.fromstring(xml)
    except (urllib.error.URLError, ET.ParseError) as exc:
        logger.error("Failed to fetch NeoForge maven metadata: %s", exc)
        return entries

    versions = [v.text for v in root.findall(".//versions/version") if v.text]
    candidates_by_mc: dict[str, list[str]] = {}
    for version in versions:
        mc_version = _neoforge_version_to_mc(version)
        if mc_version is None or mc_version not in mc_versions:
            continue
        candidates_by_mc.setdefault(mc_version, []).append(version)

    selected_versions: list[tuple[str, str]] = []
    for mc_version, group in candidates_by_mc.items():
        sorted_group = sorted(group, key=_version_key, reverse=True)
        chosen = sorted_group[:per_mc_limit] if per_mc_limit else sorted_group
        selected_versions.extend((v, mc_version) for v in chosen)

    for version, mc_version in selected_versions:
        source_url = (
            f"https://maven.neoforged.net/releases/net/neoforged/neoforge/{version}"
            f"/neoforge-{version}-installer.jar"
        )
        cache_name = f"neoforge-{version}-installer.jar"
        try:
            jar_path = _download_to_cache(source_url, cache_name)
        except urllib.error.URLError as exc:
            logger.error("Failed to download NeoForge installer %s: %s", version, exc)
            continue

        jar_sha = _sha256_hex(jar_path.read_bytes())
        version_json_sha = _extract_version_json_sha256(jar_path)
        entry: dict[str, Any] = {
            "mc_version": mc_version,
            "loader_version": version,
            "source_url": source_url,
            "sha256": jar_sha,
            "file_name": f"neoforge-{version}-installer.jar",
            "file_type": "installer_jar",
        }
        if version_json_sha:
            entry["version_json_sha256"] = version_json_sha
            logger.info(
                "Added NeoForge %s for MC %s (jar=%s..., version.json=%s...)",
                version,
                mc_version,
                jar_sha[:16],
                version_json_sha[:16],
            )
        else:
            logger.warning(
                "Added NeoForge %s for MC %s but version.json extraction failed",
                version,
                mc_version,
            )
        entries.append(entry)

    return entries


def _fetch_forge(
    mc_versions: list[str], per_mc_limit: int | None = None
) -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []
    metadata_url = (
        "https://maven.minecraftforge.net/net/minecraftforge/forge/maven-metadata.xml"
    )
    try:
        xml = _fetch_bytes(metadata_url)
        root = ET.fromstring(xml)
    except (urllib.error.URLError, ET.ParseError) as exc:
        logger.error("Failed to fetch Forge maven metadata: %s", exc)
        return entries

    versions = [v.text for v in root.findall(".//versions/version") if v.text]
    candidates_by_mc: dict[str, list[tuple[str, str]]] = {}
    for version in versions:
        # Forge versions look like "1.21-51.0.0" (mc_version-build).
        if "-" not in version:
            continue
        mc_version, loader_version = version.split("-", 1)
        if mc_version not in mc_versions:
            continue
        candidates_by_mc.setdefault(mc_version, []).append((version, loader_version))

    selected_versions: list[tuple[str, str, str]] = []
    for mc_version, group in candidates_by_mc.items():
        sorted_group = sorted(group, key=lambda pair: _version_key(pair[1]), reverse=True)
        chosen = sorted_group[:per_mc_limit] if per_mc_limit else sorted_group
        selected_versions.extend((version, loader_version, mc_version) for version, loader_version in chosen)

    for version, loader_version, mc_version in selected_versions:
        source_url = (
            f"https://maven.minecraftforge.net/net/minecraftforge/forge/{version}"
            f"/forge-{version}-installer.jar"
        )
        cache_name = f"forge-{version}-installer.jar"
        try:
            jar_path = _download_to_cache(source_url, cache_name)
        except urllib.error.URLError as exc:
            logger.error("Failed to download Forge installer %s: %s", version, exc)
            continue

        jar_sha = _sha256_hex(jar_path.read_bytes())
        version_json_sha = _extract_version_json_sha256(jar_path)
        entry: dict[str, Any] = {
            "mc_version": mc_version,
            "loader_version": loader_version,
            "source_url": source_url,
            "sha256": jar_sha,
            "file_name": f"forge-{version}-installer.jar",
            "file_type": "installer_jar",
        }
        if version_json_sha:
            entry["version_json_sha256"] = version_json_sha
            logger.info(
                "Added Forge %s for MC %s (jar=%s..., version.json=%s...)",
                version,
                mc_version,
                jar_sha[:16],
                version_json_sha[:16],
            )
        else:
            logger.warning(
                "Added Forge %s for MC %s but version.json extraction failed",
                version,
                mc_version,
            )
        entries.append(entry)

    return entries


# ---------------------------------------------------------------------------
# Manifest persistence
# ---------------------------------------------------------------------------


def _load_existing_manifest() -> dict[str, Any]:
    path = LOADER_MANIFESTS_DIR / "loader_manifests.json"
    if path.exists():
        with path.open("r", encoding="utf-8") as fh:
            return json.load(fh)
    return {
        "domain_allowlist": sorted(DOMAIN_ALLOWLIST),
        "loaders": {"fabric": [], "quilt": [], "neoforge": [], "forge": []},
    }


def _merge_entries(existing: list[dict[str, Any]], new_entries: list[dict[str, Any]]) -> list[dict[str, Any]]:
    seen: dict[tuple[str, str], dict[str, Any]] = {
        (e["mc_version"], e["loader_version"]): e for e in existing
    }
    for entry in new_entries:
        seen[(entry["mc_version"], entry["loader_version"])] = entry
    return list(seen.values())


def _sort_entries(entries: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return sorted(
        entries,
        key=lambda e: (_version_key(e["mc_version"]), _version_key(e["loader_version"])),
    )


def _limit_entries(entries: list[dict[str, Any]], per_mc: int | None) -> list[dict[str, Any]]:
    """Keep only the latest *per_mc* loader versions for each Minecraft version."""
    if per_mc is None or per_mc <= 0:
        return entries

    groups: dict[str, list[dict[str, Any]]] = {}
    for entry in entries:
        groups.setdefault(entry["mc_version"], []).append(entry)

    limited: list[dict[str, Any]] = []
    for group in groups.values():
        sorted_desc = sorted(
            group, key=lambda e: _version_key(e["loader_version"]), reverse=True
        )
        limited.extend(sorted_desc[:per_mc])

    return limited


def _write_loader_manifests(manifest: dict[str, Any]) -> None:
    LOADER_MANIFESTS_DIR.mkdir(parents=True, exist_ok=True)
    path = LOADER_MANIFESTS_DIR / "loader_manifests.json"
    for loader in manifest["loaders"]:
        manifest["loaders"][loader] = _sort_entries(manifest["loaders"][loader])
    manifest["domain_allowlist"] = sorted(set(manifest["domain_allowlist"]))

    with path.open("w", encoding="utf-8") as fh:
        json.dump(manifest, fh, indent=2, sort_keys=False)
        fh.write("\n")
    logger.info("Wrote %s", path)


def _write_known_good_hashes(manifest: dict[str, Any]) -> None:
    loader_hashes: dict[str, dict[str, str | None]] = {}
    for loader, entries in manifest["loaders"].items():
        loader_hashes[loader] = {}
        for entry in entries:
            sha = entry.get("sha256")
            loader_hashes[loader][entry["file_name"]] = (
                f"sha256:{sha}" if sha else None
            )

    data = {
        "domain_allowlist": manifest["domain_allowlist"],
        "loader_hashes": loader_hashes,
        "_source": (
            "Generated from loader_manifests.json by scripts/fetch_loader_manifests.py. "
            "Do not edit manually."
        ),
    }
    path = LOADER_MANIFESTS_DIR / "known_good_hashes.json"
    with path.open("w", encoding="utf-8") as fh:
        json.dump(data, fh, indent=2, sort_keys=False)
        fh.write("\n")
    logger.info("Wrote %s", path)


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Fetch official modloader manifests and pin their SHA-256 hashes."
    )
    parser.add_argument(
        "--mc-versions",
        nargs="+",
        default=DEFAULT_MC_VERSIONS,
        help="Minecraft versions to query (default: 1.21)",
    )
    parser.add_argument(
        "--latest-per-mc",
        type=int,
        default=5,
        help="Keep only the N latest loader versions per Minecraft version (default: 5, 0 = unlimited)",
    )
    parser.add_argument(
        "--all-versions",
        action="store_true",
        help="Disable the per-Minecraft-version limit and keep every available loader version",
    )
    args = parser.parse_args()

    mc_versions = sorted(set(args.mc_versions), key=_version_key)
    per_mc_limit: int | None = None if args.all_versions else args.latest_per_mc
    logger.info("Querying loaders for Minecraft versions: %s", mc_versions)
    logger.info("Per-MC version limit: %s", "unlimited" if per_mc_limit is None else per_mc_limit)

    manifest = _load_existing_manifest()
    # Ensure the manifest always has the canonical domain allowlist.
    manifest["domain_allowlist"] = sorted(
        set(manifest.get("domain_allowlist", []) + DOMAIN_ALLOWLIST)
    )
    loaders = manifest.setdefault("loaders", {})
    for loader in ("fabric", "quilt", "neoforge", "forge"):
        loaders.setdefault(loader, [])

    for mc_version in mc_versions:
        logger.info("Fetching Fabric versions for %s", mc_version)
        loaders["fabric"] = _merge_entries(
            loaders["fabric"], _fetch_fabric(mc_version, per_mc_limit)
        )

        logger.info("Fetching Quilt versions for %s", mc_version)
        loaders["quilt"] = _merge_entries(
            loaders["quilt"], _fetch_quilt(mc_version, per_mc_limit)
        )

    logger.info("Fetching NeoForge versions for %s", mc_versions)
    loaders["neoforge"] = _merge_entries(
        loaders["neoforge"], _fetch_neoforge(mc_versions, per_mc_limit)
    )

    logger.info("Fetching Forge versions for %s", mc_versions)
    loaders["forge"] = _merge_entries(
        loaders["forge"], _fetch_forge(mc_versions, per_mc_limit)
    )

    # Final safety filter in case any function ignored the limit.
    for loader in loaders:
        loaders[loader] = _limit_entries(loaders[loader], per_mc_limit)

    _write_loader_manifests(manifest)
    _write_known_good_hashes(manifest)

    total = sum(len(entries) for entries in loaders.values())
    logger.info("Done. %d loader entries in loader_manifests.json", total)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
