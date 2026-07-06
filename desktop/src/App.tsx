import { useEffect, useState } from 'react';
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

export default function App() {
  const [activeTab, setActiveTab] = useState<Tab>('home');
  const [selectedModId, setSelectedModId] = useState<string | null>(null);
  const [editingInstanceId, setEditingInstanceId] = useState<string | null>(null);
  const [onboardingComplete, setOnboardingComplete] = useState<boolean | null>(null);
  const [aiChatEnabled, setAiChatEnabled] = useState<boolean>(false);
  const [commandPaletteOpen, setCommandPaletteOpen] = useState(false);
  const handleNavigate = (tab: Tab) => {
    setSelectedModId(null);
    setEditingInstanceId(null);
    setActiveTab(tab);
  };

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

  if (onboardingComplete === null) {
    return null;
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
        <Sidebar tabs={tabs} activeTab={effectiveTab} onSelectTab={(t) => { setSelectedModId(null); setEditingInstanceId(null); setActiveTab(t); }} />
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

