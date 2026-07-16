#!/usr/bin/env python3
"""Unit tests for pure functions in Agora utility scripts."""

import hashlib
import json
import os
import sys
import tempfile
import unittest
import urllib.error
from pathlib import Path
from unittest import mock

sys.path.insert(0, os.path.dirname(__file__))

import fetch_loader_manifests
import fetch_registry_db
import refresh_loader_manifests


class TestFetchBytes(unittest.TestCase):
    """Tests for network error normalization in _fetch_bytes."""

    @mock.patch("fetch_loader_manifests.urllib.request.urlopen")
    def test_read_timeout_is_normalized_to_url_error(self, urlopen):
        response = mock.MagicMock()
        response.__enter__.return_value.read.side_effect = TimeoutError("timed out")
        urlopen.return_value = response

        with self.assertRaises(urllib.error.URLError) as raised:
            fetch_loader_manifests._fetch_bytes("https://example.test/file.jar")

        self.assertIsInstance(raised.exception.__cause__, TimeoutError)

    @mock.patch("fetch_loader_manifests.urllib.request.urlopen")
    def test_url_error_is_not_wrapped_again(self, urlopen):
        original = urllib.error.URLError("not found")
        urlopen.side_effect = original

        with self.assertRaises(urllib.error.URLError) as raised:
            fetch_loader_manifests._fetch_bytes("https://example.test/file.jar")

        self.assertIs(raised.exception, original)

    @mock.patch("fetch_loader_manifests.urllib.request.urlopen")
    def test_custom_timeout_is_passed_to_urlopen(self, urlopen):
        response = mock.MagicMock()
        response.__enter__.return_value.read.return_value = b"data"
        urlopen.return_value = response

        fetch_loader_manifests._fetch_bytes(
            "https://example.test/file.jar", timeout=1
        )

        _, kwargs = urlopen.call_args
        self.assertEqual(kwargs["timeout"], 1)


class TestIsStandardRelease(unittest.TestCase):
    """Tests for refresh_loader_manifests._is_standard_release."""

    def test_stable_two_part(self):
        """Two-part numeric version like '1.21' is standard."""
        self.assertTrue(refresh_loader_manifests._is_standard_release("1.21"))

    def test_stable_three_part(self):
        """Three-part numeric version like '1.21.1' is standard."""
        self.assertTrue(refresh_loader_manifests._is_standard_release("1.21.1"))

    def test_stable_minor_patch(self):
        """Version '1.20.6' is standard."""
        self.assertTrue(refresh_loader_manifests._is_standard_release("1.20.6"))

    def test_snapshot_weekly(self):
        """Weekly snapshot format like '24w14a' is NOT standard (regex excludes it)."""
        self.assertFalse(refresh_loader_manifests._is_standard_release("24w14a"))

    def test_snapshot_26w(self):
        """26w-prefixed snapshot like '26w07a' is NOT standard."""
        self.assertFalse(refresh_loader_manifests._is_standard_release("26w07a"))

    def test_prerelease(self):
        """Prerelease like '1.21-pre1' is NOT standard (regex excludes suffixes)."""
        self.assertFalse(refresh_loader_manifests._is_standard_release("1.21-pre1"))

    def test_release_candidate(self):
        """Release candidate like '1.21-rc1' is NOT standard."""
        self.assertFalse(refresh_loader_manifests._is_standard_release("1.21-rc1"))

    def test_invalid(self):
        """Non-numeric / malformed version is not standard."""
        self.assertFalse(refresh_loader_manifests._is_standard_release("0.0.0-invalid"))

    def test_empty(self):
        """Empty string is not standard."""
        self.assertFalse(refresh_loader_manifests._is_standard_release(""))








