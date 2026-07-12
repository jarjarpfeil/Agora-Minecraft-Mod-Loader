//! Governance network actions: live triage-poll fetching and comment-flag
//! submission with rate limiting (Â§5.5 / Â§5.6).
//!
//! Pure logic is in `agora_core::governance`. This module provides Tauri-coupled
//! wrappers that resolve `AppHandle` to connections / auth tokens.

use crate::auth;
use crate::db;
use crate::error::{LauncherError, LauncherResult};

pub use agora_core::governance::{TriagePoll, AGORA_ADMIN_ALERTS_REPO, AGORA_GOVERNANCE_REPO};

use tauri::AppHandle;

// --- Triage poll ---

/// Fetch the live triage poll for `mod_id` from GitHub Discussions.
///
/// Searches for a "[Community Triage]" discussion matching the mod_id, then
/// tallies reaction votes (thumbs-up/+1/hooray â†’ keep, thumbs-down/-1 â†’ remove).
pub async fn fetch_triage_poll(app: &AppHandle, mod_id: String) -> LauncherResult<TriagePoll> {
    let token = auth::get_token(app).ok_or(LauncherError::AuthRequired)?;

    let _permit = agora_core::github_ratelimit::acquire_github_permit().await;

    // Step 1: search for the triage discussion by title pattern.
    let search_query = format!(
        "repo:{owner}/{repo} [Community Triage] {mod_id}",
        owner = AGORA_GOVERNANCE_REPO.split('/').next().unwrap_or(""),
        repo = AGORA_GOVERNANCE_REPO.split('/').nth(1).unwrap_or(""),
        mod_id = mod_id,
    );

    let search_body = serde_json::json!({
        "query": r#"
            query ($q: String!) {
              search(query: $q, type: DISCUSSION, first: 5) {
                nodes {
                  ... on Discussion {
                    id
                    url
                    title
                  }
                }
              }
            }
        "#,
        "variables": { "q": search_query },
    });

    let resp = agora_core::github_ratelimit::github_client()
        .post("https://api.github.com/graphql")
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "agora-launcher")
        .header("Content-Type", "application/json")
        .json(&search_body)
        .send()
        .await
        .map_err(|_| LauncherError::NetworkOffline)?;

    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        crate::auth::log_line("GitHub token expired during triage poll search; clearing token");
        let _ = auth::clear_token(app);
        return Err(LauncherError::AuthExpired);
    }
    if agora_core::github_ratelimit::is_rate_limit_response(&resp) {
        let retry = agora_core::github_ratelimit::parse_retry_after(&resp);
        agora_core::github_ratelimit::report_rate_limit(retry).await;
        return Err(LauncherError::Generic {
            code: "ERR_RATE_LIMITED".to_string(),
            message: format!("GitHub rate limited triage poll for {mod_id}."),
        });
    }
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(LauncherError::Generic {
            code: "ERR_TRIAGE_POLL".to_string(),
            message: format!("Triage poll search failed (status {status}): {body}"),
        });
    }

    #[derive(Debug, serde::Deserialize)]
    struct SearchResponse {
        search: Option<SearchPayload>,
    }

    #[derive(Debug, serde::Deserialize)]
    struct SearchPayload {
        nodes: Option<Vec<DiscussionNode>>,
    }

    #[derive(Debug, serde::Deserialize)]
    struct DiscussionNode {
        id: String,
        url: String,
        title: String,
    }

    let search_resp: SearchResponse = resp.json().await.map_err(|_| LauncherError::Generic {
        code: "ERR_TRIAGE_POLL".to_string(),
        message: "Failed to parse triage poll search response.".to_string(),
    })?;

    let nodes = search_resp.search.and_then(|s| s.nodes).unwrap_or_default();

    // Find the first discussion whose title contains the mod_id.
    let discussion = nodes
        .into_iter()
        .find(|d| d.title.contains(&mod_id))
        .ok_or_else(|| LauncherError::Generic {
            code: "ERR_TRIAGE_POLL".to_string(),
            message: format!("No triage discussion found for mod '{mod_id}'."),
        })?;

    // Step 2: fetch reactions for the discussion.
    let reactions_body = serde_json::json!({
        "query": r#"
            query ($id: ID!) {
              node(id: $id) {
                ... on Discussion {
                  url
                  reactions(first: 100) {
                    nodes {
                      user { login }
                      content
                    }
                  }
                }
              }
            }
        "#,
        "variables": { "id": discussion.id },
    });

    let resp2 = agora_core::github_ratelimit::github_client()
        .post("https://api.github.com/graphql")
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "agora-launcher")
        .header("Content-Type", "application/json")
        .json(&reactions_body)
        .send()
        .await
        .map_err(|_| LauncherError::NetworkOffline)?;

    if resp2.status() == reqwest::StatusCode::UNAUTHORIZED {
        crate::auth::log_line("GitHub token expired during triage poll reactions; clearing token");
        let _ = auth::clear_token(app);
        return Err(LauncherError::AuthExpired);
    }
    if agora_core::github_ratelimit::is_rate_limit_response(&resp2) {
        let retry = agora_core::github_ratelimit::parse_retry_after(&resp2);
        agora_core::github_ratelimit::report_rate_limit(retry).await;
        return Err(LauncherError::Generic {
            code: "ERR_RATE_LIMITED".to_string(),
            message: format!("GitHub rate limited triage poll reactions for {mod_id}."),
        });
    }
    if !resp2.status().is_success() {
        let status = resp2.status();
        let body = resp2.text().await.unwrap_or_default();
        return Err(LauncherError::Generic {
            code: "ERR_TRIAGE_POLL".to_string(),
            message: format!("Triage poll reactions failed (status {status}): {body}"),
        });
    }

    #[derive(Debug, serde::Deserialize)]
    struct ReactionsResponse {
        node: Option<ReactionsPayload>,
    }

    #[derive(Debug, serde::Deserialize)]
    struct ReactionsPayload {
        reactions: Option<ReactionsPayloadInner>,
    }

    #[derive(Debug, serde::Deserialize)]
    struct ReactionsPayloadInner {
        nodes: Option<Vec<ReactionNode>>,
    }

    #[derive(Debug, serde::Deserialize)]
    struct ReactionNode {
        content: String,
    }

    let reactions_resp: ReactionsResponse =
        resp2.json().await.map_err(|_| LauncherError::Generic {
            code: "ERR_TRIAGE_POLL".to_string(),
            message: "Failed to parse triage poll reactions response.".to_string(),
        })?;

    let (keep_votes, remove_votes) = reactions_resp
        .node
        .and_then(|n| n.reactions)
        .and_then(|r| r.nodes)
        .map(|nodes| {
            let mut keep = 0i64;
            let mut remove = 0i64;
            for rxn in nodes {
                match rxn.content.as_str() {
                    "THUMBS_UP" | "+1" | "HOORAY" => keep += 1,
                    "THUMBS_DOWN" | "-1" => remove += 1,
                    _ => {}
                }
            }
            (keep, remove)
        })
        .unwrap_or((0, 0));

    Ok(TriagePoll {
        discussion_url: Some(discussion.url),
        keep_votes,
        remove_votes,
    })
}

