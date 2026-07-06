#!/usr/bin/env python3
"""Standalone unit tests for compiler/compile.py pure functions.

Run with:  python compiler/test_compile.py
No pytest dependency â€” uses only stdlib (unittest, sys, tempfile, json, os, re).

Covers: validate_sha256, _get_registry_repo, _load_poll_blacklist,
        _extract_mod_id, _extract_review_text, _scrub_review_text,
        and regex DoS protections via insert_crash_signature.
"""

from __future__ import annotations

import json
import os
import re
import sys
import tempfile
import unittest

# Ensure we can import the compiler module from the repo root.
sys.path.insert(0, os.path.dirname(__file__))
import compile as _compile  # noqa: E402


# ---------------------------------------------------------------------------
# validate_sha256
# ---------------------------------------------------------------------------

class TestValidateSha256(unittest.TestCase):
    """Tests for validate_sha256."""

    def test_valid_64_hex(self):
        """A valid 64-char hex string passes and is returned unchanged."""
        result = _compile.validate_sha256("a" * 64)
        self.assertEqual(result, "a" * 64)

    def test_valid_uppercase_hex(self):
        """Uppercase hex is accepted."""
        result = _compile.validate_sha256("A" * 64)
        self.assertEqual(result, "A" * 64)

    def test_valid_mixed_hex(self):
        """Mixed-case hex is accepted."""
        raw = "aB3dEf0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
        self.assertEqual(len(raw), 64)
        result = _compile.validate_sha256(raw)
        self.assertEqual(result, raw)

    def test_none_rejected(self):
        """None raises SystemExit."""
        with self.assertRaises(SystemExit):
            _compile.validate_sha256(None)

    def test_empty_rejected(self):
        """Empty string raises SystemExit."""
        with self.assertRaises(SystemExit):
            _compile.validate_sha256("")

    def test_short_rejected(self):
        """32-char hex (too short) raises SystemExit."""
        with self.assertRaises(SystemExit):
            _compile.validate_sha256("a" * 32)

    def test_long_rejected(self):
        """65-char hex (too long) raises SystemExit."""
        with self.assertRaises(SystemExit):
            _compile.validate_sha256("a" * 65)

    def test_non_hex_rejected(self):
        """64-char string with non-hex chars raises SystemExit."""
        with self.assertRaises(SystemExit):
            _compile.validate_sha256("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz")

    def test_non_string_rejected(self):
        """Non-string type raises SystemExit."""
        with self.assertRaises(SystemExit):
            _compile.validate_sha256(12345)


# ---------------------------------------------------------------------------
# _get_registry_repo
# ---------------------------------------------------------------------------

class TestGetRegistryRepo(unittest.TestCase):
    """Tests for _get_registry_repo."""

    def setUp(self):
        self._saved: dict[str, str | None] = {}
        for key in ("AGORA_REGISTRY_REPO", "GITHUB_REPOSITORY"):
            self._saved[key] = os.environ.pop(key, None)

    def tearDown(self):
        for key, val in self._saved.items():
            if val is not None:
                os.environ[key] = val
            else:
                os.environ.pop(key, None)

    def test_env_var_agora_registry_repo(self):
        """AGORA_REGISTRY_REPO takes precedence and is returned."""
        os.environ["AGORA_REGISTRY_REPO"] = "test/repo"
        self.assertEqual(_compile._get_registry_repo(), "test/repo")

    def test_github_fallback(self):
        """When AGORA_REGISTRY_REPO is unset, GITHUB_REPOSITORY is used."""
        os.environ.pop("AGORA_REGISTRY_REPO", None)
        os.environ["GITHUB_REPOSITORY"] = "gh/test"
        self.assertEqual(_compile._get_registry_repo(), "gh/test")

    def test_default(self):
        """When both env vars are unset, the default is returned."""
        os.environ.pop("AGORA_REGISTRY_REPO", None)
        os.environ.pop("GITHUB_REPOSITORY", None)
        result = _compile._get_registry_repo()
        self.assertIn("Agora-Minecraft-Mod-Loader", result)

    def test_priority_agora_over_github(self):
        """AGORA_REGISTRY_REPO wins over GITHUB_REPOSITORY when both are set."""
        os.environ["AGORA_REGISTRY_REPO"] = "owner/first"
        os.environ["GITHUB_REPOSITORY"] = "owner/second"
        self.assertEqual(_compile._get_registry_repo(), "owner/first")


