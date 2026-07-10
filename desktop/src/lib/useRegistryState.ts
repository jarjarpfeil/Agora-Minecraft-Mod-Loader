import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  checkRegistryUpdate,
  formatError,
  getRegistryStatus,
  type RegistryStatus,
} from './tauri';

/**
 * Registry availability state for the current session.
 *
 * | State | Meaning |
 * |---|---|
 * | `unknown` | Status not yet loaded. |
 * | `loading` | A sync (download/check) is in flight. |
 * | `ready` | Cached database exists and no error from last sync. |
 * | `offline` | Cached database exists but last sync failed. Continue offline. |
 * | `missing` | No cached database — recovery action required. |
 */
export type RegistryState =
  | 'unknown'
  | 'loading'
  | 'ready'
  | 'offline'
  | 'missing';

export interface RegistryActions {
  /** Check for updates / download registry (calls checkRegistryUpdate(true)). */
  sync: () => Promise<RegistryStatus | null>;
  /** Re-read status without network access. */
  refreshStatus: () => Promise<void>;
  /** Clear displayed error without retrying. */
  clearError: () => void;
}

export function useRegistryState(): {
  state: RegistryState;
  status: RegistryStatus | null;
  loading: boolean;
  error: string | null;
  hasCachedDb: boolean;
  actions: RegistryActions;
} {
  const [status, setStatus] = useState<RegistryStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const mountedRef = useRef(true);

  // Reset mountedRef in the effect setup so React Strict Mode's
  // development-only double-invoke does not leave it permanently false.
  // See comment in GithubStep for why the unmount-ref pattern is fragile.
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const refreshStatus = useCallback(async () => {
    try {
      const s = await getRegistryStatus();
      if (mountedRef.current) setStatus(s);
    } catch (e) {
      // Expose the error in `unknown` state so the UI can show a Retry action
      if (mountedRef.current) setError(formatError(e));
    }
  }, []);

  const sync = useCallback(async () => {
    if (mountedRef.current) setLoading(true);
    if (mountedRef.current) setError(null);
    try {
      const result = await checkRegistryUpdate(true);
      if (mountedRef.current) {
        setStatus(result);
        setError(null);
      }
      return result;
    } catch (e) {
      if (mountedRef.current) setError(formatError(e));
      return null;
    } finally {
      if (mountedRef.current) setLoading(false);
    }
  }, []);

  const clearError = useCallback(() => {
    if (mountedRef.current) setError(null);
  }, []);

  // Stable actions object — does not change every render.
  const actions = useMemo(
    () => ({ sync, refreshStatus, clearError }),
    [sync, refreshStatus, clearError],
  );

  // Initial load
  useEffect(() => {
    refreshStatus().finally(() => {
      if (mountedRef.current) setLoading(false);
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const hasCachedDb = status?.has_cached_db ?? false;

  // Derive state
  let state: RegistryState;
  if (status === null && !loading) {
    state = 'unknown';
  } else if (loading) {
    state = 'loading';
  } else if (hasCachedDb && error === null) {
    state = 'ready';
  } else if (hasCachedDb && error !== null) {
    state = 'offline';
  } else {
    // !hasCachedDb
    state = 'missing';
  }

  return {
    state,
    status,
    loading,
    error,
    hasCachedDb,
    actions,
  };
}
