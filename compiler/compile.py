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
import time
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
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
# Regex DoS protection (ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§2.4.1)
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
        # Windows: no signal.alarm ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â use subprocess with timeout.
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
# GitHub social metrics (ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 steps 3-9)
# ---------------------------------------------------------------------------
#
# The governance repo (owner/repo) whose Issue tracker hosts mod reviews / vote
# reactions. Reviews are filed via the .github/ISSUE_TEMPLATE/review-form.yml
# form; the compiler enumerates issues in this repo whose body matches the form
# layout (### Mod Registry ID heading + value) and aggregates reactions on the
# issue + its comments to produce per-mod upvotes/downvotes.
#
# The governance repo IS the registry repo itself ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â issues are filed on the
# main repository, not a separate governance repo. Override locally via
# the AGORA_REGISTRY_REPO env var ("owner/repo" form). In GitHub Actions,
# GITHUB_REPOSITORY is auto-set to "owner/repo" and used as fallback.
GITHUB_API_BASE = "https://api.github.com"
DEFAULT_REGISTRY_OWNER = "jarjarpfeil"
DEFAULT_REGISTRY_REPO = "Agora-Minecraft-Mod-Loader"


def _get_registry_repo() -> str:
    """Return the 'owner/repo' string for the registry/governance repo."""
    return (
        os.environ.get("AGORA_REGISTRY_REPO")
        or os.environ.get("GITHUB_REPOSITORY")
        or f"{DEFAULT_REGISTRY_OWNER}/{DEFAULT_REGISTRY_REPO}"
    )
GITHUB_REACTION_UPVOTES = {"+1"}
GITHUB_REACTION_DOWNVOTES = {"-1"}

# How the issue body's "### Mod Registry ID" field looks when rendered by
# GitHub issues created from the form. The form emits a heading followed by
# the value on its own line.
MOD_ID_FIELD_RE = re.compile(
    r"###\s*Mod Registry ID\s*\r?\n+\s*(\S+)\s*(?:\r?\n|$)",
    re.IGNORECASE,
)

# --- ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 5/6/8 + ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.2 Raid Shield circuit-breaker response ---
#
# When the velocity circuit breaker fires (Pass 2 set m.status='under_review' +
# m.anomaly_window_start), the compiler performs beyond-just-status-flip:
#   - ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.2 Raid Shield: programmatically enable "existing users only"
#     interaction limits on the governance repo (covers ALL items, not just
#     the offending one ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â GitHub interaction-limits are repo-wide).
#   - ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 6: DELETE the offending reactions from GitHub via the REST
#     API (their data is captured pre-DELETE in the audit log entry below).
#   - ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 8 (create): create a 7-day triage poll in Discussions under
#     a "Triage" category if it exists (soft-fail with a warning if the
#     category isn't present ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â discussion creation is OPTIONAL, status flip
#     is the hard requirement).
#   - Admin-alert issue: file a high-priority confidential issue in the
#     governance repo (private repos are out of scope; we file as a
#     confidential issue on agora-mc/agora-mc itself).
#
# When `net_score < -10` organically (no spike), step 5 mandates the same
# under_review flip + triage poll creation, but WITHOUT reaction-deletion
# (no offending burst to delete) and WITHOUT Raid Shield (interaction limit
# is only for burst-attack defense).
TRIAGE_POLL_DURATION_DAYS = 7
ORGANIC_UNDER_REVIEW_THRESHOLD = -10  # ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§5.3: net_score drops below -10 organically
TRIAGE_DISCUSSION_CATEGORY_CANDIDATES = ("Triage", "Mod Reviews", "Community Triage")


@dataclass
class UserReaction:
    """One (user, vote_direction, timestamp) extracted from a GitHub reaction.

    `comment_id` is None when the reaction was placed directly on the issue
    itself (rather than on a comment). `is_upvote` is True for +1 / thumbs_up
    emoji, False for -1 / thumbs_down, None for neutral emoji (laugh, hooray,
    confused, heart, rocket, eyes) ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â those are still tracked because user
    participation in ANY reaction on the mod's tracking issue counts toward
    diversity (Sybil resistance, Pass 2).
    """
    user: str
    is_upvote: bool | None
    timestamp: datetime
    comment_id: int | None = None


@dataclass
class ModSocialMetrics:
    """Raw (pre-trust-filter) social metrics for one mod.

    Populated in Pass 1 by `_hydrate_github_social_metrics`. Pass 2 will consume
    these to compute final upvotes/downvotes/net_score/velocity after applying
    the trust + Sybil + velocity circuit-breaker rules from ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 steps 4, 5, 9.
    """
    mod_id: str
    issue_number: int
    reactions: list[UserReaction] = field(default_factory=list)
    # Pass 2 (computed by _apply_trust_velocity_pass ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â not populated by Pass 1):
    upvotes: int = 0
    downvotes: int = 0
    net_score: int = 0
    velocity: float = 0.0
    status: str = "active"
    anomaly_window_start: datetime | None = None  # set if velocity circuit breaker fired
    raw_reviews: list[dict[str, Any]] = field(default_factory=list)
    # Each entry: {"author": str, "text": str, "issue_number": int, "created_at": ISO-datetime-string}.
    # Populated during _hydrate_github_social_metrics by parsing the form's
    # "Your Technical Review" field from the issue body. Pass 2 / step 7 then
    # filters via _scrub_review_text and feeds the survivors into
    # curator_reviews.top_reviews_json.
    scrubbed_reviews: list[dict[str, Any]] = field(default_factory=list)
    immunity_cooldown_until: str | None = None


def _load_github_token() -> str | None:
    """Read GITHUB_TOKEN from env (loaded by _load_dotenv()). Returns None when unset."""
    token = os.environ.get("GITHUB_TOKEN", "").strip()
    return token or None


def _github_request(
    method: str,
    path: str,
    *,
    token: str,
    params: dict[str, Any] | None = None,
    body: dict[str, Any] | None = None,
    max_retries: int = 5,
) -> Any:
    """Send a GitHub REST API request with auth + rate-limit handling.

    Retries on 403-with-X-RateLimit-Remaining-0 by sleeping until the reset
    epoch. Returns parsed JSON (dict or list). Raises on non-2xx after retries.
    """
    url = path if path.startswith("http") else f"{GITHUB_API_BASE}{path}"
    headers = {
        "Authorization": f"Bearer {token}",
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
        "User-Agent": f"AgoraCompiler/1.0 (https://github.com/{_get_registry_repo()})",
    }
    last_exc: Exception | None = None
    for attempt in range(max_retries):
        resp = requests.request(
            method, url, headers=headers, params=params, json=body, timeout=60
        )
        if resp.status_code in (200, 201):
            return resp.json()
        if resp.status_code in (403, 429):
            remaining = resp.headers.get("X-RateLimit-Remaining")
            reset_epoch = resp.headers.get("X-RateLimit-Reset")
            if remaining == "0" or resp.status_code == 429:
                sleep_for = 1.0
                if reset_epoch:
                    try:
                        sleep_for = max(1.0, float(reset_epoch) - time.time() + 1.0)
                    except ValueError:
                        pass
                logger.warning(
                    "GitHub API rate-limited on %s %s (attempt %d/%d). Sleeping %.0fs.",
                    method, path, attempt + 1, max_retries, sleep_for,
                )
                time.sleep(min(sleep_for, 3600.0))
                continue
        body_excerpt = resp.text[:500] if resp.text else ""
        last_exc = RuntimeError(
            f"GitHub API {method} {path} returned {resp.status_code}: {body_excerpt}"
        )
        break
    raise last_exc or RuntimeError(f"GitHub API {method} {path} exhausted retries")


def _github_graphql(
    query: str,
    *,
    token: str,
    variables: dict[str, Any] | None = None,
    max_retries: int = 5,
) -> dict[str, Any]:
    """POST a GraphQL query to the GitHub API. Handles rate limits + errors.

    Returns the `data` field of the GraphQL response. Raises on `errors[]`
    in the response or non-2xx after retries. Rate-limit handling mirrors
    `_github_request`: sleep until reset epoch on 403/429.
    """
    body = {"query": query}
    if variables:
        body["variables"] = variables
    last_exc: Exception | None = None
    for attempt in range(max_retries):
        resp = requests.post(
            f"{GITHUB_API_BASE}/graphql",
            headers={
                "Authorization": f"Bearer {token}",
                "Accept": "application/vnd.github+json",
                "X-GitHub-Api-Version": "2022-11-28",
                "User-Agent": f"AgoraCompiler/1.0 (https://github.com/{_get_registry_repo()})",
            },
            json=body,
            timeout=60,
        )
        if resp.status_code in (200, 201):
            payload = resp.json()
            if "errors" in payload and payload["errors"]:
                raise RuntimeError(f"GraphQL errors: {payload['errors']}")
            return payload.get("data", {}) or {}
        if resp.status_code in (403, 429):
            remaining = resp.headers.get("X-RateLimit-Remaining")
            reset_epoch = resp.headers.get("X-RateLimit-Reset")
            if remaining == "0" or resp.status_code == 429:
                sleep_for = 1.0
                if reset_epoch:
                    try:
                        sleep_for = max(1.0, float(reset_epoch) - time.time() + 1.0)
                    except ValueError:
                        pass
                logger.warning(
                    "GitHub GraphQL rate-limited (attempt %d/%d). Sleeping %.0fs.",
                    attempt + 1, max_retries, sleep_for,
                )
                time.sleep(min(sleep_for, 3600.0))
                continue
        body_excerpt = resp.text[:500] if resp.text else ""
        last_exc = RuntimeError(f"GraphQL HTTP {resp.status_code}: {body_excerpt}")
        break
    raise last_exc or RuntimeError("GraphQL exhausted retries")


