import { useCallback, useEffect, useReducer, useState } from 'react';
import {
  type InstallIntent,
  type ResolvedInstallPlan,
  type InstallOutcome,
  type ProgressEvent,
  type DepConflict,
  type ResolvedDep,
  resolveInstallPlan,
  applyInstallPlan,
  cancelInstall,
  subscribeProgress,
} from '../lib/installFlow';
import { formatError, restoreSnapshot } from '../lib/tauri';
import {
  Dialog,
  DialogContent,
  DialogTitle,
  DialogDescription,
} from '@/components/ui/dialog';

// ---------------------------------------------------------------------------
// User choices model
// ---------------------------------------------------------------------------

interface PlanChoices {
  optionalIncluded: Set<string>;
  conflictResolutions: Map<string, string>;
}

// ---------------------------------------------------------------------------
// State machine
// ---------------------------------------------------------------------------

type FlowState =
  | { phase: 'resolving'; plan?: ResolvedInstallPlan; error?: string }
  | { phase: 'review'; plan: ResolvedInstallPlan; choices: PlanChoices; dirty: boolean }
  | { phase: 'executing'; plan: ResolvedInstallPlan; progress: ProgressEvent }
  | { phase: 'result'; outcome: InstallOutcome }
  | { phase: 'error'; message: string; retryable: boolean }
  | { phase: 'closed' };

type FlowAction =
  | { type: 'resolved'; plan: ResolvedInstallPlan }
  | { type: 'resolve-error'; error: string }
  | { type: 'patch-choice'; modJarId: string; included: boolean }
  | { type: 'resolve-conflict'; conflictId: string; resolution: string }
  | { type: 'confirm' }
  | { type: 'confirm-replan' }
  | { type: 'progress'; event: ProgressEvent }
  | { type: 'outcome'; outcome: InstallOutcome }
  | { type: 'fail'; message: string; retryable: boolean }
  | { type: 'retry' }
  | { type: 'close' };

function flowReducer(state: FlowState, action: FlowAction): FlowState {
  switch (action.type) {
    case 'resolved':
      if (state.phase !== 'resolving') return state;
      return {
        phase: 'review',
        plan: action.plan,
        choices: defaultChoices(action.plan),
        dirty: false,
      };

    case 'resolve-error':
      return { phase: 'error', message: action.error, retryable: true };

    case 'patch-choice':
      if (state.phase !== 'review') return state;
      return {
        ...state,
        choices: {
          ...state.choices,
          optionalIncluded: (() => {
            const next = new Set(state.choices.optionalIncluded);
            if (action.included) next.add(action.modJarId);
            else next.delete(action.modJarId);
            return next;
          })(),
        },
        dirty: true,
      };

    case 'resolve-conflict':
      if (state.phase !== 'review') return state;
      return {
        ...state,
        choices: {
          ...state.choices,
          conflictResolutions: new Map(state.choices.conflictResolutions).set(action.conflictId, action.resolution),
        },
        dirty: true,
      };

    case 'confirm':
      if (state.phase !== 'review') return state;
      return {
        phase: 'executing',
        plan: state.plan,
        progress: {
          planId: state.plan.fingerprint,
          phase: 'staging' as const,
          step: 0, totalSteps: 0, bytesDownloaded: 0, bytesTotal: 0,
          message: 'Starting…',
        },
      };

    case 'progress':
      if (state.phase !== 'executing') return state;
      return { ...state, progress: action.event };

    case 'outcome':
      return { phase: 'result', outcome: action.outcome };

    case 'fail':
      return { phase: 'error', message: action.message, retryable: action.retryable };

    case 'retry':
      return { phase: 'resolving', plan: state.phase === 'review' ? state.plan : undefined };

    case 'close':
      return { phase: 'closed' };

    default:
      return state;
  }
}

function defaultChoices(plan: ResolvedInstallPlan): PlanChoices {
  return {
    optionalIncluded: new Set(
      plan.dependencies
        .filter((d) => d.requirement === 'optional')
        .map((d) => d.modJarId),
    ),
    conflictResolutions: new Map(
      plan.conflicts
        .filter((c) => c.chosen)
        .map((c) => [c.conflictId, c.chosen!]),
    ),
  };
}

// ---------------------------------------------------------------------------
// Props
// ---------------------------------------------------------------------------

