import { useCallback, useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import {
  cancelJavaRuntime,
  checkInstanceHealth,
  ensureJavaRuntime,
  formatError,
  getSetting,
  inspectJavaExecutable,
  killProcess,
  launchInstance,
  launchInstanceDirect,
  parseLauncherError,
  pickOpenFile,
  queryLaunchState,
  repairInstanceLoader,
  updateInstanceJava,
  type HealthBlocker,
  type HealthReport,
  type HealthWarning,
  type LauncherAction,
  type JavaRuntimeProgressEvent,
  type RecoverableJavaIssue,
  type RecoverableProfileIssue,
} from './tauri';

function silenceKey(item: HealthWarning | HealthBlocker): string {
  return `health_silenced_${item.kind}_${item.mod_id ?? 'global'}`;
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type LaunchPhase =
  | 'idle'
  | 'checking-health'
  | 'awaiting-decision'
  | 'launching'
  | 'running'
  | 'stopping'
  | 'delegated'
  | 'exited'
  | 'failed';

export interface ProcessState {
  phase: LaunchPhase;
  instanceId: string | null;
  pid: number | null;
  error: string | null;
  healthReport: HealthReport | null;
  /** The launch mode (direct vs delegated) captured at the start of the launch flow. */
  directLaunch: boolean;
  exitCode: number | null;
  outcome: 'success' | 'crash' | 'cancelled' | 'unknown' | 'abandoned' | null;
  snapshotId: string | null;
  exitedAt: string | null;
  /** Structured recoverable profile issue (populated on direct-launch profile adoption failures). */
  recoverableIssue: RecoverableProfileIssue | null;
  /** Structured recoverable Java issue (populated on Java runtime missing/catalog errors). */
  recoverableJavaIssue: RecoverableJavaIssue | null;
  /** Progress of Java runtime provisioning. */
  runtimeProgress: JavaRuntimeProgressEvent | null;
  /** Available user actions for the current recoverable issue. */
  availableActions: LauncherAction[];
}

// ---------------------------------------------------------------------------
// Controller hook — intended to live at App level and survive page navigation.
// ---------------------------------------------------------------------------

export interface ProcessController {
  state: ProcessState;
  /** Bounded log buffer for the tracked instance. */
  logs: LogLine[];
  /** Start a health-gated launch. Shows the health dialog when warnings/blockers exist. */
  /** Returns true only when a launch command actually started. Health-deferred,
   * concurrent, and failed attempts return false. */
  startLaunch: (instanceId: string, directLaunch: boolean) => Promise<boolean>;
  /** Continue a launch after the user approved health warnings. Uses the mode captured in startLaunch. Returns null on success or an error string. */
  approveLaunch: () => Promise<string | null>;
  /** Cancel the launch flow (health dialog dismissal). */
  cancelLaunch: () => void;
  /** Kill the running process. */
  kill: () => Promise<void>;
  /** Clear a terminal error. */
  clearError: () => void;
  /**
   * Reinstall the current instance's loader and retry direct launch.
   * Phase is set to 'launching' during the repair-and-retry flow.
   * If repair or retry fails the parsed error is preserved.
   * Rejects with an error string if no instance or not in a failed/recoverable state.
   */
  repairAndRetry: () => Promise<void>;
  /**
   * Explicitly switch to delegated launch for the current instance,
   * bypassing only the Direct profile adoption (health checks already completed).
   * Calls executeLaunch(..., false) and transitions to 'delegated'.
   * Rejects with an error string if no instance is set.
   */
  useDelegatedLaunch: () => Promise<void>;
  /**
   * Download a Java runtime for the required major version and retry launch.
   * Calls ensureJavaRuntime(major) then re-launches.
   */
  downloadRuntimeAndRetry: () => Promise<void>;
  /**
   * Open file picker, inspect the selected Java executable, update the instance
   * Java path, and retry launch. Optionally allows incompatible major override.
   */
  chooseJavaAndRetry: () => Promise<void>;
  /**
   * Clear the Java recovery issue without launching.
   */
  cancelJavaRecovery: () => void;
  /**
   * Cancel an in-progress Java runtime provisioning for the tracked instance.
   * Sets phase to 'failed' with a cancelled message on success.
   */
  cancelJavaRuntimeForInstance: () => Promise<void>;
}

const INITIAL_STATE: ProcessState = {
  phase: 'idle',
  instanceId: null,
  pid: null,
  error: null,
  healthReport: null,
  directLaunch: false,
  exitCode: null,
  outcome: null,
  snapshotId: null,
  exitedAt: null,
  recoverableIssue: null,
  recoverableJavaIssue: null,
  runtimeProgress: null,
  availableActions: [],
};

// Bounded log buffer per instance ID.
const MAX_LOG_LINES = 5000;

export interface LogLine {
  line: string;
  stream: 'stdout' | 'stderr';
  instance_id: string;
}

export function useProcessController(): ProcessController {
  const [state, setState] = useState<ProcessState>(INITIAL_STATE);
  const [logs, setLogs] = useState<LogLine[]>([]);
  const stateRef = useRef(state);
  stateRef.current = state;

  // Hydrate from backend on mount — recover running state after reload.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const running = await queryLaunchState();
        if (!cancelled && running) {
          setState({
            phase: 'running',
            instanceId: running.instance_id,
            pid: running.pid,
            error: null,
            healthReport: null,
            directLaunch: true,
            exitCode: null,
            outcome: null,
            snapshotId: null,
            exitedAt: null,
            recoverableIssue: null,
            recoverableJavaIssue: null,
            runtimeProgress: null,
            availableActions: [],
          });
        }
      } catch {
        // Backend unavailable — stay with default idle state.
      }
    })();
    return () => { cancelled = true; };
  }, []);

  // Preserve the terminal outcome so users can see whether the last session
  // succeeded, crashed, or was cancelled after the process exits.
  useEffect(() => {
    const unlisten = listen<{
      instance_id: string;
      exit_code: number | null;
      outcome: 'success' | 'crash' | 'cancelled' | 'unknown' | 'abandoned';
      snapshot_id: string;
    }>(
      'game-exited',
      (event) => {
        const current = stateRef.current;
        if (
          current.instanceId === event.payload.instance_id &&
          (current.phase === 'running' || current.phase === 'stopping' || current.phase === 'delegated')
        ) {
          setState((previous) => ({
            ...previous,
            phase: 'exited',
            pid: null,
            error: null,
            healthReport: null,
            recoverableIssue: null,
            recoverableJavaIssue: null,
            runtimeProgress: null,
            availableActions: [],
            exitCode: event.payload.exit_code,
            outcome: event.payload.outcome,
            snapshotId: event.payload.snapshot_id,
            exitedAt: new Date().toISOString(),
          }));
        }
      },
    );
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Listen for game-log events and buffer them.
  useEffect(() => {
    const unlisten = listen<{ line: string; stream: string; instance_id: string }>(
      'game-log',
      (event) => {
        const current = stateRef.current;
        // Only buffer logs for the tracked instance.
        if (current.instanceId !== event.payload.instance_id) return;
        setLogs((prev) => {
          const next = [...prev, {
            line: event.payload.line,
            stream: event.payload.stream as 'stdout' | 'stderr',
            instance_id: event.payload.instance_id,
          }];
          if (next.length > MAX_LOG_LINES) {
            return next.slice(-MAX_LOG_LINES);
          }
          return next;
        });
      },
    );
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Listen for java-runtime-progress events.
  useEffect(() => {
    const unlisten = listen<JavaRuntimeProgressEvent>(
      'java-runtime-progress',
      (event) => {
        const current = stateRef.current;
        // Only track progress for the monitored instance.
        if (current.instanceId && current.instanceId !== event.payload.instance_id) return;
        setState((prev) => ({
          ...prev,
          runtimeProgress: event.payload,
        }));
      },
    );
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const clearError = useCallback(() => {
    setState((prev) => ({
      ...prev,
      error: null,
      phase: 'idle',
      recoverableIssue: null,
      recoverableJavaIssue: null,
      runtimeProgress: null,
      availableActions: [],
    }));
  }, []);

  // ---- Java recovery helpers ----

  const downloadRuntimeAndRetry = useCallback(async () => {
    const current = stateRef.current;
    if (!current.instanceId) return;
    const javaIssue = current.recoverableJavaIssue;
    if (!javaIssue) return;

    setState((prev) => ({
      ...prev,
      phase: 'launching',
      error: null,
      recoverableIssue: null,
      recoverableJavaIssue: null,
      availableActions: [],
    }));

    try {
      // Provision the runtime
      await ensureJavaRuntime(javaIssue.major);

      // Retry direct launch
      const newState = await executeLaunch(current.instanceId, true);
      setState(newState);
    } catch (e) {
      const parsed = parseLauncherError(e);
      setState((prev) => ({
        ...prev,
        phase: 'failed',
        error: parsed.message,
        recoverableIssue: parsed.recoverableIssue,
        recoverableJavaIssue: parsed.recoverableJavaIssue,
        availableActions: parsed.availableActions,
      }));
    }
  }, []);

  const chooseJavaAndRetry = useCallback(async () => {
    const current = stateRef.current;
    if (!current.instanceId) return;
    const javaIssue = current.recoverableJavaIssue;
    if (!javaIssue) return;

    setState((prev) => ({
      ...prev,
      phase: 'launching',
      error: null,
    }));

    try {
      // Open file picker for Java executable
      const chosen = await pickOpenFile('Select Java executable', ['exe', 'bat', 'sh', 'java']);
      if (!chosen) {
        // User cancelled the picker — go back to failed state
        setState((prev) => ({
          ...prev,
          phase: 'failed',
          recoverableJavaIssue: javaIssue,
          availableActions: ['download_runtime', 'choose_java', 'cancel'],
        }));
        return;
      }

      // Inspect the selected executable
      await inspectJavaExecutable(chosen);

      // Save as per-instance Java path
      await updateInstanceJava(current.instanceId, chosen, false);

      // Retry direct launch
      const newState = await executeLaunch(current.instanceId, true);
      setState(newState);
    } catch (e) {
      const parsed = parseLauncherError(e);
      setState((prev) => ({
        ...prev,
        phase: 'failed',
        error: parsed.message,
        recoverableJavaIssue: javaIssue,
        availableActions: ['download_runtime', 'choose_java', 'cancel'],
      }));
    }
  }, []);

  const cancelJavaRecovery = useCallback(() => {
    setState((prev) => ({
      ...prev,
      phase: 'idle',
      error: null,
      recoverableJavaIssue: null,
      runtimeProgress: null,
      availableActions: [],
    }));
  }, []);

  const cancelJavaRuntimeForInstance = useCallback(async () => {
    const current = stateRef.current;
    if (!current.instanceId) return;
    const opId = `java-runtime-${current.instanceId}-${current.runtimeProgress?.major ?? 0}`;
    try {
      await cancelJavaRuntime(opId);
      setState((prev) => ({
        ...prev,
        phase: 'failed',
        error: 'Java runtime provisioning was cancelled.',
        recoverableJavaIssue: prev.recoverableJavaIssue,
        availableActions: ['cancel'],
        runtimeProgress: null,
      }));
    } catch {
      // Operation may already be done — nothing to do
    }
  }, []);

  // ---- End Java recovery helpers ----

  const startLaunch = useCallback(
    async (instanceId: string, directLaunch: boolean) => {
      // Reject if any non-terminal phase is active (concurrent-launch guard).
      const current = stateRef.current;
      const activePhases: LaunchPhase[] = ['checking-health', 'awaiting-decision', 'launching', 'running'];
      if (activePhases.includes(current.phase)) {
        setState((prev) => ({
          ...prev,
          error: 'A launch is already in progress. Wait for it to complete before launching another instance.',
        }));
        return false;
      }

      setState({
        phase: 'checking-health',
        instanceId,
        pid: null,
        error: null,
        healthReport: null,
        directLaunch,
        exitCode: null,
        outcome: null,
        snapshotId: null,
        exitedAt: null,
        recoverableIssue: null,
        recoverableJavaIssue: null,
        runtimeProgress: null,
        availableActions: [],
      });

      try {
        const report = await checkInstanceHealth(instanceId);

        // Check which warnings/blockers the user has muted.
        const silencedKeys = new Set(
          (
            await Promise.all(
              [...report.warnings, ...report.blockers].map(async (item) => {
                const key = silenceKey(item);
                const val = await getSetting(key);
                return val === 'true' ? key : null;
              }),
            )
          ).filter((k): k is string => k !== null),
        );

        const activeBlockers = report.blockers.filter(b => !silencedKeys.has(silenceKey(b)));
        const activeWarnings = report.warnings.filter(w => !silencedKeys.has(silenceKey(w)));

        if (activeBlockers.length > 0 || activeWarnings.length > 0) {
          setState((prev) => ({
            ...prev,
            phase: 'awaiting-decision',
            healthReport: report,
          }));
          return false;
        }

        // All clear — launch immediately with the captured mode.
        const newState = await executeLaunch(instanceId, directLaunch);
        setState(newState);
        return true;
      } catch (e) {
        const parsed = parseLauncherError(e);
        setState((prev) => ({
          ...prev,
          phase: 'failed',
          error: parsed.message,
          recoverableIssue: parsed.recoverableIssue,
          recoverableJavaIssue: parsed.recoverableJavaIssue,
          availableActions: parsed.availableActions,
        }));
        return false;
      }
    },
    [],
  );

  const approveLaunch = useCallback(
    async (): Promise<string | null> => {
      const current = stateRef.current;
      if (!current.instanceId) return 'No instance selected';

      setState((prev) => ({ ...prev, phase: 'launching', error: null, healthReport: prev.healthReport }));

      try {
        const newState = await executeLaunch(current.instanceId, current.directLaunch);
        setState(newState);
        return null;
      } catch (e) {
        const parsed = parseLauncherError(e);
        // Stay in awaiting-decision so the HealthDialog remains open
        // with the error visible. The user can try again or cancel.
        setState((prev) => ({
          ...prev,
          phase: 'awaiting-decision',
          error: parsed.message,
          recoverableIssue: parsed.recoverableIssue,
          recoverableJavaIssue: parsed.recoverableJavaIssue,
          availableActions: parsed.availableActions,
        }));
        return parsed.message;
      }
    },
    [],
  );

  const cancelLaunch = useCallback(() => {
    setState(INITIAL_STATE);
  }, []);

  const kill = useCallback(async () => {
    const current = stateRef.current;
    // Delegated launches have no owned PID — nothing to kill.
    if (current.pid == null) return;
    setState((previous) => ({ ...previous, phase: 'stopping', error: null }));
    try {
      await killProcess(current.pid);
      // The backend retains ownership until its process waiter emits the
      // classified game-exited event.
    } catch (e) {
      const msg = formatError(e);
      // ERR_PROCESS_STALE means the process identity no longer matches —
      // the backend already detached the stale record.  Treat as idle
      // rather than stuck in a retry loop.
      if (msg.includes('ERR_PROCESS_STALE') || msg.includes('stale')) {
        setState((prev) => ({
          ...prev,
          phase: 'idle',
          pid: null,
          error: null,
        }));
        return;
      }
      setState((prev) => ({
        ...prev,
        phase: 'running',
        error: msg,
      }));
    }
  }, []);

  const repairAndRetry = useCallback(async () => {
    const current = stateRef.current;
    if (!current.instanceId) throw new Error('No instance selected');

    // Prevent concurrent repair actions.
    if (current.phase === 'launching') return;

    setState((prev) => ({
      ...prev,
      phase: 'launching',
      error: null,
      recoverableIssue: null,
      recoverableJavaIssue: null,
      runtimeProgress: null,
      availableActions: [],
    }));

    try {
      // Reinstall the loader.
      await repairInstanceLoader(current.instanceId);

      // Retry direct launch.
      const newState = await executeLaunch(current.instanceId, true);
      setState(newState);
    } catch (e) {
      const parsed = parseLauncherError(e);
      setState((prev) => ({
        ...prev,
        phase: 'failed',
        error: parsed.message,
        recoverableIssue: parsed.recoverableIssue,
        recoverableJavaIssue: parsed.recoverableJavaIssue,
        availableActions: parsed.availableActions,
      }));
    }
  }, []);

  const useDelegatedLaunch = useCallback(async () => {
    const current = stateRef.current;
    if (!current.instanceId) throw new Error('No instance selected');

    // Prevent concurrent actions.
    if (current.phase === 'launching') return;

    setState((prev) => ({
      ...prev,
      phase: 'launching',
      error: null,
      recoverableIssue: null,
      recoverableJavaIssue: null,
      runtimeProgress: null,
      availableActions: [],
    }));

    try {
      // Use delegated launch (bypasses direct profile adoption).
      await launchInstance(current.instanceId);
      setState({
        phase: 'delegated',
        instanceId: current.instanceId,
        pid: null,
        error: null,
        healthReport: null,
        directLaunch: false,
        exitCode: null,
        outcome: null,
        snapshotId: null,
        exitedAt: null,
        recoverableIssue: null,
        recoverableJavaIssue: null,
        runtimeProgress: null,
        availableActions: [],
      });
    } catch (e) {
      const parsed = parseLauncherError(e);
      setState((prev) => ({
        ...prev,
        phase: 'failed',
        error: parsed.message,
        recoverableIssue: parsed.recoverableIssue,
        recoverableJavaIssue: parsed.recoverableJavaIssue,
        availableActions: parsed.availableActions,
      }));
    }
  }, []);

  return {
    state,
    logs,
    startLaunch,
    approveLaunch,
    cancelLaunch,
    kill,
    clearError,
    repairAndRetry,
    useDelegatedLaunch,
    downloadRuntimeAndRetry,
    chooseJavaAndRetry,
    cancelJavaRecovery,
    cancelJavaRuntimeForInstance,
  };
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

function launchedState(
  instanceId: string,
  directLaunch: boolean,
  pid: number | null,
): ProcessState {
  return {
    phase: directLaunch ? 'running' : 'delegated',
    instanceId,
    pid: directLaunch ? pid : null,
    error: null,
    healthReport: null,
    directLaunch,
    exitCode: null,
    outcome: null,
    snapshotId: null,
    exitedAt: null,
    recoverableIssue: null,
    recoverableJavaIssue: null,
    runtimeProgress: null,
    availableActions: [],
  };
}

async function executeLaunch(
  instanceId: string,
  directLaunch: boolean,
): Promise<ProcessState> {
  if (directLaunch) {
    const pid = await launchInstanceDirect(instanceId);
    return launchedState(instanceId, true, pid);
  } else {
    await launchInstance(instanceId);
    return launchedState(instanceId, false, null);
  }
}