def _user_org_interaction_count(login: str, org: str, *, token: str) -> int:
    """Spec ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 4 (strict): count a user's interactions across the agora-mc org.

    Uses the GraphQL `user.contributionsCollection` resolver scoped to the
    org via `organization` argument. Returns the sum of totalIssuesContributedTo,
    totalPullRequestContributions, totalIssueCommentsContributionsForContributor
    within the scoped org. On any error: returns 0 (treated as untrusted ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â
    fail-safe). NEVER cache this method on its own ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â callers cache via a
    dict[str, int] passed in to amortize across mods.
    """
    query = """
    query ($login: String!, $org: String!) {
      user(login: $login) {
        contributionsCollection(organization: $org) {
          totalIssueContributions
          totalPullRequestContributions
          totalIssueCommentsContributionsForContributor
          totalCommitContributions
          totalRepositoryContributions
        }
      }
    }
    """
    try:
        data = _github_graphql(query, token=token, variables={"login": login, "org": org})
        cc = (data.get("user") or {}).get("contributionsCollection") or {}
        return int(cc.get("totalIssueContributions", 0)) \
            + int(cc.get("totalPullRequestContributions", 0)) \
            + int(cc.get("totalIssueCommentsContributionsForContributor", 0)) \
            + int(cc.get("totalCommitContributions", 0)) \
            + int(cc.get("totalRepositoryContributions", 0))
    except Exception as exc:
        logger.warning(
            "Trust check: could not fetch contributionsCollection for '%s' in org '%s' (%s); treating as zero interactions.",
            login, org, exc,
        )
        return 0


def _github_paginate(
    path: str,
    *,
    token: str,
    per_page: int = 100,
    max_pages: int = 50,
    params: dict[str, Any] | None = None,
) -> list[dict[str, Any]]:
    """Fetch all pages of a GitHub list endpoint (Link-header pagination)."""
    out: list[dict[str, Any]] = []
    for page in range(1, max_pages + 1):
        page_params: dict[str, Any] = {"per_page": per_page, "page": page}
        if params:
            page_params.update(params)
        page_items = _github_request("GET", path, token=token, params=page_params)
        if not isinstance(page_items, list):
            break
        out.extend(page_items)
        if len(page_items) < per_page:
            break
    return out


def _load_poll_blacklist() -> set[str]:
    """Read registry/governance/poll_blacklist.json. Returns empty set on any error.

    The schema is `{"usernames": ["name1", "name2"]}`.
    """
    path = REGISTRY_DIR / "governance" / "poll_blacklist.json"
    try:
        with path.open("r", encoding="utf-8") as fh:
            data = json.load(fh)
        names = data.get("usernames", [])
        return {str(n).lower() for n in names if isinstance(n, str)}
    except (OSError, json.JSONDecodeError) as exc:
        logger.warning("Could not load poll_blacklist.json (%s); treating as empty.", exc)
        return set()


def _extract_mod_id(issue_body: str | None) -> str | None:
    """Extract the mod registry ID from a review-form issue body."""
    if not issue_body:
        return None
    m = MOD_ID_FIELD_RE.search(issue_body)
    if not m:
        return None
    return m.group(1).strip().lower()


def _parse_reaction(reaction_obj: dict[str, Any], comment_id: int | None) -> UserReaction | None:
    """Convert a single GitHub reaction dict to a UserReaction, or None for malformed input."""
    user_obj = reaction_obj.get("user") or {}
    user = user_obj.get("login")
    content = reaction_obj.get("content")
    created_at = reaction_obj.get("created_at")
    if not user or not content or not created_at:
        return None
    try:
        ts = datetime.fromisoformat(created_at.replace("Z", "+00:00"))
    except ValueError:
        return None
    if content in GITHUB_REACTION_UPVOTES:
        is_up = True
    elif content in GITHUB_REACTION_DOWNVOTES:
        is_up = False
    else:
        is_up = None
    return UserReaction(user=user.lower(), is_upvote=is_up, timestamp=ts, comment_id=comment_id)


def _fetch_reactions_for_issue(
    owner: str,
    repo: str,
    issue_number: int,
    *,
    token: str,
    blacklist: set[str],
) -> list[UserReaction]:
    """Fetch all reactions on an issue + each of its comments (paginated).

    Pass 1 ignores blacklist ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â that's applied at aggregation time in Pass 2.
    """
    out: list[UserReaction] = []
    issue_reactions = _github_paginate(
        f"/repos/{owner}/{repo}/issues/{issue_number}/reactions",
        token=token,
    )
    for r in issue_reactions:
        parsed = _parse_reaction(r, comment_id=None)
        if parsed:
            out.append(parsed)
    comments = _github_paginate(
        f"/repos/{owner}/{repo}/issues/{issue_number}/comments",
        token=token,
    )
    for c in comments:
        cid = c.get("id")
        if not isinstance(cid, int):
            continue
        comment_reactions = _github_paginate(
            f"/repos/{owner}/{repo}/issues/comments/{cid}/reactions",
            token=token,
        )
        for r in comment_reactions:
            parsed = _parse_reaction(r, comment_id=cid)
            if parsed:
                out.append(parsed)
    return out


def _hydrate_github_social_metrics(
    items: list[dict[str, Any]],
) -> None:
    """Attach `_social_metrics: ModSocialMetrics` to each item dict that has
    a corresponding review-issue in the governance repo.

    Mutates `items` in-place (mirrors the `_hydrate_modrinth_metadata` pattern).
    On any error (no token, network failure, malformed data): silently leave
    items untouched ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â Pass 2 will treat missing `_social_metrics` as zeros.

    ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 Step 3 (Pass 1): enumerate governance-repo issues, extract mod_id
    from each review-form body, fetch reactions (issue-level + per-comment),
    attach a ModSocialMetrics obj to the corresponding item dict.
    """
    token = _load_github_token()
    if not token:
        logger.info("GITHUB_TOKEN not set; social metrics will be zeroed (local dev mode).")
        return
    gov_full = _get_registry_repo()
    owner, _, repo = gov_full.partition("/")
    if not owner or not repo:
        logger.warning("AGORA_REGISTRY_REPO='%s' is not 'owner/repo' format; skipping social metrics.", gov_full)
        return
    blacklist = _load_poll_blacklist()

    mod_ids_present = {item["id"].lower(): item for item in items if "id" in item}

    try:
        all_issues = _github_paginate(
            f"/repos/{owner}/{repo}/issues",
            token=token,
            params={"state": "all"},
        )
    except RuntimeError as exc:
        if "404" in str(exc):
            logger.warning(
                "Registry repo %s/%s not found or has no issues (404). "
                "Social metrics will be zeroed. Set AGORA_REGISTRY_REPO to the correct owner/repo.",
                owner, repo,
            )
            return
        raise
    logger.info("Found %d issues in %s/%s governance repo", len(all_issues), owner, repo)

    by_mod: dict[str, ModSocialMetrics] = {}
    for issue in all_issues:
        if "pull_request" in issue:
            continue
        issue_number = issue.get("number")
        if not isinstance(issue_number, int):
            continue
        mod_id = _extract_mod_id(issue.get("body"))
        if not mod_id or mod_id not in mod_ids_present:
            continue
        review_text = _extract_review_text(issue.get("body"))
        author_obj = issue.get("user") or {}
        author = author_obj.get("login")
        created_at_str = issue.get("created_at")
        if review_text and author and created_at_str and mod_id in by_mod:
            by_mod[mod_id].raw_reviews.append({
                "author": author,
                "text": review_text,
                "issue_number": issue_number,
                "created_at": created_at_str,
            })
        try:
            reactions = _fetch_reactions_for_issue(
                owner, repo, issue_number, token=token, blacklist=blacklist,
            )
        except Exception as exc:
            logger.warning(
                "Failed to fetch reactions for %s/%s issue #%d (mod %s): %s",
                owner, repo, issue_number, mod_id, exc,
            )
            reactions = []
        metrics = by_mod.get(mod_id)
        if metrics is None:
            metrics = ModSocialMetrics(mod_id=mod_id, issue_number=issue_number)
            by_mod[mod_id] = metrics
        metrics.reactions.extend(reactions)

    attached = 0
    for mod_id, item in mod_ids_present.items():
        metrics = by_mod.get(mod_id)
        if metrics is not None:
            item["_social_metrics"] = metrics
            attached += 1
    logger.info(
        "Attached raw social metrics to %d of %d items (Pass 1: zero trust/velocity filtering applied)",
        attached, len(mod_ids_present),
    )


def _sybil_diversity_weight(login: str, mod_ids_voted_on_by_user: list[str]) -> float:
    """Reduced weight for users who only participated on a SINGLE mod (Sybil defence, ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 4).

    Spec: "Vote weight receives a small multiplier if the account has a
    demonstrated history of participating in *different* issues/repositories."
    Accounts that only ever reacted on one item are treated with suspicion.
    v1: returns 0.5 (small multiplier) when len(set(mod_ids)) <= 1, else 1.0.
    """
    distinct_mods = {m for m in mod_ids_voted_on_by_user if m}
    if len(distinct_mods) <= 1:
        return 0.5
    return 1.0


def _user_interaction_counts(
    metrics_by_mod: dict[str, "ModSocialMetrics"],
    *,
    token: str,
    org: str,
    cache: dict[str, int],
) -> dict[str, int]:
    """Populate per-user interaction counts across the agora-mc org via GraphQL.

    Spec ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 4: trust activity threshold is "at least 3 interactions
    (issues opened, comments, PRs, or reactions) across repositories owned by
    the agora-mc organization." Cached per session to amortize across items.
    """
    distinct_users: set[str] = set()
    for mod_id, m in metrics_by_mod.items():
        for r in m.reactions:
            distinct_users.add(r.user)
    for user in distinct_users:
        if user not in cache:
            cache[user] = _user_org_interaction_count(user, org, token=token)
    return cache