interface InstallFlowProps {
  intent: InstallIntent;
  instanceName: string;
  onOpenInstance?: (instanceId: string) => void;
  onClose?: () => void;
  open: boolean;
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export function InstallFlow({
  intent,
  instanceName,
  onOpenInstance,
  onClose,
  open,
}: InstallFlowProps) {
  const [state, dispatch] = useReducer(flowReducer, { phase: 'closed' } as FlowState);
  const [resolutionIntent, setResolutionIntent] = useState(intent);

  // Start resolving on first open.
  useEffect(() => {
    if (!open) return;
    setResolutionIntent(intent);
    dispatch({ type: 'retry' });
  }, [open, intent]);

  // Resolve when entering resolving phase.
  useEffect(() => {
    if (state.phase !== 'resolving') return;
    let cancelled = false;
    (async () => {
      try {
        const plan = await resolveInstallPlan(resolutionIntent);
        if (!cancelled) dispatch({ type: 'resolved', plan });
      } catch (e) {
        if (!cancelled) dispatch({ type: 'resolve-error', error: formatError(e) });
      }
    })();
    return () => { cancelled = true; };
  }, [state.phase, resolutionIntent]);

  // Execute plan.
  useEffect(() => {
    if (state.phase !== 'executing') return;
    let cancelled = false;
    let unsubscribe: (() => void) | undefined;
    (async () => {
      try {
        unsubscribe = await subscribeProgress(state.plan.fingerprint, (event) => {
          dispatch({ type: 'progress', event });
        });
        const outcome = await applyInstallPlan(state.plan);
        if (!cancelled) dispatch({ type: 'outcome', outcome });
      } catch (e) {
        if (!cancelled) dispatch({ type: 'fail', message: formatError(e), retryable: false });
      }
    })();
    return () => {
      cancelled = true;
      unsubscribe?.();
    };
  }, [state.phase]);

  const handleCancel = useCallback(() => {
    if (state.phase === 'executing') {
      void cancelInstall(state.plan.fingerprint).catch(() => {
        // The executor may already be past its cancellable staging phase.
      });
      return; // Dialog stays open — user must wait for outcome.
    }
    if (state.phase === 'review') {
      void cancelInstall(state.plan.fingerprint).catch(() => {});
      dispatch({ type: 'close' });
      onClose?.();
      return;
    }
    if (state.phase === 'resolving' || state.phase === 'error') {
      dispatch({ type: 'close' });
      onClose?.();
    }
  }, [state, onClose]);

  const handleConfirm = useCallback(() => {
    if (state.phase !== 'review') return;
    if (state.dirty || state.plan.pendingChoices.length > 0) {
      setResolutionIntent({
        ...resolutionIntent,
        optionalDeps: {
          type: 'include',
          deps: [...state.choices.optionalIncluded].sort(),
        },
        overrides: {
          ...resolutionIntent.overrides,
          forceConflictResolution: Object.fromEntries(state.choices.conflictResolutions),
        },
      });
      dispatch({ type: 'retry' });
      return;
    }
    dispatch({ type: 'confirm' });
  }, [state, resolutionIntent]);

  const handleClose = useCallback(() => {
    dispatch({ type: 'close' });
    onClose?.();
  }, [onClose]);

  const renderContent = () => {
    switch (state.phase) {
      case 'resolving':
        return <ResolvingView />;
      case 'review':
        return <ReviewView
          plan={state.plan}
          choices={state.choices}
          onToggleOptional={(id, inc) => dispatch({ type: 'patch-choice', modJarId: id, included: inc })}
          onResolveConflict={(id, res) => dispatch({ type: 'resolve-conflict', conflictId: id, resolution: res })}
          onConfirm={handleConfirm}
          onCancel={handleCancel}
        />;
      case 'executing':
        return <ProgressView progress={state.progress} onCancel={handleCancel} />;
      case 'result':
        return <ResultView
          outcome={state.outcome}
          instanceId={intent.targetInstance}
          onOpenInstance={() => onOpenInstance?.(intent.targetInstance)}
          onClose={handleClose}
        />;
      case 'error':
        return <ErrorView
          message={state.message}
          retryable={state.retryable}
          onRetry={() => dispatch({ type: 'retry' })}
          onClose={handleClose}
        />;
      default:
        return null;
    }
  };

  return (
    <Dialog open={open} onOpenChange={(o) => { if (!o) handleCancel(); }}>
      <DialogContent className="max-w-2xl">
        <DialogTitle>Review Instance Changes</DialogTitle>
        <DialogDescription>
          {instanceName}
        </DialogDescription>
        {renderContent()}
      </DialogContent>
    </Dialog>
  );
}

// ---------------------------------------------------------------------------
// Sub-views
// ---------------------------------------------------------------------------

function ResolvingView() {
  return (
    <div className="flex flex-col items-center justify-center py-8 gap-3">
      <div className="h-6 w-6 animate-spin rounded-full border-2 border-primary border-t-transparent" />
      <p className="text-sm text-muted-foreground">Resolving dependencies…</p>
    </div>
  );
}

function ReviewView({
  plan,
  choices,
  onToggleOptional,
  onResolveConflict,
  onConfirm,
  onCancel,
}: {
  plan: ResolvedInstallPlan;
  choices: PlanChoices;
  onToggleOptional: (id: string, inc: boolean) => void;
  onResolveConflict: (id: string, res: string) => void;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const canInstall = plan.blockingErrors.length === 0;
  const hasUnresolvedBlockingConflict = plan.conflicts.some(
    (conflict) => conflict.blocking
      && !conflict.chosen
      && !choices.conflictResolutions.has(conflict.conflictId),
  );
  const needsReplan = plan.pendingChoices.length > 0;
  const actionLabel = plan.intent.action.type === 'remove'
    ? 'Remove Safely'
    : plan.intent.action.type === 'batch-update'
      ? 'Apply Updates'
      : plan.intent.action.type === 'batch-install'
        ? 'Install Batch'
        : 'Install';

  return (
    <div className="space-y-4">
      {/* Warnings */}
      {plan.warnings.length > 0 && (
        <div className="rounded-lg bg-amber-50 dark:bg-amber-900/20 p-3 text-xs text-amber-700 dark:text-amber-300 space-y-1">
          {plan.warnings.map((w, i) => <p key={i}>{w.message}</p>)}
        </div>
      )}

      {/* Blocking errors */}
      {plan.blockingErrors.length > 0 && (
        <div className="rounded-lg bg-destructive/10 p-3 text-xs text-destructive space-y-1">
          {plan.blockingErrors.map((e, i) => <p key={i}>{e.message}</p>)}
        </div>
      )}

      {/* Dependencies */}
      {plan.dependencies.length > 0 && (
        <div>
          <h4 className="text-sm font-semibold mb-2">Dependencies</h4>
          <div className="space-y-1 max-h-40 overflow-y-auto">
            {plan.dependencies.map((dep, i) => (
              <DepRow key={i} dep={dep} checked={choices.optionalIncluded.has(dep.modJarId)} onToggle={onToggleOptional} />
            ))}
          </div>
        </div>
      )}

      {/* Conflicts */}
      {plan.conflicts.length > 0 && (
        <div>
          <h4 className="text-sm font-semibold mb-2">Conflicts</h4>
          <div className="space-y-2">
            {plan.conflicts.map((c, i) => (
              <ConflictRow key={i} conflict={c} selected={choices.conflictResolutions.get(c.conflictId)} onSelect={(r) => onResolveConflict(c.conflictId, r)} />
            ))}
          </div>
        </div>
      )}

      {/* File changes */}
      {(plan.filesToAdd.length > 0 || plan.filesToRemove.length > 0) && (
        <div>
          <h4 className="text-sm font-semibold mb-2">File Changes</h4>
          <p className="text-xs text-muted-foreground">
            {plan.filesToAdd.length > 0 && <span>+{plan.filesToAdd.length} to add </span>}
            {plan.filesToRemove.length > 0 && <span>-{plan.filesToRemove.length} to remove </span>}
            {plan.filesToDisable.length > 0 && <span>~{plan.filesToDisable.length} to disable</span>}
          </p>
        </div>
      )}

      {/* Snapshot info */}
      <div className="text-xs text-muted-foreground">
        Snapshot: {plan.snapshot.label} ({formatBytes(plan.snapshot.estimatedBytes)})
      </div>

      {/* Actions */}
      <div className="flex justify-end gap-2 pt-2">
        <button onClick={onCancel} className="rounded-lg border border-input px-4 py-2 text-sm font-medium hover:bg-accent">Cancel</button>
        <button
          onClick={onConfirm}
          disabled={!canInstall || hasUnresolvedBlockingConflict}
          className="rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
        >
          {hasUnresolvedBlockingConflict
            ? 'Resolve Conflicts First'
            : !canInstall
              ? 'Cannot Apply'
              : needsReplan
                ? 'Review Selected Changes'
                : actionLabel}
        </button>
      </div>
    </div>
  );
}

function DepRow({ dep, checked, onToggle }: { dep: ResolvedDep; checked: boolean; onToggle: (id: string, inc: boolean) => void }) {
  const isOptional = dep.requirement === 'optional';
  return (
    <div className="flex items-center gap-2 text-sm">
      {isOptional && (
        <input
          type="checkbox"
          checked={checked}
          onChange={(e) => onToggle(dep.modJarId, e.target.checked)}
          className="rounded"
        />
      )}
      <span className={isOptional ? '' : 'font-medium'}>{dep.modJarId}</span>
      <span className="text-xs text-muted-foreground">{dep.requirement}</span>
      {dep.disposition.type !== 'reuse-existing' && dep.disposition.type !== 'excluded' && (
        <span className="text-xs text-muted-foreground">⬇ will be installed</span>
      )}
      {dep.disposition.type === 'reuse-existing' && (
        <span className="text-xs text-green-600">✓ already installed</span>
      )}
    </div>
  );
}

function ConflictRow({ conflict, selected, onSelect }: { conflict: DepConflict; selected?: string; onSelect: (r: string) => void }) {
  return (
    <div className="rounded border border-border bg-muted p-2 text-sm">
      <p className="text-xs">{conflict.message}</p>
      <div className="flex gap-2 mt-1">
        {conflict.resolutionOptions.map((opt) => (
          <button
            key={opt}
            onClick={() => onSelect(opt)}
            className={`rounded px-2 py-0.5 text-xs border ${selected === opt ? 'bg-primary text-primary-foreground border-primary' : 'border-border hover:bg-accent'}`}
          >
            {opt}
          </button>
        ))}
      </div>
    </div>
  );
}

function ProgressView({ progress, onCancel }: { progress: ProgressEvent; onCancel: () => void }) {
  const phaseLabel = installPhaseLabel(progress.phase);
  const label = progress.message || phaseLabel;
  const hasBytes = progress.bytesTotal > 0;
  const hasSteps = progress.totalSteps > 0;
  const pct = hasBytes
    ? Math.round((progress.bytesDownloaded / progress.bytesTotal) * 100)
    : hasSteps
      ? Math.round((progress.step / progress.totalSteps) * 100)
      : null;

  return (
    <div className="space-y-4 py-4">
      <div className="flex items-center gap-3">
        <div className="h-5 w-5 animate-spin rounded-full border-2 border-primary border-t-transparent" />
        <div className="min-w-0">
          <p className="text-sm font-medium">{phaseLabel}</p>
          <p className="truncate text-xs text-muted-foreground" title={label}>{label}</p>
        </div>
      </div>
      {pct !== null && (
        <div className="space-y-1">
          <div className="h-2 rounded-full bg-muted overflow-hidden">
            <div className="h-full bg-primary transition-all duration-300" style={{ width: `${pct}%` }} />
          </div>
          <div className="flex justify-between text-xs text-muted-foreground">
            <span>{pct}%</span>
            {hasBytes ? (
              <span>{formatBytes(progress.bytesDownloaded)} / {formatBytes(progress.bytesTotal)}</span>
            ) : (
              <span>File {Math.min(progress.step, progress.totalSteps)} of {progress.totalSteps}</span>
            )}
          </div>
        </div>
      )}
      {pct === null && (
        <div className="h-2 overflow-hidden rounded-full bg-muted">
          <div className="h-full w-1/3 animate-pulse rounded-full bg-primary" />
        </div>
      )}
      <div className="flex justify-end">
        <button onClick={onCancel} className="rounded-lg border border-input px-4 py-2 text-sm font-medium hover:bg-accent">Cancel</button>
      </div>
    </div>
  );
}

function installPhaseLabel(phase: ProgressEvent['phase']): string {
  switch (phase) {
    case 'resolving': return 'Preparing installation';
    case 'staging': return 'Loading files';
    case 'snapshotting': return 'Creating recovery snapshot';
    case 'applying': return 'Applying instance changes';
    case 'health-scan': return 'Checking pack health';
    case 'done': return 'Finishing installation';
    case 'failed': return 'Installation failed';
    case 'cancelled': return 'Installation cancelled';
    default: return 'Installing';
  }
}

function ResultView({ outcome, instanceId, onOpenInstance, onClose }: {
  outcome: InstallOutcome;
  instanceId: string;
  onOpenInstance: () => void;
  onClose: () => void;
}) {
  const [rollbackState, setRollbackState] = useState<'idle' | 'restoring' | 'restored'>('idle');
  const [rollbackError, setRollbackError] = useState<string | null>(null);
  const snapshotId =
    outcome.type === 'success' || outcome.type === 'health-rollback' || outcome.type === 'failed'
      ? outcome.snapshotId
      : undefined;
  const canRestore =
    rollbackState !== 'restored'
    && (outcome.type === 'success'
      || (outcome.type === 'failed' && Boolean(outcome.snapshotId) && !outcome.rollbackPerformed));

  const rollback = async () => {
    if (!snapshotId) return;
    setRollbackState('restoring');
    setRollbackError(null);
    try {
      await restoreSnapshot(instanceId, snapshotId);
      setRollbackState('restored');
    } catch (cause) {
      setRollbackState('idle');
      setRollbackError(formatError(cause));
    }
  };

  return (
    <div className="space-y-4 py-4">
      {outcome.type === 'success' && (
        <>
          <div className="rounded-lg bg-green-500/10 p-3 text-sm text-green-700 dark:text-green-300">
            All verified changes were applied successfully.
          </div>
          <p className="text-xs text-muted-foreground">
            Recovery snapshot: {outcome.snapshotId}
          </p>
        </>
      )}
      {outcome.type === 'health-rollback' && (
        <div className="rounded-lg bg-amber-500/10 p-3 text-sm text-amber-700 dark:text-amber-300">
          The health scan found blockers, so Agora restored the recovery snapshot. No planned changes remain active.
        </div>
      )}
      {outcome.type === 'failed' && (
        <>
          <div className="rounded-lg bg-destructive/10 p-3 text-sm text-destructive">{outcome.error}</div>
          {outcome.rollbackPerformed && (
            <p className="text-xs text-muted-foreground">The recovery snapshot was restored automatically.</p>
          )}
          {outcome.snapshotId && !outcome.rollbackPerformed && (
            <p className="text-xs text-muted-foreground">Snapshot {outcome.snapshotId} is available for recovery.</p>
          )}
        </>
      )}
      {outcome.type === 'cancelled' && (
        <div className="rounded-lg bg-muted p-3 text-sm text-muted-foreground">
          The operation was cancelled before live instance changes were committed.
        </div>
      )}
      {rollbackState === 'restored' && (
        <div className="rounded-lg bg-green-500/10 p-3 text-sm text-green-700 dark:text-green-300">
          The recovery snapshot was restored.
        </div>
      )}
      {rollbackError && (
        <div className="rounded-lg bg-destructive/10 p-3 text-sm text-destructive">
          Restore failed: {rollbackError}
        </div>
      )}
      <div className="flex justify-end gap-2">
        <button onClick={onClose} className="rounded-lg border border-input px-4 py-2 text-sm font-medium hover:bg-accent">
          Close
        </button>
        {canRestore && (
          <button
            onClick={() => { void rollback(); }}
            disabled={rollbackState === 'restoring'}
            className="rounded-lg border border-destructive/40 px-4 py-2 text-sm font-medium text-destructive hover:bg-destructive/10 disabled:opacity-50"
          >
            {rollbackState === 'restoring' ? 'Restoring…' : 'Roll Back'}
          </button>
        )}
        {(outcome.type === 'success' || outcome.type === 'health-rollback' || rollbackState === 'restored') && (
          <button onClick={onOpenInstance} className="rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90">
            Open Instance
          </button>
        )}
      </div>
    </div>
  );
}

function ErrorView({ message, retryable, onRetry, onClose }: {
  message: string;
  retryable: boolean;
  onRetry: () => void;
  onClose: () => void;
}) {
  return (
    <div className="space-y-4 py-4">
      <div className="rounded-lg bg-destructive/10 p-3 text-sm text-destructive">{message}</div>
      <div className="flex justify-end gap-2">
        <button onClick={onClose} className="rounded-lg border border-input px-4 py-2 text-sm font-medium hover:bg-accent">Close</button>
        {retryable && <button onClick={onRetry} className="rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90">Retry</button>}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function formatBytes(bytes: number): string {
  if (bytes >= 1_000_000_000) return `${(bytes / 1_000_000_000).toFixed(1)} GB`;
  if (bytes >= 1_000_000) return `${(bytes / 1_000_000).toFixed(1)} MB`;
  if (bytes >= 1_000) return `${(bytes / 1_000).toFixed(1)} KB`;
  return `${bytes} B`;
}
