import { useEffect, useRef, useState } from 'react';
import { Sidebar } from './components/Sidebar';
import { CommandPalette } from './components/command-palette';
import { Home } from './pages/Home';
import { Browse } from './pages/Browse';
import { Instances } from './pages/Instances';
import { Governance } from './pages/Governance';
import { Guide } from './pages/Guide';
import { Settings } from './pages/Settings';
import AiChatPage from './pages/AiChatPage';
import { Onboarding } from './pages/Onboarding';
import { ModDetail } from './pages/ModDetail';
import { InstanceEditor } from './pages/InstanceEditor';
import { getSetting } from './lib/tauri';
import { OfflineBanner } from './components/offline-banner';
import { HealthDialog } from './components/HealthDialog';
import { CrashInvestigator } from './components/CrashInvestigator';
import { ToastContainer } from './components/Toast';
import { useDestination, type Destination, type Tab } from './lib/useDestination';
import { useProcessController } from './lib/useProcessController';
import { BrandMark } from './components/BrandMark';
import { BookOpen, Bot, Boxes, Compass, HomeIcon, Landmark, SettingsIcon } from 'lucide-react';

const BASE_TABS = [
  { id: 'home' as Tab, label: 'Home', icon: HomeIcon },
  { id: 'browse' as Tab, label: 'Browse', icon: Compass },
  { id: 'instances' as Tab, label: 'My Instances', icon: Boxes },
  { id: 'governance' as Tab, label: 'Community Governance', icon: Landmark },
  { id: 'guide' as Tab, label: 'Help & Guide', icon: BookOpen },
  { id: 'settings' as Tab, label: 'Settings', icon: SettingsIcon },
];

const AI_TAB = {
  id: 'ai' as Tab,
  label: 'AI Assistant',
  icon: Bot,
};

interface ShellLayout {
  version: 1;
  sidebar: {
    collapsed: boolean;
    width: number;
    lastExpandedWidth: number;
  };
}

const SHELL_LAYOUT_KEY = 'agora-shell-layout';
const DEFAULT_SHELL_LAYOUT: ShellLayout = {
  version: 1,
  sidebar: { collapsed: false, width: 256, lastExpandedWidth: 256 },
};

function loadShellLayout(): ShellLayout {
  try {
    const parsed = JSON.parse(localStorage.getItem(SHELL_LAYOUT_KEY) ?? 'null') as Partial<ShellLayout> | null;
    const sidebar = parsed?.sidebar;
    if (parsed?.version !== 1 || !sidebar || typeof sidebar.width !== 'number') return DEFAULT_SHELL_LAYOUT;
    const width = Math.min(420, Math.max(180, sidebar.width));
    const lastExpandedWidth = typeof sidebar.lastExpandedWidth === 'number'
      ? Math.min(420, Math.max(180, sidebar.lastExpandedWidth))
      : width;
    return {
      version: 1,
      sidebar: { collapsed: sidebar.collapsed === true, width, lastExpandedWidth },
    };
  } catch {
    return DEFAULT_SHELL_LAYOUT;
  }
}

function storeShellLayout(layout: ShellLayout) {
  try {
    localStorage.setItem(SHELL_LAYOUT_KEY, JSON.stringify(layout));
  } catch {
    // Layout remains usable for the current session when storage is unavailable.
  }
}

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
        <BrandMark className="justify-center" />
        <p className="mt-4 text-sm text-muted-foreground">Preparing your library…</p>
        <div className="mt-4 flex justify-center">
          <div className="h-5 w-5 animate-spin rounded-full border-2 border-primary border-t-transparent" />
        </div>
      </div>
    </div>
  );
}

/** Derive the effective tab from a destination. */
function destToTab(dest: Destination): Tab {
  if (dest.type === 'tab') return dest.tab;
  if (dest.type === 'instance-detail') return 'instances';
  return 'home'; // mod-detail doesn't change the tab
}