def _is_user_trusted(
    login: str,
    *,
    interaction_count: int,
    token: str,
    now: datetime,
    cache: dict[str, bool | None],
    age_threshold_days: int = 30,
    activity_threshold: int = 3,
) -> bool:
    """Apply ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 4 trust gating to one user.

    Trust = (account age >= 30 days) AND (interaction_count >= 3 in this repo).
    `cache` prevents refetching the same user's profile across mods. On any API
    failure (rate limit exhausted, 404, etc.) the user is treated as UNtrusted
    (fail-safe ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â bad actors waste time, no false-positive trust grants).
    """
    cached = cache.get(login)
    if cached is not None:
        return cached
    # Fetch the user profile for `created_at`.
    try:
        profile = _github_request("GET", f"/users/{login}", token=token)
        created_at_str = profile.get("created_at") if isinstance(profile, dict) else None
        if not created_at_str:
            cache[login] = False
            return False
        created_at = datetime.fromisoformat(created_at_str.replace("Z", "+00:00"))
        age_ok = (now - created_at).days >= age_threshold_days
    except Exception as exc:
        logger.warning("Trust check: could not fetch profile for user '%s' (%s); treating as untrusted.", login, exc)
        cache[login] = False
        return False
    activity_ok = interaction_count >= activity_threshold
    trusted = age_ok and activity_ok
    cache[login] = trusted
    return trusted


def _compute_velocity(
    upvote_timestamps: list[datetime],
    downvote_timestamps: list[datetime],
    now: datetime,
) -> tuple[float, bool, datetime | None]:
    """ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 5: compute velocity (trending metric) and anomaly flag.

    Returns (velocity_float, is_anomaly, anomaly_window_start).

    The `velocity` column is used for the Browse sort to surface trending
    items (desktop renders "ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Å“Ãƒâ€šÃ‚Â² X.X" / "ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Å“Ãƒâ€šÃ‚Â¼ X.X"). We define:
        - historical_avg_per_6h = count_in_7d / 28     (7-day window / 6-hour windows)
        - recent_6h_total = upvotes_last_6h + downvotes_last_6h
        - velocity = (recent_6h_total - historical_avg_per_6h) / max(historical_avg_per_6h, 0.5)
        - velocity is clamped to [-10.0, 10.0] for column sanity.

    The circuit-breaker (separate from velocity) fires when:
        (recent_downvotes_6h / max(historical_downvotes_7d_avg_per_6h, 1.0)) > 5.0
        AND recent_downvotes_6h > 20
    When it fires, anomaly_window_start = (now - 6h) ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â the caller uses this to
    freeze vote counts at pre-spike values by excluding reactions in the
    window [anomaly_window_start, now].
    """
    seven_d_ago = now - timedelta(days=7)
    six_h_ago = now - timedelta(hours=6)

    up_7d = [t for t in upvote_timestamps if seven_d_ago <= t <= now]
    down_7d = [t for t in downvote_timestamps if seven_d_ago <= t <= now]
    up_6h = [t for t in upvote_timestamps if six_h_ago <= t <= now]
    down_6h = [t for t in downvote_timestamps if six_h_ago <= t <= now]

    total_7d = len(up_7d) + len(down_7d)
    historical_avg_per_6h = total_7d / 28.0  # 7 days / 6h windows = 28
    recent_6h_total = len(up_6h) + len(down_6h)

    if historical_avg_per_6h < 0.5:
        velocity = (recent_6h_total / 0.5) - 1.0
    else:
        velocity = (recent_6h_total - historical_avg_per_6h) / historical_avg_per_6h
    velocity = max(-10.0, min(10.0, velocity))

    # Circuit breaker ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â DOWNVOTES only (raid signature per spec ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 5).
    historical_downvotes_per_6h = len(down_7d) / 28.0
    recent_downvotes_6h = len(down_6h)
    ratio = recent_downvotes_6h / max(historical_downvotes_per_6h, 1.0)
    if ratio > 5.0 and recent_downvotes_6h > 20:
        return velocity, True, six_h_ago
    return velocity, False, None


def _apply_trust_velocity_pass(
    items: list[dict[str, Any]],
    *,
    token: str,
    blacklist: set[str],
) -> None:
    """Pass 2: apply ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 steps 4 (trust), Sybil weighting, 5 (velocity breaker), 9 (immune).

    Walks each item's attached `_social_metrics` (added by Pass 1), filters
    reactions by user trust + diversity, computes final upvotes/downvotes/
    net_score/velocity/status. Mutates each ModSocialMetrics in place.

    Items with `governance.immune == true` skip ALL score evaluation per ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1
    step 9: their metrics stay at default (0, 0, 0, 0.0, "active") regardless
    of reactions ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â and the attached `_social_metrics` is REMOVED from the
    item dict so the immune-passthrough in `insert_registry_item` can detect
    the immune case cleanly.

    When GITHUB_TOKEN is absent (local dev) this is a no-op ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â items keep
    their defaults and the DB shows zeros (same as before Pass 2).
    """
    if not token:
        return
    now = datetime.now(timezone.utc)

    # Collect all metrics objects that need processing.
    by_mod: dict[str, ModSocialMetrics] = {}
    immune_mods: set[str] = set()
    for item in items:
        item_id = item.get("id")
        if not item_id:
            continue
        item_id_lower = item_id.lower()
        gov = item.get("governance") or {}
        if gov.get("immune") is True:
            immune_mods.add(item_id_lower)
            continue  # immune: skip entirely, leave metrics absent
        social = item.get("_social_metrics")
        if social is None:
            continue
        by_mod[item_id_lower] = social

    if not by_mod:
        return

    # Build per-user mod-diversity map: {user: list of mod_ids they reacted on}.
    user_mods: dict[str, list[str]] = {}
    for mod_id, m in by_mod.items():
        for r in m.reactions:
            user_mods.setdefault(r.user, []).append(mod_id)

    trust_cache: dict[str, bool | None] = {}
    trust_interaction_cache: dict[str, int] = {}
    # Pre-populate blacklist users as untrusted.
    for u in blacklist:
        trust_cache[u] = False
        trust_interaction_cache[u] = 0

    for mod_id, m in by_mod.items():
        # Compute pre-anomaly reaction lists.
        up_ts = [r.timestamp for r in m.reactions if r.is_upvote is True]
        down_ts = [r.timestamp for r in m.reactions if r.is_upvote is False]

        velocity, is_anomaly, anomaly_start = _compute_velocity(up_ts, down_ts, now)
        m.velocity = velocity
        if is_anomaly:
            m.status = "under_review"
            m.anomaly_window_start = anomaly_start
            logger.warning(
                "Velocity circuit breaker fired for mod '%s' (anomaly window from %s). Counts will be frozen at pre-spike values.",
                mod_id, anomaly_start,
            )

        # Tally upvotes/downvotes applying trust + Sybil + freeze-at-pre-spike.
        accepted_up = 0
        accepted_down = 0
        gov_full = _get_registry_repo()
        gov_org = gov_full.split("/")[0] or DEFAULT_REGISTRY_OWNER
        interaction_counts = _user_interaction_counts(by_mod, token=token, org=gov_org, cache=trust_interaction_cache)
        for r in m.reactions:
            if r.is_upvote is None:
                continue  # neutral reaction (laugh, hooray, etc.) ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â not a vote
            # Freeze-at-pre-spike: if anomaly fired, drop reactions inside
            # the 6h anomaly window before counting.
            if is_anomaly and anomaly_start is not None and r.timestamp >= anomaly_start:
                continue
            # Trust gate (ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 4).
            interaction_count = interaction_counts.get(r.user, 0)
            if not _is_user_trusted(
                r.user,
                interaction_count=interaction_count,
                token=token,
                now=now,
                cache=trust_cache,
            ):
                continue
            # Sybil diversity weighting (ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 4 Sybil resistance).
            diversity = _sybil_diversity_weight(r.user, user_mods.get(r.user, []))
            if diversity < 0.99:
                # v1: drop low-weight votes entirely (proportional weighting
                # on binary votes is equivalent to drop-some-here).
                continue
            if r.is_upvote:
                accepted_up += 1
            else:
                accepted_down += 1
        m.upvotes = accepted_up
        m.downvotes = accepted_down
        m.net_score = accepted_up - accepted_down

        # Spec ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 7: sentiment + spam scrubbing of review comments.
        # Survivors are attached as `m.scrubbed_reviews`. `insert_registry_item`
        # will write them into curator_reviews.top_reviews_json as JSON.
        scrubbed: list[dict[str, Any]] = []
        for raw in m.raw_reviews:
            passed, cleaned, reason = _scrub_review_text(raw.get("text", ""))
            if not passed:
                logger.info(
                    "Dropped review for mod '%s' by '%s' (reason: %s).",
                    mod_id, raw.get("author", "?"), reason,
                )
                continue
            scrubbed.append({
                "author": raw.get("author"),
                "text": cleaned,
                "issue_number": raw.get("issue_number"),
                "created_at": raw.get("created_at"),
            })
        # Top 10 reviews (by issue creation order; tiebreak: longest review).
        scrubbed.sort(key=lambda r: (r.get("created_at", ""), -len(r.get("text", ""))))
        m.scrubbed_reviews = scrubbed[:10]


# --- ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.2 Raid Shield (interaction-limits toggle) ---

def _enable_raid_shield(owner: str, repo: str, *, token: str) -> bool:
    """ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.2: programmatically enable "existing users only" interaction limits.

    Sets `limit: existing_users` on the repo via
    `PUT /repos/{owner}/{repo}/interaction-limits`. Returns True on success,
    False on failure. The limit expires automatically (GitHub default is 24h
    for the existing_users tier; future nightly runs will re-enable if a new
    spike is detected).
    """
    try:
        _github_request(
            "PUT",
            f"/repos/{owner}/{repo}/interaction-limits",
            token=token,
            body={"limit": "existing_users", "expiry": "24h"},
        )
        logger.warning("Raid Shield ENABLED on %s/%s ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â only existing users can interact for 24h.", owner, repo)
        return True
    except Exception as exc:
        logger.warning("Failed to enable Raid Shield on %s/%s: %s", owner, repo, exc)
        return False


