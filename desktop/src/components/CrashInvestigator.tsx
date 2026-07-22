import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  confirmCrashFix,
  createSnapshot,
  disableModForTest,
  formatError,
  getDisablePlan,
  investigateCrash,
  investigateManual,
  readCrashLog,
  reportStillCrashing,
  restoreSnapshot,
  type DisablePlan,
  type InvestigationResult,
  type SuspectScore,
  type SuggestedAction,
} from '../lib/tauri';
import { DependencyPrompt } from './DependencyPrompt';
import { AiAssistant } from './AiAssistant';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from '@/components/ui/dialog';

interface CrashInvestigatorProps {
  instanceId: string;
  crashFilename?: string | null;
  manualLogText?: string | null;
  onClose: () => void;
  /** Called to re-launch the instance after disabling a suspected mod. */
  /** Returns true only when the canonical launch controller actually started. */
  onLaunch: () => Promise<boolean>;
}

const SIGNAL_LABELS: Record<string, string> = {
  stack_frames: 'Stack frames',
  stack_frame_score: 'Stack frames',
  curated_conflicts: 'Curated conflicts',
  curated_conflict_score: 'Curated conflicts',
  prior_local_crashes: 'Prior local crashes',
  local_history_score: 'Prior local crashes',
  dependency_relationships: 'Dependency relationships',
  dependency_score: 'Dependency relationships',
  confirmed_prior_fixes: 'Confirmed prior fixes',
  confirmed_fix_score: 'Confirmed prior fixes',
};

/** Render one deterministic signal with its evidence source. */
function BreakdownEntry({ signal, value }: { signal: string; value: unknown }) {
  let displayValue: string;
  if (value === null || value === undefined) {
    displayValue = '—';
  } else if (typeof value === 'number') {
    displayValue = value.toFixed(2);
  } else if (typeof value === 'boolean') {
    displayValue = value ? 'true' : 'false';
  } else if (Array.isArray(value)) {
    displayValue = `[${value.length} item${value.length === 1 ? '' : 's'}]`;
  } else if (typeof value === 'object') {
    displayValue = JSON.stringify(value);
  } else {
    displayValue = String(value);
  }
  return (
    <div className="flex items-center justify-between text-sm py-1">
      <span className="text-muted-foreground" data-testid={`breakdown-key-${signal}`}>
        {SIGNAL_LABELS[signal] ?? signal.replace(/_/g, ' ')}
      </span>
      <span className="font-mono text-xs text-muted-foreground">
        {displayValue}
      </span>
    </div>
  );
}

/** Render the per-signal breakdown for a suspect. */
function BreakdownList({ breakdown }: { breakdown: Record<string, unknown> }) {
  const entries = Object.entries(breakdown);
  if (entries.length === 0) return null;
  return (
    <div className="mt-2 space-y-0.5 border-t border-border pt-2">
      {entries.map(([k, v]) => (
        <BreakdownEntry key={k} signal={k} value={v} />
      ))}
    </div>
  );
}

/** Render a single suspect card. */
function SuspectCard({
  suspect,
  rank,
  isTop,
  action,
  onAction,
  loading,
}: {
  suspect: SuspectScore;
  rank: number;
  isTop: boolean;
  action?: SuggestedAction;
  onAction?: () => void;
  loading: boolean;
}) {
  const score = suspect.total_score.toFixed(2);
  return (
    <div
      className={[
        'rounded-xl border p-4 transition-colors',
        isTop
          ? 'border-primary/30 bg-primary/10'
          : 'border-border bg-card',
      ].join(' ')}
    >
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <span
            className={[
              'inline-flex h-6 w-6 items-center justify-center rounded-full text-xs font-bold',
              isTop
                ? 'bg-primary text-primary-foreground'
                : 'bg-card text-muted-foreground',
            ].join(' ')}
          >
            {rank}
          </span>
          <div>
            <p className="text-sm font-semibold">{suspect.filename}</p>
            {suspect.mod_id && suspect.mod_id !== suspect.filename && (
              <p className="text-xs text-muted-foreground">{suspect.mod_id}</p>
            )}
            {suspect.is_dependent_of && (
              <span className="mt-1 inline-block rounded-md bg-amber-100 dark:bg-amber-900/30 px-2 py-0.5 text-xs font-medium text-amber-800 dark:text-amber-300">
                Indirect — depends on {suspect.is_dependent_of}
              </span>
            )}
          </div>
        </div>
        <span className="font-mono text-sm font-bold text-muted-foreground">
          {score}
        </span>
      </div>
      <BreakdownList breakdown={suspect.breakdown} />
      {isTop && action && (
        <div className="mt-3 pt-3 border-t border-primary/20">
          {action.kind === 'GuidedDisable' && (
            <button
              disabled={loading}
              onClick={onAction}
              className={[
                'w-full rounded-lg px-3 py-2 text-sm font-medium transition-colors',
                'bg-primary text-primary-foreground hover:bg-primary/90',
                'disabled:opacity-50 disabled:cursor-not-allowed',
              ].join(' ')}
            >
              Disable &quot;{suspect.filename}&quot; &amp; Relaunch
            </button>
          )}
          {action.kind === 'ConfidenceAutoDisable' && (
            <button
              disabled={loading}
              onClick={onAction}
              className={[
                'w-full rounded-lg px-3 py-2 text-sm font-medium transition-colors',
                'bg-primary text-primary-foreground hover:bg-primary/90',
                'disabled:opacity-50 disabled:cursor-not-allowed',
              ].join(' ')}
            >
              Disable known culprit &quot;{action.mod_id}&quot; &amp; Relaunch
            </button>
          )}
        </div>
      )}
    </div>
  );
}