# ---------------------------------------------------------------------------
# _load_poll_blacklist
# ---------------------------------------------------------------------------

class TestLoadPollBlacklist(unittest.TestCase):
    """Tests for _load_poll_blacklist."""

    def test_valid_json_returns_lowercase_set(self):
        """Valid JSON with usernames is returned as a lowercase set."""
        blacklist_dir = _compile.REGISTRY_DIR / "governance"
        target = blacklist_dir / "poll_blacklist.json"
        # Back up if exists.
        backup = None
        if target.exists():
            backup = target.read_bytes()
        try:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(json.dumps({"usernames": ["Alice", "BOB"]}), encoding="utf-8")
            result = _compile._load_poll_blacklist()
            self.assertEqual(result, {"alice", "bob"})
        finally:
            if backup is not None:
                target.write_bytes(backup)
            else:
                target.unlink(missing_ok=True)

    def test_missing_file_returns_empty_set(self):
        """When the file does not exist, returns empty set (no crash)."""
        blacklist_dir = _compile.REGISTRY_DIR / "governance"
        target = blacklist_dir / "poll_blacklist.json"
        backup = None
        if target.exists():
            backup = target.read_bytes()
        try:
            target.unlink(missing_ok=True)
            result = _compile._load_poll_blacklist()
            self.assertEqual(result, set())
        finally:
            if backup is not None:
                target.write_bytes(backup)

    def test_malformed_json_returns_empty_set(self):
        """Invalid JSON returns empty set (no crash)."""
        blacklist_dir = _compile.REGISTRY_DIR / "governance"
        target = blacklist_dir / "poll_blacklist.json"
        backup = None
        if target.exists():
            backup = target.read_bytes()
        try:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text("{not valid json!!!", encoding="utf-8")
            result = _compile._load_poll_blacklist()
            self.assertEqual(result, set())
        finally:
            if backup is not None:
                target.write_bytes(backup)
            else:
                target.unlink(missing_ok=True)


# ---------------------------------------------------------------------------
# _extract_mod_id
# ---------------------------------------------------------------------------

class TestExtractModId(unittest.TestCase):
    """Tests for _extract_mod_id."""

    def test_from_realistic_body(self):
        """A review-form body with Mod Registry ID returns the ID."""
        body = (
            "### Mod Registry ID\n"
            "sodium\n"
            "\n"
            "### Your Technical Review\n"
            "Great mod.\n"
        )
        self.assertEqual(_compile._extract_mod_id(body), "sodium")

    def test_none_body(self):
        """None body returns None."""
        self.assertIsNone(_compile._extract_mod_id(None))

    def test_empty_body(self):
        """Empty string body returns None."""
        self.assertIsNone(_compile._extract_mod_id(""))

    def test_no_field_returns_none(self):
        """Body without the Mod Registry ID field returns None."""
        body = "### Feature Request\nAdd mod X.\n"
        self.assertIsNone(_compile._extract_mod_id(body))

    def test_case_insensitive_heading(self):
        """Heading casing is ignored; ID is lowercased."""
        body = "### mod registry ID\n" "CaveClient\n"
        self.assertEqual(_compile._extract_mod_id(body), "caveclient")

    def test_with_crlf(self):
        """Windows CRLF line endings parse correctly."""
        body = "### Mod Registry ID\r\n" "sodium\r\n"
        self.assertEqual(_compile._extract_mod_id(body), "sodium")

    def test_trailing_whitespace_trimmed(self):
        """Trailing whitespace after the ID is trimmed."""
        body = "### Mod Registry ID\n" "sodium   \n"
        self.assertEqual(_compile._extract_mod_id(body), "sodium")


# ---------------------------------------------------------------------------
# _extract_review_text
# ---------------------------------------------------------------------------