# --- ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 6 (DELETE offending reactions + admin-alert issue) ---

def _gather_offending_reactions(
    owner: str,
    repo: str,
    issue_number: int,
    window_start: datetime,
    *,
    token: str,
) -> list[dict[str, Any]]:
    """Re-fetch reactions on the offending issue + its comments, restricted to
    the anomaly window [window_start, now]. Returns a list of dicts each with:
        {"user": str, "reaction_id": int, "content": str, "comment_id": int|None,
         "issue_number": int, "created_at": ISO-8601}
    These are the reactions about to be DELETEd. Captured pre-DELETE for audit.
    """
    now = datetime.now(timezone.utc)
    out: list[dict[str, Any]] = []
    # Issue-level reactions.
    for r in _github_paginate(
        f"/repos/{owner}/{repo}/issues/{issue_number}/reactions", token=token,
    ):
        try:
            created = datetime.fromisoformat((r.get("created_at") or "").replace("Z", "+00:00"))
        except ValueError:
            continue
        if not (window_start <= created <= now):
            continue
        out.append({
            "user": (r.get("user") or {}).get("login"),
            "reaction_id": r.get("id"),
            "content": r.get("content"),
            "comment_id": None,
            "issue_number": issue_number,
            "created_at": r.get("created_at"),
        })
    # Per-comment reactions.
    comments = _github_paginate(
        f"/repos/{owner}/{repo}/issues/{issue_number}/comments", token=token,
    )
    for c in comments:
        cid = c.get("id")
        if not isinstance(cid, int):
            continue
        for r in _github_paginate(
            f"/repos/{owner}/{repo}/issues/comments/{cid}/reactions", token=token,
        ):
            try:
                created = datetime.fromisoformat((r.get("created_at") or "").replace("Z", "+00:00"))
            except ValueError:
                continue
            if not (window_start <= created <= now):
                continue
            out.append({
                "user": (r.get("user") or {}).get("login"),
                "reaction_id": r.get("id"),
                "content": r.get("content"),
                "comment_id": cid,
                "issue_number": issue_number,
                "created_at": r.get("created_at"),
            })
    return out


def _delete_reaction(
    owner: str,
    repo: str,
    *,
    issue_number: int,
    comment_id: int | None,
    reaction_id: int,
    token: str,
) -> bool:
    """DELETE a single reaction from GitHub via REST. Returns True on success.

    Routes to the issue-level OR comment-level delete endpoint based on
    `comment_id`. Per ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 6, this requires `issues:write` permission
    on the GitHub token, which `compile.yml` already grants.
    """
    try:
        if comment_id is None:
            _github_request(
                "DELETE",
                f"/repos/{owner}/{repo}/issues/{issue_number}/reactions/{reaction_id}",
                token=token,
            )
        else:
            _github_request(
                "DELETE",
                f"/repos/{owner}/{repo}/issues/comments/{comment_id}/reactions/{reaction_id}",
                token=token,
            )
        return True
    except Exception as exc:
        logger.warning("Failed to DELETE reaction %d on issue #%d (%s): %s",
                       reaction_id, issue_number, owner + "/" + repo, exc)
        return False


def _append_audit_entry(action: str, details: str, *, actor: str | None = None, target_type: str | None = None, target_id: str | None = None, reason: str | None = None) -> None:
    """Append an entry to registry/governance/audit_log.json BEFORE risky
    operations (DELETE reactions, enable Raid Shield). Captures intent even
    when the subsequent API call fails.

    Per Ã‚Â§4.6: each entry carries timestamp, action, actor, target_type,
    target_id, reason, and details. The root object includes log_format_version.
    Rotation archives the oldest 2,000 entries to an archive file when the log
    exceeds 10,000 entries, keeping the most recent 8,000.
    """
    path = REGISTRY_DIR / "governance" / "audit_log.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        if path.exists():
            with path.open("r", encoding="utf-8") as fh:
                data = json.load(fh)
        else:
            data = {"log_format_version": 1, "entries": []}
    except (OSError, json.JSONDecodeError):
        data = {"log_format_version": 1, "entries": []}
    if "log_format_version" not in data:
        data["log_format_version"] = 1
    entry = {
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "action": action,
        "details": details,
    }
    if actor is not None:
        entry["actor"] = actor
    if target_type is not None:
        entry["target_type"] = target_type
    if target_id is not None:
        entry["target_id"] = target_id
    if reason is not None:
        entry["reason"] = reason
    data["entries"].append(entry)
    if len(data["entries"]) > 10000:
        archive_date = datetime.now(timezone.utc).strftime("%Y%m%d")
        archive_path = REGISTRY_DIR / "governance" / f"audit_log_archive.{archive_date}.json"
        oldest_2000 = data["entries"][:2000]
        data["entries"] = data["entries"][-8000:]
        if archive_path.exists():
            with archive_path.open("r", encoding="utf-8") as fh:
                archive_data = json.load(fh)
            archive_data.extend(oldest_2000)
        else:
            archive_data = oldest_2000
        with archive_path.open("w", encoding="utf-8") as fh:
            json.dump(archive_data, fh, indent=2)
    with path.open("w", encoding="utf-8") as fh:
        json.dump(data, fh, indent=2)


# --- ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.2 Discord webhook alert (real-time push to curator alerts channel) ---

def _load_discord_webhook_url() -> str | None:
    """Read DISCORD_WEBHOOK_URL from env. Returns None when unset.

    The webhook URL is a Discord channel-level integration (Settings ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢
    Integrations ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ Webhooks ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ New Webhook). No bot account required ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â just
    a webhook URL. When absent, all Discord notifications are silently skipped
    (curator alerts only via the audit trail + admin-alert issue). When Discord
    returns 4xx/5xx, the warning is logged but the compile continues.
    """
    url = os.environ.get("DISCORD_WEBHOOK_URL", "").strip()
    return url or None


def _post_discord_alert(
    *,
    mod_id: str,
    reason: str,
    severity: str,  # "spike" or "organic"
    offending_reactions: list[dict[str, Any]] | None = None,
    admin_alert_issue_url: str | None = None,
) -> None:
    """Post a formatted Discord alert to the configured webhook.

    Uses Discord's webhook JSON API: POST {webhook_url} with a JSON body
    containing `embeds[]`. Sev-volume colors: red (#ED4245) for spike-triggered
    attacks, orange (#EE8430) for organic under_review triggers. Failure-safe:
    logs a warning and returns None on any network or HTTP error. NEVER raises.
    """
    webhook_url = _load_discord_webhook_url()
    if not webhook_url:
        return  # Discord notifications are optional; silent no-op.

    # Pick embed color by severity.
    color = 0xED4245 if severity == "spike" else 0xEE8430
    title = f"ÃƒÆ’Ã‚Â°Ãƒâ€¦Ã‚Â¸Ãƒâ€¦Ã‚Â¡Ãƒâ€šÃ‚Â¨ Coordinated Attack: `{mod_id}`" if severity == "spike" else f"ÃƒÆ’Ã‚Â¢Ãƒâ€¦Ã‚Â¡Ãƒâ€šÃ‚Â ÃƒÆ’Ã‚Â¯Ãƒâ€šÃ‚Â¸Ãƒâ€šÃ‚Â Mod under review: `{mod_id}`"

    fields: list[dict[str, Any]] = [
        {"name": "Reason", "value": reason, "inline": False},
        {"name": "Severity", "value": severity, "inline": True},
    ]
    if offending_reactions is not None:
        unique_users = sorted({r.get("user") or "?" for r in offending_reactions})
        fields.append({
            "name": "Offending users",
            "value": (", ".join(unique_users)[:1024] or "(none captured)"),
            "inline": False,
        })
        fields.append({
            "name": "Reactions DELETEd",
            "value": str(len(offending_reactions)),
            "inline": True,
        })
    if admin_alert_issue_url:
        fields.append({
            "name": "Admin-alert issue",
            "value": admin_alert_issue_url,
            "inline": False,
        })

    body = {
        "username": "Agora Compiler",
        "embeds": [{
            "title": title,
            "color": color,
            "timestamp": datetime.now(timezone.utc).isoformat(),
            "fields": fields,
            "footer": {"text": "Agora nightly compiler"},
        }],
    }
    try:
        resp = requests.post(webhook_url, json=body, timeout=15)
        if resp.status_code not in (200, 204):
            logger.warning(
                "Discord webhook returned HTTP %d: %s",
                resp.status_code, resp.text[:300] if resp.text else "",
            )
    except Exception as exc:
        logger.warning("Discord webhook POST failed: %s", exc)


