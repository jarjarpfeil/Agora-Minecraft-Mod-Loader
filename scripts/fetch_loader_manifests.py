#!/usr/bin/env python3
"""Generate loader manifests and pinned hashes from official upstream APIs.

Each refresh queries at most the requested number of new versions per
Minecraft version and merges them append-only into the existing catalog.

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
    "maven.fabricmc.net",
    "maven.minecraftforge.net",
    "neoforged.net",
    "repo1.maven.org",
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


class UpstreamMetadataError(RuntimeError):
    """Raised when a loader metadata source is unavailable or malformed."""


class ExistingEntryMutationError(ValueError):
    """Raised when upstream changes an already-published loader tuple."""

    def __init__(self, mutations: list[dict[str, Any]]) -> None:
        self.mutations = mutations
        super().__init__(
            "Existing loader entries changed; manual review is required: "
            + json.dumps(mutations, sort_keys=True)
        )


IMMUTABLE_ENTRY_FIELDS = (
    "mc_version",
    "loader_version",
    "source_url",
    "sha256",
    "file_name",
    "file_type",
    "version_json_sha256",
    "installer_spec",
)


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


def _extract_install_profile(jar_path: Path) -> dict[str, Any] | None:
    """Extract and parse install_profile.json from a Forge/NeoForge installer JAR."""
    try:
        with zipfile.ZipFile(jar_path, "r") as zf:
            if "install_profile.json" in zf.namelist():
                data = zf.read("install_profile.json")
                return json.loads(data.decode("utf-8"))
    except (zipfile.BadZipFile, OSError, json.JSONDecodeError) as exc:
        logger.warning("Could not read install_profile.json from %s: %s", jar_path.name, exc)
    return None


def _extract_version_json(jar_path: Path) -> dict[str, Any] | None:
    """Extract and parse version.json from a Forge/NeoForge installer JAR."""
    try:
        with zipfile.ZipFile(jar_path, "r") as zf:
            if "version.json" in zf.namelist():
                data = zf.read("version.json")
                return json.loads(data.decode("utf-8"))
    except (zipfile.BadZipFile, OSError, json.JSONDecodeError) as exc:
        logger.warning("Could not read version.json from %s: %s", jar_path.name, exc)
    return None


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


# ---------------------------------------------------------------------------
# Per-run download diagnostics
#
# Failed candidates are intentionally not persisted. A transient upstream
# outage must not permanently blacklist a published loader version, and an
# ephemeral GitHub Actions runner cannot provide meaningful persistence.
# ---------------------------------------------------------------------------

_DOWNLOAD_FAILURES: list[dict[str, str]] = []


def get_download_failures() -> list[dict[str, str]]:
    """Return candidate download failures observed during this process."""
    return list(_DOWNLOAD_FAILURES)


def _failed_key(loader: str, mc_version: str, loader_version: str) -> str:
    return f"{loader}/{mc_version}/{loader_version}"


def _failed_should_skip(key: str) -> bool:
    # Never skip a candidate based on failures from a previous run.
    return False


def _failed_record_success(key: str) -> None:
    del key


def _failed_record_failure(key: str) -> None:
    if not any(failure["key"] == key for failure in _DOWNLOAD_FAILURES):
        _DOWNLOAD_FAILURES.append({"key": key})
    logger.warning("Could not download candidate %s; it will be retried next run", key)


def _fetch_bytes(url: str, timeout: float = 60) -> bytes:
    headers = {
        "User-Agent": (
            "AgoraLoaderManifestBot/1.0 "
            "(repository configured by AGORA_REGISTRY_REPO)"
        ),
    }
    req = urllib.request.Request(url, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.read()
    except urllib.error.URLError:
        raise
    except TimeoutError as exc:
        # urllib wraps connection-time timeouts in URLError, but a timeout
        # raised while reading the response body escapes as TimeoutError.
        # Normalize both phases so loader-specific skip logic can handle them.
        raise urllib.error.URLError(f"timeout reading {url}: {exc}") from exc


def _fetch_json(url: str) -> Any:
    return json.loads(_fetch_bytes(url).decode("utf-8"))


def _download_to_cache(
    url: str, cache_name: str, timeout: float = 60
) -> Path:
    CACHE_DIR.mkdir(parents=True, exist_ok=True)
    cache_path = CACHE_DIR / cache_name
    if cache_path.exists():
        logger.debug("Using cached %s", cache_name)
        return cache_path

    logger.info("Downloading %s", url)
    data = _fetch_bytes(url, timeout=timeout)
    cache_path.write_bytes(data)
    return cache_path


def _fetch_profile_json(url: str, cache_name: str, *, refresh: bool = False) -> bytes:
    """Fetch a profile JSON, caching it in ``.cache/profile-json/``."""
    cache_dir = CACHE_DIR / "profile-json"
    cache_dir.mkdir(parents=True, exist_ok=True)
    cache_path = cache_dir / cache_name
    if cache_path.exists() and not refresh:
        logger.debug("Using cached profile JSON %s", cache_name)
        return cache_path.read_bytes()
    logger.info("Downloading profile JSON %s", url)
    try:
        data = _fetch_bytes(url)
    except urllib.error.URLError:
        if cache_path.exists():
            logger.warning("Profile refresh failed; using cached %s", cache_name)
            return cache_path.read_bytes()
        raise
    cache_path.write_bytes(data)
    return data


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


def _fetch_fabric(
    mc_version: str,
    per_mc_limit: int | None = None,
    refresh_profiles: bool = False,
) -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []
    url = f"https://meta.fabricmc.net/v2/versions/loader/{mc_version}"
    try:
        versions = _fetch_json(url)
    except urllib.error.HTTPError as exc:
        if exc.code == 404:
            logger.warning("Fabric has no loader versions for MC %s", mc_version)
            return []
        raise UpstreamMetadataError(
            f"Fabric loader list for {mc_version}: {exc}"
        ) from exc
    except (urllib.error.URLError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise UpstreamMetadataError(
            f"Fabric loader list for {mc_version}: {exc}"
        ) from exc
    if not isinstance(versions, list):
        raise UpstreamMetadataError(
            f"Fabric loader list for {mc_version} was not a JSON array"
        )
    if any(
        not isinstance(info, dict)
        or not isinstance(info.get("loader"), dict)
        or not isinstance(info["loader"].get("version"), str)
        for info in versions
    ):
        raise UpstreamMetadataError(
            f"Fabric loader list for {mc_version} contained a malformed entry"
        )

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

        key = _failed_key("fabric", mc_version, loader_version)
        if _failed_should_skip(key):
            logger.debug("Skipping Fabric %s/%s (previously failed)", mc_version, loader_version)
            continue

        profile_url = (
            f"https://meta.fabricmc.net/v2/versions/loader/{mc_version}"
            f"/{loader_version}/profile/json"
        )
        cache_name = re.sub(r'[^a-zA-Z0-9._-]', '_', f"fabric-{mc_version}-{loader_version}.json")
        try:
            data = _fetch_profile_json(profile_url, cache_name, refresh=refresh_profiles)
        except urllib.error.URLError as exc:
            _failed_record_failure(key)
            logger.error(
                "Failed to fetch Fabric profile %s/%s: %s",
                mc_version,
                loader_version,
                exc,
            )
            continue

        _failed_record_success(key)
        file_name = f"fabric-loader-{loader_version}-{mc_version}.json"
        try:
            sha = _stable_json_sha256(data)
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            raise UpstreamMetadataError(
                f"Fabric profile {mc_version}/{loader_version}: {exc}"
            ) from exc

        entries.append({
            "mc_version": mc_version,
            "loader_version": loader_version,
            "source_url": profile_url,
            "sha256": sha,
            "file_name": file_name,
            "file_type": "profile_json",
        })
        logger.info(
            "Added Fabric loader %s for MC %s (stable sha256=%s...)",
            loader_version,
            mc_version,
            sha[:16],
        )

    return entries


def _fetch_quilt(
    mc_version: str,
    per_mc_limit: int | None = None,
    refresh_profiles: bool = False,
) -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []
    url = f"https://meta.quiltmc.org/v3/versions/loader/{mc_version}"
    try:
        versions = _fetch_json(url)
    except urllib.error.HTTPError as exc:
        if exc.code == 404:
            logger.warning("Quilt has no loader versions for MC %s", mc_version)
            return []
        raise UpstreamMetadataError(
            f"Quilt loader list for {mc_version}: {exc}"
        ) from exc
    except (urllib.error.URLError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise UpstreamMetadataError(
            f"Quilt loader list for {mc_version}: {exc}"
        ) from exc
    if not isinstance(versions, list):
        raise UpstreamMetadataError(
            f"Quilt loader list for {mc_version} was not a JSON array"
        )
    if any(
        not isinstance(info, dict)
        or not isinstance(info.get("loader"), dict)
        or not isinstance(info["loader"].get("version"), str)
        for info in versions
    ):
        raise UpstreamMetadataError(
            f"Quilt loader list for {mc_version} contained a malformed entry"
        )

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

        key = _failed_key("quilt", mc_version, loader_version)
        if _failed_should_skip(key):
            logger.debug("Skipping Quilt %s/%s (previously failed)", mc_version, loader_version)
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
        cache_name = re.sub(r'[^a-zA-Z0-9._-]', '_', f"quilt-{mc_version}-{loader_version}.json")
        data: bytes | None = None
        profile_url = profile_url_mc_first
        try:
            data = _fetch_profile_json(
                profile_url_mc_first, cache_name, refresh=refresh_profiles
            )
        except urllib.error.HTTPError as exc:
            if exc.code == 404:
                logger.info(
                    "Quilt profile path %s 404, trying swapped order",
                    profile_url_mc_first,
                )
                profile_url = profile_url_loader_first
                cache_name = re.sub(r'[^a-zA-Z0-9._-]', '_', f"quilt-{mc_version}-{loader_version}-alt.json")
                try:
                    data = _fetch_profile_json(
                        profile_url_loader_first, cache_name, refresh=refresh_profiles
                    )
                except urllib.error.URLError as exc2:
                    _failed_record_failure(key)
                    logger.error(
                        "Failed to fetch Quilt profile %s/%s: %s",
                        mc_version,
                        loader_version,
                        exc2,
                    )
                    continue
            else:
                _failed_record_failure(key)
                logger.error(
                    "Failed to fetch Quilt profile %s/%s: %s",
                    mc_version,
                    loader_version,
                    exc,
                )
                continue
        except urllib.error.URLError as exc:
            _failed_record_failure(key)
            logger.error(
                "Failed to fetch Quilt profile %s/%s: %s",
                mc_version,
                loader_version,
                exc,
            )
            continue

        if data is None:
            continue

        _failed_record_success(key)
        file_name = f"quilt-loader-{loader_version}-{mc_version}.json"
        try:
            sha = _stable_json_sha256(data)
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            raise UpstreamMetadataError(
                f"Quilt profile {mc_version}/{loader_version}: {exc}"
            ) from exc

        entries.append({
            "mc_version": mc_version,
            "loader_version": loader_version,
            "source_url": profile_url,
            "sha256": sha,
            "file_name": file_name,
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
    mc_versions: list[str],
    per_mc_limit: int | None = None,
) -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []
    metadata_url = (
        "https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml"
    )
    try:
        xml = _fetch_bytes(metadata_url)
        root = ET.fromstring(xml)
    except (urllib.error.URLError, ET.ParseError) as exc:
        raise UpstreamMetadataError(f"NeoForge Maven metadata: {exc}") from exc

    if root.find(".//versions") is None:
        raise UpstreamMetadataError("NeoForge Maven metadata was malformed")
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
        key = _failed_key("neoforge", version, version)
        if _failed_should_skip(key):
            logger.debug("Skipping NeoForge %s (previously failed)", version)
            continue

        source_url = (
            f"https://maven.neoforged.net/releases/net/neoforged/neoforge/{version}"
            f"/neoforge-{version}-installer.jar"
        )
        cache_name = f"neoforge-{version}-installer.jar"
        try:
            jar_path = _download_to_cache(source_url, cache_name)
        except urllib.error.URLError as exc:
            _failed_record_failure(key)
            logger.error("Failed to download NeoForge installer %s: %s", version, exc)
            continue

        _failed_record_success(key)

        jar_sha = _sha256_hex(jar_path.read_bytes())
        version_json_sha = _extract_version_json_sha256(jar_path)

        install = _extract_install_profile(jar_path)
        file_name = f"neoforge-{version}-installer.jar"

        installer_spec = None
        if install is not None:
            spec_val = install.get("spec")
            if isinstance(spec_val, int):
                installer_spec = spec_val

        entry: dict[str, Any] = {
            "mc_version": mc_version,
            "loader_version": version,
            "source_url": source_url,
            "sha256": jar_sha,
            "file_name": file_name,
            "file_type": "installer_jar",
        }
        if version_json_sha:
            entry["version_json_sha256"] = version_json_sha
        if installer_spec is not None:
            entry["installer_spec"] = installer_spec

        logger.info(
            "Added NeoForge %s for MC %s (jar=%s..., version.json=%s..., spec=%s)",
            version,
            mc_version,
            jar_sha[:16],
            (version_json_sha or "N/A")[:16],
            installer_spec,
        )
        entries.append(entry)

    return entries


def _fetch_forge(
    mc_versions: list[str],
    per_mc_limit: int | None = None,
) -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []
    metadata_url = (
        "https://maven.minecraftforge.net/net/minecraftforge/forge/maven-metadata.xml"
    )
    try:
        xml = _fetch_bytes(metadata_url)
        root = ET.fromstring(xml)
    except (urllib.error.URLError, ET.ParseError) as exc:
        raise UpstreamMetadataError(f"Forge Maven metadata: {exc}") from exc

    if root.find(".//versions") is None:
        raise UpstreamMetadataError("Forge Maven metadata was malformed")
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
        key = _failed_key("forge", mc_version, version)
        if _failed_should_skip(key):
            logger.debug("Skipping Forge %s (previously failed)", version)
            continue

        source_url = (
            f"https://maven.minecraftforge.net/net/minecraftforge/forge/{version}"
            f"/forge-{version}-installer.jar"
        )
        cache_name = f"forge-{version}-installer.jar"
        try:
            jar_path = _download_to_cache(source_url, cache_name, timeout=1)
        except urllib.error.URLError as exc:
            _failed_record_failure(key)
            logger.error("Failed to download Forge installer %s: %s", version, exc)
            continue

        _failed_record_success(key)

        jar_sha = _sha256_hex(jar_path.read_bytes())
        version_json_sha = _extract_version_json_sha256(jar_path)

        install = _extract_install_profile(jar_path)
        file_name = f"forge-{version}-installer.jar"

        installer_spec = None
        if install is not None:
            spec_val = install.get("spec")
            if isinstance(spec_val, int):
                installer_spec = spec_val

        entry: dict[str, Any] = {
            "mc_version": mc_version,
            "loader_version": loader_version,
            "source_url": source_url,
            "sha256": jar_sha,
            "file_name": file_name,
            "file_type": "installer_jar",
        }
        if version_json_sha:
            entry["version_json_sha256"] = version_json_sha
        if installer_spec is not None:
            entry["installer_spec"] = installer_spec

        logger.info(
            "Added Forge %s for MC %s (jar=%s..., version.json=%s..., spec=%s)",
            version,
            mc_version,
            jar_sha[:16],
            (version_json_sha or "N/A")[:16],
            installer_spec,
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
    mutations: list[dict[str, Any]] = []
    for entry in new_entries:
        key = (entry["mc_version"], entry["loader_version"])
        previous = seen.get(key)
        if previous is None:
            seen[key] = entry
            continue

        changed_fields = {
            field: {"before": previous.get(field), "after": entry.get(field)}
            for field in IMMUTABLE_ENTRY_FIELDS
            if previous.get(field) != entry.get(field)
        }
        if changed_fields:
            mutations.append({
                "key": f"{key[0]}/{key[1]}",
                "fields": changed_fields,
            })

    if mutations:
        raise ExistingEntryMutationError(mutations)

    return list(seen.values())


def _sort_entries(entries: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return sorted(
        entries,
        key=lambda e: (_version_key(e["mc_version"]), _version_key(e["loader_version"])),
    )


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
        help="Query at most N new loader versions per Minecraft version (default: 5, 0 = unlimited); existing entries are retained",
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
            loaders["fabric"],
            _fetch_fabric(mc_version, per_mc_limit),
        )

        logger.info("Fetching Quilt versions for %s", mc_version)
        loaders["quilt"] = _merge_entries(
            loaders["quilt"],
            _fetch_quilt(mc_version, per_mc_limit),
        )

    logger.info("Fetching NeoForge versions for %s", mc_versions)
    loaders["neoforge"] = _merge_entries(
        loaders["neoforge"],
        _fetch_neoforge(mc_versions, per_mc_limit),
    )

    logger.info("Fetching Forge versions for %s", mc_versions)
    loaders["forge"] = _merge_entries(
        loaders["forge"],
        _fetch_forge(mc_versions, per_mc_limit),
    )

    _write_loader_manifests(manifest)
    _write_known_good_hashes(manifest)

    total = sum(len(entries) for entries in loaders.values())
    logger.info("Done. %d loader entries in loader_manifests.json", total)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