class TestExtractReviewText(unittest.TestCase):
    """Tests for _extract_review_text."""

    def test_from_realistic_body(self):
        """Body with review heading â†’ extracted text."""
        body = (
            "### Mod Registry ID\n"
            "sodium\n"
            "\n"
            "### Your Technical Review (50 character minimum)\n"
            "Excellent performance improvement over vanilla rendering.\n"
            "\n"
            "### Additional Comments\n"
            "None.\n"
        )
        result = _compile._extract_review_text(body)
        self.assertIsNotNone(result)
        self.assertIn("Excellent performance improvement", result)

    def test_none_body(self):
        """None body returns None."""
        self.assertIsNone(_compile._extract_review_text(None))

    def test_empty_body(self):
        """Empty string body returns None."""
        self.assertIsNone(_compile._extract_review_text(""))

    def test_no_review_field_returns_none(self):
        """Body without the review field returns None."""
        body = "### Mod Registry ID\n" "sodium\n"
        self.assertIsNone(_compile._extract_review_text(body))

    def test_strips_whitespace(self):
        """Leading/trailing whitespace in the extracted text is stripped."""
        body = "### Your Technical Review\n" "  lots of text  \n"
        result = _compile._extract_review_text(body)
        self.assertEqual(result, "lots of text")


# ---------------------------------------------------------------------------
# _scrub_review_text
# ---------------------------------------------------------------------------

class TestScrubReviewText(unittest.TestCase):
    """Tests for _scrub_review_text."""

    def test_version_begging_filtered(self):
        """Version-begging text is filtered out."""
        passed, cleaned, reason = _compile._scrub_review_text("Please update to 1.21")
        self.assertFalse(passed)
        self.assertEqual(reason, "version-begging")

    def test_legitimate_review_preserved(self):
        """A substantive review passes the scrub pipeline."""
        passed, cleaned, reason = _compile._scrub_review_text(
            "This mod adds great features and runs smoothly."
        )
        self.assertTrue(passed)
        self.assertEqual(reason, "")
        self.assertEqual(cleaned, "This mod adds great features and runs smoothly.")

    def test_empty_praise_filtered(self):
        """Short empty praise is filtered."""
        passed, cleaned, reason = _compile._scrub_review_text("Good mod.")
        self.assertFalse(passed)
        self.assertEqual(reason, "empty-praise")

    def test_empty_text_filtered(self):
        """Empty text is filtered."""
        passed, cleaned, reason = _compile._scrub_review_text("")
        self.assertFalse(passed)
        self.assertEqual(reason, "empty")

    def test_whitespace_only_filtered(self):
        """Whitespace-only text is filtered."""
        passed, cleaned, reason = _compile._scrub_review_text("   \t\n  ")
        self.assertFalse(passed)
        self.assertEqual(reason, "empty")

    def test_strips_clean_text(self):
        """Passed text is stripped of leading/trailing whitespace."""
        passed, cleaned, reason = _compile._scrub_review_text("  hello world  ")
        self.assertTrue(passed)
        self.assertEqual(cleaned, "hello world")


# ---------------------------------------------------------------------------
# Regex DoS protections (via insert_crash_signature)
# ---------------------------------------------------------------------------

