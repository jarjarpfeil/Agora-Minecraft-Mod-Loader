import { useCallback, useEffect, useRef, useState } from 'react';

export type Tab = 'home' | 'browse' | 'instances' | 'governance' | 'ai' | 'guide' | 'settings';

/**
 * A single typed application destination. Replaces the previous pattern of
 * three independent state variables (activeTab, selectedModId, editingInstanceId).
 *
 * Destinations:
 * - `tab` — one of the sidebar tabs (home, browse, instances, governance, ai, guide, settings).
 * - `mod-detail` — browsing a specific curated item.
 * - `instance-detail` — editing a specific instance.
 */
export type Destination =
  | { type: 'tab'; tab: Tab; browseInstanceId?: string; browseContentType?: string }
  | { type: 'mod-detail'; itemId: string }
  | { type: 'instance-detail'; instanceId: string };

export interface UseDestinationReturn {
  destination: Destination;
  canGoBack: boolean;
  navigate: (dest: Destination) => void;
  goBack: () => void;
  navigateToTab: (tab: Tab) => void;
  navigateToBrowse: (instanceId?: string, contentType?: string) => void;
  navigateToModDetail: (itemId: string) => void;
  navigateToInstanceDetail: (instanceId: string) => void;
}

const MAX_HISTORY = 50;

function isValidDestination(d: unknown): d is Destination {
  if (!d || typeof d !== 'object') return false;
  const dest = d as Record<string, unknown>;
  if (dest.type === 'tab') {
    return typeof dest.tab === 'string'
      && (dest.browseInstanceId === undefined || typeof dest.browseInstanceId === 'string')
      && (dest.browseContentType === undefined || typeof dest.browseContentType === 'string');
  }
  if (dest.type === 'mod-detail') return typeof dest.itemId === 'string';
  if (dest.type === 'instance-detail') return typeof dest.instanceId === 'string';
  return false;
}

export function useDestination(): UseDestinationReturn {
  const historyRef = useRef<Destination[]>([{ type: 'tab', tab: 'home' }]);
  const [destination, setDestination] = useState<Destination>(historyRef.current[0]);
  const [canGoBack, setCanGoBack] = useState(false);

  const push = useCallback((dest: Destination) => {
    historyRef.current.push(dest);
    if (historyRef.current.length > MAX_HISTORY) {
      historyRef.current = historyRef.current.slice(-MAX_HISTORY);
    }
    setCanGoBack(historyRef.current.length > 1);
    setDestination(dest);
    window.history.pushState({ __agora: dest }, '');
  }, []);

  // Handle browser back/forward via popstate.
  useEffect(() => {
    // Initialize from existing history state on page reload to preserve
    // the destination across refreshes. Only write Home when no valid
    // Agora destination exists in the history.
    const existingState = window.history.state as Record<string, unknown> | null;
    const restoredDest = existingState?.__agora as Destination | undefined;
    if (restoredDest && isValidDestination(restoredDest)) {
      historyRef.current[0] = restoredDest;
      setDestination(restoredDest);
      setCanGoBack(false);
    } else {
      window.history.replaceState({ __agora: historyRef.current[0] }, '');
    }

    const handlePopState = (e: PopStateEvent) => {
      const state = e.state as Record<string, unknown> | null;
      const restored = state?.__agora as Destination | undefined;
      if (restored && isValidDestination(restored)) {
        setDestination(restored);
        historyRef.current.push(restored);
        if (historyRef.current.length > MAX_HISTORY) {
          historyRef.current = historyRef.current.slice(-MAX_HISTORY);
        }
        setCanGoBack(historyRef.current.length > 1);
      } else {
        setDestination({ type: 'tab', tab: 'home' });
        setCanGoBack(false);
      }
    };
    window.addEventListener('popstate', handlePopState);
    return () => window.removeEventListener('popstate', handlePopState);
  }, []);

  const navigate = useCallback((dest: Destination) => push(dest), [push]);
  const goBack = useCallback(() => window.history.back(), []);
  const navigateToTab = useCallback((tab: Tab) => push({ type: 'tab', tab }), [push]);
  const navigateToBrowse = useCallback(
    (instanceId?: string, contentType?: string) => push({
      type: 'tab',
      tab: 'browse',
      ...(instanceId ? { browseInstanceId: instanceId } : {}),
      ...(contentType ? { browseContentType: contentType } : {}),
    }),
    [push],
  );
  const navigateToModDetail = useCallback((itemId: string) => push({ type: 'mod-detail', itemId }), [push]);
  const navigateToInstanceDetail = useCallback(
    (instanceId: string) => push({ type: 'instance-detail', instanceId }),
    [push],
  );

  return {
    destination,
    canGoBack,
    navigate,
    goBack,
    navigateToTab,
    navigateToBrowse,
    navigateToModDetail,
    navigateToInstanceDetail,
  };
}
