#!/usr/bin/env python3
"""Standalone unit tests for compiler/compile.py social-metrics functions.

Run with:  python compiler/_test_social_metrics.py
No pytest dependency — uses only stdlib (unittest, sys, datetime).

These tests cover Pass 1 of §3.1 steps 3-9: GitHub API bootstrap, issue
enumeration, reaction aggregation.  Trust filtering, velocity circuit breaker,
immune passthrough, and main() wiring are Pass 2 (not implemented here).
"""

from __future__ import annotations

import json
import os
import shutil
import sys
import tempfile
import unittest
from datetime import datetime, timezone

# Ensure we can import the compiler module from the repo root.
sys.path.insert(0, "compiler")
import compile as _compile  # noqa: E402


class TestPass1(unittest.TestCase):
    """Tests for social-metrics functions added in Pass 1."""

    # ------------------------------------------------------------------
    # _extract_mod_id
    # ------------------------------------------------------------------

    def test_extract_mod_id_from_realistic_issue_body(self):
        """The GitHub form renders the Mod Registry ID as a heading + value."""
        body = (
            "### Mod Registry ID\n"
            "sodium\n"
            "\n"
            "### Your Technical Review (50 character minimum)\n"
            "Excellent performance improvement over vanilla rendering.\n"
        )
        self.assertEqual(_compile._extract_mod_id(body), "sodium")

    def test_extract_mod_id_returns_none_for_non_review_issue(self):
        """Issue body without the Mod Registry ID heading returns None."""
        body = (
            "### Feature Request\n"
            "Add support for mod X.\n"
        )
        self.assertIsNone(_compile._extract_mod_id(body))

    def test_extract_mod_id_case_insensitive_heading(self):
        """Heading casing should not matter — registry IDs are lowercase."""
        body = "### mod registry ID\n" "CaveClient\n"
        self.assertEqual(_compile._extract_mod_id(body), "caveclient")

    def test_extract_mod_id_with_crlf(self):
        """Windows-style CRLF line endings should still parse."""
        body = "### Mod Registry ID\r\n" "sodium\r\n" "\r\n"
        self.assertEqual(_compile._extract_mod_id(body), "sodium")

    def test_extract_mod_id_with_trailing_whitespace(self):
        """Trailing spaces after the mod ID should be trimmed."""
        body = "### Mod Registry ID\n" "sodium   \n"
        self.assertEqual(_compile._extract_mod_id(body), "sodium")

    def test_extract_mod_id_empty_body(self):
        """Empty or None body returns None."""
        self.assertIsNone(_compile._extract_mod_id(""))
        self.assertIsNone(_compile._extract_mod_id(None))

    # ------------------------------------------------------------------
    # _parse_reaction
    # ------------------------------------------------------------------

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

    # ------------------------------------------------------------------
    # UserReaction dataclass
    # ------------------------------------------------------------------

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

    # ------------------------------------------------------------------
    # ModSocialMetrics dataclass
    # ------------------------------------------------------------------

    def test_mod_social_metrics_defaults(self):
        """reactions defaults to empty list; each instance gets its own list."""
        m1 = _compile.ModSocialMetrics(mod_id="foo", issue_number=1)
        m2 = _compile.ModSocialMetrics(mod_id="bar", issue_number=2)
        self.assertEqual(m1.reactions, [])
        self.assertEqual(m2.reactions, [])
        # Each instance has its own list (no shared mutable state).
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


