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


# ---------------------------------------------------------------------------
# Library pin helpers
# ---------------------------------------------------------------------------

#: Regex matching a valid 64-char lowercase hex SHA-256.
_SHA256_RE = re.compile(r"^[0-9a-f]{64}$")

#: Regex matching a safe relative Maven path (no leading slash, no `..`, no `:`).
#: Accepts ``.jar``, ``.zip``, ``.txt``, ``.tsrg`` extensions.
_SAFE_JAR_PATH_RE = re.compile(r"^[a-zA-Z0-9_./+-]+\.(jar|zip|txt|tsrg)$")

# NOTE: Stage 2 removed direct-launch tuple gating.
# Forge/NeoForge resolve now routes through installed-profile adoption
# rather than network-only direct launch.


def _is_valid_sha256(s: str) -> bool:
    """Return True if *s* is a 64-character lowercase hex string."""
    return bool(_SHA256_RE.match(s))


def _is_safe_maven_path(s: str) -> bool:
    """Return True if *s* is a relative Maven artifact path without traversal or drive letters.

    Accepts ``.jar``, ``.zip``, ``.txt``, ``.tsrg`` extensions.
    Rejects leading ``/``, ``..``, ``//``, and colons.
    """
    if not any(s.endswith(ext) for ext in (".jar", ".zip", ".txt", ".tsrg")):
        return False
    if s.startswith("/") or s.startswith("..") or s.startswith("//"):
        return False
    if ":" in s:
        return False
    return bool(_SAFE_JAR_PATH_RE.match(s))


def _maven_name_to_path(name: str) -> str:
    """Convert Maven coordinate to relative path.

    Official grammar: group:artifact:version[:classifier][@extension]

    ``net.fabricmc:fabric-loader:0.19.0`` →
        ``net/fabricmc/fabric-loader/0.19.0/fabric-loader-0.19.0.jar``

    ``org.ow2.asm:asm:9.7`` →
        ``org/ow2/asm/asm/9.7/asm-9.7.jar``

    ``org.lwjgl:lwjgl:3.3.1:natives-windows`` →
        ``org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1-natives-windows.jar``

    ``de.oceanlabs.mcp:mcp_config:1.20.1-20230612.114412@zip`` →
        ``de/oceanlabs/mcp/mcp_config/1.20.1-20230612.114412/mcp_config-1.20.1-20230612.114412.zip``

    ``net.neoforged:AutoRenamingTool:2.0.3:all`` →
        ``net/neoforged/AutoRenamingTool/2.0.3/AutoRenamingTool-2.0.3-all.jar``
    """
    # Parse optional @extension suffix
    ext = "jar"
    name_noext = name
    if "@" in name:
        # Rightmost @ is the extension separator
        idx = name.rindex("@")
        ext = name[idx + 1:]
        name_noext = name[:idx]

    parts = name_noext.split(":")
    if len(parts) < 3:
        # Fallback for malformed coordinates
        path = name_noext.replace(":", "/")
        return f"{path}.{ext}"

    group = parts[0].replace(".", "/")
    artifact = parts[1]
    version = parts[2]
    classifier = f"-{parts[3]}" if len(parts) > 3 else ""
    return f"{group}/{artifact}/{version}/{artifact}-{version}{classifier}.{ext}"


def _extract_library_paths_from_profile(profile_data: dict) -> list[str]:
    """Extract all library paths (without requiring SHA-256) from a profile JSON.

    Returns a sorted de-duplicated list of Maven-relative JAR paths found
    in the profile's ``libraries`` array.  Handles both
    ``downloads.artifact.path`` and Maven ``name`` + ``url`` forms.
    """
    paths: set[str] = set()
    for lib in profile_data.get("libraries", []):
        if not isinstance(lib, dict):
            continue

        path: str | None = None

        # Form 1: explicit downloads.artifact
        downloads = lib.get("downloads")
        if isinstance(downloads, dict):
            artifact = downloads.get("artifact")
            if isinstance(artifact, dict):
                path = artifact.get("path")

        # Form 2: Maven name + repository url
        if path is None:
            name = lib.get("name")
            if isinstance(name, str):
                path = _maven_name_to_path(name)

        if isinstance(path, str) and _is_safe_maven_path(path):
            paths.add(path)

    return sorted(paths)