class TestSha256Hex(unittest.TestCase):
    """Tests for fetch_loader_manifests._sha256_hex."""

    def test_known_hello(self):
        """SHA-256 of b'hello' matches the known constant."""
        expected = (
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        )
        self.assertEqual(
            fetch_loader_manifests._sha256_hex(b"hello"),
            expected,
        )

    def test_empty(self):
        """SHA-256 of empty bytes matches the known empty-hash constant."""
        expected = (
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        )
        self.assertEqual(fetch_loader_manifests._sha256_hex(b""), expected)

    def test_deterministic(self):
        """Same input always produces the same hash."""
        data = b"test data for determinism"
        self.assertEqual(
            fetch_loader_manifests._sha256_hex(data),
            fetch_loader_manifests._sha256_hex(data),
        )


class TestStableJsonSha256(unittest.TestCase):
    """Tests for fetch_loader_manifests._stable_json_sha256."""

    def test_canonicalizes_key_order(self):
        """Different key orderings in JSON produce the same hash when keys are sorted."""
        obj1 = {"b": 1, "a": 2}
        obj2 = {"a": 2, "b": 1}
        hash1 = fetch_loader_manifests._stable_json_sha256(
            json.dumps(obj1, separators=(",", ":")).encode()
        )
        hash2 = fetch_loader_manifests._stable_json_sha256(
            json.dumps(obj2, separators=(",", ":")).encode()
        )
        self.assertEqual(hash1, hash2)

    def test_drops_default_keys(self):
        """Default drop set removes 'time' and 'releaseTime' before hashing."""
        payload_with_time = json.dumps(
            {"keep": 1, "time": "2025-01-01T00:00:00Z", "releaseTime": "2025-01-02T00:00:00Z"}
        ).encode()
        payload_without_time = json.dumps(
            {"keep": 1}
        ).encode()
        self.assertEqual(
            fetch_loader_manifests._stable_json_sha256(payload_with_time),
            fetch_loader_manifests._stable_json_sha256(payload_without_time),
        )

    def test_custom_drop(self):
        """Custom drop set removes specified keys before hashing."""
        payload_with_ignore = json.dumps(
            {"keep": 1, "ignore_me": "should not matter"}
        ).encode()
        payload_without_ignore = json.dumps(
            {"keep": 1}
        ).encode()
        self.assertEqual(
            fetch_loader_manifests._stable_json_sha256(
                payload_with_ignore, drop={"ignore_me"}
            ),
            fetch_loader_manifests._stable_json_sha256(payload_without_ignore),
        )

    def test_different_content_different_hash(self):
        """Different JSON content produces different hashes."""
        hash1 = fetch_loader_manifests._stable_json_sha256(b'{"a":1}')
        hash2 = fetch_loader_manifests._stable_json_sha256(b'{"a":2}')
        self.assertNotEqual(hash1, hash2)


class TestSha256File(unittest.TestCase):
    """Tests for fetch_registry_db.sha256_file."""

    def test_known_content(self):
        """SHA-256 of a temp file with known content matches expected hash."""
        expected = (
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        )
        with tempfile.NamedTemporaryFile(delete=False) as tmp:
            tmp.write(b"hello")
            tmp_path = tmp.name
        try:
            self.assertEqual(fetch_registry_db.sha256_file(Path(tmp_path)), expected)
        finally:
            os.unlink(tmp_path)

    def test_empty_file(self):
        """SHA-256 of an empty file matches the known empty-hash constant."""
        expected = (
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        )
        with tempfile.NamedTemporaryFile(delete=False) as tmp:
            tmp_path = tmp.name
        try:
            self.assertEqual(fetch_registry_db.sha256_file(Path(tmp_path)), expected)
        finally:
            os.unlink(tmp_path)