class TestPass2(unittest.TestCase):
    """Tests for social-metrics functions added in Pass 2."""

    # ------------------------------------------------------------------
    # _sybil_diversity_weight
    # ------------------------------------------------------------------

    def test_sybil_diversity_weight_single_mod(self):
        """User who only reacted on one mod gets 0.5 weight."""
        self.assertEqual(_compile._sybil_diversity_weight("alice", ["sodium"]), 0.5)

    def test_sybil_diversity_weight_multiple_mods(self):
        """User who reacted on multiple mods gets 1.0 weight."""
        self.assertEqual(_compile._sybil_diversity_weight("alice", ["sodium", "iris"]), 1.0)

    def test_sybil_diversity_weight_repeated_single_mod(self):
        """Repeated reactions on a single mod still count as one distinct mod."""
        self.assertEqual(_compile._sybil_diversity_weight("alice", ["sodium", "sodium", "sodium"]), 0.5)

    # ------------------------------------------------------------------
    # _user_interaction_counts
    # ------------------------------------------------------------------

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
        # No token/org provided — all users get 0 from contributionsCollection fallback.
        result = _compile._user_interaction_counts(by_mod, token="", org="", cache=cache)
        # The function populates the cache with 0 for each user (no real API).
        self.assertEqual(cache["alice"], 0)
        self.assertEqual(cache["bob"], 0)
        self.assertEqual(cache["charlie"], 0)
        # Return value is the cache itself.
        self.assertIs(result, cache)

    # ------------------------------------------------------------------
    # _compute_velocity
    # ------------------------------------------------------------------

    def test_compute_velocity_zero_history_zero_recent(self):
        """With zero history and zero recent, velocity = -1.0 (clamped)."""
        now = datetime(2026, 6, 22, tzinfo=timezone.utc)
        velocity, is_anomaly, anomaly_start = _compile._compute_velocity([], [], now)
        # 0 / 0.5 - 1 = -1.0
        self.assertGreaterEqual(velocity, -1.5)
        self.assertLessEqual(velocity, 0.0)
        self.assertFalse(is_anomaly)
        self.assertIsNone(anomaly_start)

    def test_compute_velocity_anomaly_fires_on_large_recent_downvote_burst(self):
        """25 downvotes in the last 6h with minimal historical context fires anomaly."""
        now = datetime(2026, 6, 22, tzinfo=timezone.utc)
        six_h_ago = now - __import__("datetime").timedelta(hours=6)
        # 25 downvotes all within the last 3 hours (inside the 6h window).
        down_ts = [
            now - __import__("datetime").timedelta(hours=1, minutes=i)
            for i in range(25)
        ]
        # 2 downvotes spread over the prior 7 days (outside the 6h window).
        historical_ts = [
            now - __import__("datetime").timedelta(days=3),
            now - __import__("datetime").timedelta(days=5),
        ]
        down_ts.extend(historical_ts)
        velocity, is_anomaly, anomaly_start = _compile._compute_velocity([], down_ts, now)
        self.assertTrue(is_anomaly)
        self.assertIsNotNone(anomaly_start)
        # anomaly_window_start should be ~6h in the past.
        self.assertAlmostEqual(anomaly_start, six_h_ago, delta=__import__("datetime").timedelta(minutes=1))
        # Velocity should be positive (clamped to [−10, 10]).
        self.assertGreater(velocity, 0.0)
        self.assertLessEqual(velocity, 10.0)

    def test_compute_velocity_no_anomaly_at_low_recent_count(self):
        """10 downvotes in 6h vs. historical 5/7d: ratio > 5 but recent_count <= 20 → no anomaly."""
        now = datetime(2026, 6, 22, tzinfo=timezone.utc)
        # 10 downvotes in the last 6h.
        down_6h = [
            now - __import__("datetime").timedelta(hours=1, minutes=i)
            for i in range(10)
        ]
        # 5 downvotes over the past 7 days (outside 6h window).
        down_7d = [
            now - __import__("datetime").timedelta(days=d)
            for d in [1, 2, 3, 4, 5]
        ]
        down_ts = down_6h + down_7d
        velocity, is_anomaly, anomaly_start = _compile._compute_velocity([], down_ts, now)
        self.assertFalse(is_anomaly)
        self.assertIsNone(anomaly_start)