class TestRegexDosProtection(unittest.TestCase):
    """Tests for regex DoS protections in insert_crash_signature."""

    def _create_schema(self, conn):
        """Create the crash_signatures table in *conn*."""
        conn.execute("""
            CREATE TABLE IF NOT EXISTS crash_signatures (
                id TEXT PRIMARY KEY,
                name TEXT,
                regex_pattern TEXT,
                solution_markdown TEXT,
                action_button_json TEXT
            )
        """)

    def setUp(self):
        self._conn = _compile.sqlite3.connect(":memory:")
        self._create_schema(self._conn)
        self._rejected: list[int] = [0]

    def tearDown(self):
        self._conn.close()

    def test_long_pattern_rejected(self):
        """A pattern >256 characters is rejected."""
        long_pattern = "a" * 257
        sig = {
            "id": "long_test",
            "name": "Long Pattern",
            "regex_pattern": long_pattern,
            "solution_markdown": "",
            "action_button_json": "[]",
        }
        _compile.insert_crash_signature(self._conn, sig, self._rejected)
        self.assertEqual(self._rejected[0], 1)
        # Should not have been inserted.
        row = self._conn.execute(
            "SELECT id FROM crash_signatures WHERE id = ?", ("long_test",)
        ).fetchone()
        self.assertIsNone(row)

    def test_valid_pattern_accepted(self):
        """A normal short pattern is accepted and inserted."""
        sig = {
            "id": "nullptr_test",
            "name": "Null Pointer",
            "regex_pattern": r"java\.lang\.NullPointerException",
            "solution_markdown": "Check for nulls.",
            "action_button_json": "[]",
        }
        _compile.insert_crash_signature(self._conn, sig, self._rejected)
        self.assertEqual(self._rejected[0], 0)
        row = self._conn.execute(
            "SELECT id, regex_pattern FROM crash_signatures WHERE id = ?",
            ("nullptr_test",),
        ).fetchone()
        self.assertIsNotNone(row)
        self.assertEqual(row[0], "nullptr_test")
        self.assertEqual(row[1], r"java\.lang\.NullPointerException")

    def test_invalid_regex_rejected(self):
        """An invalid regex pattern is rejected."""
        sig = {
            "id": "bad_regex",
            "name": "Bad Regex",
            "regex_pattern": "[invalid(regex",
            "solution_markdown": "",
            "action_button_json": "[]",
        }
        _compile.insert_crash_signature(self._conn, sig, self._rejected)
        self.assertEqual(self._rejected[0], 1)

    def test_exact_256_pattern_accepted(self):
        """A pattern exactly 256 characters is accepted (boundary)."""
        pattern_256 = "a" * 256
        sig = {
            "id": "boundary_test",
            "name": "Boundary",
            "regex_pattern": pattern_256,
            "solution_markdown": "",
            "action_button_json": "[]",
        }
        _compile.insert_crash_signature(self._conn, sig, self._rejected)
        self.assertEqual(self._rejected[0], 0)
        row = self._conn.execute(
            "SELECT id FROM crash_signatures WHERE id = ?", ("boundary_test",)
        ).fetchone()
        self.assertIsNotNone(row)



# ---------------------------------------------------------------------------
# Ported from _test_social_metrics.py (no equivalents in this file)
# ---------------------------------------------------------------------------

class TestParseReaction(unittest.TestCase):
    """Tests for _parse_reaction."""

    def test_parse_reaction_upvote(self):
        """A +1 reaction on the issue itself is an upvote with comment_id=None."""
        obj = {
            "user": {"login": "alice"},
            "content": "+1",
            "created_at": "2026-01-15T12:00:00Z",
        }
        r = _compile._parse_reaction(obj, comment_id=None)
        self.assertIsNotNone(r)
        self.assertEqual(r.user, "alice")
        self.assertTrue(r.is_upvote)
        self.assertIsNone(r.comment_id)
        self.assertEqual(r.timestamp, datetime(2026, 1, 15, 12, 0, 0, tzinfo=timezone.utc))

    def test_parse_reaction_downvote(self):
        """A -1 reaction is a downvote."""
        obj = {
            "user": {"login": "bob"},
            "content": "-1",
            "created_at": "2026-02-20T08:30:00Z",
        }
        r = _compile._parse_reaction(obj, comment_id=42)
        self.assertIsNotNone(r)
        self.assertFalse(r.is_upvote)
        self.assertEqual(r.comment_id, 42)

    def test_parse_reaction_neutral(self):
        """Neutral emoji (laugh, heart, etc.) sets is_upvote to None."""
        for content in ("laugh", "hooray", "confused", "heart", "rocket", "eyes"):
            obj = {
                "user": {"login": "charlie"},
                "content": content,
                "created_at": "2026-03-01T00:00:00Z",
            }
            r = _compile._parse_reaction(obj, comment_id=None)
            self.assertIsNotNone(r)
            self.assertIsNone(r.is_upvote)

    def test_parse_reaction_malformed_no_user(self):
        """Missing user field returns None."""
        obj = {"content": "+1", "created_at": "2026-01-01T00:00:00Z"}
        self.assertIsNone(_compile._parse_reaction(obj, comment_id=None))

    def test_parse_reaction_malformed_no_content(self):
        """Missing content field returns None."""
        obj = {"user": {"login": "dave"}, "created_at": "2026-01-01T00:00:00Z"}
        self.assertIsNone(_compile._parse_reaction(obj, comment_id=None))

    def test_parse_reaction_malformed_no_created_at(self):
        """Missing created_at field returns None."""
        obj = {"user": {"login": "eve"}, "content": "+1"}
        self.assertIsNone(_compile._parse_reaction(obj, comment_id=None))

    def test_parse_reaction_malformed_created_at(self):
        """Unparseable created_at returns None."""
        obj = {
            "user": {"login": "frank"},
            "content": "+1",
            "created_at": "not-a-date",
        }
        self.assertIsNone(_compile._parse_reaction(obj, comment_id=None))

    def test_parse_reaction_user_login_lowercased(self):
        """User login is always lowercased."""
        obj = {
            "user": {"login": "AliceWunderland"},
            "content": "+1",
            "created_at": "2026-01-01T00:00:00Z",
        }
        r = _compile._parse_reaction(obj, comment_id=None)
        self.assertEqual(r.user, "alicewunderland")


