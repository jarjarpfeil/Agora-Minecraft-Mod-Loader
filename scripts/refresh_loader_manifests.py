#!/usr/bin/env python3
"""Refresh loader manifests, compile the registry database, and verify hashes.

Auto-discovers available stable Minecraft versions from Mojang's official
version manifest,
optionally limited to the latest N or versions released since a given version.
You can still pass an explicit list with --mc-versions.

Usage examples:
    python scripts/refresh_loader_manifests.py --skip-sign
    python scripts/refresh_loader_manifests.py --auto-versions --latest 5 --skip-sign
    python scripts/refresh_loader_manifests.py --auto-versions --skip-sign      # all stable MC versions (default)
    python scripts/refresh_loader_manifests.py --mc-versions 1.21 1.20.6 --skip-sign
    python scripts/refresh_loader_manifests.py --auto-versions --since 1.20 --skip-sign
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import logging
import os
import re
import subprocess
import sys
import zipfile
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT / "scripts"))

import fetch_loader_manifests as fetch  # noqa: E402

COMPILE_SCRIPT = REPO_ROOT / "compiler" / "compile.py"
LOADER_MANIFESTS_PATH = REPO_ROOT / "loader-manifests" / "loader_manifests.json"

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(message)s",
)
logger = logging.getLogger("refresh_loader_manifests")


def _is_standard_release(version: str) -> bool:
    """Return True for clean numeric versions like 1.21, 1.21.4, 26.1, 26.1.1.

    Ignores snapshots (snapshot/alpha/beta/combat tags, dated versions like
    26w13a, suffixed rc/pre builds). This is a secondary safety net on top of
    Fabric's ``stable`` flag — both must pass for a version to be included.
    """
    return bool(re.fullmatch(r"\d+\.\d+(?:\.\d+)?", version))


def discover_stable_mc_versions(
    since: str | None = None, latest: int | None = None
) -> list[str]:
    """Return stable Minecraft versions from Mojang's official version manifest.

    Mojang's manifest at https://piston-meta.mojang.com/mc/game/version_manifest_v2.json
    lists every version ever released (1.0 onward). We filter to type=='release'
    and apply the existing _is_standard_release() regex (drops snapshots,
    prereleases, experimentals). Versions are listed newest-first by Mojang.
    """
    url = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json"
    logger.info("Discovering stable Minecraft versions from Mojang: %s", url)
    data: dict[str, Any] = fetch._fetch_json(url)

    stable = [
        v["id"]
        for v in data.get("versions", [])
        if v.get("type") == "release" and _is_standard_release(v["id"])
    ]

    if since:
        since_key = fetch._version_key(since)
        stable = [v for v in stable if fetch._version_key(v) >= since_key]

    if latest:
        stable = stable[:latest]

    if not stable:
        raise RuntimeError("Could not discover any stable Minecraft versions from Mojang")

    logger.info("Discovered %d stable Minecraft versions (%s ... %s)",
                len(stable), stable[-1] if stable else "?", stable[0] if stable else "?")
    return stable


def verify_manifest(path: Path) -> int:
    """Re-download every pinned loader file and compare its SHA-256 to the manifest.

    Uses the same download cache as fetch_loader_manifests.py, so re-running is
    fast once jars have been downloaded.
    """
    logger.info("Verifying pinned hashes in %s", path)
    manifest: dict[str, Any] = json.loads(path.read_text(encoding="utf-8"))
    total = 0
    ok = 0
    mismatch = 0
    errors = 0

    for loader, entries in manifest.get("loaders", {}).items():
        for entry in entries:
            total += 1
            source_url = entry.get("source_url")
            expected_sha = entry.get("sha256")
            file_name = entry.get("file_name", source_url)

            if not source_url or not expected_sha:
                logger.warning("Skipping %s — missing URL or hash", file_name)
                errors += 1
                continue

            try:
                cache_name = source_url.split("/")[-1]
                # Profile JSON URLs all end in /profile/json; give them unique cache names
                # so we don't reuse the same cached file for every entry.
                if not cache_name.endswith(".jar"):
                    ext = os.path.splitext(cache_name)[1] or ".bin"
                    cache_name = (
                        hashlib.sha256(source_url.encode("utf-8")).hexdigest()[:32] + ext
                    )
                cache_path = fetch._download_to_cache(source_url, cache_name)
                data = cache_path.read_bytes()

                # Profile/version JSON is hashed with volatile keys stripped.
                if entry.get("file_type") == "profile_json":
                    actual_sha = fetch._stable_json_sha256(data)
                else:
                    actual_sha = fetch._sha256_hex(data)

                if actual_sha != expected_sha:
                    logger.error(
                        "Hash mismatch for %s %s\n  expected: %s\n  actual:   %s",
                        loader,
                        file_name,
                        expected_sha,
                        actual_sha,
                    )
                    mismatch += 1
                    continue

                if (
                    entry.get("file_type") == "installer_jar"
                    and entry.get("version_json_sha256")
                ):
                    try:
                        with zipfile.ZipFile(io.BytesIO(data)) as zf:
                            if "version.json" in zf.namelist():
                                actual_vj = fetch._stable_json_sha256(zf.read("version.json"))
                                if actual_vj != entry["version_json_sha256"]:
                                    logger.error(
                                        "version.json hash mismatch for %s %s",
                                        loader,
                                        file_name,
                                    )
                                    mismatch += 1
                                    continue
                    except (zipfile.BadZipFile, OSError) as exc:
                        logger.error("Could not inspect %s: %s", file_name, exc)
                        errors += 1
                        continue

                ok += 1
            except Exception as exc:  # noqa: BLE001
                logger.error("Failed to verify %s %s: %s", loader, file_name, exc)
                errors += 1

    logger.info(
        "Verification complete: %d total, %d OK, %d mismatches, %d errors",
        total,
        ok,
        mismatch,
        errors,
    )
    return 0 if mismatch == 0 and errors == 0 else 1


def fetch_loader_manifests(
    mc_versions: list[str], per_mc_limit: int | None, keep_others: bool = False
) -> None:
    """Run the same logic as fetch_loader_manifests.py from this process."""
    logger.info("Fetching loader manifests for Minecraft versions: %s", mc_versions)

    manifest = fetch._load_existing_manifest()
    manifest["domain_allowlist"] = sorted(
        set(manifest.get("domain_allowlist", []) + fetch.DOMAIN_ALLOWLIST)
    )
    loaders = manifest.setdefault("loaders", {})
    for loader in ("fabric", "quilt", "neoforge", "forge"):
        loaders.setdefault(loader, [])

    if not keep_others:
        preserve = set(mc_versions)
        for loader in loaders:
            before = len(loaders[loader])
            loaders[loader] = [
                e for e in loaders[loader] if e.get("mc_version") in preserve
            ]
            removed = before - len(loaders[loader])
            if removed:
                logger.info(
                    "Pruned %d %s entries for MC versions not in target list",
                    removed,
                    loader,
                )

    for mc_version in mc_versions:
        logger.info("Fetching Fabric versions for %s", mc_version)
        loaders["fabric"] = fetch._merge_entries(
            loaders["fabric"], fetch._fetch_fabric(mc_version, per_mc_limit)
        )

        logger.info("Fetching Quilt versions for %s", mc_version)
        loaders["quilt"] = fetch._merge_entries(
            loaders["quilt"], fetch._fetch_quilt(mc_version, per_mc_limit)
        )

    logger.info("Fetching NeoForge versions for %s", mc_versions)
    loaders["neoforge"] = fetch._merge_entries(
        loaders["neoforge"], fetch._fetch_neoforge(mc_versions, per_mc_limit)
    )

    logger.info("Fetching Forge versions for %s", mc_versions)
    loaders["forge"] = fetch._merge_entries(
        loaders["forge"], fetch._fetch_forge(mc_versions, per_mc_limit)
    )

    for loader in loaders:
        loaders[loader] = fetch._limit_entries(loaders[loader], per_mc_limit)

    fetch._write_loader_manifests(manifest)
    fetch._write_known_good_hashes(manifest)

    total = sum(len(entries) for entries in loaders.values())
    logger.info("Done. %d loader entries written", total)


def compile_registry(out: str, skip_sign: bool) -> None:
    """Run compiler/compile.py as a subprocess."""
    cmd = [sys.executable, str(COMPILE_SCRIPT), "--out", out]
    if skip_sign:
        cmd.append("--skip-sign")

    logger.info("Running compiler: %s", " ".join(cmd))
    subprocess.run(cmd, cwd=REPO_ROOT, check=True)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Refresh loader manifests, compile the registry, and verify hashes."
    )
    source = parser.add_mutually_exclusive_group()
    source.add_argument(
        "--mc-versions",
        nargs="+",
        help="Explicit Minecraft versions to refresh (default: 1.21)",
    )
    source.add_argument(
        "--auto-versions",
        action="store_true",
        help="Auto-discover stable Minecraft versions from Mojang's official version manifest",
    )
    parser.add_argument(
        "--latest",
        type=int,
        help="With --auto-versions, keep only the N latest stable versions (default: all)",
    )
    parser.add_argument(
        "--since",
        help="With --auto-versions, keep only versions >= this (e.g. 1.20)",
    )
    parser.add_argument(
        "--latest-per-mc",
        type=int,
        default=5,
        help="Latest loader versions to keep per Minecraft version (default: 5)",
    )
    parser.add_argument(
        "--keep-others",
        action="store_true",
        help="Keep loader entries for MC versions not in the requested/auto list",
    )
    parser.add_argument(
        "--out",
        default="registry.db",
        help="Output path for the compiled registry database (default: registry.db)",
    )
    parser.add_argument(
        "--skip-sign",
        action="store_true",
        help="Write a placeholder signature instead of a real Ed25519 signature",
    )
    parser.add_argument(
        "--no-verify",
        action="store_true",
        help="Skip the post-compile hash verification step",
    )
    args = parser.parse_args()

    if args.auto_versions:
        # Default to ALL stable versions; user can pass --latest N to slice
        # or --since X to keep only versions newer than X.
        latest = args.latest
        mc_versions = discover_stable_mc_versions(since=args.since, latest=latest)

        # Write minecraft_versions.json for the desktop app to bake in.
        mc_versions_path = Path(__file__).resolve().parent.parent / "loader-manifests" / "minecraft_versions.json"
        mc_versions_path.parent.mkdir(parents=True, exist_ok=True)
        mc_versions_path.write_text(json.dumps(mc_versions, indent=2) + "\n", encoding="utf-8")
        logger.info("Wrote %d Minecraft versions to %s", len(mc_versions), mc_versions_path)
    elif args.mc_versions:
        mc_versions = args.mc_versions
    else:
        mc_versions = ["1.21"]

    logger.info("Target Minecraft versions: %s", mc_versions)

    per_mc_limit: int | None = None if args.latest_per_mc <= 0 else args.latest_per_mc
    fetch_loader_manifests(mc_versions, per_mc_limit, keep_others=args.keep_others)
    compile_registry(args.out, args.skip_sign)

    if not args.no_verify:
        return verify_manifest(LOADER_MANIFESTS_PATH)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