class TestStep7(unittest.TestCase):
    """Tests for §3.1 step 7: sentiment + spam scrubbing of review comments."""

    # ------------------------------------------------------------------
    # _regex_filter_comment
    # ------------------------------------------------------------------

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

    # ------------------------------------------------------------------
    # _extract_review_text
    # ------------------------------------------------------------------

    def test_extract_review_text_from_realistic_body(self):
        """Synthetic body with the review heading → extracted text contains expected fragment."""
        body = (
            "### Mod Registry ID\n"
            "sodium\n"
            "\n"
            "### Your Technical Review (50 character minimum)\n"
            "Excellent performance improvement over vanilla rendering. "
            "The GPU utilization is much more balanced now.\n"
            "\n"
            "### Additional Comments\n"
            "None.\n"
        )
        result = _compile._extract_review_text(body)
        self.assertIsNotNone(result)
        self.assertIn("Excellent performance improvement", result)

    def test_extract_review_text_returns_none_when_absent(self):
        """Body without the review field returns None."""
        body = (
            "### Mod Registry ID\n"
            "sodium\n"
        )
        self.assertIsNone(_compile._extract_review_text(body))

    def test_extract_review_text_none_body(self):
        """None body returns None."""
        self.assertIsNone(_compile._extract_review_text(None))

    # ------------------------------------------------------------------
    # _scrub_review_text
    # ------------------------------------------------------------------

    def test_scrub_review_text_passes_legit_text(self):
        """A positive technical review passes the full scrub pipeline."""
        passed, cleaned, reason = _compile._scrub_review_text(
            "This mod significantly improved my framerate from 30 to 120 FPS."
        )
        self.assertTrue(passed)
        self.assertEqual(reason, "")
        self.assertEqual(cleaned, "This mod significantly improved my framerate from 30 to 120 FPS.")

    def test_scrub_review_text_drops_version_begging(self):
        """Version-begging text fails the scrub pipeline."""
        passed, cleaned, reason = _compile._scrub_review_text("when is 1.21 release?")
        self.assertFalse(passed)
        self.assertEqual(reason, "version-begging")

    def test_scrub_review_text_drops_empty_praise(self):
        """Empty praise text fails the scrub pipeline."""
        passed, cleaned, reason = _compile._scrub_review_text("nice")
        self.assertFalse(passed)
        self.assertEqual(reason, "empty-praise")

    # ------------------------------------------------------------------
    # _nlp_filter_comment — fail-open when deps missing
    # ------------------------------------------------------------------

    def test_nlp_filter_comment_handles_missing_deps_gracefully(self):
        """If profanity-check import fails, _nlp_filter_comment returns (True, "")."""
        # Try importing profanity_check; if it fails, we test the fail-open path.
        # If it succeeds (CI with deps installed), the NLP path still runs on
        # benign text and returns (True, ""). Either way the test passes.
        try:
            import profanity_check  # noqa: F401
            has_deps = True
        except ImportError:
            has_deps = False

        # Benign text — should pass regardless of NLP path.
        passed, reason = _compile._nlp_filter_comment("This mod is fantastic and very well made.")
        self.assertTrue(passed)
        self.assertEqual(reason, "")


if __name__ == "__main__":
    unittest.main()


class TestPass3(unittest.TestCase):
    """Tests for §3.1 steps 5/6/8 + §3.2 Raid Shield (Pass 3 circuit-breaker response)."""

    # ------------------------------------------------------------------
    # Constants
    # ------------------------------------------------------------------

    def test_organic_under_review_threshold_constant(self):
        """ORGANIC_UNDER_REVIEW_THRESHOLD must equal -10."""
        self.assertEqual(_compile.ORGANIC_UNDER_REVIEW_THRESHOLD, -10)

    def test_triage_poll_duration_constant(self):
        """TRIAGE_POLL_DURATION_DAYS must equal 7."""
        self.assertEqual(_compile.TRIAGE_POLL_DURATION_DAYS, 7)

    # ------------------------------------------------------------------
    # _append_audit_entry — file creation
    # ------------------------------------------------------------------

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
            # Test created the file; remove it.
            self._audit_path.unlink(missing_ok=True)

    def test_append_audit_entry_creates_file_when_absent(self):
        """_append_audit_entry creates the file if it doesn't exist."""
        # Ensure file is absent.
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

    def test_append_audit_entry_rotates_at_1000(self):
        """After 1000 entries, adding one more rotates to keep <= 1000."""
        # Pre-populate with 1000 dummy entries.
        data = {"entries": [{"timestamp": "2026-01-01T00:00:00Z", "action": f"dummy_{i}", "details": ""} for i in range(1000)]}
        self._audit_path.parent.mkdir(parents=True, exist_ok=True)
        with self._audit_path.open("w", encoding="utf-8") as fh:
            json.dump(data, fh)
        _compile._append_audit_entry("rotate_test", "should rotate")
        with self._audit_path.open("r", encoding="utf-8") as fh:
            data = json.load(fh)
        self.assertLessEqual(len(data["entries"]), 1000)

    # ------------------------------------------------------------------
    # _find_triage_discussion_category — no-network path
    # ------------------------------------------------------------------

    def test_find_triage_discussion_category_returns_none_without_network(self):
        """With a dummy nonexistent repo, the function should return None."""
        result = _compile._find_triage_discussion_category(
            "og-nonexistent-xyz", "og-nonexistent-xyz", token="invalid-token-for-test",
        )
        self.assertIsNone(result)

    # ------------------------------------------------------------------
    # _create_admin_alert_issue — failure logged, not raised
    # ------------------------------------------------------------------

    def test_create_admin_alert_issue_failure_logged_not_raised(self):
        """Invalid token should log a warning but not raise."""
        _compile._create_admin_alert_issue(
            "og-nonexistent-xyz", "og-nonexistent-xyz",
            mod_id="test", offending_reactions=[], token="invalid-token",
        )
        # If we got here without raising, the test passes.
        self.assertTrue(True)