class TestVerifySha256AgainstDigest(unittest.TestCase):
    """Tests for fetch_registry_db.verify_sha256_against_digest."""

    def test_no_digest_skips(self):
        """When digest_field is None, no verification is performed (no exit)."""
        with tempfile.NamedTemporaryFile(delete=False) as tmp:
            tmp.write(b"hello")
            tmp_path = tmp.name
        try:
            # Should not raise or call sys.exit
            fetch_registry_db.verify_sha256_against_digest(Path(tmp_path), None)
        except SystemExit:
            self.fail("verify_sha256_against_digest called sys.exit with no digest")
        finally:
            os.unlink(tmp_path)

    def test_hex_digest_matches(self):
        """When digest is a hex string matching the file's SHA-256, no exit occurs."""
        expected = hashlib.sha256(b"hello").hexdigest()
        digest_field = f"sha256:{expected}"
        with tempfile.NamedTemporaryFile(delete=False) as tmp:
            tmp.write(b"hello")
            tmp_path = tmp.name
        try:
            fetch_registry_db.verify_sha256_against_digest(Path(tmp_path), digest_field)
        except SystemExit:
            self.fail("verify_sha256_against_digest called sys.exit on matching digest")
        finally:
            os.unlink(tmp_path)











# ═══════════════════════════════════════════════════════════════════════════
# Runtime catalog tests
# ═══════════════════════════════════════════════════════════════════════════

import generate_runtime_catalog as grc


class TestRuntimeCatalogSchema(unittest.TestCase):
    """Tests for runtime catalog schema validation."""

    CATALOG_PATH = Path(__file__).resolve().parent.parent / "runtime-catalog" / "runtime_catalog.json"

    @classmethod
    def setUpClass(cls):
        with cls.CATALOG_PATH.open("r", encoding="utf-8") as fh:
            cls.catalog = json.load(fh)

    def test_schema_version(self):
        """schema_version must be 1."""
        self.assertEqual(self.catalog["schema_version"], 1)

    def test_generated_at_present(self):
        """generated_at is a non-empty string."""
        self.assertIsInstance(self.catalog.get("generated_at"), str)
        self.assertTrue(self.catalog["generated_at"])

    def test_source_present(self):
        """source metadata is present."""
        self.assertIsInstance(self.catalog.get("source"), str)
        self.assertTrue(self.catalog["source"])

    def test_entries_is_list(self):
        """entries must be a non-empty list."""
        entries = self.catalog.get("entries")
        self.assertIsInstance(entries, list)
        self.assertGreater(len(entries), 0)

    def test_check_catalog_no_errors(self):
        """Built-in validation passes on the file."""
        errors = grc.check_catalog(self.catalog)
        self.assertEqual(errors, [], f"check_catalog returned errors: {errors}")


