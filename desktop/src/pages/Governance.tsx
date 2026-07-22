import { useState, useEffect } from 'react';
import {
  listAuditLog,
  formatError,
  AuditLogEntry,
  listUnderReviewItems,
  UnderReviewItem,
  fetchTriagePoll,
  TriagePoll,
  listRecentResolutions,
  getAuthStatus,
  isAuthExpired,
} from '../lib/tauri';

export function Governance() {
  // Transparency log state
  const [logEntries, setLogEntries] = useState<AuditLogEntry[]>([]);
  const [logLoading, setLogLoading] = useState(true);
  const [logError, setLogError] = useState<string | null>(null);

  // Active Triage Polls state
  const [underReviewItems, setUnderReviewItems] = useState<UnderReviewItem[]>([]);
  const [polls, setPolls] = useState<Record<string, TriagePoll | null>>({});
  const [pollsLoading, setPollsLoading] = useState(true);
  const [pollsError, setPollsError] = useState<string | null>(null);
  const [authenticated, setAuthenticated] = useState<boolean | null>(null);

  // Recent Resolutions state
  const [resolutions, setResolutions] = useState<AuditLogEntry[]>([]);
  const [resolutionsLoading, setResolutionsLoading] = useState(true);
  const [resolutionsError, setResolutionsError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    // --- Transparency log ---
    listAuditLog(200)
      .then((data) => {
        if (!cancelled) setLogEntries(data);
      })
      .catch((e) => {
        if (!cancelled) setLogError(formatError(e));
      })
      .finally(() => {
        if (!cancelled) setLogLoading(false);
      });

    // --- Auth check ---
    getAuthStatus()
      .then((auth) => {
        if (!cancelled) setAuthenticated(auth);
      })
      .catch(() => {
        if (!cancelled) setAuthenticated(false);
      });

    // --- Under-review items ---
    listUnderReviewItems()
      .then((items) => {
        if (!cancelled) setUnderReviewItems(items);
      })
      .catch((e) => {
        if (!cancelled) setPollsError(formatError(e));
      })
      .finally(() => {
        if (!cancelled) setPollsLoading((prev) => !prev);
      });

    // --- Recent resolutions ---
    listRecentResolutions(50)
      .then((data) => {
        if (!cancelled) setResolutions(data);
      })
      .catch((e) => {
        if (!cancelled) setResolutionsError(formatError(e));
      })
      .finally(() => {
        if (!cancelled) setResolutionsLoading(false);
      });

    return () => {
      cancelled = true;
    };
  }, []);

  // Fetch polls for each under-review item (only if authenticated)
  useEffect(() => {
    if (!authenticated || underReviewItems.length === 0) return;

    const cancelled = false;
    const fetchPolls = async () => {
      const results: Record<string, TriagePoll | null> = {};
      const errors: string[] = [];

      let sawAuthExpired = false;
      await Promise.all(
        underReviewItems.map(async (item) => {
          try {
            results[item.id] = await fetchTriagePoll(item.id);
          } catch (e) {
            if (isAuthExpired(e)) sawAuthExpired = true;
            errors.push(formatError(e));
            results[item.id] = null;
          }
        }),
      );

      if (!cancelled) {
        if (sawAuthExpired) {
          // Token was cleared by the backend; re-check auth state.
          setAuthenticated(false);
        }
        setPolls(results);
        if (errors.length > 0) {
          setPollsError(errors.join('; '));
        }
        setPollsLoading(false);
      }
    };

    fetchPolls();

    return () => {
      // cancelled is captured above; we set a ref-like flag
    };
  }, [authenticated, underReviewItems]);

  // --- Helpers ---

  const actionBadgeColor = (action: string): string => {
    switch (action) {
      case 'triage_archive':
        return 'bg-destructive/20 text-destructive';
      case 'triage_keep':
        return 'bg-green-200 dark:bg-green-900 text-green-800 dark:text-green-200';
      case 'organic_under_review':
        return 'bg-orange-200 dark:bg-orange-900 text-orange-800 dark:text-orange-200';
      case 'raid_breaker_offenders':
        return 'bg-destructive/20 text-destructive';
      default:
        return 'bg-muted';
    }
  };

  const openDiscussion = (url: string | null) => {
    if (url && url.startsWith('https://')) {
      window.open(url, '_blank');
    }
  };

  return (
    <div className="space-y-6">
      <section>
        <h2 className="text-2xl font-bold mb-2">Community Governance</h2>
        <p className="text-muted-foreground">
          Active triage polls, recent resolutions, and the transparency log.
        </p>
      </section>

      {/* Auth banner for unauthenticated users */}
      {authenticated === false && (
        <div className="rounded-xl p-4 border border-dashed border-border bg-muted text-center">
          <p className="text-muted-foreground">
            Sign in with GitHub to see live triage poll results.
          </p>
        </div>
      )}

      {/* Active Triage Polls */}
      <section className="rounded-xl border border-border bg-card p-6">
        <h3 className="text-lg font-semibold mb-4">Active Triage Polls</h3>

        {pollsError && (
          <div className="mb-4 p-4 rounded-lg bg-destructive/10 border border-destructive text-destructive">
            {pollsError}
          </div>
        )}

        {pollsLoading && (
          <p className="text-muted-foreground">Loading triage polls…</p>
        )}

        {!pollsLoading && !pollsError && underReviewItems.length === 0 && (
          <p className="text-muted-foreground">No items under review.</p>
        )}

        {!pollsLoading && !pollsError && underReviewItems.length > 0 && (
          <div className="space-y-3">
            {underReviewItems.map((item) => {
              const poll = polls[item.id] ?? null;
              const totalVotes = (poll?.keep_votes ?? 0) + (poll?.remove_votes ?? 0);
              const keepPct =
                totalVotes > 0
                  ? Math.round(((poll?.keep_votes ?? 0) / totalVotes) * 100)
                  : 0;
              const removePct = 100 - keepPct;
              const canViewPoll = authenticated && poll !== null;

              return (
                <div
                  key={item.id}
                  className="p-4 rounded-lg bg-muted border border-border"
                >
                  <div className="flex items-start gap-3">
                    {item.icon_url && (
                      <img
                        src={item.icon_url}
                        alt=""
                        className="w-8 h-8 rounded flex-shrink-0"
                      />
                    )}
                    <div className="flex-1 min-w-0">
                      <div className="flex items-center gap-2 flex-wrap">
                        <span className="font-medium truncate">{item.name}</span>
                        <span className="text-xs px-2 py-0.5 rounded-full bg-muted text-muted-foreground capitalize">
                          {item.content_type}
                        </span>
                        <span className="text-xs px-2 py-0.5 rounded-full bg-blue-100 dark:bg-blue-900 text-blue-800 dark:text-blue-200">
                          Score: {item.net_score}
                        </span>
                      </div>

                      {/* Poll bars or placeholder */}
                      {canViewPoll && totalVotes > 0 && (
                        <div className="mt-3">
                          <div className="flex rounded-full overflow-hidden h-3 bg-muted">
                            <div
                              className="bg-green-500"
                              style={{ width: `${keepPct}%` }}
                            />
                            <div
                              className="bg-red-500"
                              style={{ width: `${removePct}%` }}
                            />
                          </div>
                          <div className="flex justify-between text-xs text-muted-foreground mt-1">
                            <span>Keep {keepPct}%</span>
                            <span>Remove {removePct}%</span>
                          </div>
                        </div>
                      )}

                      {canViewPoll && totalVotes === 0 && (
                        <p className="mt-2 text-sm text-muted-foreground">
                          No votes yet.
                        </p>
                      )}

                      {!authenticated && (
                        <p className="mt-2 text-sm text-muted-foreground">
                          Sign in to view live results
                        </p>
                      )}

                      {/* Vote button */}
                      {poll && poll.discussion_url ? (
                        <button
                          onClick={() => openDiscussion(poll.discussion_url)}
                          className="mt-2 text-sm text-blue-600 dark:text-blue-400 hover:underline"
                        >
                          Cast Your Vote ↗
                        </button>
                      ) : (
                        poll && (
                          <p className="mt-2 text-sm text-muted-foreground">
                            Poll not available
                          </p>
                        )
                      )}
                    </div>
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </section>

      {/* Recent Resolutions */}
      <section className="rounded-xl border border-border bg-card p-6">
        <h3 className="text-lg font-semibold mb-4">Recent Resolutions</h3>

        {resolutionsError && (
          <div className="mb-4 p-4 rounded-lg bg-destructive/10 border border-destructive text-destructive">
            {resolutionsError}
          </div>
        )}

        {resolutionsLoading && (
          <p className="text-muted-foreground">Loading resolutions…</p>
        )}

        {!resolutionsLoading && !resolutionsError && resolutions.length === 0 && (
          <p className="text-muted-foreground">No triage resolutions yet.</p>
        )}

        {!resolutionsLoading && !resolutionsError && resolutions.length > 0 && (
          <div className="max-h-96 overflow-y-auto space-y-3 pr-2">
            {resolutions.map((entry) => (
              <div
                key={entry.id}
                className="p-3 rounded-lg bg-muted border border-border"
              >
                <div className="flex items-center gap-2 mb-1">
                  <time
                    dateTime={entry.timestamp}
                    className="text-sm text-muted-foreground font-mono"
                  >
                    {entry.timestamp}
                  </time>
                  <span
                    className={`text-xs px-2 py-0.5 rounded-full capitalize ${actionBadgeColor(
                      entry.action,
                    )}`}
                  >
                    {entry.action}
                  </span>
                </div>
                {entry.details && (
                  <p className="text-sm text-muted-foreground">{entry.details}</p>
                )}
              </div>
            ))}
          </div>
        )}
      </section>

      {/* Transparency Log */}
      <section className="rounded-xl border border-border bg-card p-6">
        <h3 className="text-lg font-semibold mb-4">Transparency Log</h3>

        {logError && (
          <div className="mb-4 p-4 rounded-lg bg-destructive/10 border border-destructive text-destructive">
            {logError}
          </div>
        )}

        {logLoading && (
          <p className="text-muted-foreground">Loading transparency log…</p>
        )}

        {!logLoading && !logError && logEntries.length === 0 && (
          <p className="text-muted-foreground">No governance actions recorded yet.</p>
        )}

        {!logLoading && !logError && logEntries.length > 0 && (
          <div className="max-h-96 overflow-y-auto space-y-3 pr-2">
            {logEntries.map((entry) => (
              <div
                key={entry.id}
                className="p-3 rounded-lg bg-muted border border-border"
              >
                <div className="flex items-center gap-2 mb-1">
                  <time
                    dateTime={entry.timestamp}
                    className="text-sm text-muted-foreground font-mono"
                  >
                    {entry.timestamp}
                  </time>
                  <span className="text-xs px-2 py-0.5 rounded-full bg-muted text-muted-foreground capitalize">
                    {entry.action}
                  </span>
                </div>
                {entry.details && (
                  <p className="text-sm text-muted-foreground">{entry.details}</p>
                )}
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}
