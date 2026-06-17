import { useEffect, useState } from 'react';
import { getSetting, setSetting } from '../lib/tauri';

export function Settings() {
  const [modrinth, setModrinth] = useState(false);
  const [aiMcp, setAiMcp] = useState(false);
  const [launcherPath, setLauncherPath] = useState('');
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const [m, a, p] = await Promise.all([
          getSetting('modrinth_enabled'),
          getSetting('ai_mcp_enabled'),
          getSetting('mojang_launcher_path'),
        ]);
        if (cancelled) return;
        setModrinth(Boolean(m));
        setAiMcp(Boolean(a));
        if (typeof p === 'string') setLauncherPath(p);
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
      alert(String(e));
    }
  };

  const toggleAiMcp = async (value: boolean) => {
    setAiMcp(value);
    try {
      await setSetting('ai_mcp_enabled', value);
    } catch (e) {
      setAiMcp(!value);
      alert(String(e));
    }
  };

  const saveLauncherPath = async () => {
    try {
      await setSetting('mojang_launcher_path', launcherPath);
    } catch (e) {
      alert(String(e));
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
        </>
      )}
    </div>
  );
}