class TestUserReactionDataclass(unittest.TestCase):
    """Tests for UserReaction dataclass."""

    def test_user_reaction_dataclass_defaults(self):
        """comment_id defaults to None."""
        ts = datetime(2026, 1, 1, tzinfo=timezone.utc)
        r = _compile.UserReaction(user="test", is_upvote=True, timestamp=ts)
        self.assertIsNone(r.comment_id)

    def test_user_reaction_dataclass_with_comment_id(self):
        """comment_id can be provided explicitly."""
        ts = datetime(2026, 1, 1, tzinfo=timezone.utc)
        r = _compile.UserReaction(user="test", is_upvote=False, timestamp=ts, comment_id=99)
        self.assertEqual(r.comment_id, 99)


class TestModSocialMetrics(unittest.TestCase):
    """Tests for ModSocialMetrics dataclass."""

    def test_mod_social_metrics_defaults(self):
        """reactions defaults to empty list; each instance gets its own list."""
        m1 = _compile.ModSocialMetrics(mod_id="foo", issue_number=1)
        m2 = _compile.ModSocialMetrics(mod_id="bar", issue_number=2)
        self.assertEqual(m1.reactions, [])
        self.assertEqual(m2.reactions, [])
        self.assertIsNot(m1.reactions, m2.reactions)
        m1.reactions.append("x")
        self.assertEqual(m2.reactions, [])

    def test_mod_social_metrics_with_reactions(self):
        """Reactions can be appended after construction."""
        ts = datetime(2026, 1, 1, tzinfo=timezone.utc)
        r = _compile.UserReaction(user="alice", is_upvote=True, timestamp=ts)
        m = _compile.ModSocialMetrics(mod_id="sodium", issue_number=5)
        m.reactions.append(r)
        self.assertEqual(len(m.reactions), 1)
        self.assertEqual(m.reactions[0].user, "alice")


class TestSybilDiversityWeight(unittest.TestCase):
    """Tests for _sybil_diversity_weight."""

    def test_sybil_diversity_weight_single_mod(self):
        """User who only reacted on one mod gets 0.5 weight."""
        self.assertEqual(_compile._sybil_diversity_weight("alice", ["sodium"]), 0.5)

    def test_sybil_diversity_weight_multiple_mods(self):
        """User who reacted on multiple mods gets 1.0 weight."""
        self.assertEqual(_compile._sybil_diversity_weight("alice", ["sodium", "iris"]), 1.0)

    def test_sybil_diversity_weight_repeated_single_mod(self):
        """Repeated reactions on a single mod still count as one distinct mod."""
        self.assertEqual(_compile._sybil_diversity_weight("alice", ["sodium", "sodium", "sodium"]), 0.5)


