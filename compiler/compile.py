#!/usr/bin/env python3
from __future__ import annotations

"""
Nightly compiler skeleton for the Agora curated mod registry.

Reads JSON manifests from registry/ and crash-signatures/, builds an in-memory
SQLite database matching the client-side schema, and emits registry.db plus an
Ed25519 signature file registry.db.sig.
"""

import argparse
import json
import logging
import os
import platform
import re
import signal
import sqlite3
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import requests

# Optional signing dependency. The compiler will refuse to produce a real
# signature unless PyNaCl is installed and ED25519_PRIVATE_KEY is configured.
try:
    import nacl.signing  # type: ignore
    import nacl.encoding  # type: ignore
except ImportError:
    nacl = None  # type: ignore

REPO_ROOT = Path(__file__).resolve().parent.parent
REGISTRY_DIR = REPO_ROOT / "registry"
CRASH_SIGNATURES_DIR = REPO_ROOT / "crash-signatures"
LOADER_MANIFESTS_DIR = REPO_ROOT / "loader-manifests"

# ---------------------------------------------------------------------------
# Regex DoS protection (§2.4.1)
# ---------------------------------------------------------------------------

# 100 KB corpus for catastrophic-backtracking detection.
_REGEX_TEST_CORPUS = "a" * 100000


def _test_regex_timeout(pattern: re.Pattern[str], timeout_secs: float = 1.0) -> bool:
    """Return True if the pattern matches the test corpus within *timeout_secs*.

    On Unix this uses ``signal.alarm``.  On Windows it spawns a subprocess with
    a timeout because ``signal.alarm`` is unavailable.

    Returns False on any error (timeout, crash, etc.) to fail-safe.
    """
    if platform.system() != "Windows":
        # Unix: signal.alarm provides a hard process-level timeout.
        old_handler = signal.signal(signal.SIGALRM, lambda *_: (_ for _ in ()).throw(TimeoutError()))
        signal.setitimer(signal.ITIMER_REAL, timeout_secs)
        try:
            pattern.search(_REGEX_TEST_CORPUS)
            return True
        except TimeoutError:
            return False
        finally:
            signal.setitimer(signal.ITIMER_REAL, 0)  # cancel the alarm
            signal.signal(signal.SIGALRM, old_handler)
    else:
        # Windows: no signal.alarm — use subprocess with timeout.
        # Embed the corpus in the code string to avoid CLI length limits.
        code = (
            "import re, sys; "
            "p = re.compile(sys.argv[1]); "
            "p.search('a' * 100000)"
        )
        try:
            subprocess.run(
                [sys.executable, "-c", code, pattern.pattern],
                timeout=timeout_secs,
                check=True,
            )
            return True
        except subprocess.TimeoutExpired:
            return False
        except Exception:
            return False


def _load_dotenv(path: Path) -> None:
    """Load KEY=VALUE pairs from a .env file into os.environ without overriding."""
    if not path.exists():
        return
    with path.open("r", encoding="utf-8") as fh:
        for raw_line in fh:
            line = raw_line.strip()
            if not line or line.startswith("#"):
                continue
            if "=" not in line:
                continue
            key, value = line.split("=", 1)
            key = key.strip()
            if key.startswith("export "):
                key = key[7:].strip()
            value = value.strip()
            if len(value) >= 2 and value[0] == value[-1] and value[0] in ('"', "'"):
                value = value[1:-1]
            if key and key not in os.environ:
                os.environ[key] = value


_load_dotenv(REPO_ROOT / ".env")

# ---------------------------------------------------------------------------
# Logging
# ---------------------------------------------------------------------------

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(message)s",
)
logger = logging.getLogger("compiler")


# ---------------------------------------------------------------------------
# Schema setup
# ---------------------------------------------------------------------------

SCHEMA_VERSION = 2


