import { useEffect, useState } from 'react';
import { formatError, getSetting, setSetting } from '../lib/tauri';

export function Settings() {
  const [modrinth, setModrinth] = useState(false);
  const [aiMcp, setAiMcp] = useState(false);
  const [launcherPath, setLauncherPath] = useState('');
  const [alwaysPreTouch, setAlwaysPreTouch] = useState(true);
  const [crashTelemetry, setCrashTelemetry] = useState(false);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const [m, a, p, apt, ct] = await Promise.all([
          getSetting('modrinth_enabled'),
          getSetting('ai_mcp_enabled'),
          getSetting('mojang_launcher_path'),
          getSetting('jvm_always_pre_touch'),
          getSetting('crash_telemetry_opt_in'),
        ]);
        if (cancelled) return;
        setModrinth(Boolean(m));
        setAiMcp(Boolean(a));
        if (typeof p === 'string') setLauncherPath(p);
        if (typeof apt === 'boolean') setAlwaysPreTouch(apt);
        if (typeof ct === 'boolean') setCrashTelemetry(ct);
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const toggleModrinth = async (value: boolean) => {
    setModrinth(value);
    try {
      await setSetting('modrinth_enabled', value);
    } catch (e) {
      setModrinth(!value);
      alert(formatError(e));
    }
  };

  const toggleAiMcp = async (value: boolean) => {
    setAiMcp(value);
    try {
      await setSetting('ai_mcp_enabled', value);
    } catch (e) {
      setAiMcp(!value);
      alert(formatError(e));
    }
  };

  const saveLauncherPath = async () => {
    try {
      await setSetting('mojang_launcher_path', launcherPath);
    } catch (e) {
      alert(formatError(e));
    }
  };

  const toggleAlwaysPreTouch = async (value: boolean) => {
    setAlwaysPreTouch(value);
    try {
      await setSetting('jvm_always_pre_touch', value);
    } catch (e) {
      setAlwaysPreTouch(!value);
      alert(formatError(e));
    }
  };

  const toggleCrashTelemetry = async (value: boolean) => {
    setCrashTelemetry(value);
    try {
      await setSetting('crash_telemetry_opt_in', value);
    } catch (e) {
      setCrashTelemetry(!value);
      alert(formatError(e));
    }
  };

  return (
    <div className="space-y-6">
      <section>
        <h2 className="text-2xl font-bold mb-2">⚙️ Settings</h2>
        <p className="text-[rgb(var(--muted))]">
          Integration toggles, launcher path, and application preferences.
        </p>
      </section>

      {loading ? (
        <p className="text-[rgb(var(--muted))]">Loading settings…</p>
      ) : (
        <>
          <div className="rounded-xl border border-gray-200 dark:border-gray-700 surface p-4 space-y-4">
            <h3 className="font-semibold">External Services</h3>

            <label className="flex items-center justify-between">
              <span className="text-sm">Modrinth Access</span>
              <input
                type="checkbox"
                checked={modrinth}
                onChange={(e) => toggleModrinth(e.target.checked)}
                className="h-5 w-5 accent-brand-600"
              />
            </label>
            <p className="text-xs text-[rgb(var(--muted))]">
              Allow live Modrinth API queries and show Modrinth-sourced curated mods.
            </p>

            <label className="flex items-center justify-between pt-2 border-t border-gray-200 dark:border-gray-700">
              <span className="text-sm">AI / MCP Server</span>
              <input
                type="checkbox"
                checked={aiMcp}
                onChange={(e) => toggleAiMcp(e.target.checked)}
                className="h-5 w-5 accent-brand-600"
              />
            </label>
            <p className="text-xs text-[rgb(var(--muted))]">
              Enable the local MCP server for external AI tools.
            </p>
          </div>

          <div className="rounded-xl border border-gray-200 dark:border-gray-700 surface p-4 space-y-3">
            <h3 className="font-semibold">Launcher Path</h3>
            <input
              value={launcherPath}
              onChange={(e) => setLauncherPath(e.target.value)}
              placeholder="Auto-discovered if empty"
              className="w-full rounded-lg border border-gray-300 dark:border-gray-600 bg-transparent px-3 py-2 text-sm"
            />
            <button
              onClick={saveLauncherPath}
              className="rounded-lg bg-brand-600 px-4 py-2 text-sm font-medium text-white hover:bg-brand-700"
            >
              Save
            </button>
            <p className="text-xs text-[rgb(var(--muted))]">
              Override the official Mojang launcher executable location.
            </p>
          </div>

          <div className="rounded-xl border border-gray-200 dark:border-gray-700 surface p-4 space-y-3">
            <h3 className="font-semibold">JVM Defaults</h3>
            <label className="flex items-center justify-between">
              <span className="text-sm">AlwaysPreTouch</span>
              <input
                type="checkbox"
                checked={alwaysPreTouch}
                onChange={(e) => toggleAlwaysPreTouch(e.target.checked)}
                className="h-5 w-5 accent-brand-600"
              />
            </label>
            <p className="text-xs text-[rgb(var(--muted))]">
              Recommended for G1GC, may cause issues with ZGC/Shenandoah.
            </p>
          </div>

          <div className="rounded-xl border border-gray-200 dark:border-gray-700 surface p-4 space-y-3">
            <h3 className="font-semibold">Crash Telemetry</h3>
            <label className="flex items-center justify-between">
              <span className="text-sm">Allow anonymous crash telemetry</span>
              <input
                type="checkbox"
                checked={crashTelemetry}
                onChange={(e) => toggleCrashTelemetry(e.target.checked)}
                className="h-5 w-5 accent-brand-600"
              />
            </label>
            <p className="text-xs text-[rgb(var(--muted))]">
              Allow anonymous local crash telemetry to be collected for mod-incompatibility research. Aggregates are never uploaded unless you opt in. Saying no disables all telemetry.
            </p>
            <p className="text-xs text-[rgb(var(--muted))] mt-2">
              Local crash learning (mod isolation & co-crash detection) runs automatically and never leaves your machine. This toggle only controls future anonymous aggregate sharing, which is not yet active.
            </p>
          </div>
        </>
      )}
    </div>
  );
}