def _extract_pins_from_profile(profile_data: dict) -> dict[str, str]:
    """Extract library path → SHA-256 pins from a Fabric/Quilt profile JSON.

    Inspects every library entry in both forms:

    1. ``downloads.artifact`` with explicit ``path`` / ``url`` / ``sha1``
    2. Maven ``name`` + repository ``url`` with top-level ``sha256`` / ``sha1`` / ``size``

    Returns a dict of ``{maven_relative_path: sha256_hex}`` using the
    profile-embedded SHA-256 when present (preferred, since the profile itself
    is pinned by stable SHA-256). Libraries without SHA-256 are noted but not
    populated — the caller should download and compute them via
    :func:`_compute_library_sha256`.
    """
    pins: dict[str, str] = {}
    for lib in profile_data.get("libraries", []):
        if not isinstance(lib, dict):
            continue

        path: str | None = None
        url: str | None = None
        sha256: str | None = lib.get("sha256")

        # Form 1: explicit downloads.artifact
        downloads = lib.get("downloads")
        if isinstance(downloads, dict):
            artifact = downloads.get("artifact")
            if isinstance(artifact, dict):
                path = artifact.get("path")
                url = artifact.get("url")

        # Form 2: Maven name + repository url
        if path is None:
            name = lib.get("name")
            repo_url = lib.get("url")
            if isinstance(name, str) and isinstance(repo_url, str):
                path = _maven_name_to_path(name)
                url = repo_url.rstrip("/") + "/" + path

        if not isinstance(path, str) or not _is_safe_maven_path(path):
            continue

        if sha256 and _is_valid_sha256(sha256):
            pins[path] = sha256

    return pins


def _merge_pins_into(
    accumulator: dict[str, str],
    new_pins: dict[str, str],
    source_label: str = "",
) -> None:
    """Merge *new_pins* into *accumulator*, detecting path/hash conflicts.

    If the same path maps to two different SHA-256 values across profiles,
    raises :class:`ValueError` with a descriptive message.  Callers must not
    silently choose one.
    """
    for path, sha in new_pins.items():
        existing = accumulator.get(path)
        if existing is None:
            accumulator[path] = sha
        elif existing != sha:
            raise ValueError(
                f"SHA-256 conflict for library path {path!r}:\n"
                f"  existing (from earlier profile): {existing}\n"
                f"  new (from {source_label or 'profile'}):        {sha}\n"
                f"Loader library pins must be consistent across all profiles."
            )


# ---------------------------------------------------------------------------
# Forge/NeoForge installer JAR analysis
# ---------------------------------------------------------------------------


# NOTE: Processor allowlist and bracketed-Maven-artifact helpers removed.
# Forge/NeoForge uses installed-profile adoption; managed installer
# processor scanning is no longer performed.


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


def _extract_pins_from_install_profile(version_json: dict[str, Any] | None, pins_accumulator: dict[str, str]) -> None:
    """Extract SHA-256 pins from the version.json inside an installer JAR.

    Only version.json libraries (runtime libraries with downloadable upstream
    artifacts) are processed. Install-profile libraries, processor jars,
    classpath entries, data entries, and processor args are NOT scanned —
    the managed installer processor subsystem has been replaced by
    installed-profile adoption.
    """
    if version_json:
        for lib in version_json.get("libraries", []):
            _extract_pin_from_lib_entry(lib, pins_accumulator, None)


def _extract_pin_from_lib_entry(lib: dict[str, Any], pins_accumulator: dict[str, str], mirror_list_url: str | None) -> None:
    """Extract a pin from a single library entry, downloading if needed."""
    if not isinstance(lib, dict):
        return

    path: str | None = None
    url: str | None = None
    sha256: str | None = lib.get("sha256")

    downloads = lib.get("downloads")
    if isinstance(downloads, dict):
        artifact = downloads.get("artifact")
        if isinstance(artifact, dict):
            path = artifact.get("path")
            url = artifact.get("url")
            # Some entries have sha256 inside downloads.artifact
            if not sha256:
                sha256 = artifact.get("sha256")

    if path is None:
        name = lib.get("name")
        if isinstance(name, str):
            path = _maven_name_to_path(name)
            repo_url = lib.get("url")
            if isinstance(repo_url, str):
                url = repo_url.rstrip("/") + "/" + path

    if not isinstance(path, str) or not _is_safe_maven_path(path):
        return

    if sha256 and _is_valid_sha256(sha256):
        pins_accumulator[path] = sha256
    elif path not in pins_accumulator:
        _download_and_pin(path, pins_accumulator)