CREATE_INDEXES = [
    "CREATE INDEX IF NOT EXISTS idx_registry_items_content_type_status_score ON registry_items(content_type, status, net_score)",
    "CREATE INDEX IF NOT EXISTS idx_registry_items_velocity ON registry_items(velocity)",
    "CREATE INDEX IF NOT EXISTS idx_registry_items_date_added ON registry_items(date_added)",
    "CREATE INDEX IF NOT EXISTS idx_item_categories_category_id ON item_categories(category_id)",
    "CREATE INDEX IF NOT EXISTS idx_pack_mods_pack_id ON pack_mods(pack_id)",
    "CREATE INDEX IF NOT EXISTS idx_pack_mods_mod_id ON pack_mods(mod_id)",
]


def create_tables(conn: sqlite3.Connection) -> None:
    """Create all registry tables in the supplied connection."""
    cursor = conn.cursor()

    cursor.execute("""
        CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER PRIMARY KEY
        )
    """)

    cursor.execute("""
        CREATE TABLE IF NOT EXISTS registry_items (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            content_type TEXT NOT NULL,
            download_strategy TEXT NOT NULL,
            source_identifier TEXT NOT NULL,
            sha256 TEXT NOT NULL,
            upvotes INTEGER DEFAULT 0,
            downvotes INTEGER DEFAULT 0,
            net_score INTEGER DEFAULT 0,
            velocity REAL DEFAULT 0.0,
            status TEXT DEFAULT 'active',
            is_immune BOOLEAN DEFAULT 0,
            immunity_reason TEXT,
            allow_comments BOOLEAN DEFAULT 1,
            immunity_cooldown_until TEXT,
            icon_url TEXT,
            gallery_urls_json TEXT,
            date_added TEXT,
            compatible_versions_json TEXT,
            -- Supplementary metadata hydrated by the nightly compiler from the
            -- upstream source (e.g. Modrinth bulk project API). Stored as TEXT
            -- only — image *URLs* are kept, never binary image data — so the
            -- signed registry.db stays compact (§6.3 / §4.2 "media strategy").
            -- Manifest/curator-provided values always take precedence over
            -- API-fetched values for these fields.
            description TEXT,
            body_markdown TEXT,
            page_url TEXT,
            license_id TEXT,
            source_updated_at TEXT
        )
    """)

    cursor.execute("""
        CREATE TABLE IF NOT EXISTS categories (
            id TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            is_community BOOLEAN DEFAULT 0
        )
    """)

    cursor.execute("""
        CREATE TABLE IF NOT EXISTS item_categories (
            item_id TEXT,
            category_id TEXT,
            PRIMARY KEY (item_id, category_id),
            FOREIGN KEY (item_id) REFERENCES registry_items(id),
            FOREIGN KEY (category_id) REFERENCES categories(id)
        )
    """)

    cursor.execute("""
        CREATE TABLE IF NOT EXISTS curator_reviews (
            item_id TEXT PRIMARY KEY,
            curator_note TEXT,
            top_reviews_json TEXT,
            FOREIGN KEY (item_id) REFERENCES registry_items(id)
        )
    """)

    cursor.execute("""
        CREATE TABLE IF NOT EXISTS crash_signatures (
            id TEXT PRIMARY KEY,
            name TEXT,
            regex_pattern TEXT,
            solution_markdown TEXT,
            action_button_json TEXT
        )
    """)

    # Holds loader hashes and domain allowlist from loader-manifests/.
    cursor.execute("""
        CREATE TABLE IF NOT EXISTS system_config (
            key TEXT PRIMARY KEY,
            value_json TEXT NOT NULL
        )
    """)

    # Pack membership: which mods belong to which curated packs.
    cursor.execute("""
        CREATE TABLE IF NOT EXISTS pack_mods (
            pack_id TEXT,
            mod_id TEXT,
            source TEXT,
            version TEXT,
            status TEXT,
            description TEXT,
            PRIMARY KEY (pack_id, mod_id),
            FOREIGN KEY (mod_id) REFERENCES registry_items(id)
        )
    """)

    for index_sql in CREATE_INDEXES:
        cursor.execute(index_sql)


# ---------------------------------------------------------------------------
# File discovery
# ---------------------------------------------------------------------------