// --- Comment flag submission ---

/// Submit a comment flag for moderation review.
///
/// Creates an issue in the admin-alerts repo with the flagged content, then
/// records the submission for rate-limit tracking.
pub async fn flag_review(
    app: &AppHandle,
    mod_id: String,
    mod_name: String,
    issue_number: i64,
    author: String,
    quoted_text: String,
    reporter_login: String,
) -> LauncherResult<String> {
    let token = auth::get_token(app).ok_or(LauncherError::AuthRequired)?;

    // Rate-limit check.
    let conn = db::local_state_connection(app).map_err(|_| LauncherError::LocalStateFailed)?;
    let now_unix = chrono::Utc::now().timestamp();
    let rate_limit = db::get_flag_rate_limit_status(&conn, now_unix)
        .map_err(|_| LauncherError::LocalStateFailed)?;

    if !rate_limit.can_flag {
        let reset_unix = if rate_limit.remaining_hour <= 0 {
            rate_limit.reset_hour_at_unix
        } else {
            rate_limit.reset_day_at_unix
        };
        let limit_type = if rate_limit.remaining_hour <= 0 {
            "hourly"
        } else {
            "daily"
        };
        let reset_iso = chrono::DateTime::<chrono::Utc>::from_timestamp(reset_unix, 0)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_else(|| reset_unix.to_string());
        return Err(LauncherError::Generic {
            code: "ERR_RATE_LIMITED".to_string(),
            message: format!("{limit_type} flag limit reached. Resets at <{reset_iso}>."),
        });
    }

    let _permit = agora_core::github_ratelimit::acquire_github_permit().await;

    let tracking_url = format!(
        "https://github.com/{owner}/{repo}/issues/{num}",
        owner = AGORA_GOVERNANCE_REPO.split('/').next().unwrap_or(""),
        repo = AGORA_GOVERNANCE_REPO.split('/').nth(1).unwrap_or(""),
        num = issue_number,
    );

    let body_json = serde_json::json!({
        "title": format!("[REPORT] Review on mod: {}", mod_name),
        "body": format!(
            "Flagged comment report\n\
            \n\
            **Mod ID:** `{mod_id}`\n\
            **Mod Name:** {mod_name}\n\
            **Tracking Issue:** {tracking_url}\n\
            **Reported by:** {reporter_login}\n\
            **Review Author:** {author}\n\
            \n\
            > {quoted_text}",
        ),
        "labels": ["triage", "comment-report"],
    });

    let issues_url = format!(
        "https://api.github.com/repos/{owner}/{repo}/issues",
        owner = AGORA_ADMIN_ALERTS_REPO.split('/').next().unwrap_or(""),
        repo = AGORA_ADMIN_ALERTS_REPO.split('/').nth(1).unwrap_or(""),
    );

    let resp = agora_core::github_ratelimit::github_client()
        .post(&issues_url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "agora-launcher")
        .header("Content-Type", "application/json")
        .json(&body_json)
        .send()
        .await
        .map_err(|_| LauncherError::NetworkOffline)?;

    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        crate::auth::log_line("GitHub token expired during flag submission; clearing token");
        let _ = auth::clear_token(app);
        return Err(LauncherError::AuthExpired);
    }
    if agora_core::github_ratelimit::is_rate_limit_response(&resp) {
        let retry = agora_core::github_ratelimit::parse_retry_after(&resp);
        agora_core::github_ratelimit::report_rate_limit(retry).await;
        return Err(LauncherError::Generic {
            code: "ERR_RATE_LIMITED".to_string(),
            message: format!("GitHub rate limited the flag submission."),
        });
    }

    if resp.status() != reqwest::StatusCode::CREATED {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(LauncherError::Generic {
            code: "ERR_FLAG_REVIEW".to_string(),
            message: format!("Flag review failed (status {status}): {body}"),
        });
    }

    #[derive(Debug, serde::Deserialize)]
    struct IssueResponse {
        html_url: String,
    }

    let issue_resp: IssueResponse = resp.json().await.map_err(|_| LauncherError::Generic {
        code: "ERR_FLAG_REVIEW".to_string(),
        message: "Failed to parse flag review response.".to_string(),
    })?;

    // Record the submission for rate-limit tracking.
    if let Ok(conn) = db::local_state_connection(app) {
        let _ = db::record_flag_submission(&conn, now_unix);
    }

    Ok(issue_resp.html_url)
}

// --- Rate limit status ---

/// Return the current flag rate-limit status for the local state database.
pub fn get_flag_rate_limit(app: &AppHandle) -> LauncherResult<agora_core::db::FlagRateLimit> {
    let conn = db::local_state_connection(app).map_err(|_| LauncherError::LocalStateFailed)?;
    agora_core::governance::get_flag_rate_limit(&conn)
}
