import { useCallback, useEffect, useState } from 'react';
import { useRegistryState } from '../lib/useRegistryState';
import {
  checkInstanceCrash,
  checkInstanceUpdates,
  detectDrift,
  getLkgMarker,
  getSetting,
  listInstances,
  listSnapshots,
  restoreSnapshot,
  forYouItems,
  setSetting,
  checkRegistryUpdate,
  type InstanceRow,
  type UpdateInfo,
  type RegistryItem,
} from '../lib/tauri';
import type { Tab } from '../lib/useDestination';

// ---------------------------------------------------------------------------
// D1: Action-oriented Home
// 4-zone layout: Alerts → Hero → Maintenance → Discovery
// ---------------------------------------------------------------------------

export function Home({
  onNavigateTab,
  onOpenInstance,
  onOpenMod,
  onLaunch,
}: {
  onNavigateTab: (tab: Tab) => void;
  onOpenInstance: (instanceId: string) => void;
  onOpenMod: (itemId: string) => void;
  onLaunch: (instanceId: string, directLaunch: boolean) => Promise<boolean>;
}) {
  const { state: regState, hasCachedDb } = useRegistryState();

  const [instances, setInstances] = useState<InstanceRow[]>([]);
  const [instancesLoading, setInstancesLoading] = useState(true);
  const [lastCrash, setLastCrash] = useState<{ instanceId: string; name: string; filename?: string } | null>(null);
  const [updatesByInstance, setUpdatesByInstance] = useState<Record<string, UpdateInfo[]>>({});
  const [knownGood, setKnownGood] = useState<{
    instanceId: string;
    instanceName: string;
    id: string;
    label: string;
    promotedAt: string | null;
    added: number;
    removed: number;
    disabled: number;
    updated: number;
  }[]>([]);
  const [knownGoodChecked, setKnownGoodChecked] = useState(false);
  const [recommendations, setRecommendations] = useState<RegistryItem[]>([]);
  const [restoringSnapshotId, setRestoringSnapshotId] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  // Load instances on mount.
  const loadData = useCallback(async () => {
    setInstancesLoading(true);
    setKnownGoodChecked(false);
    try {
      const all = await listInstances();
      setInstances(all);

      // Check for crash on the most recently launched instance.
      const launched = all.filter((i) => i.last_launched_at).sort(
        (a, b) => new Date(b.last_launched_at!).getTime() - new Date(a.last_launched_at!).getTime(),
      );
      if (launched.length > 0) {
        const latest = launched[0];
        try {
          const crash = await checkInstanceCrash(latest.instance_id);
          if (crash) {
            setLastCrash({ instanceId: latest.instance_id, name: latest.name, filename: crash.filename ?? undefined });
          } else {
            setLastCrash(null);
          }
        } catch { setLastCrash(null); }
      } else {
        setLastCrash(null);
      }

      // Check for updates on launched instances.
      const updates: Record<string, UpdateInfo[]> = {};
      for (const inst of all) {
        if (inst.is_locked) continue;
        try {
          const u = await checkInstanceUpdates(inst.instance_id);
          if (u.length > 0) updates[inst.instance_id] = u;
        } catch { /* skip */ }
      }
      setUpdatesByInstance(updates);

      // Resolve exact promoted LKG pointers and current drift, never arbitrary snapshots.
      const lkgResults: typeof knownGood = [];
      for (const inst of all.slice(0, 5)) {
        try {
          const marker = await getLkgMarker(inst.instance_id);
          const snapshotId = typeof marker?.currentLkgSnapshotId === 'string'
            ? marker.currentLkgSnapshotId
            : null;
          if (!snapshotId) continue;
          const snapList = await listSnapshots(inst.instance_id);
          const snapshot = snapList.find((candidate) => candidate.id === snapshotId);
          const diff = await detectDrift(inst.instance_id, snapshotId);
          const entries = (key: 'added' | 'removed' | 'modified') => diff[key];
          const addedEntries = entries('added');
          const removedEntries = entries('removed');
          const removedPaths = new Set(removedEntries.map((entry) => String(entry.path ?? '')));
          const disabled = addedEntries.filter((entry) => {
            const path = String(entry.path ?? '');
            return path.endsWith('.disabled') && removedPaths.has(path.slice(0, -'.disabled'.length));
          }).length;
          lkgResults.push({
            instanceId: inst.instance_id,
            instanceName: inst.name,
            id: snapshotId,
            label: snapshot?.label ?? 'Last known good',
            promotedAt: typeof marker?.lastPromotedAt === 'string' ? marker.lastPromotedAt : null,
            added: addedEntries.length - disabled,
            removed: removedEntries.length - disabled,
            disabled,
            updated: entries('modified').length,
          });
        } catch { /* skip */ }
      }
      setKnownGood(lkgResults);

      if (hasCachedDb && launched[0]) {
        const active = launched[0];
        try {
          const modrinthEnabled = (await getSetting('modrinth_enabled')) === true;
          setRecommendations(await forYouItems(
            modrinthEnabled,
            active.minecraft_version,
            active.loader,
            3,
          ));
        } catch {
          setRecommendations([]);
        }
      } else {
        setRecommendations([]);
      }
    } catch { /* ignore */ }
    setKnownGoodChecked(true);
    setInstancesLoading(false);
  }, [hasCachedDb]);

  useEffect(() => {
    loadData();
  }, [loadData]);

  // Track last home visit for change detection.
  useEffect(() => {
    getSetting('last_home_visit').catch(() => {});
    return () => {
      setSetting('last_home_visit', new Date().toISOString()).catch(() => {});
    };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Group cards by zone.
  const sortedByLaunched = [...instances].sort(
    (a, b) => new Date(b.last_launched_at ?? 0).getTime() - new Date(a.last_launched_at ?? 0).getTime(),
  );
  const lastLaunched = sortedByLaunched[0] ?? null;
  const totalUpdates = Object.values(updatesByInstance).reduce((s, u) => s + u.length, 0);
  const heroInstance = lastLaunched ?? sortedByLaunched[0] ?? null;
  const crashKnownGood = lastCrash
    ? knownGood.find((entry) => entry.instanceId === lastCrash.instanceId) ?? null
    : null;

  const handleContinuePlaying = useCallback(async () => {
    if (!heroInstance) return;
    setActionError(null);
    try {
      const launchMode = await getSetting('launch_mode');
      await onLaunch(heroInstance.instance_id, launchMode === 'direct');
    } catch (error) {
      setActionError(error instanceof Error ? error.message : String(error));
    }
  }, [heroInstance, onLaunch]);

  const handleRestoreSnapshot = useCallback(async (snapshot: {
    instanceId: string;
    instanceName: string;
    id: string;
    label: string;
  }) => {
    const confirmed = window.confirm(
      `Restore "${snapshot.instanceName}" to snapshot "${snapshot.label || snapshot.id}"? Agora will create an undo snapshot first.`,
    );
    if (!confirmed) return;
    setActionError(null);
    setRestoringSnapshotId(snapshot.id);
    try {
      await restoreSnapshot(snapshot.instanceId, snapshot.id);
      await loadData();
    } catch (error) {
      setActionError(error instanceof Error ? error.message : String(error));
    } finally {
      setRestoringSnapshotId(null);
    }
  }, [loadData]);

  return (
    <div className="space-y-6">
      {/* Header */}
      <section>
        <h2 className="text-2xl font-bold mb-2">Home</h2>
        <p className="text-muted-foreground">Your modding dashboard.</p>
      </section>

      {/* Zone A: Alerts — compact warnings */}
      {lastCrash && (
        <CrashAlert
          instanceName={lastCrash.name}
          crashFilename={lastCrash.filename}
          canRestore={Boolean(crashKnownGood)}
          onRestore={() => {
            if (crashKnownGood) {
              void handleRestoreSnapshot(crashKnownGood);
            } else {
              onOpenInstance(lastCrash.instanceId);
            }
          }}
        />
      )}

      {regState === 'missing' && (
        <RegistryAlert
          hasCachedDb={hasCachedDb}
        />
      )}

      {/* Zone B: Hero — Continue Playing */}
      <ContinuePlayingCard
        instance={heroInstance}
        loading={instancesLoading}
        onLaunch={() => {
          if (heroInstance) {
            void handleContinuePlaying();
          }
        }}
        onBrowsePacks={() => onNavigateTab('browse')}
      />

      {/* Zone C: Maintenance — only when triggered */}
      {totalUpdates > 0 && (
        <UpdatesCard
          totalUpdates={totalUpdates}
          onReview={() => onNavigateTab('instances')}
        />
      )}

      {knownGood.length > 0 && (
        <KnownGoodCard
          snapshots={knownGood}
          restoringSnapshotId={restoringSnapshotId}
          onRestore={(snapshot) => void handleRestoreSnapshot(snapshot)}
        />
      )}
      {knownGoodChecked && instances.length > 0 && knownGood.length === 0 && (
        <div className="rounded-xl border border-dashed border-border bg-card p-4">
          <h4 className="text-sm font-semibold">No last-known-good state yet</h4>
          <p className="mt-1 text-xs text-muted-foreground">
            Play an instance successfully for at least 60 seconds. Agora will then promote its exact pre-launch snapshot for one-click recovery.
          </p>
        </div>
      )}

      {actionError && (
        <div role="alert" className="rounded-lg bg-destructive/10 p-3 text-sm text-destructive">
          {actionError}
        </div>
      )}

      {/* Zone D: Discovery — always present */}
      <RecommendationsCard
        hasInstances={instances.length > 0}
        hasCachedDb={hasCachedDb}
        loading={instancesLoading}
        activeInstance={lastLaunched}
        recommendations={recommendations}
        onOpenMod={onOpenMod}
        onBrowseMore={() => onNavigateTab('browse')}
      />
    </div>
  );
}

// ---------------------------------------------------------------------------
// Card components
// ---------------------------------------------------------------------------

function CrashAlert({ instanceName, crashFilename, canRestore, onRestore }: {
  instanceName: string;
  crashFilename?: string;
  canRestore: boolean;
  onRestore: () => void;
}) {
  return (
    <div className="rounded-lg border border-destructive bg-destructive/10 p-3 flex items-center justify-between gap-3">
      <div className="text-xs text-destructive flex-1">
        <span className="font-semibold">{instanceName}</span> did not exit cleanly.
        {crashFilename && <span className="text-muted-foreground ml-1">({crashFilename})</span>}
      </div>
      <button onClick={onRestore} className="rounded-lg bg-destructive px-3 py-1.5 text-xs font-medium text-destructive-foreground hover:bg-destructive/90">
        {canRestore ? 'View & restore' : 'View instance'}
      </button>
    </div>
  );
}

function RegistryAlert({ hasCachedDb }: {
  hasCachedDb: boolean;
}) {
  const [downloading, setDownloading] = useState(false);

  const handleDownload = async () => {
    setDownloading(true);
    try {
      await checkRegistryUpdate(true);
    } catch {
      // error handled by the registry status view
    } finally {
      setDownloading(false);
    }
  };

  return (
    <div className="rounded-lg border border-amber-500 bg-amber-50 dark:bg-amber-900/20 p-3 flex items-center justify-between gap-3">
      <p className="text-xs text-amber-700 dark:text-amber-300">
        {hasCachedDb
          ? 'Using cached registry — updates, recommendations, and governance are offline.'
          : 'Registry not downloaded yet. Download it to enable updates, recommendations, and governance.'}
      </p>
      <button onClick={handleDownload} disabled={downloading} className="rounded-lg bg-amber-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-amber-700 disabled:opacity-50">
        {downloading ? 'Downloading…' : 'Download registry'}
      </button>
    </div>
  );
}

function ContinuePlayingCard({ instance, loading, onLaunch, onBrowsePacks }: {
  instance: InstanceRow | null;
  loading: boolean;
  onLaunch: () => void;
  onBrowsePacks: () => void;
}) {
  if (loading) {
    return (
      <div className="rounded-xl border border-border bg-card p-6 space-y-2">
        <div className="h-5 w-32 bg-muted animate-pulse rounded" />
        <div className="h-4 w-48 bg-muted animate-pulse rounded" />
      </div>
    );
  }

  if (!instance) {
    return (
      <div className="rounded-xl border border-border bg-card p-6">
        <h3 className="text-lg font-semibold mb-2">Welcome to Agora</h3>
        <p className="text-sm text-muted-foreground mb-4">
          No instances yet. Create one from a mod pack to start playing.
        </p>
        <button onClick={onBrowsePacks} className="rounded-lg bg-primary px-5 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90">
          Browse mod packs
        </button>
      </div>
    );
  }

  const timeAgo = instance.last_launched_at
    ? timeSince(new Date(instance.last_launched_at))
    : 'Not launched yet';

  return (
    <div className="rounded-xl border border-border bg-card p-6">
      <h3 className="text-lg font-semibold mb-1">{instance.name}</h3>
      <p className="text-xs text-muted-foreground mb-1">
        {instance.loader} {instance.loader_version} · MC {instance.minecraft_version}
      </p>
      <p className="text-xs text-muted-foreground mb-4">{timeAgo}</p>
      <div className="flex gap-2">
        <button onClick={onLaunch} className="rounded-lg bg-primary px-5 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90">
          Continue Playing
        </button>
      </div>
    </div>
  );
}

function UpdatesCard({ totalUpdates, onReview }: {
  totalUpdates: number;
  onReview: () => void;
}) {
  return (
    <div className="rounded-xl border border-border bg-card p-4 flex items-center justify-between">
      <div>
        <h4 className="font-semibold text-sm">Updates Available</h4>
        <p className="text-xs text-muted-foreground">{totalUpdates} mods can be updated</p>
      </div>
      <button onClick={onReview} className="rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90">
        Review
      </button>
    </div>
  );
}

function KnownGoodCard({
  snapshots,
  restoringSnapshotId,
  onRestore,
}: {
  snapshots: {
    instanceId: string;
    instanceName: string;
    id: string;
    label: string;
    promotedAt: string | null;
    added: number;
    removed: number;
    disabled: number;
    updated: number;
  }[];
  restoringSnapshotId: string | null;
  onRestore: (snapshot: { instanceId: string; instanceName: string; id: string; label: string }) => void;
}) {
  return (
    <div className="rounded-xl border border-border bg-card p-4 space-y-2">
      <h4 className="font-semibold text-sm">Last Known Good</h4>
      <div className="space-y-1">
        {snapshots.slice(0, 3).map((s) => (
          <div key={s.id} className="flex items-center justify-between text-xs">
            <div>
              <p className="font-medium">{s.instanceName}: {s.label}</p>
              <p className="text-muted-foreground">
                {s.promotedAt ? new Date(s.promotedAt).toLocaleString() : 'Promotion time unavailable'}
                {' · '}{s.added} new · {s.removed} removed · {s.disabled} disabled · {s.updated} updated
              </p>
            </div>
            <button
              onClick={() => onRestore(s)}
              disabled={restoringSnapshotId !== null}
              className="text-primary hover:underline text-xs disabled:opacity-50"
            >
              {restoringSnapshotId === s.id ? 'Restoring…' : 'Restore'}
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}

function RecommendationsCard({
  hasInstances,
  hasCachedDb,
  loading,
  activeInstance,
  recommendations,
  onOpenMod,
  onBrowseMore,
}: {
  hasInstances: boolean;
  hasCachedDb: boolean;
  loading: boolean;
  activeInstance: InstanceRow | null;
  recommendations: RegistryItem[];
  onOpenMod: (itemId: string) => void;
  onBrowseMore: () => void;
}) {
  if (loading) return null;

  if (!hasInstances) {
    return (
      <div className="rounded-xl border border-dashed border-border bg-card p-6 text-center">
        <p className="text-muted-foreground">
          Once you have an instance, we&apos;ll show mods that work with it.
        </p>
        <button onClick={onBrowseMore} className="mt-3 rounded-lg border border-border px-4 py-2 text-sm font-medium hover:bg-accent">
          Browse all mods
        </button>
      </div>
    );
  }

  if (!hasCachedDb) {
    return (
      <div className="rounded-xl border border-dashed border-border bg-card p-6 text-center">
        <p className="text-muted-foreground">
          Download the registry to see compatible recommendations.
        </p>
      </div>
    );
  }

  if (recommendations.length === 0) {
    return (
      <div className="rounded-xl border border-dashed border-border bg-card p-6 text-center">
        <p className="text-muted-foreground">
          No new curated matches were found for {activeInstance?.name ?? 'this instance'}.
        </p>
        <button onClick={onBrowseMore} className="mt-3 rounded-lg border border-border px-4 py-2 text-sm font-medium hover:bg-accent">
          Browse catalog
        </button>
      </div>
    );
  }

  return (
    <div className="rounded-xl border border-border bg-card p-4 space-y-3">
      <div>
        <h4 className="font-semibold text-sm">Compatible recommendations</h4>
        <p className="text-xs text-muted-foreground">
          Ranked by category overlap with mods in {activeInstance?.name}, then filtered for MC {activeInstance?.minecraft_version} and {activeInstance?.loader}.
        </p>
      </div>
      <div className="grid gap-2 md:grid-cols-3">
        {recommendations.map((item) => (
          <button
            key={item.id}
            onClick={() => onOpenMod(item.id)}
            className="rounded-lg border border-border bg-muted p-3 text-left hover:bg-accent"
          >
            <span className="block text-sm font-medium">{item.name}</span>
            <span className="mt-1 block text-xs text-muted-foreground line-clamp-2">
              {item.description || `Curated ${item.content_type} from ${item.download_strategy}.`}
            </span>
            <span className="mt-2 block text-[11px] text-primary">
              {item.status === 'active' ? 'Curated and active' : item.status} · {item.download_strategy}
            </span>
          </button>
        ))}
      </div>
      <button onClick={onBrowseMore} className="rounded-lg border border-border px-4 py-2 text-sm font-medium hover:bg-accent">
        Browse more
      </button>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function timeSince(date: Date): string {
  const now = new Date();
  const diffMs = now.getTime() - date.getTime();
  const mins = Math.floor(diffMs / 60000);
  if (mins < 1) return 'Just now';
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days < 7) return `${days}d ago`;
  return date.toLocaleDateString();
}