def load_json_files(directory: Path) -> list[tuple[Path, dict[str, Any]]]:
    """Recursively load every .json file under *directory*, sorted for determinism."""
    results: list[tuple[Path, dict[str, Any]]] = []
    if not directory.exists():
        logger.warning("Directory does not exist: %s", directory)
        return results

    for path in sorted(directory.rglob("*.json")):
        # Skip archived items per spec.
        if "archived" in path.parts:
            continue
        try:
            with path.open("r", encoding="utf-8") as fh:
                data = json.load(fh)
            results.append((path, data))
        except (json.JSONDecodeError, OSError) as exc:
            logger.error("Failed to load %s: %s", path, exc)
            raise
    return results


# ---------------------------------------------------------------------------
# Category handling
# ---------------------------------------------------------------------------


def display_name(slug: str) -> str:
    """Convert a category slug into a human-readable display name."""
    return slug.replace("-", " ").replace("_", " ").title()


def register_category(conn: sqlite3.Connection, category_id: str, is_community: bool) -> None:
    cursor = conn.cursor()
    cursor.execute(
        """
        INSERT INTO categories (id, display_name, is_community)
        VALUES (?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET is_community=excluded.is_community
        """,
        (category_id, display_name(category_id), int(is_community)),
    )


def link_item_category(conn: sqlite3.Connection, item_id: str, category_id: str) -> None:
    cursor = conn.cursor()
    cursor.execute(
        "INSERT OR IGNORE INTO item_categories (item_id, category_id) VALUES (?, ?)",
        (item_id, category_id),
    )


# ---------------------------------------------------------------------------
# Validators
# ---------------------------------------------------------------------------


def validate_sha256(raw: Any) -> str:
    """Ensure *raw* is a valid 64-char hex string. Reject None/empty.

    Per §2.1, sha256 is required for all download strategies. The compiler
    must either populate it (from GitHub/Modrinth API) or fail loudly.
    """
    if raw is None or raw == "":
        logger.error("sha256 is required and must not be empty or null")
        raise SystemExit(1)
    if not isinstance(raw, str):
        logger.error("sha256 must be a string, got %s", type(raw).__name__)
        raise SystemExit(1)
    if not re.fullmatch(r"[0-9a-fA-F]{64}", raw):
        logger.error("sha256 must be exactly 64 hex characters: %s", raw)
        raise SystemExit(1)
    return raw


# ---------------------------------------------------------------------------
# Mod / pack insertion
# ---------------------------------------------------------------------------


def default_compatible_versions(item: dict[str, Any]) -> list[dict[str, str]]:
    """Return a sensible compatibility fallback when none is provided."""
    return [
        {
            "mc_version": "1.21",
            "loader": "fabric",
            "mod_version": "latest",
        }
    ]


def manifest_date_added(path: Path) -> str:
    """Return the date *path* first appeared in the registry.

    Uses `git log` to find the author date of the first commit touching the
    file. Falls back to filesystem mtime for untracked local-dev files.

    This is deterministic across CI runs (actions/checkout preserves git history),
    unlike st_mtime which is overwritten to checkout time on every clone.
    """
    import subprocess

    try:
        result = subprocess.run(
            ["git", "log", "--reverse", "--format=%aI", "--", str(path)],
            capture_output=True,
            text=True,
            check=True,
            timeout=10,
        )
        first_line = result.stdout.strip().splitlines()
        if first_line:
            return first_line[0]
    except (subprocess.CalledProcessError, FileNotFoundError, subprocess.TimeoutExpired):
        pass

    # Fallback for untracked files during local development.
    mtime = path.stat().st_mtime
    return datetime.fromtimestamp(mtime, tz=timezone.utc).isoformat()