class TestRuntimeCatalogEntryFields(unittest.TestCase):
    """Tests for field presence and types in each entry."""

    @classmethod
    def setUpClass(cls):
        with TestRuntimeCatalogSchema.CATALOG_PATH.open("r", encoding="utf-8") as fh:
            cls.catalog = json.load(fh)
        cls.entries = cls.catalog["entries"]

    def test_all_required_fields_present(self):
        """Every entry has all required string and int fields."""
        required_str = [
            "vendor", "full_version", "openjdk_version",
            "os", "arch", "image_type", "jvm_impl",
            "archive_type", "url", "sha256", "java_relative_path",
            "license", "source_api_url",
        ]
        required_int = ["major", "size"]
        for i, entry in enumerate(self.entries):
            for field in required_str:
                self.assertIsInstance(
                    entry.get(field), str,
                    f"entry[{i}].{field} is not a string",
                )
                self.assertTrue(
                    entry[field],
                    f"entry[{i}].{field} is empty",
                )
            for field in required_int:
                self.assertIsInstance(
                    entry.get(field), int,
                    f"entry[{i}].{field} is not an int",
                )
                self.assertGreater(
                    entry[field], 0,
                    f"entry[{i}].{field} is not positive",
                )

    def test_vendor_is_eclipse_temurin(self):
        """Every entry's vendor is 'eclipse-temurin'."""
        for entry in self.entries:
            self.assertEqual(entry["vendor"], "eclipse-temurin")

    def test_license_spdx(self):
        """Every entry has the correct SPDX license."""
        expected = "GPL-2.0-only WITH Classpath-exception-2.0"
        for entry in self.entries:
            self.assertEqual(entry["license"], expected)

    def test_image_type_is_jre_or_jdk(self):
        """Every entry has image_type 'jre' or 'jdk' (JRE fallback to JDK when no JRE published)."""
        for entry in self.entries:
            self.assertIn(entry["image_type"], ("jre", "jdk"),
                          f"Unexpected image_type {entry.get('image_type')} for {entry.get('url')}")

    def test_jvm_impl_is_hotspot(self):
        """Every entry has jvm_impl 'hotspot'."""
        for entry in self.entries:
            self.assertEqual(entry["jvm_impl"], "hotspot")

    def test_sha256_lowercase_hex(self):
        """Every sha256 is 64 lowercase hex chars."""
        for entry in self.entries:
            sha = entry["sha256"]
            self.assertRegex(sha, r"^[0-9a-f]{64}$",
                             f"Invalid SHA-256 for {entry.get('url')}")

    def test_url_https(self):
        """All URLs start with https://."""
        for entry in self.entries:
            self.assertTrue(
                entry["url"].startswith("https://"),
                f"URL not HTTPS: {entry['url']}",
            )

    def test_url_adoptium_github_release(self):
        """All URLs match official Adoptium GitHub release pattern."""
        pattern = grc.ADOPTIUM_GITHUB_RELEASE_RE
        for entry in self.entries:
            self.assertRegex(
                entry["url"], pattern,
                f"URL not official Adoptium GitHub: {entry['url']}",
            )

    def test_os_in_known_set(self):
        """os is one of windows, linux, macos."""
        for entry in self.entries:
            self.assertIn(entry["os"], ["windows", "linux", "macos"])

    def test_arch_in_known_set(self):
        """arch is one of x64, aarch64."""
        for entry in self.entries:
            self.assertIn(entry["arch"], ["x64", "aarch64"])

    def test_windows_entries_use_zip(self):
        """Windows entries use archive_type 'zip'."""
        for entry in self.entries:
            if entry["os"] == "windows":
                self.assertEqual(entry["archive_type"], "zip")

    def test_linux_macos_entries_use_tar_gz(self):
        """Linux and macOS entries use archive_type 'tar.gz'."""
        for entry in self.entries:
            if entry["os"] in ("linux", "macos"):
                self.assertEqual(entry["archive_type"], "tar.gz")

    def test_java_relative_path_correct_per_os(self):
        """java_relative_path matches expected per-OS value."""
        for entry in self.entries:
            os_name = entry["os"]
            jrp = entry["java_relative_path"]
            if os_name == "windows":
                self.assertEqual(jrp, "bin/java.exe")
            elif os_name == "macos":
                self.assertEqual(jrp, "Contents/Home/bin/java")
            else:
                self.assertEqual(jrp, "bin/java")

    def test_size_reasonable(self):
        """Size must be at least 10 MB (10,000,000 bytes)."""
        for entry in self.entries:
            self.assertGreaterEqual(
                entry["size"], 10_000_000,
                f"Suspiciously small size for {entry['url']}",
            )

    def test_major_in_requested_set(self):
        """major is in the set of requested majors."""
        for entry in self.entries:
            self.assertIn(entry["major"], grc.REQUESTED_MAJORS)