/** Deliberate recoverable not-found view for unrecognized destinations. */
function NotFoundView({ canGoBack, onGoHome, onGoBack }: { canGoBack: boolean; onGoHome: () => void; onGoBack: () => void }) {
  return (
    <div className="space-y-6">
      {canGoBack ? (
        <button
          onClick={onGoBack}
          className="rounded-lg border border-border px-3 py-1.5 text-sm font-medium hover:bg-accent"
        >
          ← Back
        </button>
      ) : (
        <button
          onClick={onGoHome}
          className="rounded-lg border border-border px-3 py-1.5 text-sm font-medium hover:bg-accent"
        >
          ← Back to Home
        </button>
      )}
      <div className="rounded-xl border border-destructive bg-destructive/10 p-6 text-center" data-testid="not-found-view">
        <h2 className="text-xl font-bold text-foreground">Page Not Found</h2>
        <p className="text-sm text-muted-foreground mt-2">
          The requested page could not be found.
        </p>
      </div>
    </div>
  );
}

/** The three known destination types used for validation. */
const KNOWN_DEST_TYPES = new Set(['tab', 'mod-detail', 'instance-detail']);

export default function App() {
  const {
    destination,
    canGoBack,
    navigateToTab,
    navigateToBrowse,
    navigateToModDetail,
    navigateToInstanceDetail,
    goBack,
  } = useDestination();

  const processController = useProcessController();
  const mainRef = useRef<HTMLElement>(null);
  const previousDestinationRef = useRef<Destination>(destination);
  const browseScrollTopRef = useRef(0);
  const instanceEditorScrollTopRef = useRef(0);
  const modDetailOriginRef = useRef<Destination | null>(null);
  const [browseVisited, setBrowseVisited] = useState(false);
  const [instanceEditorVisited, setInstanceEditorVisited] = useState(false);

  const [onboardingComplete, setOnboardingComplete] = useState<boolean | null>(null);
  const [aiChatEnabled, setAiChatEnabled] = useState<boolean>(false);
  const [commandPaletteOpen, setCommandPaletteOpen] = useState(false);
  const [shellLayout, setShellLayout] = useState<ShellLayout>(loadShellLayout);
  const [crashInvestigation, setCrashInvestigation] = useState<{
    instanceId: string;
    crashFilename: string | null;
    manualLogText: string | null;
    directLaunch: boolean;
  } | null>(null);

  useEffect(() => {
    if (destination.type === 'tab' && destination.tab === 'browse') {
      setBrowseVisited(true);
    }
    if (destination.type === 'instance-detail') {
      setInstanceEditorVisited(true);
    }

    const previous = previousDestinationRef.current;
    const cameFromBrowse = previous.type === 'tab' && previous.tab === 'browse';
    const cameFromInstanceEditor = previous.type === 'instance-detail';
    const returnedToBrowse =
      destination.type === 'tab'
      && destination.tab === 'browse'
      && previous.type === 'mod-detail'
      && modDetailOriginRef.current?.type === 'tab'
      && modDetailOriginRef.current.tab === 'browse';
    const returnedToInstanceEditor =
      destination.type === 'instance-detail'
      && previous.type === 'mod-detail'
      && modDetailOriginRef.current?.type === 'instance-detail';

    if (destination.type === 'mod-detail' && (cameFromBrowse || cameFromInstanceEditor)) {
      modDetailOriginRef.current = previous;
      mainRef.current?.scrollTo({ top: 0, behavior: 'auto' });
    } else if (returnedToBrowse) {
      requestAnimationFrame(() => {
        mainRef.current?.scrollTo({ top: browseScrollTopRef.current, behavior: 'auto' });
      });
    } else if (returnedToInstanceEditor) {
      requestAnimationFrame(() => {
        mainRef.current?.scrollTo({ top: instanceEditorScrollTopRef.current, behavior: 'auto' });
      });
    }

    previousDestinationRef.current = destination;
  }, [destination]);

  // Legacy bridge: the CommandPalette still uses (tab, instanceId?) signature.
  const handleNavigate = (tab: Tab, instanceId?: string) => {
    if (instanceId) {
      navigateToInstanceDetail(instanceId);
    } else {
      navigateToTab(tab);
    }
  };

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

  // Re-read the ai_chat_enabled toggle whenever the destination changes
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
  }, [destination]);

  // React to the agora-navigate custom event (used by external code).
  useEffect(() => {
    const handler = (e: Event) => {
      const detail = (e as CustomEvent).detail as string;
      if (detail === 'settings') {
        navigateToTab('settings');
      }
    };
    window.addEventListener('agora-navigate', handler);
    return () => window.removeEventListener('agora-navigate', handler);
  }, [navigateToTab]);

  // Ctrl+K / Cmd+K opens the command palette.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === 'k') {
        const target = e.target instanceof HTMLElement ? e.target : null;
        const tag = target?.tagName;
        if (tag === 'INPUT' || tag === 'TEXTAREA' || target?.isContentEditable || target?.getAttribute('role') === 'textbox') {
          return;
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
    BASE_TABS[5],
  ];

  // Resolve the current UI state from the destination.
  const effectiveTab: Tab =
    destination.type === 'tab' && destination.tab === 'ai' && !aiChatEnabled
      ? 'home'
      : destToTab(destination);

  // Validate destination type — corrupt state or future versions must not
  // silently fall to home. This is a defense-in-depth check; the type system
  // already prevents invalid Destination types at compile time.
  const isKnownDestType = KNOWN_DEST_TYPES.has(destination.type);

  const showModDetail = destination.type === 'mod-detail';
  const previousDestination = previousDestinationRef.current;
  const shouldRenderBrowse =
    effectiveTab === 'browse'
    || (browseVisited
      && showModDetail
      && previousDestination.type === 'tab'
      && previousDestination.tab === 'browse');
  const shouldRenderInstanceEditor =
    destination.type === 'instance-detail'
    || (showModDetail
      && instanceEditorVisited
      && previousDestination.type === 'instance-detail');
  const browseInstanceId =
    destination.type === 'tab' && destination.tab === 'browse'
      ? destination.browseInstanceId
      : undefined;
  const browseContentType =
    destination.type === 'tab' && destination.tab === 'browse'
      ? destination.browseContentType
      : undefined;
  const instanceEditorId =
    destination.type === 'instance-detail'
      ? destination.instanceId
      : showModDetail && previousDestination.type === 'instance-detail'
        ? previousDestination.instanceId
        : undefined;

  // Render the HealthDialog at the App level so it survives page navigation.
  const {
    state: processState,
    logs: processLogs,
    startLaunch,
    approveLaunch,
    cancelLaunch,
    kill: killProcess,
    clearError,
    repairAndRetry,
    useDelegatedLaunch,
  } = processController;

  const handleInstanceEditorLaunch = async (instanceId: string) => {
    let directLaunch = false;
    try {
      directLaunch = (await getSetting('launch_mode')) === 'direct';
    } catch {
      // Delegated launch is the safe default when the setting is unavailable.
    }
    return startLaunch(instanceId, directLaunch);
  };

  const handleModDetailBack = () => {
    if (canGoBack) {
      goBack();
    } else {
      navigateToTab('browse');
    }
  };

  const handleBrowseSelectMod = (id: string) => {
    browseScrollTopRef.current = mainRef.current?.scrollTop ?? 0;
    navigateToModDetail(id);
  };

  const handleInstanceEditorOpenMod = (id: string) => {
    instanceEditorScrollTopRef.current = mainRef.current?.scrollTop ?? 0;
    navigateToModDetail(id);
  };

  return (
    <div className="flex h-screen w-screen overflow-hidden">
        <OfflineBanner />
        <Sidebar
          tabs={tabs}
          activeTab={effectiveTab}
          onSelectTab={navigateToTab}
          onOpenCommandPalette={() => setCommandPaletteOpen(true)}
          collapsed={shellLayout.sidebar.collapsed}
          width={shellLayout.sidebar.width}
          onCollapsedChange={(collapsed) => {
            setShellLayout((current) => {
              const next = {
                ...current,
                sidebar: {
                  ...current.sidebar,
                  collapsed,
                  width: collapsed ? current.sidebar.width : current.sidebar.lastExpandedWidth,
                  lastExpandedWidth: collapsed ? current.sidebar.width : current.sidebar.lastExpandedWidth,
                },
              };
              storeShellLayout(next);
              return next;
            });
          }}
          onWidthChange={(width) => {
            setShellLayout((current) => ({
              ...current,
              sidebar: { ...current.sidebar, width, lastExpandedWidth: width },
            }));
          }}
          onWidthCommit={(width) => {
            setShellLayout((current) => {
              const next = {
                ...current,
                sidebar: { ...current.sidebar, width, lastExpandedWidth: width },
              };
              storeShellLayout(next);
              return next;
            });
          }}
        />

        {processState.phase === 'failed' && processState.healthReport && (
          <HealthDialog
            instanceId={processState.instanceId!}
            instanceName={processState.instanceId!}
            initialReport={processState.healthReport}
            onConfirm={approveLaunch}
            onCancel={cancelLaunch}
          />
        )}

        <main ref={mainRef} className="flex-1 overflow-y-auto bg-background p-6 text-background-foreground">
          <div className="contents">
            {!isKnownDestType ? (
              <NotFoundView
                canGoBack={canGoBack}
                onGoHome={() => navigateToTab('home')}
                onGoBack={goBack}
              />
            ) : showModDetail ? (
              <ModDetail
                itemId={destination.itemId}
                onBack={handleModDetailBack}
                onOpenInstanceEditor={(id) => {
                  navigateToInstanceDetail(id);
                }}
              />
            ) : destination.type === 'instance-detail' ? null : (
              <>
                {effectiveTab === 'home' && (
                  <Home
                    onNavigateTab={navigateToTab}
                    onOpenInstance={navigateToInstanceDetail}
                    onOpenMod={navigateToModDetail}
                    onLaunch={startLaunch}
                  />
                )}
                {effectiveTab === 'instances' && (
                  <Instances
                    onEditInstance={(id) => navigateToInstanceDetail(id)}
                    processState={processState}
                    processLogs={processLogs}
                    onStartLaunch={startLaunch}
                    onKillProcess={killProcess}
                    onStartCrashInvestigation={setCrashInvestigation}
                    onRepairAndRetry={repairAndRetry}
                    onUseDelegatedLaunch={useDelegatedLaunch}
                    onClearError={clearError}
                  />
                )}
                {effectiveTab === 'governance' && <Governance />}
                {effectiveTab === 'ai' && aiChatEnabled && <AiChatPage />}
                {effectiveTab === 'guide' && <Guide onNavigateTab={navigateToTab} />}
                {effectiveTab === 'settings' && (
                  <Settings
                    onResetLayout={() => {
                      const reset = {
                        ...DEFAULT_SHELL_LAYOUT,
                        sidebar: { ...DEFAULT_SHELL_LAYOUT.sidebar },
                      };
                      setShellLayout(reset);
                      storeShellLayout(reset);
                    }}
                  />
                )}
              </>
            )}
          </div>
          <div className="contents">
            {shouldRenderBrowse && (
              <div className={showModDetail ? 'hidden' : undefined}>
                <Browse
                  onSelectMod={handleBrowseSelectMod}
                  initialInstanceId={browseInstanceId}
                  initialContentType={browseContentType}
                />
              </div>
            )}
            {shouldRenderInstanceEditor && instanceEditorId && (
              <div className={showModDetail ? 'hidden' : undefined}>
                <InstanceEditor
                  instanceId={instanceEditorId}
                  onBack={() => navigateToTab('instances')}
                  onOpenInstanceEditor={(id) => navigateToInstanceDetail(id)}
                  onOpenModDetail={handleInstanceEditorOpenMod}
                  onOpenBrowseForInstance={navigateToBrowse}
                  onLaunch={handleInstanceEditorLaunch}
                />
              </div>
            )}
          </div>
        </main>

        <CommandPalette
          open={commandPaletteOpen}
          onOpenChange={setCommandPaletteOpen}
          onNavigate={handleNavigate}
        />
        <ToastContainer />
        {crashInvestigation && (
          <CrashInvestigator
            instanceId={crashInvestigation.instanceId}
            crashFilename={crashInvestigation.crashFilename}
            manualLogText={crashInvestigation.manualLogText}
            onClose={() => setCrashInvestigation(null)}
            onLaunch={() => startLaunch(
              crashInvestigation.instanceId,
              crashInvestigation.directLaunch,
            )}
          />
        )}
    </div>
  );
}