def _download_and_pin(maven_path: str, pins_accumulator: dict[str, str]) -> None:
    """Download a Maven artifact by its relative path and compute SHA-256.

    Tries the known Forge/NeoForge/Fabric/Quilt repos.
    """
    repos = [
        "https://maven.minecraftforge.net",
        "https://maven.neoforged.net/releases",
        "https://libraries.minecraft.net",
        "https://maven.fabricmc.net",
    ]
    for repo in repos:
        url = f"{repo}/{maven_path}"
        cache_name = maven_path.replace("/", "_").replace("@", "_at_")
        try:
            cached = _download_to_cache(url, cache_name)
            sha = _sha256_hex(cached.read_bytes())
            pins_accumulator[maven_path] = sha
            logger.debug("Pinned %s = %s...", maven_path, sha[:16])
            return
        except (urllib.error.URLError, OSError, Exception):
            continue

    logger.warning("Could not download %s from any known repo", maven_path)


# _is_processor_output_path removed — managed installer processor subsystem
# replaced by installed-profile adoption. Processor-generated output paths
# are no longer scanned during library path extraction.


def _extract_installer_library_paths(version_json: dict[str, Any] | None) -> list[str]:
    """Extract Maven-relative library paths from the version.json inside an installer JAR.

    Only version.json runtime libraries with downloadable upstream artifacts are
    included. Install-profile libraries, processor jars, classpath entries, data
    entries, and processor args are NOT scanned — the managed installer processor
    subsystem has been replaced by installed-profile adoption. Generated no-hash
    outputs are receipt-bound and excluded from manifest pins.

    Returns sorted unique paths.
    """
    paths: set[str] = set()

    if version_json is None:
        return []

    for lib in version_json.get("libraries", []):
        if not isinstance(lib, dict):
            continue
        p = lib.get("downloads", {}).get("artifact", {}).get("path")
        if p and _is_safe_maven_path(p):
            paths.add(p)
        else:
            name = lib.get("name")
            if isinstance(name, str):
                p2 = _maven_name_to_path(name)
                if _is_safe_maven_path(p2):
                    paths.add(p2)

    return sorted(paths)


def _verify_pin_coverage(
    paths: list[str],
    pins: dict[str, str],
    logger_prefix: str = "",
) -> bool:
    """Return True iff every Maven path in *paths* has a SHA-256 entry in *pins*.

    Logs a debug message for each missing path and returns False if any
    path is uncovered. Used to enforce runtime library pin coverage after refresh.
    """
    all_covered = True
    for p in paths:
        if p not in pins:
            logger.warning(
                "%s: missing library pin for %s",
                logger_prefix, p,
            )
            all_covered = False
    return all_covered


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


def _fetch_profile_json(url: str, cache_name: str) -> bytes:
    """Fetch a profile JSON, caching it in ``.cache/profile-json/``."""
    cache_dir = CACHE_DIR / "profile-json"
    cache_dir.mkdir(parents=True, exist_ok=True)
    cache_path = cache_dir / cache_name
    if cache_path.exists():
        logger.debug("Using cached profile JSON %s", cache_name)
        return cache_path.read_bytes()
    logger.info("Downloading profile JSON %s", url)
    data = _fetch_bytes(url)
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
    library_pins_accumulator: dict[str, str] | None = None,
    profile_library_paths_accumulator: dict[str, list[str]] | None = None,
) -> list[dict[str, Any]]:
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
        cache_name = re.sub(r'[^a-zA-Z0-9._-]', '_', f"fabric-{mc_version}-{loader_version}.json")
        try:
            data = _fetch_profile_json(profile_url, cache_name)
        except urllib.error.URLError as exc:
            logger.error(
                "Failed to fetch Fabric profile %s/%s: %s",
                mc_version,
                loader_version,
                exc,
            )
            continue

        # Extract library pins and per-profile library paths from the profile JSON.
        profile_json = None
        if library_pins_accumulator is not None:
            try:
                profile_json = json.loads(data)
                pins = _extract_pins_from_profile(profile_json)
                _merge_pins_into(
                    library_pins_accumulator,
                    pins,
                    source_label=f"Fabric {mc_version}/{loader_version}",
                )
            except (json.JSONDecodeError, ValueError) as exc:
                logger.warning(
                    "Failed to extract library pins from Fabric %s/%s: %s",
                    mc_version,
                    loader_version,
                    exc,
                )

        file_name = f"fabric-loader-{loader_version}-{mc_version}.json"
        sha = _stable_json_sha256(data)

        # Extract and store per-profile library paths.
        if profile_library_paths_accumulator is not None:
            if profile_json is None:
                try:
                    profile_json = json.loads(data)
                except json.JSONDecodeError:
                    profile_json = None
            if profile_json is not None:
                paths = _extract_library_paths_from_profile(profile_json)
                profile_library_paths_accumulator[file_name] = paths

        entries.append({
            "mc_version": mc_version,
            "loader_version": loader_version,
            "source_url": profile_url,
            "sha256": sha,
            "file_name": file_name,
            "file_type": "profile_json",
        })
        logger.info(
            "Added Fabric loader %s for MC %s (stable sha256=%s..., %d lib paths)",
            loader_version,
            mc_version,
            sha[:16],
            len(profile_library_paths_accumulator.get(file_name, []) if profile_library_paths_accumulator else []),
        )

    return entries