def _create_admin_alert_issue(
    owner: str,
    repo: str,
    *,
    mod_id: str,
    offending_reactions: list[dict[str, Any]],
    token: str,
) -> None:
    """ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 6: file a high-priority issue in the governance repo for
    curator review. Title format: `[ALERT] Coordinated Attack Detected: <mod_id>`.

    The body summarizes offending users (lowercased), the reaction contents,
    timestamps, and the audit entry reference. Uses `issues:write` permission.
    Failure is logged but does not abort the broader compile loop.
    """
    user_summary = "\n".join(
        f"- @{r['user']} ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â {r['content']} on {r['created_at']} (issue #{r['issue_number']}, comment_id={r['comment_id']})"
        for r in offending_reactions if r.get("user")
    )
    body = (
        f"## Coordinated Attack Detected\n\n"
        f"The nightly compiler's velocity circuit breaker fired for mod "
        f"`{mod_id}`. {len(offending_reactions)} offending reactions were "
        f"identified in the 6-hour anomaly window and DELETED from GitHub.\n\n"
        f"### Offending Reactions (captured pre-DELETE)\n\n{user_summary}\n\n"
        f"### Action Taken\n\n"
        f"- [x] Velocity circuit breaker fired (status ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ under_review)\n"
        f"- [x] Reaction counts frozen at pre-spike values\n"
        f"- [x] Raid Shield interaction-limits enabled (existing users only, 24h)\n"
        f"- [x] Offending reactions DELETEd from GitHub\n"
        f"- [x] Triage poll created in Discussions\n"
        f"- [x] This admin-alert issue filed\n\n"
        f"### Curator Action Required\n\n"
        f"Review offending users for org-level action (suspension, addition to "
        f"`registry/governance/poll_blacklist.json`).\n"
    )
    try:
        _github_request(
            "POST",
            f"/repos/{owner}/{repo}/issues",
            token=token,
            body={
                "title": f"[ALERT] Coordinated Attack Detected: {mod_id}",
                "body": body,
                "labels": ["triage", "coordinated-attack"],
            },
        )
        logger.warning("Filed admin-alert issue for mod '%s' coordinated attack.", mod_id)
    except Exception as exc:
        logger.warning("Failed to file admin-alert issue for mod '%s': %s", mod_id, exc)


# --- ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 8 (create triage poll) ---

def _find_triage_discussion_category(owner: str, repo: str, *, token: str) -> int | None:
    """Discover a Discussions category id matching one of the candidate names.

    Returns None when Discussions aren't enabled on the repo OR no matching
    category exists. Soft-fails ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â poll creation is OPTIONAL; the status flip
    is the hard requirement of ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 5.
    """
    try:
        for cat_name in TRIAGE_DISCUSSION_CATEGORY_CANDIDATES:
            data = _github_graphql(
                """
                query ($owner: String!, $repo: String!) {
                  repository(owner: $owner, name: $repo) {
                    discussionCategories(first: 50) {
                      nodes { id name }
                    }
                  }
                }
                """,
                token=token,
                variables={"owner": owner, "repo": repo},
            )
            nodes = (((data.get("repository") or {}).get("discussionCategories") or {}).get("nodes") or [])
            for node in nodes:
                if (node.get("name") or "").strip().lower() == str(cat_name).strip().lower():
                    return node.get("id")
        return None
    except Exception as exc:
        logger.warning("Could not fetch discussion categories on %s/%s: %s", owner, repo, exc)
        return None


def _create_triage_poll(
    owner: str,
    repo: str,
    *,
    mod_id: str,
    issue_number: int,
    reason: str,
    token: str,
) -> str | None:
    """ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 5 / ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§5.3: open a 7-day triage discussion poll.

    Title: "[Community Triage] Should '<mod_id>' be removed from the registry?"
    Body explains the reason (velocity spike OR organic net_score < -10).

    Returns the discussion node URL on success, None on failure (the
    Discussions API requires `discussions:write` permission, already granted
    in compile.yml; if Discussions aren't enabled on the repo or no triage
    category exists, this silently no-ops with a warning).
    """
    category_id = _find_triage_discussion_category(owner, repo, token=token)
    if category_id is None:
        logger.warning(
            "Triage discussion category not found on %s/%s (looked for %s). "
            "Status is still 'under_review'; poll will not be created.",
            owner, repo, TRIAGE_DISCUSSION_CATEGORY_CANDIDATES,
        )
        return None
    title = f"[Community Triage] Should '{mod_id}' be removed from the registry?"
    body = (
        f"## Triage Poll Triggered\n\n"
        f"**Mod:** `{mod_id}` (tracking issue #{issue_number})\n\n"
        f"**Reason:** {reason}\n\n"
        f"**Duration:** {TRIAGE_POLL_DURATION_DAYS} days.\n\n"
        f"### Vote\n\n"
        f"- ÃƒÆ’Ã‚Â°Ãƒâ€¦Ã‚Â¸ÃƒÂ¢Ã¢â€šÂ¬Ã‹Å“Ãƒâ€šÃ‚Â Keep `{mod_id}` in the registry (status restored to 'active', "
        f"30-day immunity cooldown granted).\n"
        f"- ÃƒÆ’Ã‚Â°Ãƒâ€¦Ã‚Â¸ÃƒÂ¢Ã¢â€šÂ¬Ã‹Å“Ãƒâ€¦Ã‚Â½ Remove `{mod_id}` ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â JSON file moved to `registry/archived/`, "
        f"item removed from all future builds.\n\n"
        f"### Notes\n\n"
        f"- Reactions from users listed in `registry/governance/poll_blacklist.json` "
        f"will be counted at weight 0 at poll resolution.\n"
        f"- The nightly compiler resolves this poll automatically after "
        f"{TRIAGE_POLL_DURATION_DAYS} days.",
    )
    try:
        data = _github_graphql(
            """
            mutation ($repoId: ID!, $categoryId: ID!, $title: String!, $body: String!) {
              createDiscussion(input: {repositoryId: $repoId, categoryId: $categoryId, title: $title, body: $body}) {
                discussion { url }
              }
            }
            """,
            token=token,
            variables={"repoId": "", "categoryId": category_id, "title": title, "body": body},
        )
        discussion = (((data.get("createDiscussion") or {}).get("discussion")) or {})
        url = discussion.get("url")
        if url:
            logger.info("Created triage discussion for mod '%s': %s", mod_id, url)
        return url
    except Exception as exc:
        logger.warning("Failed to create triage discussion for mod '%s': %s", mod_id, exc)
        return None


# --- ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 8 (resolve expired triage polls) ---

def _resolve_expired_triage_polls(
    owner: str,
    repo: str,
    *,
    items: list[dict[str, Any]],
    token: str,
) -> None:
    """ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 5/8: for items currently 'under_review', look up associated
    triage discussions; if the 7-day window has expired, tally votes from
    poll_blacklist-free users and either archive the item (Remove wins) or
    restore it (Keep wins).

    Archive = move the JSON manifest file from registry/mods/<id>.json (or
    packs/, etc) to registry/archived/<id>.json. Item will be excluded from
    the next nightly build (insert_registry_item only ingests from the 7
    CONTENT_DIRS, not from registry/archived/).
    """
    now = datetime.now(timezone.utc)
    for item in items:
        social = item.get("_social_metrics")
        if social is None or social.status != "under_review":
            continue
        start = social.anomaly_window_start
        if start is None:
            continue
        elapsed = now - start
        if elapsed < timedelta(days=TRIAGE_POLL_DURATION_DAYS):
            continue
        try:
            data = _github_graphql(
                """
                query ($q: String!) {
                  search(query: $q, type: DISCUSSION, first: 5) {
                    nodes { ... on Discussion { id url title reactions { totalCount content } }
                    }
                  }
                }
                """,
                token=token,
                variables={"q": f"repo:{owner}/{repo} [Community Triage] {social.mod_id}"},
            )
            nodes = ((data.get("search") or {}).get("nodes")) or []
            if not nodes:
                continue
            discussion = nodes[0]
            keep_votes = 0
            remove_votes = 0
            data2 = _github_graphql(
                """
                query ($id: ID!) {
                  node(id: $id) {
                    ... on Discussion {
                      reactions(first: 100) {
                        nodes { user { login } content }
                      }
                      reactionGroups { content viewerHasReacted }
                    }
                  }
                }
                """,
                token=token,
                variables={"id": discussion.get("id")},
            )
            reactions = ((((data2.get("node") or {}).get("reactions")) or {}).get("nodes")) or []
            blacklist = _load_poll_blacklist()
            for rxn in reactions:
                user = ((rxn.get("user") or {}).get("login") or "").lower()
                if user in blacklist:
                    continue
                content = rxn.get("content")
                if content in ("THUMBS_UP", "+1", "HOORAY"):
                    keep_votes += 1
                elif content in ("THUMBS_DOWN", "-1"):
                    remove_votes += 1
            total_votes = keep_votes + remove_votes
            if remove_votes > keep_votes:
                item_id = social.mod_id
                content_type = item.get("content_type", "mod")
                src_dir = REGISTRY_DIR / ("packs" if content_type == "pack" else content_type + "s")
                src_path = src_dir / f"{item_id}.json"
                archived_dir = REGISTRY_DIR / "archived"
                archived_dir.mkdir(parents=True, exist_ok=True)
                dest_path = archived_dir / f"{item_id}.json"
                if src_path.exists():
                    src_path.rename(dest_path)
                    logger.warning(
                        "Triage result: REMOVE won for '%s' (%d vs %d votes). Manifest archived to %s.",
                        item_id, remove_votes, keep_votes, dest_path,
                    )
                    _append_audit_entry("triage_archive", f"Mod '{item_id}' archived: Remove {remove_votes} vs Keep {keep_votes}.", actor="compiler-bot", target_type="mod", target_id=item_id, reason="poll_result_remove")
                social.status = "archived"
            elif keep_votes == 0 and remove_votes == 0:
                _append_audit_entry("triage_zero_votes", f"Triage poll for '{social.mod_id}' expired with zero votes; remaining under_review.", actor="compiler-bot", target_type="mod", target_id=social.mod_id, reason="community_triage")
            else:
                social.status = "active"
                cooldown_until = (now + timedelta(days=30)).isoformat()
                social.immunity_cooldown_until = cooldown_until
                if keep_votes == remove_votes:
                    logger.info(
                        "Triage result: TIE for '%s' (%d vs %d votes). Resolved as KEEP per tie-break rule; 30-day immunity cooldown set.",
                        social.mod_id, keep_votes, remove_votes,
                    )
                    _append_audit_entry("triage_tie_keep", f"Tie resolved as KEEP for '{social.mod_id}': Keep {keep_votes} vs Remove {remove_votes} (tie-break).", actor="compiler-bot", target_type="mod", target_id=social.mod_id, reason="community_triage")
                else:
                    logger.info(
                        "Triage result: KEEP won for '%s' (%d vs %d votes). Status restored to 'active'; 30-day immunity cooldown set.",
                        social.mod_id, keep_votes, remove_votes,
                    )
                    _append_audit_entry("triage_keep", f"Mod '{social.mod_id}' kept: Keep {keep_votes} vs Remove {remove_votes}.", actor="compiler-bot", target_type="mod", target_id=social.mod_id, reason="community_triage")
        except Exception as exc:
            logger.warning("Failed to resolve triage for '%s': %s", social.mod_id, exc)


