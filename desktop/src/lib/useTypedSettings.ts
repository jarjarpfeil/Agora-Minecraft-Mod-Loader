import { useCallback, useEffect, useRef, useState } from 'react';
import { formatError, getSetting, setSetting } from './tauri';

// ---------------------------------------------------------------------------
// Typed setting definitions
// ---------------------------------------------------------------------------

/** Each setting is defined with a key, a parser, and a stored-value mapper. */
interface SettingDef<T> {
  key: string;
  parse: (raw: unknown) => T;
  toStoredValue: (value: T) => unknown;
}

export function boolDef(key: string): SettingDef<boolean> {
  return {
    key,
    parse(raw) {
      if (typeof raw === 'boolean') return raw;
      if (typeof raw === 'string') return raw === 'true' || raw === '1';
      if (typeof raw === 'number') return raw === 1;
      return false;
    },
    toStoredValue: (v) => v,
  };
}

export function stringDef(key: string): SettingDef<string> {
  return {
    key,
    parse(raw) {
      if (typeof raw === 'string') return raw;
      if (typeof raw === 'number' || typeof raw === 'boolean') return String(raw);
      return '';
    },
    toStoredValue: (v) => v,
  };
}

export function numberDef(key: string, fallback: number): SettingDef<number> {
  return {
    key,
    parse(raw) {
      if (typeof raw === 'number') return Number.isFinite(raw) ? raw : fallback;
      if (typeof raw === 'string') {
        const n = Number(raw);
        return Number.isFinite(n) ? n : fallback;
      }
      return fallback;
    },
    toStoredValue: (v) => v,
  };
}

export function enumDef<T extends string>(key: string, valid: readonly T[], fallback: T): SettingDef<T> {
  return {
    key,
    parse(raw) {
      if (typeof raw === 'string' && valid.includes(raw as T)) return raw as T;
      return fallback;
    },
    toStoredValue: (v) => v,
  };
}

/** Nullable setting: empty string / `null` / missing key → `null` */
export function nullableDef<T>(key: string, inner: SettingDef<T>): SettingDef<T | null> {
  return {
    key,
    parse(raw) {
      if (raw === null || raw === undefined || raw === '') return null;
      return inner.parse(raw);
    },
    toStoredValue: (v) => (v === null ? null : inner.toStoredValue(v)),
  };
}

// ---------------------------------------------------------------------------
// Per-key status type
// ---------------------------------------------------------------------------

export type SettingStatus = 'idle' | 'reading' | 'ready' | 'write-pending' | 'error';

export interface SettingEntry {
  status: SettingStatus;
  error?: string;
}

// ---------------------------------------------------------------------------
// Define all known settings
// ---------------------------------------------------------------------------

export const SETTINGS = {
  modrinthEnabled: boolDef('modrinth_enabled'),
  aiMcpEnabled: boolDef('ai_mcp_enabled'),
  aiChatEnabled: boolDef('ai_chat_enabled'),
  launcherPath: stringDef('mojang_launcher_path'),
  javaPath: nullableDef('java_path', stringDef('java_path')),
  javaRuntimeMode: enumDef('java_runtime_mode', ['automatic', 'prompt', 'manual'] as const, 'automatic'),
  alwaysPreTouch: boolDef('always_pre_touch'),
  launchMode: enumDef('launch_mode', ['direct', 'delegation'] as const, 'delegation'),
  onboardingComplete: boolDef('onboarding_complete'),
} as const;

// ---------------------------------------------------------------------------
// Hook
// ---------------------------------------------------------------------------

export interface UseTypedSettingsReturn {
  /** All parsed settings. Failed-to-load settings hold their default. */
  values: Record<string, unknown>;
  /** Per-key status covering read and write transitions. */
  statuses: Record<string, SettingEntry>;
  /** True while any read is in flight (legacy convenience). */
  loading: boolean;
  /** Per-key errors (legacy convenience). */
  errors: Record<string, string>;
  /** Update a single setting in the backend and locally. */
  update: <T>(def: SettingDef<T>, value: T) => Promise<void>;
  /** Reload all defined settings. */
  reload: () => Promise<void>;
}

/** Read all defined settings from the backend with independent error handling. */
async function readAll(): Promise<{
  values: Record<string, unknown>;
  statuses: Record<string, SettingEntry>;
}> {
  const values: Record<string, unknown> = {};
  const statuses: Record<string, SettingEntry> = {};

  const results = await Promise.allSettled(
    Object.entries(SETTINGS).map(async ([name, def]) => {
      try {
        const raw = await getSetting(def.key);
        return { name, key: def.key, value: def.parse(raw) };
      } catch (e) {
        // Tag the rejection with both identifiers: values use the typed
        // property name while status/error UI consistently uses the persisted key.
        // handler below can attribute it correctly.
        throw Object.assign(e instanceof Error ? e : new Error(formatError(e)), {
          settingName: name,
          settingKey: def.key,
        });
      }
    }),
  );

  for (const result of results) {
    if (result.status === 'fulfilled') {
      values[result.value.name] = result.value.value;
      statuses[result.value.key] = { status: 'ready' };
    } else {
      const reason = result.reason as Error & { settingName?: string; settingKey?: string };
      const name = reason.settingName ?? 'unknown';
      const key = reason.settingKey ?? name;
      statuses[key] = { status: 'error', error: formatError(reason) };
      // Also store a default value so callers don't crash on missing keys
      values[name] = null;
    }
  }

  return { values, statuses };
}

export function useTypedSettings(): UseTypedSettingsReturn {
  const [values, setValues] = useState<Record<string, unknown>>({});
  const [statuses, setStatuses] = useState<Record<string, SettingEntry>>({});
  const [loading, setLoading] = useState(true);
  const mountedRef = useRef(true);

  useEffect(() => {
    mountedRef.current = true;
    return () => { mountedRef.current = false; };
  }, []);

  const reload = useCallback(async () => {
    setLoading(true);
    const result = await readAll();
    if (mountedRef.current) {
      setValues(result.values);
      setStatuses(result.statuses);
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    reload();
  }, [reload]);

  const update = useCallback(
    async <T>(def: SettingDef<T>, value: T) => {
      // Map persisted key back to typed property name so `values` stays
      // consistent with the initial `readAll` shape.
      const name =
        Object.entries(SETTINGS).find(([, d]) => d.key === def.key)?.[0] ?? def.key;

      setStatuses((prev) => ({
        ...prev,
        [def.key]: { status: 'write-pending' },
      }));

      const storedValue = def.toStoredValue(value);
      try {
        await setSetting(def.key, storedValue);
        if (mountedRef.current) {
          setValues((prev) => ({ ...prev, [name]: value }));
          setStatuses((prev) => ({
            ...prev,
            [def.key]: { status: 'ready' },
          }));
        }
      } catch (e) {
        const msg = formatError(e);
        if (mountedRef.current) {
          setStatuses((prev) => ({
            ...prev,
            [def.key]: { status: 'error', error: msg },
          }));
        }
        // Rethrow so callers' try/catch can roll back optimistic state and show toasts.
        throw new Error(msg);
      }
    },
    [],
  );

  // Legacy convenience: derive errors from statuses
  const errors: Record<string, string> = {};
  for (const [key, entry] of Object.entries(statuses)) {
    if (entry.status === 'error' && entry.error) {
      errors[key] = entry.error;
    }
  }

  return { values, statuses, loading, errors, update, reload };
}