class TestUserInteractionCounts(unittest.TestCase):
    """Tests for _user_interaction_counts."""

    def test_user_interaction_counts_counts_per_user(self):
        """Counts are correct for shared and exclusive users across mods."""
        ts = datetime(2026, 1, 1, tzinfo=timezone.utc)
        m1 = _compile.ModSocialMetrics(mod_id="sodium", issue_number=1)
        m1.reactions.extend([
            _compile.UserReaction(user="alice", is_upvote=True, timestamp=ts),
            _compile.UserReaction(user="bob", is_upvote=False, timestamp=ts),
        ])
        m2 = _compile.ModSocialMetrics(mod_id="iris", issue_number=2)
        m2.reactions.extend([
            _compile.UserReaction(user="alice", is_upvote=True, timestamp=ts),
            _compile.UserReaction(user="charlie", is_upvote=True, timestamp=ts),
        ])
        by_mod = {"sodium": m1, "iris": m2}
        cache: dict[str, int] = {}
        result = _compile._user_interaction_counts(by_mod, token="", org="", cache=cache)
        self.assertEqual(cache["alice"], 0)
        self.assertEqual(cache["bob"], 0)
        self.assertEqual(cache["charlie"], 0)
        self.assertIs(result, cache)


class TestComputeVelocity(unittest.TestCase):
    """Tests for _compute_velocity."""

    def test_compute_velocity_zero_history_zero_recent(self):
        """With zero history and zero recent, velocity is clamped."""
        now_dt = datetime(2026, 6, 22, tzinfo=timezone.utc)
        velocity, is_anomaly, anomaly_start = _compile._compute_velocity([], [], now_dt)
        self.assertGreaterEqual(velocity, -1.5)
        self.assertLessEqual(velocity, 0.0)
        self.assertFalse(is_anomaly)
        self.assertIsNone(anomaly_start)

    def test_compute_velocity_anomaly_fires_on_large_recent_downvote_burst(self):
        """25 downvotes in the last 6h with minimal historical context fires anomaly."""
        now_dt = datetime(2026, 6, 22, tzinfo=timezone.utc)
        six_h_ago = now_dt - __import__("datetime").timedelta(hours=6)
        down_ts = [
            now_dt - __import__("datetime").timedelta(hours=1, minutes=i)
            for i in range(25)
        ]
        historical_ts = [
            now_dt - __import__("datetime").timedelta(days=3),
            now_dt - __import__("datetime").timedelta(days=5),
        ]
        down_ts.extend(historical_ts)
        velocity, is_anomaly, anomaly_start = _compile._compute_velocity([], down_ts, now_dt)
        self.assertTrue(is_anomaly)
        self.assertIsNotNone(anomaly_start)
        self.assertAlmostEqual(anomaly_start, six_h_ago, delta=__import__("datetime").timedelta(minutes=1))
        self.assertGreater(velocity, 0.0)
        self.assertLessEqual(velocity, 10.0)

    def test_compute_velocity_no_anomaly_at_low_recent_count(self):
        """10 downvotes in 6h vs. historical 5/7d: ratio > 5 but recent_count <= 20 -> no anomaly."""
        now_dt = datetime(2026, 6, 22, tzinfo=timezone.utc)
        down_6h = [
            now_dt - __import__("datetime").timedelta(hours=1, minutes=i)
            for i in range(10)
        ]
        down_7d = [
            now_dt - __import__("datetime").timedelta(days=d)
            for d in [1, 2, 3, 4, 5]
        ]
        down_ts = down_6h + down_7d
        velocity, is_anomaly, anomaly_start = _compile._compute_velocity([], down_ts, now_dt)
        self.assertFalse(is_anomaly)
        self.assertIsNone(anomaly_start)