def _fetch_quilt(
    mc_version: str,
    per_mc_limit: int | None = None,
    library_pins_accumulator: dict[str, str] | None = None,
    profile_library_paths_accumulator: dict[str, list[str]] | None = None,
) -> list[dict[str, Any]]:
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
        cache_name = re.sub(r'[^a-zA-Z0-9._-]', '_', f"quilt-{mc_version}-{loader_version}.json")
        data: bytes | None = None
        profile_url = profile_url_mc_first
        try:
            data = _fetch_profile_json(profile_url_mc_first, cache_name)
        except urllib.error.HTTPError as exc:
            if exc.code == 404:
                logger.info(
                    "Quilt profile path %s 404, trying swapped order",
                    profile_url_mc_first,
                )
                profile_url = profile_url_loader_first
                cache_name = re.sub(r'[^a-zA-Z0-9._-]', '_', f"quilt-{mc_version}-{loader_version}-alt.json")
                try:
                    data = _fetch_profile_json(profile_url_loader_first, cache_name)
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

        # Extract library pins and per-profile library paths from the profile JSON.
        profile_json = None
        if library_pins_accumulator is not None:
            try:
                profile_json = json.loads(data)
                pins = _extract_pins_from_profile(profile_json)
                _merge_pins_into(
                    library_pins_accumulator,
                    pins,
                    source_label=f"Quilt {mc_version}/{loader_version}",
                )
            except (json.JSONDecodeError, ValueError) as exc:
                logger.warning(
                    "Failed to extract library pins from Quilt %s/%s: %s",
                    mc_version,
                    loader_version,
                    exc,
                )

        file_name = f"quilt-loader-{loader_version}-{mc_version}.json"
        sha = _stable_json_sha256(data)

        # Extract and store per-profile library paths.
        if profile_library_paths_accumulator is not None:
            if profile_json is None:
                try:
                    profile_json = json.loads(data)
                except json.JSONDecodeError:
                    profile_json = None
            if profile_json is not None:
                paths = _extract_library_paths_from_profile(profile_json)
                profile_library_paths_accumulator[file_name] = paths

        entries.append({
            "mc_version": mc_version,
            "loader_version": loader_version,
            "source_url": profile_url,
            "sha256": sha,
            "file_name": file_name,
            "file_type": "profile_json",
        })
        logger.info(
            "Added Quilt loader %s for MC %s (stable sha256=%s..., %d lib paths)",
            loader_version,
            mc_version,
            sha[:16],
            len(profile_library_paths_accumulator.get(file_name, []) if profile_library_paths_accumulator else []),
        )

    return entries