class TestRuntimeCatalogDuplicates(unittest.TestCase):
    """Tests for duplicate (major, os, arch) tuple rejection."""

    @classmethod
    def setUpClass(cls):
        with TestRuntimeCatalogSchema.CATALOG_PATH.open("r", encoding="utf-8") as fh:
            cls.entries = json.load(fh)["entries"]

    def test_no_duplicate_tuples(self):
        """No two entries share the same (major, os, arch) tuple."""
        seen: set[tuple[int, str, str]] = set()
        for entry in self.entries:
            key = (entry["major"], entry["os"], entry["arch"])
            self.assertNotIn(
                key, seen,
                f"Duplicate tuple major={key[0]} os={key[1]} arch={key[2]}",
            )
            seen.add(key)

    def test_check_catalog_detects_duplicates(self):
        """check_catalog should flag duplicate tuples."""
        bad = {
            "schema_version": 1,
            "generated_at": "2025-01-01T00:00:00Z",
            "source": "test",
            "entries": [
                {
                    "vendor": "eclipse-temurin", "major": 21,
                    "full_version": "21.0.0+1", "openjdk_version": "21.0.0+1",
                    "os": "linux", "arch": "x64",
                    "image_type": "jre", "jvm_impl": "hotspot",
                    "archive_type": "tar.gz",
                    "url": "https://github.com/adoptium/temurin21-binaries/releases/download/test/OpenJDK21U-jre_x64_linux_hotspot_21.0.0_1.tar.gz",
                    "sha256": "a" * 64,
                    "size": 50000000,
                    "java_relative_path": "bin/java",
                    "license": "GPL-2.0-only WITH Classpath-exception-2.0",
                    "source_api_url": "https://api.adoptium.net/v3/assets/latest/21/hotspot",
                },
                {
                    "vendor": "eclipse-temurin", "major": 21,
                    "full_version": "21.0.0+2", "openjdk_version": "21.0.0+2",
                    "os": "linux", "arch": "x64",
                    "image_type": "jre", "jvm_impl": "hotspot",
                    "archive_type": "tar.gz",
                    "url": "https://github.com/adoptium/temurin21-binaries/releases/download/test/OpenJDK21U-jre_x64_linux_hotspot_21.0.0_2.tar.gz",
                    "sha256": "b" * 64,
                    "size": 50000001,
                    "java_relative_path": "bin/java",
                    "license": "GPL-2.0-only WITH Classpath-exception-2.0",
                    "source_api_url": "https://api.adoptium.net/v3/assets/latest/21/hotspot",
                },
            ],
        }
        errors = grc.check_catalog(bad)
        dup_errors = [e for e in errors if "duplicate" in e.lower()]
        self.assertGreater(len(dup_errors), 0)


class TestRuntimeCatalogDeterministicSort(unittest.TestCase):
    """Tests for deterministic ordering of catalog entries."""

    @classmethod
    def setUpClass(cls):
        with TestRuntimeCatalogSchema.CATALOG_PATH.open("r", encoding="utf-8") as fh:
            cls.entries = json.load(fh)["entries"]

    def test_sorted_by_major_then_os_then_arch(self):
        """Entries are sorted by major ASC, os ASC, arch ASC."""
        os_order = {"linux": 0, "macos": 1, "windows": 2}
        arch_order = {"aarch64": 0, "x64": 1}
        for i in range(len(self.entries) - 1):
            a = self.entries[i]
            b = self.entries[i + 1]
            key_a = (a["major"], os_order.get(a["os"], 99), arch_order.get(a["arch"], 99))
            key_b = (b["major"], os_order.get(b["os"], 99), arch_order.get(b["arch"], 99))
            self.assertLessEqual(
                key_a, key_b,
                f"Entries not sorted: {a['major']}/{a['os']}/{a['arch']} "
                f"before {b['major']}/{b['os']}/{b['arch']}",
            )

    def test_deterministic_json_output(self):
        """Re-serializing the same entries produces identical JSON."""
        entries_json = json.dumps(self.entries, sort_keys=True)
        entries2 = json.loads(entries_json)
        entries2_json = json.dumps(entries2, sort_keys=True)
        self.assertEqual(entries_json, entries2_json)