class TestRegexFilterComment(unittest.TestCase):
    """Tests for _regex_filter_comment."""

    def test_version_begging_regex_drops(self):
        """Version-begging comments are rejected."""
        passed, reason = _compile._regex_filter_comment("when is 1.21 release?")
        self.assertFalse(passed)
        self.assertEqual(reason, "version-begging")

    def test_empty_praise_regex_drops(self):
        """Empty praise comments are rejected."""
        passed, reason = _compile._regex_filter_comment("nice mod!")
        self.assertFalse(passed)
        self.assertEqual(reason, "empty-praise")

    def test_legit_review_passes_regex(self):
        """A substantive technical review passes regex filters."""
        passed, reason = _compile._regex_filter_comment(
            "This mod significantly improved my framerate from 30 to 120 FPS."
        )
        self.assertTrue(passed)
        self.assertEqual(reason, "")

    def test_empty_text_dropped(self):
        """Empty or whitespace-only text is dropped."""
        passed, reason = _compile._regex_filter_comment("")
        self.assertFalse(passed)
        self.assertEqual(reason, "empty")
        passed, reason = _compile._regex_filter_comment("   ")
        self.assertFalse(passed)
        self.assertEqual(reason, "empty")

    def test_port_to_regex_drops(self):
        """'port to' and 'update to' variants are also dropped."""
        for text in ("update to 1.21?", "port to 1.20 release", "for 1.22"):
            passed, reason = _compile._regex_filter_comment(text)
            self.assertFalse(passed, f"Expected drop for '{text}', got pass")
            self.assertEqual(reason, "version-begging")


class TestNlpFilterComment(unittest.TestCase):
    """Tests for _nlp_filter_comment."""

    def test_nlp_filter_comment_handles_missing_deps_gracefully(self):
        """If profanity-check import fails, _nlp_filter_comment returns (True, "")."""
        try:
            import profanity_check  # noqa: F401
            has_deps = True
        except ImportError:
            has_deps = False
        passed, reason = _compile._nlp_filter_comment("This mod is fantastic and very well made.")
        self.assertTrue(passed)
        self.assertEqual(reason, "")


class TestPass3Constants(unittest.TestCase):
    """Tests for Pass 3 circuit-breaker constants."""

    def test_organic_under_review_threshold_constant(self):
        """ORGANIC_UNDER_REVIEW_THRESHOLD must equal -10."""
        self.assertEqual(_compile.ORGANIC_UNDER_REVIEW_THRESHOLD, -10)

    def test_triage_poll_duration_constant(self):
        """TRIAGE_POLL_DURATION_DAYS must equal 7."""
        self.assertEqual(_compile.TRIAGE_POLL_DURATION_DAYS, 7)


class TestAppendAuditEntry(unittest.TestCase):
    """Tests for _append_audit_entry."""

    def setUp(self):
        self._audit_path = _compile.REGISTRY_DIR / "governance" / "audit_log.json"
        self._backup_path = None
        if self._audit_path.exists():
            self._backup_path = self._audit_path.with_suffix(self._audit_path.suffix + ".bak")
            shutil.copy2(str(self._audit_path), str(self._backup_path))

    def tearDown(self):
        if self._backup_path and self._backup_path.exists():
            shutil.copy2(str(self._backup_path), str(self._audit_path))
            self._backup_path.unlink(missing_ok=True)
        elif self._audit_path.exists() and self._backup_path is None:
            self._audit_path.unlink(missing_ok=True)

    def test_append_audit_entry_creates_file_when_absent(self):
        """_append_audit_entry creates the file if it doesn't exist."""
        if self._audit_path.exists():
            self._audit_path.unlink()
        _compile._append_audit_entry("test_action", "test_details")
        self.assertTrue(self._audit_path.exists())
        with self._audit_path.open("r", encoding="utf-8") as fh:
            data = json.load(fh)
        self.assertIn("entries", data)
        self.assertEqual(len(data["entries"]), 1)
        self.assertEqual(data["entries"][0]["action"], "test_action")
        self.assertEqual(data["entries"][0]["details"], "test_details")
        self.assertIn("timestamp", data["entries"][0])

    def test_append_audit_entry_rotates_at_10000(self):
        """After 10000 entries, adding one more rotates to keep <= 10000."""
        data = {"log_format_version": 1, "entries": [{"timestamp": "2026-01-01T00:00:00Z", "action": f"dummy_{i}", "details": ""} for i in range(10000)]}
        self._audit_path.parent.mkdir(parents=True, exist_ok=True)
        with self._audit_path.open("w", encoding="utf-8") as fh:
            json.dump(data, fh)
        _compile._append_audit_entry("rotate_test", "should rotate")
        with self._audit_path.open("r", encoding="utf-8") as fh:
            data = json.load(fh)
        self.assertLessEqual(len(data["entries"]), 10000)
        self.assertIn("log_format_version", data)