def insert_registry_item(conn: sqlite3.Connection, item: dict[str, Any], path: Path) -> None:
    """Insert a mod/pack/asset row into registry_items."""
    cursor = conn.cursor()
    item_id = item["id"]
    governance = item.get("governance", {})

    is_immune = bool(governance.get("immune", False))
    immunity_reason = governance.get("override_justification")
    allow_comments = bool(governance.get("allow_comments", True))

    sha256 = validate_sha256(item.get("sha256"))
    gallery = item.get("gallery_urls", [])
    compatible_versions = item.get("compatible_versions") or default_compatible_versions(item)

    # Supplementary metadata (description, body, etc.) is resolved by the
    # Modrinth hydrator with manifest/override precedence; read the final
    # values here. These are TEXT-only and may be None for items that were
    # not hydratable (e.g. non-modrinth strategies lacking manual fields).
    cursor.execute(
        """
        INSERT INTO registry_items (
            id, name, content_type, download_strategy, source_identifier, sha256,
            upvotes, downvotes, net_score, velocity, status,
            is_immune, immunity_reason, allow_comments, immunity_cooldown_until,
            icon_url, gallery_urls_json, date_added, compatible_versions_json,
            description, body_markdown, page_url, license_id, source_updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            item_id,
            item["name"],
            item.get("content_type", "mod"),
            item["download_strategy"],
            item["source_identifier"],
            sha256,
            0,
            0,
            0,
            0.0,
            "active",
            int(is_immune),
            immunity_reason,
            int(allow_comments),
            None,
            item.get("icon_url"),
            json.dumps(gallery, separators=(",", ":")),
            manifest_date_added(path),
            json.dumps(compatible_versions, separators=(",", ":")),
            item.get("description"),
            item.get("body_markdown"),
            item.get("page_url"),
            item.get("license_id"),
            item.get("source_updated_at"),
        ),
    )

    # Curator note.
    cursor.execute(
        """
        INSERT INTO curator_reviews (item_id, curator_note, top_reviews_json)
        VALUES (?, ?, ?)
        """,
        (item_id, item.get("curator_note", ""), json.dumps([], separators=(",", ":"))),
    )

    # Categories.
    # Curator-provided taxonomy always wins. Only when the manifest sets
    # NEITHER base_categories NOR community_categories do we fall back to the
    # Modrinth-derived `_hydrated_categories` (captured by the metadata
    # hydrator). These upstream tags are registered as community (unvetted)
    # categories so they're visually distinct and trivially overridable later
    # by adding a real base_categories/community_categories list to the manifest.
    base_cats: list[str] = list(item.get("base_categories", []))
    community_cats: list[str] = list(item.get("community_categories", []))
    fell_back_to_upstream = False
    if not base_cats and not community_cats:
        upstream = item.get("_hydrated_categories")
        if upstream:
            community_cats = upstream
            fell_back_to_upstream = True

    for category_id in base_cats:
        register_category(conn, category_id, is_community=False)
        link_item_category(conn, item_id, category_id)
    for category_id in community_cats:
        register_category(conn, category_id, is_community=True)
        link_item_category(conn, item_id, category_id)

    if fell_back_to_upstream:
        logger.info(
            "Item '%s' has no manual categories; linked %d community categories from upstream.",
            item_id,
            len(community_cats),
        )


# ---------------------------------------------------------------------------
# Modrinth batch metadata hydration
# ---------------------------------------------------------------------------

_MODRINTH_API_URL = "https://api.modrinth.com/v2/projects"
_MODRINTH_USER_AGENT = "AgoraCompiler/1.0"
_MODRINTH_BATCH_SIZE = 100
_MODRINTH_PAGE_BASE = "https://modrinth.com"

# Modrinth `project_type` → canonical site URL path segment.
_PROJECT_TYPE_URL_PATH = {
    "mod": "mod",
    "modpack": "modpack",
    "shader": "shader",
    "resourcepack": "resourcepack",
    "plugin": "plugin",
}

# Loader names Modrinth lumps into the `categories` array. These are already
# represented in `compatible_versions_json` loaders, so we exclude them from
# the category taxonomy fallback to avoid polluting category facets.
_MODRINTH_LOADER_CATEGORY_NAMES = frozenset(
    {
        "fabric",
        "quilt",
        "forge",
        "neoforge",
        "liteloader",
        "modloader",
        "rift",
        "canvas",
        "iris",
        "optifine",
        "vanilla",
    }
)


def _build_modrinth_page_url(proj: dict[str, Any]) -> str | None:
    """Construct the canonical Modrinth page URL from a bulk project payload."""
    slug = proj.get("slug")
    if not slug:
        return None
    path = _PROJECT_TYPE_URL_PATH.get(proj.get("project_type", "mod"), "mod")
    return f"{_MODRINTH_PAGE_BASE}/{path}/{slug}"


def _hydrate_modrinth_metadata(items: list[dict[str, Any]]) -> None:
    """Batch-query the Modrinth API to hydrate rich metadata for ``modrinth_id`` items.

    Pulls the short description, full markdown body, canonical page URL,
    license id, and last-updated timestamp from the bulk ``/v2/projects``
    endpoint (up to 100 projects per request), in addition to the icon and
    gallery URLs. This bakes rich, instant-on metadata into the signed
    ``registry.db`` so the client never has to hit Modrinth's API at browse
    time (§6.3 / §4.2 "media strategy": text + image *URLs* only, no binary).

    Precedence (per Gemini's "curator override" principle): a manifest- or
    curator-provided value always wins over the API value.

      - ``icon_url`` / ``gallery_urls``        : manifest value kept if present
      - ``description``                          : ``description_override`` > manifest ``description`` > API
      - ``body_markdown``                       : ``body_override`` > manifest ``body_markdown`` > API ``body``
      - ``page_url``                            : manifest ``page_url`` > API (constructed from slug)
      - ``license_id``                          : manifest ``license`` > API ``license.id``
      - ``source_updated_at``                   : manifest ``source_updated_at`` > API ``updated``

    Network failures degrade gracefully with a warning; items simply keep
    whatever manifest provided.
    """
    # Collect modrinth_id items that lack at least one hydratable field.
    hydratable_keys = (
        "icon_url",
        "gallery_urls",
        "description",
        "body_markdown",
        "page_url",
        "license_id",
        "source_updated_at",
    )
    to_hydrate: list[tuple[int, dict[str, Any], str]] = []
    for idx, item in enumerate(items):
        if item.get("download_strategy") != "modrinth_id":
            continue
        mid = item.get("modrinth_id", item.get("source_identifier", ""))
        if not mid:
            continue
        # Skip only if the manifest already supplies every hydratable field.
        if all(item.get(k) for k in hydratable_keys):
            continue
        to_hydrate.append((idx, item, mid))

    if not to_hydrate:
        return

    # Group by modrinth_id for the batch query (dedupe IDs).
    id_to_indices: dict[str, list[int]] = {}
    id_list: list[str] = []
    for idx, _item, mid in to_hydrate:
        if mid not in id_to_indices:
            id_list.append(mid)
            id_to_indices[mid] = []
        id_to_indices[mid].append(idx)

    # Batch-query in chunks via GET with the JSON-array-encoded `ids` param.
    hydrated: dict[str, dict[str, Any]] = {}
    for i in range(0, len(id_list), _MODRINTH_BATCH_SIZE):
        chunk = id_list[i : i + _MODRINTH_BATCH_SIZE]
        ids_param = json.dumps(chunk)
        try:
            resp = requests.get(
                _MODRINTH_API_URL,
                params={"ids": ids_param},
                headers={"User-Agent": _MODRINTH_USER_AGENT},
                timeout=30,
            )
            resp.raise_for_status()
            projects = resp.json()
            for proj in projects:
                proj_id = proj.get("id", "")
                if proj_id:
                    hydrated[proj_id] = proj
        except Exception as exc:  # noqa: BLE001
            logger.warning("Modrinth batch metadata hydration failed for chunk %d-%d: %s", i, i + len(chunk), exc)

    # Apply hydrated data back with manifest/override precedence.
    for idx, item, mid in to_hydrate:
        proj = hydrated.get(mid)
        if proj is None:
            continue

        # Icon + gallery: manifest value kept if present.
        if not item.get("icon_url"):
            icon = proj.get("icon_url")
            if icon:
                item["icon_url"] = icon
        if not item.get("gallery_urls"):
            gallery = proj.get("gallery", [])
            if gallery:
                item["gallery_urls"] = gallery

        # Description: override > manifest > API.
        if item.get("description_override"):
            item["description"] = item["description_override"]
        elif not item.get("description"):
            desc = proj.get("description")
            if desc:
                item["description"] = desc

        # Body markdown: override > manifest > API.
        if item.get("body_override"):
            item["body_markdown"] = item["body_override"]
        elif not item.get("body_markdown"):
            body = proj.get("body")
            if body:
                item["body_markdown"] = body

        # Canonical page URL: manifest > constructed-from-slug.
        if not item.get("page_url"):
            page_url = _build_modrinth_page_url(proj)
            if page_url:
                item["page_url"] = page_url

        # License id: manifest `license` wins; else API license.id.
        if not item.get("license_id"):
            if item.get("license"):
                item["license_id"] = item["license"]
            else:
                lic = proj.get("license") or {}
                lic_id = lic.get("id") if isinstance(lic, dict) else None
                if lic_id:
                    item["license_id"] = lic_id

        # Source-updated timestamp: manifest > API `updated`.
        if not item.get("source_updated_at"):
            updated = proj.get("updated")
            if updated:
                item["source_updated_at"] = updated

        # Upstream categories — captured as a *fallback only*. Linked later
        # (in insert_registry_item) exclusively when the manifest sets neither
        # base_categories nor community_categories. Loader-ish category names
        # are filtered out since loaders live in compatible_versions_json.
        if not item.get("_hydrated_categories"):
            raw_cats = proj.get("categories", []) or []
            cats = [
                c for c in raw_cats
                if isinstance(c, str) and c and c not in _MODRINTH_LOADER_CATEGORY_NAMES
            ]
            if cats:
                item["_hydrated_categories"] = cats


def insert_pack_mods(conn: sqlite3.Connection, pack_id: str, mods: list[dict[str, Any]]) -> None:
    """Insert pack membership rows into pack_mods."""
    cursor = conn.cursor()
    for entry in mods:
        cursor.execute(
            """
            INSERT INTO pack_mods (pack_id, mod_id, source, version, status, description)
            VALUES (?, ?, ?, ?, ?, ?)
            ON CONFLICT(pack_id, mod_id) DO UPDATE SET
                source=excluded.source,
                version=excluded.version,
                status=excluded.status,
                description=excluded.description
            """,
            (
                pack_id,
                entry["id"],
                entry.get("source", "manifest"),
                entry.get("version"),
                entry.get("status", "required"),
                entry.get("description"),
            ),
        )


# ---------------------------------------------------------------------------
# Crash signatures
# ---------------------------------------------------------------------------


def insert_crash_signature(
    conn: sqlite3.Connection,
    sig: dict[str, Any],
    rejected: list[int],
) -> None:
    """Insert a crash signature after validating its regex pattern.

    Validation per §2.4.1:
    - Reject patterns longer than 256 characters.
    - Test each pattern against a 100 KB corpus with a 1 s timeout to catch
      catastrophic backtracking.

    *rejected* is a one-element list used as a mutable counter so the caller
    can track how many signatures were skipped.
    """
    name = sig.get("name", sig["id"])
    raw_pattern = sig.get("regex_pattern", "")

    # --- Length check ---
    if len(raw_pattern) > 256:
        logger.error("Crash signature '%s' rejected: pattern exceeds 256 characters (%d)", name, len(raw_pattern))
        rejected[0] += 1
        return

    # --- Compile ---
    try:
        compiled = re.compile(raw_pattern)
    except re.error as exc:
        logger.error("Crash signature '%s' rejected: invalid regex (%s)", name, exc)
        rejected[0] += 1
        return

    # --- Catastrophic backtracking test ---
    if not _test_regex_timeout(compiled):
        logger.error("Crash signature '%s' rejected: regex timed out on 100 KB corpus (possible catastrophic backtracking)", name)
        rejected[0] += 1
        return

    # --- Insert ---
    cursor = conn.cursor()
    action_button = sig.get("action_button")
    cursor.execute(
        """
        INSERT INTO crash_signatures (id, name, regex_pattern, solution_markdown, action_button_json)
        VALUES (?, ?, ?, ?, ?)
        """,
        (
            sig["id"],
            sig["name"],
            sig["regex_pattern"],
            sig["solution_markdown"],
            json.dumps(action_button, separators=(",", ":")) if action_button else None,
        ),
    )


# ---------------------------------------------------------------------------
# Loader manifest (system config)
# ---------------------------------------------------------------------------


def load_loader_manifests(conn: sqlite3.Connection) -> None:
    """Ingest loader manifests into system_config.

    Prefers the richer loader_manifests.json schema when available and falls
    back to the legacy known_good_hashes.json for compatibility.
    """
    manifest_path = LOADER_MANIFESTS_DIR / "loader_manifests.json"
    fallback_path = LOADER_MANIFESTS_DIR / "known_good_hashes.json"
    if manifest_path.exists():
        selected_path = manifest_path
    elif fallback_path.exists():
        selected_path = fallback_path
    else:
        logger.warning("Loader manifest not found: %s", manifest_path)
        return

    with selected_path.open("r", encoding="utf-8") as fh:
        data = json.load(fh)

    cursor = conn.cursor()
    cursor.execute(
        "INSERT OR REPLACE INTO system_config (key, value_json) VALUES (?, ?)",
        ("loader_manifests", json.dumps(data, separators=(",", ":"))),
    )


# ---------------------------------------------------------------------------
# Signing helpers
# ---------------------------------------------------------------------------


def get_signing_key() -> "nacl.signing.SigningKey | None":
    """Return a SigningKey from a real Ed25519 seed in the environment."""
    if nacl is None:
        logger.error("PyNaCl is not installed; cannot sign the database.")
        return None

    hex_key = os.environ.get("ED25519_PRIVATE_KEY")
    if not hex_key:
        logger.error("ED25519_PRIVATE_KEY is not set; cannot sign the database.")
        return None

    try:
        seed = bytes.fromhex(hex_key)
    except ValueError:
        logger.error("ED25519_PRIVATE_KEY is not valid hex.")
        return None

    if len(seed) == 32:
        return nacl.signing.SigningKey(seed)
    if len(seed) == 64:
        # Ed25519 private keys are commonly stored as 64 bytes (seed || public).
        return nacl.signing.SigningKey(seed[:32])

    logger.error("ED25519_PRIVATE_KEY has unexpected length %d; expected 32 or 64 bytes.", len(seed))
    return None


def sign_database(db_bytes: bytes) -> bytes:
    """Sign database bytes and return the raw signature."""
    key = get_signing_key()
    if key is None or nacl is None:
        raise RuntimeError("no signing key available")
    return key.sign(db_bytes, encoder=nacl.encoding.RawEncoder)[:64]


PLACEHOLDER_SIGNATURE = (
    "# TODO: replace with actual Ed25519 signature once a signing key is configured\n"
)


# ---------------------------------------------------------------------------
# Main compile routine
# ---------------------------------------------------------------------------


def compile_registry(output_path: Path, skip_sign: bool) -> None:
    """Build the SQLite registry database at *output_path*."""
    logger.info("Starting nightly compile")

    conn = sqlite3.connect(":memory:")
    conn.execute("PRAGMA foreign_keys = ON")
    create_tables(conn)

    # Schema version.
    cursor = conn.cursor()
    cursor.execute("INSERT INTO schema_version (version) VALUES (?)", (SCHEMA_VERSION,))

    # Content-type directories: each subdirectory contains JSON manifests with
    # the same shape as mod manifests (id, name, content_type, download_strategy,
    # source_identifier, sha256, etc.).  The manifest's own content_type field
    # determines the row type — we do NOT derive it from the directory name.
    CONTENT_DIRS = [
        "mods",
        "packs",
        "shaders",
        "resourcepacks",
        "servers",
        "datapacks",
        "worlds",
    ]

    # Collect all items first (for Modrinth hydration), then insert.
    all_items: list[tuple[Path, dict[str, Any]]] = []
    for dir_name in CONTENT_DIRS:
        all_items.extend(load_json_files(REGISTRY_DIR / dir_name))

    # Hydrate Modrinth metadata (description, body, icon, gallery, page URL,
    # license, updated) for modrinth_id items (in-place, with override precedence).
    _hydrate_modrinth_metadata([item for _, item in all_items])

    # Insert items, handling pack-specific fields.
    mod_count = 0
    pack_count = 0
    other_count = 0
    for path, data in all_items:
        content_type = data.get("content_type", "mod")
        is_pack = content_type == "pack" or "pack_id" in data
        if is_pack:
            pack = dict(data)
            if "pack_id" in pack and "id" not in pack:
                pack["id"] = pack["pack_id"]
            pack.setdefault("content_type", "pack")
            pack.setdefault("download_strategy", "curated_pack")
            pack.setdefault("source_identifier", pack["id"])
            pack.setdefault("base_categories", [])
            pack.setdefault("community_categories", [])
            pack.setdefault("icon_url", None)
            pack.setdefault("gallery_urls", [])
            insert_registry_item(conn, pack, path)
            insert_pack_mods(conn, pack["id"], pack.get("mods", []))
            pack_count += 1
        else:
            insert_registry_item(conn, data, path)
            if content_type == "mod":
                mod_count += 1
            else:
                other_count += 1
    logger.info("Inserted %d mod(s), %d pack(s), %d other item(s)", mod_count, pack_count, other_count)

    # Crash signatures.
    sig_count = 0
    rejected: list[int] = [0]
    for path, data in load_json_files(CRASH_SIGNATURES_DIR):
        insert_crash_signature(conn, data, rejected)
        sig_count += 1
    logger.info("Inserted %d crash signature(s), rejected %d", sig_count, rejected[0])

    # Loader manifests domain/hash allowlist.
    load_loader_manifests(conn)

    # Persist in-memory database to disk.
    conn.commit()
    output_path.parent.mkdir(parents=True, exist_ok=True)
    disk_conn = sqlite3.connect(str(output_path))
    conn.backup(disk_conn)
    disk_conn.close()
    conn.close()

    logger.info("Wrote database to %s", output_path)

    # --- Audit log (§4.6) ---
    audit_log_path = REGISTRY_DIR / "governance" / "audit_log.json"
    audit_log_path.parent.mkdir(parents=True, exist_ok=True)
    total_items = mod_count + pack_count + other_count
    total_crash_sigs = sig_count
    new_entry = {
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "action": "compile",
        "details": f"Compiled registry with {total_items} items, {total_crash_sigs} crash signatures",
    }
    if audit_log_path.exists():
        with audit_log_path.open("r", encoding="utf-8") as fh:
            audit_data = json.load(fh)
    else:
        audit_data = {"entries": []}
    audit_data["entries"].append(new_entry)
    # Rotation: keep last 1000 entries.
    if len(audit_data["entries"]) > 1000:
        audit_data["entries"] = audit_data["entries"][-1000:]
    with audit_log_path.open("w", encoding="utf-8") as fh:
        json.dump(audit_data, fh, indent=2)
    logger.info("Wrote audit log to %s", audit_log_path)

    # Register audit log path in system_config.
    audit_conn = sqlite3.connect(str(output_path))
    audit_conn.execute(
        "INSERT OR REPLACE INTO system_config (key, value_json) VALUES ('audit_log_json', ?)",
        ("registry/governance/audit_log.json",),
    )
    audit_conn.commit()
    audit_conn.close()

    # Sign and write signature.
    sig_path = output_path.with_suffix(output_path.suffix + ".sig")
    with output_path.open("rb") as fh:
        db_bytes = fh.read()

    if skip_sign:
        sig_path.write_text(PLACEHOLDER_SIGNATURE, encoding="utf-8")
        logger.warning("--skip-sign was used; wrote placeholder signature to %s", sig_path)
    else:
        try:
            raw_sig = sign_database(db_bytes)
            sig_path.write_bytes(raw_sig)
            logger.info("Wrote signature to %s", sig_path)
        except RuntimeError as exc:
            logger.error("Signing failed: %s", exc)
            sys.exit(1)

    logger.info("Compile complete")


# ---------------------------------------------------------------------------
# Entrypoint
# ---------------------------------------------------------------------------


def main() -> None:
    parser = argparse.ArgumentParser(description="Compile the curated mod registry database.")
    parser.add_argument(
        "--out",
        type=Path,
        default=REPO_ROOT / "registry.db",
        help="Path for the compiled registry database (default: registry.db)",
    )
    parser.add_argument(
        "--skip-sign",
        action="store_true",
        help="Write a placeholder signature and exit 0 (intended for local dev/test only)",
    )
    args = parser.parse_args()
    compile_registry(args.out, skip_sign=args.skip_sign)


if __name__ == "__main__":
    main()
