import { useEffect, useState } from 'react';
import { Sidebar } from './components/Sidebar';
import { Home } from './pages/Home';
import { Browse } from './pages/Browse';
import { Instances } from './pages/Instances';
import { Governance } from './pages/Governance';
import { Settings } from './pages/Settings';
import { Onboarding } from './pages/Onboarding';
import { ModDetail } from './pages/ModDetail';
import { getSetting } from './lib/tauri';

type Tab = 'home' | 'browse' | 'instances' | 'governance' | 'settings';

const TABS: { id: Tab; label: string; icon: string }[] = [
  { id: 'home', label: 'Home', icon: '🏠' },
  { id: 'browse', label: 'Browse', icon: '🔍' },
  { id: 'instances', label: 'My Instances', icon: '📦' },
  { id: 'governance', label: 'Community Governance', icon: '🗳️' },
  { id: 'settings', label: 'Settings', icon: '⚙️' },
];

export default function App() {
  const [activeTab, setActiveTab] = useState<Tab>('home');
  const [selectedModId, setSelectedModId] = useState<string | null>(null);
  const [onboardingComplete, setOnboardingComplete] = useState<boolean | null>(null);

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

  return (
    <div className="flex h-screen w-screen overflow-hidden">
        <Sidebar tabs={TABS} activeTab={activeTab} onSelectTab={(t) => { setSelectedModId(null); setActiveTab(t); }} />
        <main className="flex-1 overflow-y-auto p-6 surface">
          {selectedModId !== null ? (
            <ModDetail itemId={selectedModId} onBack={() => setSelectedModId(null)} />
          ) : (
            <>
              {activeTab === 'home' && <Home />}
              {activeTab === 'browse' && (
                <Browse onSelectMod={(id) => setSelectedModId(id)} />
              )}
              {activeTab === 'instances' && <Instances />}
              {activeTab === 'governance' && <Governance />}
              {activeTab === 'settings' && <Settings />}
            </>
          )}
        </main>
    </div>
  );
}