/** Post-launch confirmation prompt. */
function FixConfirmation({
  filename,
  onFix,
  onStillCrashing,
  loading,
}: {
  filename: string;
  onFix: () => void;
  onStillCrashing: () => void;
  loading: boolean;
}) {
  return (
    <div className="rounded-xl border border-primary/30 bg-primary/10 p-4">
      <p className="text-sm font-semibold mb-3">
        Did that fix &quot;{filename}&quot;?
      </p>
      <div className="flex gap-2">
        <button
          disabled={loading}
          onClick={onFix}
          className={[
            'flex-1 rounded-lg px-3 py-2 text-sm font-medium transition-colors',
            'bg-green-600 text-white hover:bg-green-700',
            'disabled:opacity-50 disabled:cursor-not-allowed',
          ].join(' ')}
        >
          Yes, fixed
        </button>
        <button
          disabled={loading}
          onClick={onStillCrashing}
          className={[
            'flex-1 rounded-lg px-3 py-2 text-sm font-medium transition-colors',
            'bg-destructive text-destructive-foreground hover:bg-destructive/90',
            'disabled:opacity-50 disabled:cursor-not-allowed',
          ].join(' ')}
        >
          Still crashing
        </button>
      </div>
    </div>
  );
}

/** Triage banner for mods under community review. */
function TriageBanner({ modId, onViewTriage }: { modId: string; onViewTriage: () => void }) {
  return (
    <div className="rounded-xl border border-yellow-300 dark:border-yellow-700 bg-yellow-50 dark:bg-yellow-900/20 p-4">
      <p className="text-sm text-yellow-800 dark:text-yellow-200 mb-3">
        This mod ({modId}) is under community review for similar issues.
      </p>
      <button
        onClick={onViewTriage}
        className="rounded-lg px-3 py-2 text-sm font-medium transition-colors bg-yellow-600 text-white hover:bg-yellow-700"
      >
        View in Triage Center
      </button>
    </div>
  );
}

/** Success confirmation overlay. */
function SuccessBanner({ message }: { message: string }) {
  return (
    <div className="rounded-xl border border-green-300 dark:border-green-700 bg-green-50 dark:bg-green-900/20 p-4">
      <p className="text-sm font-semibold text-green-800 dark:text-green-200">
        {message}
      </p>
    </div>
  );
}

/** Error banner. */
function ErrorBanner({ message }: { message: string }) {
  return (
    <div className="rounded-xl border border-destructive/30 bg-destructive/10 p-4">
      <p className="text-sm font-semibold text-destructive">
        {message}
      </p>
    </div>
  );
}

/** Ruled-out mods list. */
function RuledOutList({ ruledOut }: { ruledOut: string[] }) {
  if (ruledOut.length === 0) return null;
  return (
    <div className="mt-2 text-xs text-muted-foreground">
      Already ruled out:{' '}
      <span className="font-medium">{ruledOut.join(', ')}</span>
    </div>
  );
}