def _fetch_neoforge(
    mc_versions: list[str],
    per_mc_limit: int | None = None,
    library_pins_accumulator: dict[str, str] | None = None,
    installer_library_paths_accumulator: dict[str, list[str]] | None = None,
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

        # Extract installer profile for library pins and coverage paths
        install = _extract_install_profile(jar_path)
        version_data = _extract_version_json(jar_path) if install else None
        file_name = f"neoforge-{version}-installer.jar"

        installer_spec = None
        if install is not None:
            spec_val = install.get("spec")
            if isinstance(spec_val, int):
                installer_spec = spec_val

        # Extract library pins and coverage paths
        paths: list[str] = []
        if library_pins_accumulator is not None:
            _extract_pins_from_install_profile(version_data, library_pins_accumulator)

        if installer_library_paths_accumulator is not None:
            paths = _extract_installer_library_paths(version_data)
            installer_library_paths_accumulator[file_name] = paths

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
    library_pins_accumulator: dict[str, str] | None = None,
    installer_library_paths_accumulator: dict[str, list[str]] | None = None,
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
            jar_path = _download_to_cache(source_url, cache_name, timeout=1)
        except urllib.error.URLError as exc:
            logger.error("Failed to download Forge installer %s: %s", version, exc)
            continue

        jar_sha = _sha256_hex(jar_path.read_bytes())
        version_json_sha = _extract_version_json_sha256(jar_path)

        # Extract installer profile for library pins and coverage paths
        install = _extract_install_profile(jar_path)
        version_data = _extract_version_json(jar_path) if install else None
        file_name = f"forge-{version}-installer.jar"

        installer_spec = None
        if install is not None:
            spec_val = install.get("spec")
            if isinstance(spec_val, int):
                installer_spec = spec_val

        # Extract library pins and coverage paths
        paths: list[str] = []
        if library_pins_accumulator is not None:
            _extract_pins_from_install_profile(version_data, library_pins_accumulator)

        if installer_library_paths_accumulator is not None:
            paths = _extract_installer_library_paths(version_data)
            installer_library_paths_accumulator[file_name] = paths

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
        "library_pins": {},
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

    # Accumulate library pins and per-profile library paths across all profiles.
    library_pins_accumulator: dict[str, str] = {}
    profile_library_paths_accumulator: dict[str, list[str]] = {}
    installer_library_paths_accumulator: dict[str, list[str]] = {}
    manifest.setdefault("library_pins", {})
    manifest.setdefault("profile_library_paths", {})
    manifest.setdefault("installer_library_paths", {})

    for mc_version in mc_versions:
        logger.info("Fetching Fabric versions for %s", mc_version)
        loaders["fabric"] = _merge_entries(
            loaders["fabric"],
            _fetch_fabric(mc_version, per_mc_limit, library_pins_accumulator, profile_library_paths_accumulator),
        )

        logger.info("Fetching Quilt versions for %s", mc_version)
        loaders["quilt"] = _merge_entries(
            loaders["quilt"],
            _fetch_quilt(mc_version, per_mc_limit, library_pins_accumulator, profile_library_paths_accumulator),
        )

    logger.info("Fetching NeoForge versions for %s", mc_versions)
    loaders["neoforge"] = _merge_entries(
        loaders["neoforge"],
        _fetch_neoforge(mc_versions, per_mc_limit, library_pins_accumulator, installer_library_paths_accumulator),
    )

    logger.info("Fetching Forge versions for %s", mc_versions)
    loaders["forge"] = _merge_entries(
        loaders["forge"],
        _fetch_forge(mc_versions, per_mc_limit, library_pins_accumulator, installer_library_paths_accumulator),
    )

    # Final safety filter in case any function ignored the limit.
    for loader in loaders:
        loaders[loader] = _limit_entries(loaders[loader], per_mc_limit)

    # Merge accumulated library pins with any existing pins.
    existing_pins = manifest.get("library_pins", {})
    _merge_pins_into(existing_pins, library_pins_accumulator, source_label="fetch")
    manifest["library_pins"] = dict(sorted(existing_pins.items()))

    # Merge accumulated profile library paths. For entries that were re-fetched,
    # the new paths supersede any old ones; existing entries that were not
    # re-fetched retain their previous paths.
    existing_plp = manifest.get("profile_library_paths", {})
    for fname, paths in profile_library_paths_accumulator.items():
        existing_plp[fname] = paths
    manifest["profile_library_paths"] = dict(sorted(existing_plp.items()))

    # Merge accumulated installer library paths.
    existing_ilp = manifest.get("installer_library_paths", {})
    for fname, paths in installer_library_paths_accumulator.items():
        existing_ilp[fname] = paths
    manifest["installer_library_paths"] = dict(sorted(existing_ilp.items()))

    _write_loader_manifests(manifest)
    _write_known_good_hashes(manifest)

    total = sum(len(entries) for entries in loaders.values())
    logger.info("Done. %d loader entries in loader_manifests.json", total)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