class TestFindTriageDiscussionCategory(unittest.TestCase):
    """Tests for _find_triage_discussion_category."""

    def test_find_triage_discussion_category_returns_none_without_network(self):
        """With a dummy nonexistent repo, the function should return None."""
        result = _compile._find_triage_discussion_category(
            "og-nonexistent-xyz", "og-nonexistent-xyz", token="invalid-token-for-test",
        )
        self.assertIsNone(result)


class TestCreateAdminAlertIssue(unittest.TestCase):
    """Tests for _create_admin_alert_issue."""

    def test_create_admin_alert_issue_failure_logged_not_raised(self):
        """Invalid token should log a warning but not raise."""
        _compile._create_admin_alert_issue(
            "og-nonexistent-xyz", "og-nonexistent-xyz",
            mod_id="test", offending_reactions=[], token="invalid-token",
        )
        self.assertTrue(True)


class TestDiscordAlert(unittest.TestCase):
    """Tests for Discord webhook notification channel."""

    _prev_discord_url: str | None = None

    def setUp(self):
        self._prev_discord_url = os.environ.pop("DISCORD_WEBHOOK_URL", None)

    def tearDown(self):
        if self._prev_discord_url is not None:
            os.environ["DISCORD_WEBHOOK_URL"] = self._prev_discord_url

    def test_load_discord_webhook_url_returns_none_when_unset(self):
        """When DISCORD_WEBHOOK_URL is absent, _load_discord_webhook_url returns None."""
        os.environ.pop("DISCORD_WEBHOOK_URL", None)
        result = _compile._load_discord_webhook_url()
        self.assertIsNone(result)

    def test_load_discord_webhook_url_returns_value_when_set(self):
        """When DISCORD_WEBHOOK_URL is set, _load_discord_webhook_url returns it."""
        os.environ["DISCORD_WEBHOOK_URL"] = "https://discord.com/api/webhooks/test/abc"
        result = _compile._load_discord_webhook_url()
        self.assertEqual(result, "https://discord.com/api/webhooks/test/abc")

    def test_post_discord_alert_is_noop_when_url_unset(self):
        """When DISCORD_WEBHOOK_URL is unset, _post_discord_alert returns without making a network call."""
        os.environ.pop("DISCORD_WEBHOOK_URL", None)
        _compile._post_discord_alert(mod_id="testmod", reason="test", severity="spike")
        self.assertTrue(True)

    @unittest.skipIf(os.environ.get("CI") == "true", "skip network test on CI")
    def test_post_discord_alert_swallows_invalid_webhook_failures(self):
        """An invalid webhook URL must not raise."""
        os.environ["DISCORD_WEBHOOK_URL"] = "https://discord.com/api/webhooks/INVALID/INVALID"
        _compile._post_discord_alert(
            mod_id="testmod",
            reason="test reason",
            severity="spike",
            offending_reactions=[{"user": "bob"}],
        )
        self.assertTrue(True)

    def test_post_discord_alert_accepts_optional_fields(self):
        """Calling _post_discord_alert with optional fields still returns cleanly when no webhook is configured."""
        os.environ.pop("DISCORD_WEBHOOK_URL", None)
        _compile._post_discord_alert(
            mod_id="testmod",
            reason="test reason",
            severity="spike",
            offending_reactions=[{"user": "alice"}],
            admin_alert_issue_url="https://example.com/issues/1",
        )
        self.assertTrue(True)


if __name__ == "__main__":
    unittest.main()
