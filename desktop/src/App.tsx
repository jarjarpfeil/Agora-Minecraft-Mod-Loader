import { useCallback, useEffect, useState } from 'react';
import { Sidebar } from './components/Sidebar';
import { CommandPalette } from './components/command-palette';
import { Home } from './pages/Home';
import { Browse } from './pages/Browse';
import { Instances } from './pages/Instances';
import { Governance } from './pages/Governance';
import { Settings } from './pages/Settings';
import AiChatPage from './pages/AiChatPage';
import { Onboarding } from './pages/Onboarding';
import { ModDetail } from './pages/ModDetail';
import { InstanceEditor } from './pages/InstanceEditor';
import { getSetting } from './lib/tauri';
import { OfflineBanner } from './components/offline-banner';

type Tab = 'home' | 'browse' | 'instances' | 'governance' | 'ai' | 'settings';

const BASE_TABS: { id: Tab; label: string; icon: string }[] = [
  { id: 'home', label: 'Home', icon: '\u{1F3E0}' },
  { id: 'browse', label: 'Browse', icon: '\u{1F50D}' },
  { id: 'instances', label: 'My Instances', icon: '\u{1F4E6}' },
  { id: 'governance', label: 'Community Governance', icon: '\u{1F5F3}\u{FE0F}' },
  { id: 'settings', label: 'Settings', icon: '\u{2699}\u{FE0F}' },
];

const AI_TAB: { id: Tab; label: string; icon: string } = {
  id: 'ai',
  label: 'AI Assistant',
  icon: '\u{1F916}',
};

/**
 * Parse a stored boolean setting strictly.
 * - `true` / `false` → as-is
 * - `"true"` / `"1"` → true
 * - `"false"` / `"0"` → false
 * - Everything else (including `null`, missing, corrupt) → fallback
 */
function parseStoredBoolean(value: unknown, fallback: boolean): boolean {
  if (typeof value === 'boolean') return value;
  if (typeof value === 'string') {
    if (value === 'true' || value === '1') return true;
    if (value === 'false' || value === '0') return false;
  }
  if (typeof value === 'number') return value === 1;
  return fallback;
}

/** Minimal branded loading shell shown while async initialization runs. */
function BrandedSplash() {
  return (
    <div className="flex h-screen w-screen items-center justify-center bg-background">
      <div className="text-center">
        <h1 className="text-3xl font-bold text-foreground">Agora</h1>
        <p className="mt-2 text-sm text-muted-foreground">Loading…</p>
        <div className="mt-4 flex justify-center">
          <div className="h-5 w-5 animate-spin rounded-full border-2 border-primary border-t-transparent" />
        </div>
      </div>
    </div>
  );
}

export default function App() {
  const [activeTab, setActiveTab] = useState<Tab>('home');
  const [selectedModId, setSelectedModId] = useState<string | null>(null);
  const [editingInstanceId, setEditingInstanceId] = useState<string | null>(null);
  const [onboardingComplete, setOnboardingComplete] = useState<boolean | null>(null);
  const [aiChatEnabled, setAiChatEnabled] = useState<boolean>(false);
  const [commandPaletteOpen, setCommandPaletteOpen] = useState(false);

  const handleNavigate = useCallback(
    (tab: Tab, instanceId?: string) => {
      setSelectedModId(null);
      if (instanceId) {
        // Open a specific instance editor directly.
        setEditingInstanceId(instanceId);
        setActiveTab('instances');
      } else {
        setEditingInstanceId(null);
        setActiveTab(tab);
      }
    },
    [],
  );

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const value = await getSetting('onboarding_complete');
        if (!cancelled) setOnboardingComplete(parseStoredBoolean(value, false));
      } catch {
        // On transient read failure, assume completed (safe for non-Tauri dev).
        if (!cancelled) setOnboardingComplete(true);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Re-read the ai_chat_enabled toggle whenever returning to a top-level tab
  // so the sidebar reflects the current setting without an app restart.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const ai = await getSetting('ai_chat_enabled');
        if (!cancelled) setAiChatEnabled(ai === true || ai === 'true');
      } catch {
        if (!cancelled) setAiChatEnabled(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [activeTab, onboardingComplete]);

  useEffect(() => {
    const handler = (e: Event) => {
      const detail = (e as CustomEvent).detail as string;
      if (detail === 'settings') {
        handleNavigate('settings');
      }
    };
    window.addEventListener('agora-navigate', handler);
    return () => window.removeEventListener('agora-navigate', handler);
  }, [handleNavigate]);

  // Ctrl+K / Cmd+K opens the command palette.
  // Prevents capture when the user is typing in a text input or textarea.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === 'k') {
        const tag = (e.target as HTMLElement)?.tagName;
        if (tag === 'INPUT' || tag === 'TEXTAREA' || (e.target as HTMLElement)?.isContentEditable) {
          return; // let the native shortcut pass through
        }
        e.preventDefault();
        setCommandPaletteOpen((prev) => !prev);
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);

  if (onboardingComplete === null) {
    return <BrandedSplash />;
  }

  if (!onboardingComplete) {
    return (
      <div className="h-screen w-screen overflow-hidden bg-card">
        <Onboarding onComplete={() => setOnboardingComplete(true)} />
      </div>
    );
  }

  // The AI Assistant tab appears between Governance and Settings when enabled.
  const tabs = [
    BASE_TABS[0],
    BASE_TABS[1],
    BASE_TABS[2],
    BASE_TABS[3],
    ...(aiChatEnabled ? [AI_TAB] : []),
    BASE_TABS[4],
  ];

  // If the user disables AI while on that tab, bounce back home.
  const effectiveTab: Tab =
    activeTab === 'ai' && !aiChatEnabled
      ? 'home' : activeTab;

  return (
    <div className="flex h-screen w-screen overflow-hidden">
        <OfflineBanner />
        <Sidebar tabs={tabs} activeTab={effectiveTab} onSelectTab={(t) => { setSelectedModId(null); setEditingInstanceId(null); setActiveTab(t); }} onOpenCommandPalette={() => setCommandPaletteOpen(true)} />
        <main className="flex-1 overflow-y-auto p-6 bg-background">
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
                />
              )}
              {effectiveTab === 'instances' && <Instances onEditInstance={(id) => setEditingInstanceId(id)} />}
              {effectiveTab === 'governance' && <Governance />}
              {effectiveTab === 'ai' && aiChatEnabled && <AiChatPage />}
              {effectiveTab === 'settings' && <Settings />}
            </>
          )}
        </main>

        <CommandPalette
          open={commandPaletteOpen}
          onOpenChange={setCommandPaletteOpen}
          onNavigate={handleNavigate}
        />
    </div>
  );
}