export function CrashInvestigator({
  instanceId,
  crashFilename,
  manualLogText,
  onClose,
  onLaunch,
}: CrashInvestigatorProps) {
  const [result, setResult] = useState<InvestigationResult | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  // Raw crash log text, stored for reportStillCrashing in file-mode investigations
  const [crashLogText, setCrashLogText] = useState<string>('');
  // Post-launch state
  const [postLaunch, setPostLaunch] = useState<{
    filename: string;
    modId: string;
  } | null>(null);
  // Success state
  const [success, setSuccess] = useState<string | null>(null);
  const [recoverySnapshotId, setRecoverySnapshotId] = useState<string | null>(null);
  const [disabledByTest, setDisabledByTest] = useState<string[]>([]);
  // Disable dependency prompt state
  const [disablePlanTarget, setDisablePlanTarget] = useState<{
    originalFilename: string;
    modId: string;
    plan: DisablePlan;
  } | null>(null);
  // AI assistant panel
  const [showAiAssistant, setShowAiAssistant] = useState(false);
  // AI crash explanation
  const [aiExplanation, setAiExplanation] = useState<string | null>(null);
  const [aiLoading, setAiLoading] = useState(false);
  const [aiError, setAiError] = useState<string | null>(null);
  const cancelledRef = useRef(false);
  const closeInProgressRef = useRef(false);

  // Run investigation on mount
  useEffect(() => {
    let cancelled = false;
    const runInvestigation = async () => {
      try {
        const snapshot = await createSnapshot(
          instanceId,
          `crash-doctor-${globalThis.crypto?.randomUUID?.() ?? Date.now()}`,
        );
        if (cancelled) return;
        setRecoverySnapshotId(snapshot.id);

        // For file-based investigation, fetch the raw log text first
        if (crashFilename) {
          const rawText = await readCrashLog(instanceId, crashFilename);
          if (!cancelled) setCrashLogText(rawText);
        }

        let invResult: InvestigationResult;
        if (manualLogText) {
          invResult = await investigateManual(instanceId, manualLogText);
        } else {
          invResult = await investigateCrash(instanceId, crashFilename || undefined);
        }
        if (!cancelled) setResult(invResult);
      } catch (e) {
        if (!cancelled) {
          setError(formatError(e));
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    };
    runInvestigation();
    return () => {
      cancelled = true;
    };
  }, [instanceId, crashFilename, manualLogText]);

  const restoreInvestigationSnapshot = useCallback(async () => {
    if (!recoverySnapshotId) {
      throw new Error('The pre-investigation recovery snapshot is unavailable.');
    }
    await restoreSnapshot(instanceId, recoverySnapshotId);
    setDisabledByTest([]);
    setPostLaunch(null);
  }, [instanceId, recoverySnapshotId]);

  const handleClose = useCallback(async () => {
    if (closeInProgressRef.current) return;
    closeInProgressRef.current = true;
    if (success) {
      onClose();
      closeInProgressRef.current = false;
      return;
    }
    if (!recoverySnapshotId && disabledByTest.length === 0) {
      onClose();
      closeInProgressRef.current = false;
      return;
    }
    setLoading(true);
    setError(null);
    try {
      await restoreInvestigationSnapshot();
      onClose();
    } catch (cause) {
      setError(`Could not restore the pre-investigation snapshot: ${formatError(cause)}`);
    } finally {
      if (!cancelledRef.current) setLoading(false);
      closeInProgressRef.current = false;
    }
  }, [disabledByTest.length, onClose, recoverySnapshotId, restoreInvestigationSnapshot, success]);

  const handleDisableAndRelaunch = useCallback(async () => {
    if (!result) return;
    setLoading(true);
    setError(null);

    let action: SuggestedAction;
    let filename: string;
    let modId: string;

    if (result.suggested_action.kind === 'GuidedDisable') {
      action = result.suggested_action;
      filename = action.next_suspect.filename;
      modId = action.next_suspect.mod_id;
    } else if (result.suggested_action.kind === 'ConfidenceAutoDisable') {
      action = result.suggested_action;
      filename = action.filename;
      modId = action.mod_id;
    } else {
      setError('No actionable suspect available.');
      setLoading(false);
      return;
    }

    let modified = false;
    try {
      const plan = await getDisablePlan(instanceId, filename);
      if (plan.dependents.length > 0) {
        setDisablePlanTarget({ originalFilename: filename, modId, plan });
        setLoading(false);
        return;
      }
      await disableModForTest(instanceId, filename);
      modified = true;
      setDisabledByTest([filename]);
      const launched = await onLaunch();
      if (!cancelledRef.current && launched) {
        setPostLaunch({ filename, modId });
      } else if (!cancelledRef.current) {
        setError('The test launch did not start. Resolve the health prompt or launch error, then retry this suspect.');
      }
    } catch (e) {
      if (modified) {
        try {
          await restoreInvestigationSnapshot();
        } catch (restoreError) {
          if (!cancelledRef.current) {
            setError(`Test launch failed and recovery also failed: ${formatError(restoreError)}`);
          }
          return;
        }
      }
      if (!cancelledRef.current) {
        setError(formatError(e));
      }
    } finally {
      if (!cancelledRef.current && !disablePlanTarget) setLoading(false);
    }
  }, [result, instanceId, onLaunch, restoreInvestigationSnapshot]);

  const handleDisableConfirm = useCallback(async (selectedKeys: string[]) => {
    if (!disablePlanTarget) return;
    const { originalFilename, modId, plan } = disablePlanTarget;
    setLoading(true);
    setError(null);

    try {
      const selectedSet = new Set(selectedKeys);
      const filenames = [
        ...plan.dependents
          .filter((dependent) => selectedSet.has(dependent.mod_id))
          .map((dependent) => dependent.filename),
        originalFilename,
      ];
      for (const filename of filenames) {
        await disableModForTest(instanceId, filename);
      }
      setDisabledByTest(filenames);
      const launched = await onLaunch();
      if (!cancelledRef.current && launched) {
        setPostLaunch({ filename: originalFilename, modId });
      } else if (!cancelledRef.current) {
        setError('The test launch did not start. Resolve the health prompt or launch error, then retry this suspect.');
      }
    } catch (e) {
      try {
        await restoreInvestigationSnapshot();
      } catch (restoreError) {
        if (!cancelledRef.current) {
          setError(`Disable test failed and recovery also failed: ${formatError(restoreError)}`);
        }
        return;
      }
      if (!cancelledRef.current) {
        setError(formatError(e));
      }
    } finally {
      if (!cancelledRef.current) {
        setDisablePlanTarget(null);
        setLoading(false);
      }
    }
  }, [disablePlanTarget, instanceId, onLaunch, restoreInvestigationSnapshot]);

  // Track whether the component is still mounted.
  // Reset on setup so StrictMode double-invocation (dev) or real remounts
  // don't leave cancelledRef stuck at true from a previous cleanup.
  useEffect(() => {
    cancelledRef.current = false;
    return () => {
      cancelledRef.current = true;
    };
  }, []);

  const handleFixConfirmed = useCallback(async () => {
    if (!result || !postLaunch) return;
    setLoading(true);
    setError(null);

    try {
      if (result.fingerprint) {
        await confirmCrashFix(result.fingerprint, postLaunch.modId);
      }
      if (!cancelledRef.current) {
        setSuccess(`Crash fix confirmed for ${postLaunch.modId}.`);
        // Auto-close after a short delay
        setTimeout(() => {
          if (!cancelledRef.current) onClose();
        }, 2000);
      }
    } catch (e) {
      if (!cancelledRef.current) {
        setError(formatError(e));
      }
    } finally {
      if (!cancelledRef.current) setLoading(false);
    }
  }, [result, postLaunch, onClose]);

  const handleStillCrashing = useCallback(async () => {
    if (!result || !postLaunch) return;
    setLoading(true);
    setError(null);

    try {
      // Restore the complete pre-investigation state, including any dependents.
      await restoreInvestigationSnapshot();

      // Determine the crash log text to pass
      let logText: string;
      if (manualLogText) {
        logText = manualLogText;
      } else {
        // File mode: re-fetch the raw log text (we may have it in state already)
        logText = crashLogText || '';
        if (!logText) {
          logText = await readCrashLog(instanceId, crashFilename || '');
          setCrashLogText(logText);
        }
      }

      // reportStillCrashing returns a new InvestigationResult (auto-advance)
      const newResult = await reportStillCrashing(
        instanceId,
        result.fingerprint!,
        postLaunch.modId,
        logText,
      );

      if (!cancelledRef.current) {
        setResult(newResult);
        setPostLaunch(null);
      }
    } catch (e) {
      if (!cancelledRef.current) {
        setError(formatError(e));
      }
    } finally {
      if (!cancelledRef.current) setLoading(false);
    }
  }, [result, postLaunch, instanceId, manualLogText, crashLogText, crashFilename, restoreInvestigationSnapshot]);

  const handleViewTriage = useCallback(() => {
    onClose();
  }, [onClose]);

  const handleAiExplain = useCallback(async () => {
    if (aiLoading || cancelledRef.current) return;
    setAiLoading(true);
    setAiError(null);
    setAiExplanation(null);
    const logText = crashLogText || manualLogText || '';
    if (!logText) {
      setAiError('No crash log available to analyze.');
      setAiLoading(false);
      return;
    }
    try {
      const explanation = await invoke<string>('explain_crash', {
        instanceId: instanceId,
        crashLog: logText,
      });
      if (!cancelledRef.current) setAiExplanation(explanation);
    } catch (e) {
      const msg = formatError(e);
      if (msg.includes('ERR_AI_NOT_AUTHENTICATED') || msg.toLowerCase().includes('not authenticated') || msg.toLowerCase().includes('not connected')) {
        if (!cancelledRef.current) setAiError('connect-github');
      } else {
        if (!cancelledRef.current) setAiError(msg);
      }
    } finally {
      if (!cancelledRef.current) setAiLoading(false);
    }
  }, [instanceId, crashLogText, manualLogText, aiLoading]);

  if (loading && !result) {
    return (
      <Dialog open onOpenChange={(open) => { if (!open) void handleClose(); }}>
        <DialogContent>
          <DialogTitle>Crash Doctor</DialogTitle>
          <DialogDescription>
            Creating a recovery snapshot and analyzing deterministic crash signals.
          </DialogDescription>
          <div className="flex flex-col items-center gap-3 py-8">
            <div role="status" aria-label="Investigating crash" className="h-8 w-8 animate-spin rounded-full border-2 border-primary border-t-transparent" />
            <p className="text-sm text-muted-foreground">Investigating crash…</p>
          </div>
        </DialogContent>
      </Dialog>
    );
  }

  if (error) {
    return (
      <Dialog open onOpenChange={(open) => { if (!open) void handleClose(); }}>
        <DialogContent>
          <DialogTitle>Crash Doctor</DialogTitle>
          <DialogDescription>
            The investigation stopped safely. Restore the recovery snapshot before closing if any test changes were made.
          </DialogDescription>
          <ErrorBanner message={error} />
          <div className="flex justify-end gap-2">
            {recoverySnapshotId && (
              <button
                onClick={() => { void restoreInvestigationSnapshot().then(onClose).catch((cause) => setError(formatError(cause))); }}
                className="rounded-lg border border-destructive/40 px-4 py-2 text-sm font-medium text-destructive hover:bg-destructive/10"
              >
                Restore All & Close
              </button>
            )}
            <button
              onClick={() => { void handleClose(); }}
              className="rounded-lg border border-border px-4 py-2 text-sm font-medium hover:bg-accent"
            >
              Close Safely
            </button>
          </div>
        </DialogContent>
      </Dialog>
    );
  }

  if (!result) return null;

  const { fingerprint, signature_name, suspects, suggested_action, ruled_out } = result;

  // Determine the action card for the top suspect
  let actionCard: SuggestedAction | undefined;
  if (suggested_action.kind === 'GuidedDisable') {
    actionCard = suggested_action;
  } else if (suggested_action.kind === 'ConfidenceAutoDisable') {
    actionCard = suggested_action;
  }

  return (
    <>
      <Dialog open onOpenChange={(open) => { if (!open) void handleClose(); }}>
        <DialogContent className="max-h-[90vh] max-w-lg overflow-y-auto">
          <div className="flex items-start justify-between gap-4 border-b border-border pb-4 pr-6">
          <div className="flex-1 min-w-0">
            <DialogTitle>Crash Doctor</DialogTitle>
            <DialogDescription>
              Ranked deterministic evidence with reversible one-mod-at-a-time tests.
            </DialogDescription>
            {fingerprint && (
              <p className="text-sm text-muted-foreground mt-1 truncate">
                {fingerprint.exception_class}
              </p>
            )}
            {signature_name && (
              <p className="text-xs text-primary mt-0.5">
                {signature_name}
              </p>
            )}
          </div>
          <div className="flex items-center gap-2">
            <button
              onClick={() => setShowAiAssistant(true)}
              className="rounded-lg bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90"
            >
              Ask AI Assistant
            </button>
          </div>
        </div>

        <div className="space-y-4">
          {recoverySnapshotId && (
            <div className="flex items-center justify-between gap-3 rounded-lg border border-border bg-muted p-3 text-xs text-muted-foreground">
              <span>Recovery snapshot ready: {recoverySnapshotId}</span>
              <button
                onClick={() => { void restoreInvestigationSnapshot().then(onClose).catch((cause) => setError(formatError(cause))); }}
                disabled={loading}
                className="shrink-0 rounded-md border border-destructive/40 px-2 py-1 font-medium text-destructive hover:bg-destructive/10 disabled:opacity-50"
              >
                Restore All & Close
              </button>
            </div>
          )}
          {/* AI Assistant panel or suspect list */}
          {showAiAssistant ? (
            <div className="h-[480px] space-y-2">
              <button
                onClick={() => setShowAiAssistant(false)}
                className="text-xs text-primary hover:underline"
              >
                Back to suspects
              </button>
              <AiAssistant
                instanceId={instanceId}
                crashLog={crashLogText || manualLogText || null}
                crashSignatures={JSON.stringify(result.signature_name ?? null)}
                suspects={JSON.stringify(result.suspects)}
                onClose={() => setShowAiAssistant(false)}
              />
            </div>
          ) : (
            <>
              {/* Suspect list */}
              {suspects.length > 0 && (
                <div className="space-y-3">
                  <p className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
                    Suspects
                  </p>
                  {suspects.map((suspect, idx) => (
                    <SuspectCard
                      key={suspect.filename}
                      suspect={suspect}
                      rank={idx + 1}
                      isTop={idx === 0}
                      action={idx === 0 ? actionCard : undefined}
                      onAction={idx === 0 ? handleDisableAndRelaunch : undefined}
                      loading={loading}
                    />
                  ))}
                </div>
              )}

              {/* Ruled out */}
              <RuledOutList ruledOut={ruled_out} />

              {/* AI Explain toggle */}
              {!aiExplanation && !aiLoading && !aiError && (
                <button
                  onClick={handleAiExplain}
                  className="w-full rounded-lg border border-border bg-card px-3 py-2 text-sm font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-accent-foreground"
                >
                  Explain with AI
                </button>
              )}

              {aiLoading && (
                <div className="flex items-center gap-2 rounded-xl border border-border bg-card p-4 text-sm text-muted-foreground">
                  <div role="status" aria-label="Analyzing crash with AI" className="h-4 w-4 animate-spin rounded-full border-2 border-primary border-t-transparent" />
                  Analyzing crash with AI…
                </div>
              )}

              {aiError === 'connect-github' && (
                <div className="rounded-xl border border-primary/20 bg-primary/5 p-4 text-sm text-muted-foreground">
                  Copilot is not connected.{' '}
                  <span className="text-primary">Connect with GitHub</span> to get AI-powered crash explanations.
                </div>
              )}

              {aiError && aiError !== 'connect-github' && (
                <div className="rounded-xl border border-destructive/30 bg-destructive/10 p-4 text-sm text-destructive">
                  {aiError}
                </div>
              )}

              {aiExplanation && (
                <div className="rounded-xl border border-primary/20 bg-primary/5 p-4">
                  <div className="flex items-center justify-between mb-2">
                    <p className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
                      AI Explanation
                    </p>
                    <button
                      onClick={() => setAiExplanation(null)}
                      className="text-xs text-muted-foreground hover:text-foreground transition-colors"
                    >
                      Dismiss
                    </button>
                  </div>
                  <p className="text-sm whitespace-pre-wrap">{aiExplanation}</p>
                </div>
              )}

              {/* Post-launch confirmation */}
              {postLaunch && (
                <FixConfirmation
                  filename={postLaunch.filename}
                  onFix={handleFixConfirmed}
                  onStillCrashing={handleStillCrashing}
                  loading={loading}
                />
              )}

              {/* Triage banner */}
              {suggested_action.kind === 'ShowTriageBanner' && (
                <TriageBanner
                  modId={suggested_action.mod_id}
                  onViewTriage={handleViewTriage}
                />
              )}

              {/* No suspects */}
              {suggested_action.kind === 'NoSuspects' && (
                <div className="rounded-xl border border-border bg-card p-4">
                  <p className="text-sm text-muted-foreground">
                    No suspects identified. The crash may not be mod-related. Use the manual log viewer for deeper inspection.
                  </p>
                </div>
              )}

              {/* Success */}
              {success && <SuccessBanner message={success} />}
            </>
          )}
        </div>
        </DialogContent>
      </Dialog>

      {/* Disable dependency prompt */}
      {disablePlanTarget && (
        <DependencyPrompt
          title="Disable mod and dependents"
          actionLabel="Disable selected"
          candidates={disablePlanTarget.plan.dependents.map((d) => ({
            key: d.mod_id,
            label: d.mod_id,
            requirement: d.requirement,
            source: d.source,
          }))}
          onConfirm={handleDisableConfirm}
          onCancel={() => setDisablePlanTarget(null)}
        />
      )}
    </>
  );
}