class TestRuntimeCatalogParserFixture(unittest.TestCase):
    """Tests for the response parser with a mock API response."""

    def test_parse_valid_response(self):
        """A well-formed API response produces a valid entry."""
        fixture = {
            "binary": {
                "architecture": "x64",
                "image_type": "jre",
                "jvm_impl": "hotspot",
                "os": "linux",
                "package": {
                    "checksum": "e5038aae3ca9ff670bc696496b0728dbd23d280026bad30291cb919221ecfdcb",
                    "link": "https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.11%2B10/OpenJDK21U-jre_x64_linux_hotspot_21.0.11_10.tar.gz",
                    "name": "OpenJDK21U-jre_x64_linux_hotspot_21.0.11_10.tar.gz",
                    "size": 52099793,
                },
            },
            "vendor": "eclipse",
            "version": {
                "major": 21,
                "minor": 0,
                "security": 11,
                "openjdk_version": "21.0.11+10-LTS",
                "semver": "21.0.11+10.0.LTS",
            },
        }
        result = grc._validate_and_extract(fixture, 21, "linux", "x64")
        self.assertIsInstance(result, dict)
        self.assertEqual(result["major"], 21)
        self.assertEqual(result["os"], "linux")
        self.assertEqual(result["arch"], "x64")
        self.assertEqual(result["sha256"], "e5038aae3ca9ff670bc696496b0728dbd23d280026bad30291cb919221ecfdcb")
        self.assertEqual(result["size"], 52099793)

    def test_parse_os_mismatch(self):
        """Mismatched OS returns an error string."""
        fixture = {
            "binary": {
                "architecture": "x64",
                "image_type": "jre",
                "jvm_impl": "hotspot",
                "os": "windows",
                "package": {
                    "checksum": "a" * 64,
                    "link": "https://github.com/adoptium/temurin21-binaries/releases/download/test/pkg.tar.gz",
                    "name": "pkg.tar.gz",
                    "size": 50000000,
                },
            },
            "vendor": "eclipse",
            "version": {"major": 21, "minor": 0, "security": 0, "openjdk_version": "21", "semver": "21"},
        }
        result = grc._validate_and_extract(fixture, 21, "linux", "x64")
        self.assertIsInstance(result, str)
        self.assertIn("os mismatch", result)

    def test_parse_missing_package_returns_error(self):
        """Missing package data returns an error."""
        fixture = {
            "binary": {
                "architecture": "x64",
                "image_type": "jre",
                "jvm_impl": "hotspot",
                "os": "linux",
            },
            "vendor": "eclipse",
            "version": {"major": 21, "minor": 0, "security": 0, "openjdk_version": "21", "semver": "21"},
        }
        result = grc._validate_and_extract(fixture, 21, "linux", "x64")
        self.assertIsInstance(result, str)
        self.assertIn("package", result.lower())

    def test_parse_bad_checksum_returns_error(self):
        """Non-hex checksum returns an error."""
        fixture = {
            "binary": {
                "architecture": "x64",
                "image_type": "jre",
                "jvm_impl": "hotspot",
                "os": "linux",
                "package": {
                    "checksum": "not-a-valid-sha",
                    "link": "https://github.com/adoptium/temurin21-binaries/releases/download/test/pkg.tar.gz",
                    "name": "pkg.tar.gz",
                    "size": 50000000,
                },
            },
            "vendor": "eclipse",
            "version": {"major": 21, "minor": 0, "security": 0, "openjdk_version": "21", "semver": "21"},
        }
        result = grc._validate_and_extract(fixture, 21, "linux", "x64")
        self.assertIsInstance(result, str)
        self.assertIn("checksum", result.lower())


class TestRuntimeCatalogUnavailableMatrix(unittest.TestCase):
    """Tests for handling of unavailable OS/arch combinations."""

    def test_windows_aarch64_in_unavailable(self):
        """windows + aarch64 is marked unavailable for all requested majors."""
        for major in grc.REQUESTED_MAJORS:
            self.assertIn(
                (major, "windows", "aarch64"),
                grc.UNAVAILABLE_COMBOS,
            )

    def test_warnings_in_catalog(self):
        """The catalog has warnings for unavailable combinations."""
        with TestRuntimeCatalogSchema.CATALOG_PATH.open("r", encoding="utf-8") as fh:
            catalog = json.load(fh)
        warnings = catalog.get("warnings", [])
        self.assertGreater(len(warnings), 0)
        has_unavailable = any("unavailable" in w for w in warnings)
        self.assertTrue(has_unavailable)