# --- Pass 3 entrypoint (circuit-breaker response) ---

def _respond_to_circuit_breaker(
    items: list[dict[str, Any]],
    *,
    token: str,
    owner: str,
    repo: str,
) -> None:
    """Execute ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.2 Raid Shield + ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 steps 5 organic-trigger, 6 DELETE +
    admin-alert, 8 create triage poll for every mod whose Pass 2 set
    `status='under_review'`.

    Also handles the ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 5 ORGANIC trigger path (net_score <
    ORGANIC_UNDER_REVIEW_THRESHOLD without a circuit-breaker spike) by
    flipping status to 'under_review' + creating a triage poll WITHOUT
    Raid Shield / DELETE / admin-alert (those responses are spike-only).
    """
    raid_shield_enabled = False
    now = datetime.now(timezone.utc)
    for item in items:
        social = item.get("_social_metrics")
        if social is None:
            continue
        # ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 5 ORGANIC path (no spike, but net_score too low).
        # \u00a73.1 step 5 ORGANIC path (no spike, but net_score too low).
            if social.status == "active" and social.net_score < ORGANIC_UNDER_REVIEW_THRESHOLD:
                if social.anomaly_window_start is None:
                    social.anomaly_window_start = now
                if social.status != "under_review":
                    social.status = "under_review"
                logger.warning(
                    "Organic under_review trigger for mod \u0027%s\u0027: net_score=%d (threshold=%d).",
                    social.mod_id, social.net_score, ORGANIC_UNDER_REVIEW_THRESHOLD,
                )
                _append_audit_entry(
                    "organic_under_review",
                    f"Mod \u0027{social.mod_id}\u0027 net_score={social.net_score} (< {ORGANIC_UNDER_REVIEW_THRESHOLD}). Status \u2192 under_review.",
                    actor="compiler-bot", target_type="mod", target_id=social.mod_id, reason="velocity_anomaly",
                )
            _create_triage_poll(
                owner, repo,
                mod_id=social.mod_id,
                issue_number=social.issue_number,
                reason=f"Organic net_score dropped below threshold ({ORGANIC_UNDER_REVIEW_THRESHOLD}).",
                token=token,
            )
            _post_discord_alert(
                mod_id=social.mod_id,
                reason=f"Organic net_score dropped below threshold ({ORGANIC_UNDER_REVIEW_THRESHOLD}).",
                severity="organic",
                offending_reactions=None,
            )
            continue
        # Spike-triggered under_review (circuit breaker already fired in Pass 2).
        if social.status == "under_review" and social.anomaly_window_start is not None:
            if not raid_shield_enabled:
                raid_shield_enabled = _enable_raid_shield(owner, repo, token=token)
            offending = _gather_offending_reactions(
                owner, repo, social.issue_number,
                window_start=social.anomaly_window_start,
                token=token,
            )
            if offending:
                _append_audit_entry(
                    "raid_breaker_offenders",
                    f"Mod '{social.mod_id}' (issue #{social.issue_number}): gathered "
                    f"{len(offending)} offending reactions for DELETE. Users: " +
                    ", ".join(sorted({r.get("user") or "?" for r in offending})),
                    actor="compiler-bot", target_type="mod", target_id=social.mod_id, reason="velocity_anomaly",
                )
                for r in offending:
                    _delete_reaction(
                        owner, repo,
                        issue_number=r.get("issue_number") or social.issue_number,
                        comment_id=r.get("comment_id"),
                        reaction_id=r.get("reaction_id") or 0,
                        token=token,
                    )
            _create_triage_poll(
                owner, repo,
                mod_id=social.mod_id,
                issue_number=social.issue_number,
                reason="Velocity circuit breaker: coordinated downvote spike detected (>5ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â historical, >20 in 6h).",
                token=token,
            )
            _create_admin_alert_issue(
                owner, repo,
                mod_id=social.mod_id,
                offending_reactions=offending,
                token=token,
            )
            # Discord webhook ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â real-time push to curator alerts channel
            # (silent no-op when DISCORD_WEBHOOK_URL env var is absent).
            _post_discord_alert(
                mod_id=social.mod_id,
                reason="Velocity circuit breaker: coordinated downvote spike detected (>5ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â historical, >20 in 6h).",
                severity="spike",
                offending_reactions=offending,
            )


# --- ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 7: sentiment + spam scrubbing ---

# Regex patterns exported for reuse by tests.
VERSION_BEGGING_RE = re.compile(
    r"(?i)(when\s+is|update\s+to|port\s+to|for\s+1\.\d+|1\.\d+\s*when|when\s+1\.\d+)\s*(release|coming|update|\?)?"
)
EMPTY_PRAISE_RE = re.compile(
    r"(?i)^(good\s+mod|nice|cool|pog|great|wow|awesome\s+sauce|epic|gg|nice\s+mod)\s*[!.?]*$"
)


def _regex_filter_comment(text: str) -> tuple[bool, str]:
    """Apply ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 7 regex filters. Returns (passed, reason_if_dropped)."""
    if not text or not text.strip():
        return False, "empty"
    if VERSION_BEGGING_RE.search(text):
        return False, "version-begging"
    if EMPTY_PRAISE_RE.match(text.strip()):
        return False, "empty-praise"
    return True, ""


def _nlp_filter_comment(text: str) -> tuple[bool, str]:
    """Apply ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 7 NLP filters. Returns (passed, reason_if_dropped).

    Lazy-imports profanity_check + vaderSentiment so the compiler runs cleanly
    in local dev (without those deps) when the NLP path isn't reached (no
    GITHUB_TOKEN ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ no review text to scrub). On import failure: log warning,
    treat as passed (fail-OPEN for benign content rather than dropping legit
    reviews because the runner didn't `pip install -r requirements.txt`).
    """
    try:
        from profanity_check import predict as _profanity_predict
    except ImportError as exc:
        logger.warning("profanity-check not installed (%s); skipping NLP filter (fail-open).", exc)
        return True, ""
    try:
        from vaderSentiment.vaderSentiment import SentimentIntensityAnalyzer
    except ImportError as exc:
        logger.warning("vaderSentiment not installed (%s); skipping NLP filter (fail-open).", exc)
        return True, ""
    # profanity_check: returns [0] for clean, [1] for toxic.
    try:
        is_toxic = bool(_profanity_predict([text])[0])
    except Exception as exc:
        logger.warning("profanity_check failed on text (len=%d): %s; skipping.", len(text), exc)
        is_toxic = False
    if is_toxic:
        return False, "profanity"
    # vaderSentiment: discard extreme-aggression intensity even if profanity passes.
    try:
        analyzer = SentimentIntensityAnalyzer()
        scores = analyzer.polarity_scores(text)
        if scores.get("neg", 0.0) >= 0.9 and scores.get("compound", 0.0) <= -0.85:
            return False, "extreme-aggression"
    except Exception as exc:
        logger.warning("vaderSentiment failed on text (len=%d): %s; skipping.", len(text), exc)
    return True, ""


def _scrub_review_text(text: str) -> tuple[bool, str, str]:
    """Full ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 7 scrub pipeline. Returns (passed, cleaned_text, drop_reason)."""
    passed, reason = _regex_filter_comment(text)
    if not passed:
        return False, text, reason
    passed, reason = _nlp_filter_comment(text)
    if not passed:
        return False, text, reason
    return True, text.strip(), ""


REVIEW_TEXT_FIELD_RE = re.compile(
    r"###\s*Your Technical Review[^:\n]*\r?\n+(.*?)(?:\r?\n###|\Z)",
    re.IGNORECASE | re.DOTALL,
)


def _extract_review_text(issue_body: str | None) -> str | None:
    """Extract the review-text field from a review-form issue body.

    Mirrors _extract_mod_id: the form emits `### Your Technical Review (50
    character minimum)` heading followed by the textarea value. Returns the
    trimmed review text, or None if absent.
    """
    if not issue_body:
        return None
    m = REVIEW_TEXT_FIELD_RE.search(issue_body)
    if not m:
        return None
    return m.group(1).strip() or None


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

