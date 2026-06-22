import { useEffect, useState } from 'react';
import { Sidebar } from './components/Sidebar';
import { Home } from './pages/Home';
import { Browse } from './pages/Browse';
import { ModrinthRaw } from './pages/ModrinthRaw';
import { Instances } from './pages/Instances';
import { Governance } from './pages/Governance';
import { Settings } from './pages/Settings';
import { Onboarding } from './pages/Onboarding';
import { ModDetail } from './pages/ModDetail';
import { InstanceEditor } from './pages/InstanceEditor';
import { getSetting, setSetting } from './lib/tauri';

type Tab = 'home' | 'browse' | 'modrinth' | 'instances' | 'governance' | 'settings';

const BASE_TABS: { id: Tab; label: string; icon: string }[] = [
  { id: 'home', label: 'Home', icon: '🏠' },
  { id: 'browse', label: 'Browse', icon: '🔍' },
  { id: 'instances', label: 'My Instances', icon: '📦' },
  { id: 'governance', label: 'Community Governance', icon: '🗳️' },
  { id: 'settings', label: 'Settings', icon: '⚙️' },
];

const MODRINTH_TAB: { id: Tab; label: string; icon: string } = {
  id: 'modrinth',
  label: 'Modrinth',
  icon: '🌐',
};

export default function App() {
  const [activeTab, setActiveTab] = useState<Tab>('home');
  const [selectedModId, setSelectedModId] = useState<string | null>(null);
  const [editingInstanceId, setEditingInstanceId] = useState<string | null>(null);
  const [onboardingComplete, setOnboardingComplete] = useState<boolean | null>(null);
  const [modrinthEnabled, setModrinthEnabled] = useState<boolean>(false);
  const [showTelemetryPrompt, setShowTelemetryPrompt] = useState(false);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const value = await getSetting('onboarding_complete');
        if (!cancelled) setOnboardingComplete(Boolean(value));
      } catch {
        if (!cancelled) setOnboardingComplete(true);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Re-read the modrinth_enabled toggle whenever returning to a top-level tab
  // so the sidebar reflects the current setting without an app restart.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const m = await getSetting('modrinth_enabled');
        if (!cancelled) setModrinthEnabled(m === true);
      } catch {
        if (!cancelled) setModrinthEnabled(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [activeTab, onboardingComplete]);

  // One-time telemetry opt-in prompt: show only when the setting has never been set.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const v = await getSetting('crash_telemetry_opt_in');
        if (!cancelled) setShowTelemetryPrompt(v === null || v === undefined);
      } catch {
        if (!cancelled) setShowTelemetryPrompt(true);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const handleTelemetryChoice = async (allow: boolean) => {
    try {
      await setSetting('crash_telemetry_opt_in', allow);
    } finally {
      setShowTelemetryPrompt(false);
    }
  };

  if (onboardingComplete === null) {
    return null;
  }

  if (!onboardingComplete) {
    return (
      <div className="h-screen w-screen overflow-hidden surface">
        <Onboarding onComplete={() => setOnboardingComplete(true)} />
      </div>
    );
  }

  // Build the tab list; the Modrinth tab only appears when the toggle is on.
  const tabs = modrinthEnabled
    ? [
        BASE_TABS[0],
        BASE_TABS[1],
        MODRINTH_TAB,
        BASE_TABS[2],
        BASE_TABS[3],
        BASE_TABS[4],
      ]
    : BASE_TABS;

  // If the user disables Modrinth while on the Modrinth tab, bounce back home.
  const effectiveTab: Tab =
    activeTab === 'modrinth' && !modrinthEnabled ? 'home' : activeTab;

  return (
    <div className="flex h-screen w-screen overflow-hidden">
        {showTelemetryPrompt && (
          <div className="fixed bottom-4 right-4 z-50 max-w-sm rounded-xl border border-gray-200 dark:border-gray-700 surface p-4 shadow-lg">
            <p className="text-sm font-medium mb-2">Help improve Agora</p>
            <p className="text-xs text-[rgb(var(--muted))] mb-3">
              Allow anonymous crash telemetry to be collected for mod-incompatibility research?
            </p>
            <div className="flex gap-2">
              <button
                onClick={() => handleTelemetryChoice(true)}
                className="rounded-lg bg-brand-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-brand-700"
              >
                Allow
              </button>
              <button
                onClick={() => handleTelemetryChoice(false)}
                className="rounded-lg border border-gray-300 dark:border-gray-600 px-3 py-1.5 text-xs font-medium text-gray-700 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-gray-800"
              >
                Not now
              </button>
            </div>
          </div>
        )}
        <Sidebar tabs={tabs} activeTab={effectiveTab} onSelectTab={(t) => { setSelectedModId(null); setEditingInstanceId(null); setActiveTab(t); }} />
        <main className="flex-1 overflow-y-auto p-6 surface">
          {editingInstanceId !== null ? (
            <InstanceEditor instanceId={editingInstanceId} onBack={() => setEditingInstanceId(null)} onOpenInstanceEditor={(id) => setEditingInstanceId(id)} />
          ) : selectedModId !== null ? (
            <ModDetail itemId={selectedModId} onBack={() => setSelectedModId(null)} onOpenInstanceEditor={(id) => { setSelectedModId(null); setEditingInstanceId(id); }} />
          ) : (
            <>
              {effectiveTab === 'home' && <Home />}
              {effectiveTab === 'browse' && (
                <Browse
                  onSelectMod={(id) => setSelectedModId(id)}
                  onOpenModrinth={modrinthEnabled ? () => setActiveTab('modrinth') : undefined}
                />
              )}
              {effectiveTab === 'modrinth' && modrinthEnabled && (
                <ModrinthRaw onOpenInstanceEditor={(id) => setEditingInstanceId(id)} />
              )}
              {effectiveTab === 'instances' && <Instances onEditInstance={(id) => setEditingInstanceId(id)} />}
              {effectiveTab === 'governance' && <Governance />}
              {effectiveTab === 'settings' && <Settings />}
            </>
          )}
        </main>
    </div>
  );
}