class TestRuntimeCatalogMaliciousHost(unittest.TestCase):
    """Tests for rejection of non-Adoptium GitHub URLs."""

    def test_malicious_host_rejected(self):
        """check_catalog rejects non-Adoptium GitHub URLs."""
        bad = {
            "schema_version": 1,
            "generated_at": "2025-01-01T00:00:00Z",
            "source": "test",
            "entries": [
                {
                    "vendor": "eclipse-temurin", "major": 21,
                    "full_version": "21.0.0+1", "openjdk_version": "21.0.0+1",
                    "os": "linux", "arch": "x64",
                    "image_type": "jre", "jvm_impl": "hotspot",
                    "archive_type": "tar.gz",
                    "url": "https://malicious.example.com/backdoor.tar.gz",
                    "sha256": "a" * 64,
                    "size": 50000000,
                    "java_relative_path": "bin/java",
                    "license": "GPL-2.0-only WITH Classpath-exception-2.0",
                    "source_api_url": "https://api.adoptium.net/v3/assets/latest/21/hotspot",
                },
            ],
        }
        errors = grc.check_catalog(bad)
        url_errors = [e for e in errors if "not official" in e]
        self.assertGreater(len(url_errors), 0)

    def test_http_url_rejected(self):
        """check_catalog rejects non-HTTPS URLs."""
        bad = {
            "schema_version": 1,
            "generated_at": "2025-01-01T00:00:00Z",
            "source": "test",
            "entries": [
                {
                    "vendor": "eclipse-temurin", "major": 21,
                    "full_version": "21.0.0+1", "openjdk_version": "21.0.0+1",
                    "os": "linux", "arch": "x64",
                    "image_type": "jre", "jvm_impl": "hotspot",
                    "archive_type": "tar.gz",
                    "url": "http://github.com/adoptium/temurin21-binaries/releases/download/test/pkg.tar.gz",
                    "sha256": "a" * 64,
                    "size": 50000000,
                    "java_relative_path": "bin/java",
                    "license": "GPL-2.0-only WITH Classpath-exception-2.0",
                    "source_api_url": "https://api.adoptium.net/v3/assets/latest/21/hotspot",
                },
            ],
        }
        errors = grc.check_catalog(bad)
        http_errors = [e for e in errors if "not HTTPS" in e]
        self.assertGreater(len(http_errors), 0)

    def test_wrong_github_repo_rejected(self):
        """A GitHub release URL from a non-Adoptium repo is rejected."""
        bad = {
            "schema_version": 1,
            "generated_at": "2025-01-01T00:00:00Z",
            "source": "test",
            "entries": [
                {
                    "vendor": "eclipse-temurin", "major": 21,
                    "full_version": "21.0.0+1", "openjdk_version": "21.0.0+1",
                    "os": "linux", "arch": "x64",
                    "image_type": "jre", "jvm_impl": "hotspot",
                    "archive_type": "tar.gz",
                    "url": "https://github.com/evil-corp/malware/releases/download/v1/pkg.tar.gz",
                    "sha256": "a" * 64,
                    "size": 50000000,
                    "java_relative_path": "bin/java",
                    "license": "GPL-2.0-only WITH Classpath-exception-2.0",
                    "source_api_url": "https://api.adoptium.net/v3/assets/latest/21/hotspot",
                },
            ],
        }
        errors = grc.check_catalog(bad)
        url_errors = [e for e in errors if "URL not official" in e]
        self.assertGreater(len(url_errors), 0)


if __name__ == "__main__":
    unittest.main()