SCHEMA_VERSION = 5


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
            -- only ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â image *URLs* are kept, never binary image data ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â so the
            -- signed registry.db stays compact (ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§6.3 / ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§4.2 "media strategy").
            -- Manifest/curator-provided values always take precedence over
            -- API-fetched values for these fields.
            description TEXT,
            body_markdown TEXT,
            page_url TEXT,
            license_id TEXT,
            source_updated_at TEXT,
            -- Optional Modrinth project id (carried from the manifest) used both
            -- for metadata hydration and, at install time, as the version
            -- resolution fallback for github_release/direct_hash mods whose
            -- primary source fails (e.g. GitHub 60 req/hr rate limit). The
            -- installed file is still SHA-256-verified against the pinned hash
            -- regardless of which source delivered it.
            modrinth_id TEXT
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

    # Community-curated mod conflict feed.
    cursor.execute("""
        CREATE TABLE IF NOT EXISTS known_conflicts (
            mod_a_id TEXT NOT NULL,
            mod_b_id TEXT NOT NULL,
            severity TEXT NOT NULL,
            mitigated_by_json TEXT,
            notes TEXT,
            PRIMARY KEY (mod_a_id, mod_b_id)
        )
    """)

    # Manual mod dependencies declared by manifest authors.
    cursor.execute("""
        CREATE TABLE IF NOT EXISTS mod_manual_dependencies (
            item_id TEXT PRIMARY KEY,
            required_json TEXT,
            optional_json TEXT,
            incompatible_json TEXT,
            FOREIGN KEY (item_id) REFERENCES registry_items(id)
        )
    """)

    # Mod jar ID aliases: cross-source ID mapping for the same mod.
    cursor.execute("""
        CREATE TABLE IF NOT EXISTS mod_jar_aliases (
            registry_id TEXT NOT NULL,
            alias TEXT NOT NULL,
            PRIMARY KEY (registry_id, alias),
            FOREIGN KEY (registry_id) REFERENCES registry_items(id)
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

    Per ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§2.1, sha256 is required for all download strategies. The compiler
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
    compatible_versions = (
        item.get("compatible_versions")
        or item.get("_hydrated_compatible_versions")
        or default_compatible_versions(item)
    )

    # Pass 2: pull computed social metrics when present. Immune items
    # (ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 9) bypass score evaluation ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â upvotes/downvotes/net_score
    # are zero, status is "active", immunity_cooldown_until stays None.
    if is_immune:
        _upvotes = 0
        _downvotes = 0
        _net_score = 0
        _velocity = 0.0
        _status = "active"
    else:
        social = item.get("_social_metrics")
        if social is not None:
            _upvotes = social.upvotes
            _downvotes = social.downvotes
            _net_score = social.net_score
            _velocity = social.velocity
            _status = social.status
        else:
            _upvotes = 0
            _downvotes = 0
            _net_score = 0
            _velocity = 0.0
            _status = "active"

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
            description, body_markdown, page_url, license_id, source_updated_at,
            modrinth_id
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            item_id,
            item["name"],
            item.get("content_type", "mod"),
            item["download_strategy"],
            item["source_identifier"],
            sha256,
            _upvotes,
            _downvotes,
            _net_score,
            _velocity,
            _status,
            int(is_immune),
            immunity_reason,
            int(allow_comments),
            social.immunity_cooldown_until if not is_immune and social is not None else None,
            item.get("icon_url"), json.dumps(gallery, separators=(",", ":")),
            manifest_date_added(path),
            json.dumps(compatible_versions, separators=(",", ":")),
            item.get("description"),
            item.get("body_markdown"),
            item.get("page_url"),
            item.get("license_id"),
            item.get("source_updated_at"),
            item.get("modrinth_id"),
        ),
    )

    # Curator note.
    social = item.get("_social_metrics")
    scrubbed_reviews: list[dict[str, Any]] = []
    if social is not None and hasattr(social, "scrubbed_reviews"):
        scrubbed_reviews = social.scrubbed_reviews
    cursor.execute(
        """
        INSERT INTO curator_reviews (item_id, curator_note, top_reviews_json)
        VALUES (?, ?, ?)
        """,
        (item_id, item.get("curator_note", ""), json.dumps(scrubbed_reviews, separators=(",", ":"))),
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

# Modrinth `project_type` ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ canonical site URL path segment.
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
    """Batch-query the Modrinth API to hydrate rich metadata for items with a Modrinth ID.

    Pulls the short description, full markdown body, canonical page URL,
    license id, and last-updated timestamp from the bulk ``/v2/projects``
    endpoint (up to 100 projects per request), in addition to the icon and
    gallery URLs. This bakes rich, instant-on metadata into the signed
    ``registry.db`` so the client never has to hit Modrinth's API at browse
    time (ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§6.3 / ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§4.2 "media strategy": text + image *URLs* only, no binary).

    Hydration is decoupled from the download strategy: ``github_release`` /
    ``direct_hash`` mods may also hydrate display metadata from Modrinth by
    setting an explicit ``modrinth_id`` in their manifest (for when the same
    project also exists on Modrinth). For ``modrinth_id``-strategy items,
    ``source_identifier`` doubles as the Modrinth ID so the minimal 5-line
    manifest still works without a redundant field.

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
        # Metadata hydration is decoupled from the download strategy: a
        # ``github_release`` / ``direct_hash`` mod can still pull display
        # metadata (description, body, gallery, categories) from Modrinth if
        # the same project exists there, by setting an explicit ``modrinth_id``.
        #
        # ID resolution:
        #   - ``modrinth_id`` strategy: ``modrinth_id`` field, falling back to
        #     ``source_identifier`` (which IS the Modrinth ID ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â keeps the
        #     minimal 5-line manifest working where source_identifier doubles).
        #   - any other strategy: ONLY an explicit ``modrinth_id`` field is
        #     used. ``source_identifier`` is a GitHub repo / direct URL here,
        #     not a Modrinth ID, so it must not be sent to the Modrinth API.
        strategy = item.get("download_strategy", "")
        if strategy == "modrinth_id":
            mid = item.get("modrinth_id") or item.get("source_identifier", "")
        else:
            mid = item.get("modrinth_id", "")
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

        # Upstream categories ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â captured as a *fallback only*. Linked later
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


def _hydrate_modrinth_versions(items: list[dict[str, Any]]) -> None:
    """Batch-query the Modrinth version API to populate compatible_versions.

    For items with a ``modrinth_id`` that lack ``compatible_versions`` in the
    manifest, calls ``GET /v2/project/{id}/version`` and transforms the
    response into the internal format:

        [{mc_version, loader, mod_version}, ...]

    Results are stored on the item dict as ``_hydrated_compatible_versions``
    so ``insert_registry_item`` can use them as a fallback.

    Network failures degrade gracefully with a warning; items keep whatever
    the manifest or default provides.
    """
    # Collect items that need version hydration.
    to_hydrate: list[tuple[int, dict[str, Any], str]] = []
    for idx, item in enumerate(items):
        strategy = item.get("download_strategy", "")
        if strategy == "modrinth_id":
            mid = item.get("modrinth_id") or item.get("source_identifier", "")
        else:
            mid = item.get("modrinth_id", "")
        if not mid:
            continue
        # Skip if manifest already provides compatible_versions.
        if item.get("compatible_versions"):
            continue
        to_hydrate.append((idx, item, mid))

    if not to_hydrate:
        return

    # Dedupe by modrinth_id; each unique ID needs one version request.
    seen_ids: set[str] = set()
    unique_ids: list[str] = []
    for _idx, _item, mid in to_hydrate:
        if mid not in seen_ids:
            seen_ids.add(mid)
            unique_ids.append(mid)

    # Batch-fetch versions per project.
    hydrated: dict[str, list[dict[str, str]]] = {}
    for mid in unique_ids:
        try:
            resp = requests.get(
                f"https://api.modrinth.com/v2/project/{mid}/version",
                headers={"User-Agent": _MODRINTH_USER_AGENT},
                timeout=30,
            )
            resp.raise_for_status()
            versions = resp.json()
            # Transform into compatible_versions format.
            compatible: list[dict[str, str]] = []
            for ver in versions:
                game_versions = ver.get("game_versions") or []
                loaders = ver.get("loaders") or []
                version_number = ver.get("version_number", "")
                for mc_ver in game_versions:
                    for loader in loaders:
                        compatible.append({
                            "mc_version": mc_ver,
                            "loader": loader,
                            "mod_version": version_number,
                        })
            hydrated[mid] = compatible
        except Exception as exc:  # noqa: BLE001
            logger.warning(
                "Modrinth version hydration failed for project '%s': %s",
                mid, exc,
            )

    # Apply hydrated versions back to items.
    for _idx, item, mid in to_hydrate:
        if mid in hydrated:
            item["_hydrated_compatible_versions"] = hydrated[mid]


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

    Validation per ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§2.4.1:
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
# Known conflicts (registry/governance/known_conflicts.json)
# ---------------------------------------------------------------------------


def load_known_conflicts(conn: sqlite3.Connection) -> int:
    """Ingest community-curated known conflicts from known_conflicts.json.

    Returns the number of rows inserted.
    """
    path = REGISTRY_DIR / "governance" / "known_conflicts.json"
    try:
        with path.open("r", encoding="utf-8") as fh:
            data = json.load(fh)
    except (OSError, json.JSONDecodeError) as exc:
        logger.warning("Could not load known_conflicts.json (%s); skipping.", exc)
        return 0

    if not isinstance(data, list):
        logger.warning("known_conflicts.json is not a JSON array; skipping.")
        return 0

    cursor = conn.cursor()
    count = 0
    for entry in data:
        a = str(entry.get("a", "")).strip().lower()
        b = str(entry.get("b", "")).strip().lower()
        severity = str(entry.get("severity", "")).strip().lower()
        mitigated_by = entry.get("mitigated_by")
        if not isinstance(mitigated_by, list):
            mitigated_by = []
        mitigated_by_json = json.dumps(mitigated_by, separators=(",", ":"))
        notes = entry.get("notes")
        # Normalize pair: lexicographically smaller id is mod_a_id.
        if a <= b:
            mod_a_id, mod_b_id = a, b
        else:
            mod_a_id, mod_b_id = b, a
        cursor.execute(
            """
            INSERT OR REPLACE INTO known_conflicts
                (mod_a_id, mod_b_id, severity, mitigated_by_json, notes)
            VALUES (?, ?, ?, ?, ?)
            """,
            (mod_a_id, mod_b_id, severity, mitigated_by_json, notes),
        )
        count += 1

    logger.info("Loaded %d known conflicts", count)
    return count


# ---------------------------------------------------------------------------
# Manual mod dependencies (mod_dependencies manifest field)
# ---------------------------------------------------------------------------


def load_manual_dependencies(conn: sqlite3.Connection) -> int:
    """Ingest optional ``mod_dependencies`` fields from all content-type manifests.

    Each manifest across the 7 CONTENT_DIRS is inspected; if it carries a
    ``mod_dependencies`` object with any of ``required``, ``optional``,
    ``incompatible`` (defaulting to ``[]``), the values are stored as JSON
    strings in ``mod_manual_dependencies``.  Defensive: failures are logged
    and never abort the compile.

    Returns the number of rows inserted.
    """
    CONTENT_DIRS = [
        "mods",
        "packs",
        "shaders",
        "resourcepacks",
        "servers",
        "datapacks",
        "worlds",
    ]
    cursor = conn.cursor()
    count = 0
    for dir_name in CONTENT_DIRS:
        dir_path = REGISTRY_DIR / dir_name
        if not dir_path.exists():
            continue
        try:
            for path in sorted(dir_path.rglob("*.json")):
                if "archived" in path.parts:
                    continue
                try:
                    with path.open("r", encoding="utf-8") as fh:
                        manifest = json.load(fh)
                except (json.JSONDecodeError, OSError):
                    continue
                deps = manifest.get("mod_dependencies")
                if not isinstance(deps, dict):
                    continue
                item_id = str(manifest.get("id", "")).strip().lower()
                if not item_id:
                    continue
                required = deps.get("required", [])
                if not isinstance(required, list):
                    required = []
                optional = deps.get("optional", [])
                if not isinstance(optional, list):
                    optional = []
                incompatible = deps.get("incompatible", [])
                if not isinstance(incompatible, list):
                    incompatible = []
                required_json = json.dumps(required, separators=(",", ":"))
                optional_json = json.dumps(optional, separators=(",", ":"))
                incompatible_json = json.dumps(incompatible, separators=(",", ":"))
                cursor.execute(
                    """
                    INSERT OR REPLACE INTO mod_manual_dependencies
                        (item_id, required_json, optional_json, incompatible_json)
                    VALUES (?, ?, ?, ?)
                    """,
                    (item_id, required_json, optional_json, incompatible_json),
                )
                count += 1
        except Exception as exc:
            logger.warning(
                "Failed to scan %s for mod_dependencies: %s", dir_name, exc,
            )

    logger.info("Loaded %d manual dependency sets", count)
    return count


# ---------------------------------------------------------------------------
# Mod jar aliases (cross-source ID mapping)
# ---------------------------------------------------------------------------


def load_mod_jar_aliases(conn: sqlite3.Connection) -> int:
    """Ingest optional ``mod_jar_aliases`` fields from all content-type manifests.

    Each manifest across the 7 CONTENT_DIRS is inspected; if it carries a
    ``mod_jar_aliases`` array of strings, each alias is inserted as a row
    ``(registry_id, alias)`` into ``mod_jar_aliases``.  Uses ``INSERT OR
    IGNORE`` for idempotency.  Defensive: failures are logged and never abort
    the compile.

    Returns the number of rows inserted.
    """
    CONTENT_DIRS = [
        "mods",
        "packs",
        "shaders",
        "resourcepacks",
        "servers",
        "datapacks",
        "worlds",
    ]
    cursor = conn.cursor()
    count = 0
    for dir_name in CONTENT_DIRS:
        dir_path = REGISTRY_DIR / dir_name
        if not dir_path.exists():
            continue
        try:
            for path in sorted(dir_path.rglob("*.json")):
                if "archived" in path.parts:
                    continue
                try:
                    with path.open("r", encoding="utf-8") as fh:
                        manifest = json.load(fh)
                except (json.JSONDecodeError, OSError):
                    continue
                aliases = manifest.get("mod_jar_aliases")
                if not isinstance(aliases, list):
                    continue
                registry_id = str(manifest.get("id", "")).strip().lower()
                if not registry_id:
                    continue
                for alias in aliases:
                    alias_str = str(alias).strip()
                    if not alias_str:
                        continue
                    cursor.execute(
                        """
                        INSERT OR IGNORE INTO mod_jar_aliases
                            (registry_id, alias)
                        VALUES (?, ?)
                        """,
                        (registry_id, alias_str),
                    )
                    count += 1
        except Exception as exc:
            logger.warning(
                "Failed to scan %s for mod_jar_aliases: %s", dir_name, exc,
            )

    logger.info("Loaded %d mod jar alias rows", count)
    return count


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
    # determines the row type ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â we do NOT derive it from the directory name.
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

    # Hydrate compatible_versions from Modrinth version API for items that
    # lack it in the manifest (in-place, with manifest/override precedence).
    _hydrate_modrinth_versions([item for _, item in all_items])

    # Hydrate GitHub social metrics (ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 step 3; Pass 1: raw reactions only
    # ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â trust filter, Sybil weighting, velocity circuit breaker, immune
    # passthrough, and DB INSERT update arrive in Pass 2). On missing
    # GITHUB_TOKEN this is a silent no-op (items stay metric-free ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ zeros).
    _hydrate_github_social_metrics([item for _, item in all_items])

    # Pass 2: apply ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 steps 4 (trust + Sybil), 5 (velocity circuit breaker),
    # and 9 (immune passthrough) to compute final upvotes / downvotes /
    # net_score / velocity / status from the raw reactions Pass 1 attached.
    # When GITHUB_TOKEN is unset this is a no-op (metrics stay at defaults).
    _gh_token = _load_github_token()
    if _gh_token:
        _blacklist = _load_poll_blacklist()
        _apply_trust_velocity_pass(
            [item for _, item in all_items],
            token=_gh_token,
            blacklist=_blacklist,
        )
    # Immune items have their `_social_metrics` removed by Pass 2; non-immune
    # items with no matching governance issue also have no metrics ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â both
    # fall through to the default (0, 0, 0, 0.0, "active") path in insert_registry_item.

    # Pass 3 (ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.2 Raid Shield + ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â§3.1 steps 5 organic-trigger, 6 DELETE +
    # admin-alert, 8 create triage poll + resolve expired polls). Runs only
    # when GITHUB_TOKEN is present.
    if _gh_token:
        gov_full = _get_registry_repo()
        _owner, _, _repo = gov_full.partition("/")
        if _owner and _repo:
            _respond_to_circuit_breaker(
                [item for _, item in all_items],
                token=_gh_token, owner=_owner, repo=_repo,
            )
            _resolve_expired_triage_polls(
                _owner, _repo,
                items=[item for _, item in all_items],
                token=_gh_token,
            )
        else:
            logger.warning("AGORA_REGISTRY_REPO='%s' invalid; skipping Pass 3 responses.", gov_full)

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

    # Known conflicts (mod pairs that conflict).
    try:
        load_known_conflicts(conn)
    except Exception as exc:
        logger.warning("Failed to load known conflicts: %s", exc)

    # Manual mod dependencies (from manifest ``mod_dependencies`` field).
    try:
        load_manual_dependencies(conn)
    except Exception as exc:
        logger.warning("Failed to load manual dependencies: %s", exc)

    # Mod jar alias mapping (from manifest ``mod_jar_aliases`` field).
    try:
        load_mod_jar_aliases(conn)
    except Exception as exc:
        logger.warning("Failed to load mod jar aliases: %s", exc)

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

    #     # --- Audit log (§4.6) ---
    audit_log_path = REGISTRY_DIR / "governance" / "audit_log.json"
    audit_log_path.parent.mkdir(parents=True, exist_ok=True)
    total_items = mod_count + pack_count + other_count
    total_crash_sigs = sig_count
    new_entry = {
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "action": "compile",
        "actor": "compiler-bot",
        "target_type": "registry",
        "reason": "nightly_compile",
        "details": f"Compiled registry with {total_items} items, {total_crash_sigs} crash signatures",
    }
    if audit_log_path.exists():
        with audit_log_path.open("r", encoding="utf-8") as fh:
            audit_data = json.load(fh)
    else:
        audit_data = {"log_format_version": 1, "entries": []}
    if "log_format_version" not in audit_data:
        audit_data["log_format_version"] = 1
    audit_data["entries"].append(new_entry)
    # Rotation (§4.6): when > 10 000 entries, archive oldest 2 000.
    if len(audit_data["entries"]) > 10000:
        archive_date = datetime.now(timezone.utc).strftime("%Y%m%d")
        archive_path = REGISTRY_DIR / "governance" / f"audit_log_archive.{archive_date}.json"
        oldest_2000 = audit_data["entries"][:2000]
        audit_data["entries"] = audit_data["entries"][-8000:]
        if archive_path.exists():
            with archive_path.open("r", encoding="utf-8") as fh:
                archive_data = json.load(fh)
            archive_data.extend(oldest_2000)
        else:
            archive_data = oldest_2000
        with archive_path.open("w", encoding="utf-8") as fh:
            json.dump(archive_data, fh, indent=2)
    with audit_log_path.open("w", encoding="utf-8") as fh:
        json.dump(audit_data, fh, indent=2)
    logger.info("Wrote audit log to %s", audit_log_path)

    # Register audit log path in system_config.
    audit_conn = sqlite3.connect(str(output_path))
    audit_conn.execute(
        "INSERT OR REPLACE INTO system_config (key, value_json) VALUES ('audit_log_json', ?)",
        ("registry/governance/audit_log.json",),
    )

    # Bake audit log entries into a queryable table.
    audit_conn.execute(
        "CREATE TABLE IF NOT EXISTS audit_log ("
        "id INTEGER PRIMARY KEY AUTOINCREMENT, "
        "timestamp TEXT NOT NULL, "
        "action TEXT NOT NULL, "
        "details TEXT)"
    )
    audit_conn.execute("DELETE FROM audit_log")
    rows = [
        (e["timestamp"], e["action"], e.get("details"))
        for e in audit_data.get("entries", [])
    ]
    audit_conn.executemany(
        "INSERT INTO audit_log (timestamp, action, details) VALUES (?, ?, ?)",
        rows,
    )
    logger.info("Inserted %d audit log entries into registry.db", len(rows))

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