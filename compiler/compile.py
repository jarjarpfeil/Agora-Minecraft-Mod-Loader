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
import re
import sqlite3
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

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

SCHEMA_VERSION = 1


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
            compatible_versions_json TEXT
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


def validate_sha256(raw: Any) -> None:
    """Ensure *raw* is either null/empty or exactly 64 hex characters."""
    if raw is None or raw == "":
        return
    if not isinstance(raw, str):
        logger.error("sha256 must be a string or null, got %s", type(raw).__name__)
        raise SystemExit(1)
    if not re.fullmatch(r"[0-9a-fA-F]{64}", raw):
        logger.error("sha256 must be exactly 64 hex characters: %s", raw)
        raise SystemExit(1)


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


def manifest_mtime(path: Path) -> str:
    """Return the manifest file's modification time as an ISO-8601 UTC string."""
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

    sha256 = item.get("sha256") or ""
    validate_sha256(sha256)
    gallery = item.get("gallery_urls", [])
    compatible_versions = item.get("compatible_versions") or default_compatible_versions(item)

    cursor.execute(
        """
        INSERT INTO registry_items (
            id, name, content_type, download_strategy, source_identifier, sha256,
            upvotes, downvotes, net_score, velocity, status,
            is_immune, immunity_reason, allow_comments, immunity_cooldown_until,
            icon_url, gallery_urls_json, date_added, compatible_versions_json
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
            manifest_mtime(path),
            json.dumps(compatible_versions, separators=(",", ":")),
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
    for category_id in item.get("base_categories", []):
        register_category(conn, category_id, is_community=False)
        link_item_category(conn, item_id, category_id)
    for category_id in item.get("community_categories", []):
        register_category(conn, category_id, is_community=True)
        link_item_category(conn, item_id, category_id)


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


def insert_crash_signature(conn: sqlite3.Connection, sig: dict[str, Any]) -> None:
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

    # Mods.
    mod_count = 0
    for path, data in load_json_files(REGISTRY_DIR / "mods"):
        insert_registry_item(conn, data, path)
        mod_count += 1
    logger.info("Inserted %d mod(s)", mod_count)

    # Packs.
    pack_count = 0
    for path, data in load_json_files(REGISTRY_DIR / "packs"):
        # Pack manifests use pack_id, but the registry_items table expects id.
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
    logger.info("Inserted %d pack(s)", pack_count)

    # Crash signatures.
    sig_count = 0
    for path, data in load_json_files(CRASH_SIGNATURES_DIR):
        insert_crash_signature(conn, data)
        sig_count += 1
    logger.info("Inserted %d crash signature(s)", sig_count)

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
