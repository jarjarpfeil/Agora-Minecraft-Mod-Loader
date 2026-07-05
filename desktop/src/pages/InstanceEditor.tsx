import { useEffect, useState } from 'react';
import { useAdvancedMode } from '../components/AdvancedModeContext';
import { DependencyPrompt } from '../components/DependencyPrompt';
import { ConsoleView } from '../components/ConsoleView';
import {
  getInstanceDetail,
  removeModFromInstance,
  addManualMod,
  exportInstancePack,
  pickOpenFile,
  importInstancePack,
  browseItems,
  listCategories,
  listModVersions,
  installModVersion,
  listPackMods,
  formatError,
  getRemovalPlan,
  unlockInstance,
  lockInstance,
  revertInstance,
  listSnapshots,
  createSnapshot,
  restoreSnapshot,
  deleteSnapshot,
  listLoadoutProfiles,
  createLoadoutProfile,
  applyLoadoutProfile,
  deleteLoadoutProfile,
  importInstance,
  type InstanceDetail,
  type RegistryItem,
  type CategoryInfo,
  type ModVersionCandidate,
  type SortOption,
  type PackModRow,
  type DependentInfo,
  type Snapshot,
  type LoadoutProfile,
} from '../lib/tauri';

const SORTS: { label: string; value: SortOption }[] = [
  { label: 'Net Score', value: 'net_score' },
  { label: 'Trending', value: 'velocity' },
  { label: 'Newest', value: 'newest' },
  { label: 'Most Upvoted', value: 'most_upvoted' },
  { label: 'Most Downvoted', value: 'most_downvoted' },
];

const CONTENT_TYPES = ['mod', 'pack', 'shader', 'resourcepack', 'server', 'datapack', 'world'];

export function InstanceEditor({ instanceId, onBack, onOpenInstanceEditor }: { instanceId: string; onBack: () => void; onOpenInstanceEditor?: (instanceId: string) => void }) {
  const [detail, setDetail] = useState<InstanceDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [removeBusy, setRemoveBusy] = useState<string | null>(null);
  const [exportBusy, setExportBusy] = useState(false);

  const { advancedMode } = useAdvancedMode();

  // Sub-sidebar active tab
  const [activeTab, setActiveTab] = useState<'mods' | 'snapshots' | 'loadout-profiles' | 'import' | 'console' | 'java-args' | 'advanced'>('mods');

  // Snapshots state (Phase 6)
  const [snapshots, setSnapshots] = useState<Snapshot[]>([]);
  const [snapshotLabelInput, setSnapshotLabelInput] = useState('');
  const [snapshotBusy, setSnapshotBusy] = useState<string | null>(null);
  const [confirmDeleteSnapshot, setConfirmDeleteSnapshot] = useState<string | null>(null);

  // Loadout profiles state (Phase 6)
  const [profiles, setProfiles] = useState<LoadoutProfile[]>([]);
  const [profileNameInput, setProfileNameInput] = useState('');
  const [profileBusy, setProfileBusy] = useState<string | null>(null);
  const [confirmDeleteProfile, setConfirmDeleteProfile] = useState<string | null>(null);

  // Import state (Phase 6)
  const [symlinkSaves, setSymlinkSaves] = useState(true);
  const [importBusy, setImportBusy] = useState(false);



  // Removal plan prompt state
  const [removePlanTarget, setRemovePlanTarget] = useState<{ filename: string; dependents: DependentInfo[] } | null>(null);

  // Add-mod state
  const [showAdd, setShowAdd] = useState(false);
  const [browseItemsList, setBrowseItemsList] = useState<RegistryItem[]>([]);
  const [categories, setCategories] = useState<CategoryInfo[]>([]);
  const [browseLoading, setBrowseLoading] = useState(false);
  const [browseFilter, setBrowseFilter] = useState('');
  const [browseContentType, setBrowseContentType] = useState<string | null>(null);
  const [browseSort, setBrowseSort] = useState<SortOption>('net_score');
  const [browseCategory, setBrowseCategory] = useState<string | null>(null);
  const [selectedAddItem, setSelectedAddItem] = useState<RegistryItem | null>(null);
  const [candidates, setCandidates] = useState<ModVersionCandidate[]>([]);
  const [selectedCandidate, setSelectedCandidate] = useState<ModVersionCandidate | null>(null);
  const [adding, setAdding] = useState(false);
  const [addError, setAddError] = useState<string | null>(null);

  // Pack install state
  const [packInstallOpen, setPackInstallOpen] = useState(false);
  const [packIdInput, setPackIdInput] = useState('');
  const [packProgress, setPackProgress] = useState<
    { modId: string; status: 'pending' | 'installing' | 'done' | 'failed'; error?: string }[] | null
  >(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const result = await getInstanceDetail(instanceId);
        if (!cancelled) {
          setDetail(result);
          if (!result) setError('Instance not found.');
        }
      } catch (e) {
        if (!cancelled) setError(formatError(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, [instanceId]);

  // Load categories once
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const cats = await listCategories();
        if (!cancelled) setCategories(cats);
      } catch { /* categories are optional */ }
    })();
    return () => { cancelled = true; };
  }, []);

  // Load snapshots when tab becomes active
  useEffect(() => {
    if (activeTab !== 'snapshots') return;
    let cancelled = false;
    (async () => {
      try {
        const result = await listSnapshots(instanceId);
        if (!cancelled) setSnapshots(result);
      } catch (e) {
        if (!cancelled) setError(formatError(e));
      }
    })();
    return () => { cancelled = true; };
  }, [instanceId, activeTab]);

  // Load loadout profiles when tab becomes active
  useEffect(() => {
    if (activeTab !== 'loadout-profiles') return;
    let cancelled = false;
    (async () => {
      try {
        const result = await listLoadoutProfiles(instanceId);
        if (!cancelled) setProfiles(result);
      } catch (e) {
        if (!cancelled) setError(formatError(e));
      }
    })();
    return () => { cancelled = true; };
  }, [instanceId, activeTab]);

  const handleRemove = async (filename: string) => {
    if (!confirm(`Remove "${filename}" from this instance?`)) return;
    setRemoveBusy(filename);
    setError(null);
    try {
      const plan = await getRemovalPlan(instanceId, filename);
      if (plan.dependents.length === 0) {
        // No dependents — proceed directly
        await removeModFromInstance(instanceId, filename);
        const result = await getInstanceDetail(instanceId);
        setDetail(result);
      } else {
        // Show dependency prompt for removal
        setRemovePlanTarget({ filename, dependents: plan.dependents });
      }
    } catch (e) {
      setError(formatError(e));
    } finally {
      setRemoveBusy(null);
    }
  };

  const handleRemoveConfirm = async (selectedKeys: string[]) => {
    if (!removePlanTarget) return;
    const { filename, dependents } = removePlanTarget;
    setRemoveBusy(filename);
    setError(null);
    try {
      // Build set of all filenames to remove (target + selected dependents)
      const selectedSet = new Set(selectedKeys);
      const allFilenames = [filename, ...dependents
        .filter((d) => selectedSet.has(d.mod_id))
        .map((d) => d.filename)];

      // Best-effort: continue past individual failures
      const errors: string[] = [];
      for (const fn of allFilenames) {
        try {
          await removeModFromInstance(instanceId, fn);
        } catch (e) {
          errors.push(`${fn}: ${formatError(e)}`);
        }
      }
      const result = await getInstanceDetail(instanceId);
      setDetail(result);
      if (errors.length > 0) {
        setError(`Removed ${allFilenames.length - errors.length} of ${allFilenames.length} mods. Errors: ${errors.join('; ')}`);
      }
    } catch (e) {
      setError(formatError(e));
    } finally {
      setRemoveBusy(null);
      setRemovePlanTarget(null);
    }
  };

  const handleBrowse = async () => {
    setBrowseLoading(true);
    setAddError(null);
    try {
      const items = await browseItems(browseContentType ?? undefined, browseCategory ?? undefined, browseSort);
      setBrowseItemsList(items);
    } catch (e) {
      setAddError(formatError(e));
    } finally {
      setBrowseLoading(false);
    }
  };

  const handleSelectAddItem = async (item: RegistryItem) => {
    setSelectedAddItem(item);
    setCandidates([]);
    setSelectedCandidate(null);
    try {
      const vers = await listModVersions(instanceId, item.id);
      setCandidates(vers.items);
    } catch (e) {
      setAddError(formatError(e));
    }
  };

  const handleConfirmAdd = async () => {
    if (!selectedAddItem || !selectedCandidate) return;
    setAdding(true);
    setAddError(null);
    try {
      await installModVersion(instanceId, selectedAddItem.id, selectedCandidate);
      const result = await getInstanceDetail(instanceId);
      setDetail(result);
      setShowAdd(false);
      setSelectedAddItem(null);
      setCandidates([]);
      setSelectedCandidate(null);
    } catch (e) {
      setAddError(formatError(e));
    } finally {
      setAdding(false);
    }
  };

  const handleInstallPackMods = async () => {
    if (!packIdInput.trim()) return;
    const packId = packIdInput.trim();
    setError(null);

    // Fetch pack mods and initialize progress state
    let mods: PackModRow[];
    try {
      mods = await listPackMods(packId);
    } catch (e) {
      setError(formatError(e));
      return;
    }

    if (mods.length === 0) {
      setError(`No mods found for pack "${packId}".`);
      return;
    }

    // Initialize progress
    setPackProgress(
      mods.map((m) => ({ modId: m.mod_id, status: 'pending' as const }))
    );

    // Sequential install
    for (let i = 0; i < mods.length; i++) {
      const mod = mods[i];
      setPackProgress((prev) =>
        prev?.map((p, idx) =>
          idx === i ? { ...p, status: 'installing' as const } : p
        ) ?? prev
      );

      try {
        const candidates = await listModVersions(instanceId, mod.mod_id);
        const items = candidates.items;
        const candidate =
          items.find((c) => c.version_compat === 'compatible')
          ?? items.find((c) => c.version_compat === 'major_match')
          ?? items[0];
        if (!candidate) {
          setPackProgress((prev) =>
            prev?.map((p, idx) =>
              idx === i
                ? { ...p, status: 'failed' as const, error: 'No compatible versions found' }
                : p
            ) ?? prev
          );
          continue;
        }
        await installModVersion(instanceId, mod.mod_id, candidate);
        setPackProgress((prev) =>
          prev?.map((p, idx) =>
            idx === i ? { ...p, status: 'done' as const } : p
          ) ?? prev
        );
      } catch (e) {
        setPackProgress((prev) =>
          prev?.map((p, idx) =>
            idx === i
              ? { ...p, status: 'failed' as const, error: formatError(e) }
              : p
          ) ?? prev
        );
      }
    }
  };

  const handleDismissPackProgress = () => {
    setPackProgress(null);
    setPackInstallOpen(false);
    setPackIdInput('');
    setError(null);
    // Reload manifest
    getInstanceDetail(instanceId).then((result) => setDetail(result));
  };

  // Refresh detail (row + manifest) after lock/unlock/revert.
  const refreshDetail = async () => {
    const result = await getInstanceDetail(instanceId);
    setDetail(result);
  };

  const handleUnlock = async () => {
    setError(null);
    try {
      await unlockInstance(instanceId);
      await refreshDetail();
    } catch (e) {
      setError(formatError(e));
    }
  };

  const handleLock = async () => {
    setError(null);
    try {
      await lockInstance(instanceId);
      await refreshDetail();
    } catch (e) {
      setError(formatError(e));
    }
  };

  const handleRevert = async () => {
    if (!confirm('Revert to the snapshot taken when this instance was unlocked? This removes any mods you added since then.')) {
      return;
    }
    setError(null);
    try {
      await revertInstance(instanceId);
      await refreshDetail();
    } catch (e) {
      setError(formatError(e));
    }
  };

  const handleImportPack = async () => {
    setError(null);
    setStatus(null);
    const path = await pickOpenFile('Import Pack', ['mrpack', 'agora-pack.json', 'json']);
    if (path === null) return;
    try {
      const newInstance = await importInstancePack(path);
      if (onOpenInstanceEditor) {
        setError(null);
        setStatus(null);
        onOpenInstanceEditor(newInstance);
      } else {
        setStatus(`Imported pack: new instance created.`);
      }
    } catch (e) {
      setError(formatError(e));
    }
  };

  const handleImportMod = async () => {
    setError(null);
    setStatus(null);
    const path = await pickOpenFile('Import Mod', ['jar']);
    if (path === null) return;
    try {
      await addManualMod(instanceId, path);
      const result = await getInstanceDetail(instanceId);
      setDetail(result);
      setStatus(`Added mod: ${path.split(/[\\/]/).pop()}`);
    } catch (e) {
      setError(formatError(e));
    }
  };

  const handleDrop = async (e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setError(null);
    setStatus(null);
    const files = e.dataTransfer.files;
    if (!files || files.length === 0) return;
    const file = files[0];
    const filePath = (file as File & { path?: string }).path;
    if (!filePath) {
      setError('Could not resolve the dropped file path.');
      return;
    }
    try {
      const ext = file.name.toLowerCase();
      // .jar → manual mod install
      if (ext.endsWith('.jar')) {
        await addManualMod(instanceId, filePath);
        const result = await getInstanceDetail(instanceId);
        setDetail(result);
        setStatus(`Added "${file.name}" to instance.`);
      }
      // .mrpack, .agora-pack.json, or .json → pack import
      else if (ext.endsWith('.mrpack') || ext.endsWith('.agora-pack.json') || (ext.endsWith('.json') && file.name.toLowerCase().endsWith('.json'))) {
        const newInstance = await importInstancePack(filePath);
        if (onOpenInstanceEditor) {
          onOpenInstanceEditor(newInstance);
        } else {
          setStatus('Imported pack: new instance created.');
        }
      }
      else {
        setError('Unsupported file type. Drop a .jar mod or a .mrpack/.agora-pack.json pack.');
      }
    } catch (e) {
      setError(formatError(e));
    }
  };

  const handleExportPack = async (format: 'json' | 'mrpack') => {
    setExportBusy(true);
    setError(null);
    setStatus(null);
    try {
      const path = await exportInstancePack(instanceId, format);
      setStatus(`Exported ${format === 'json' ? 'pack' : '.mrpack'} to: ${path}`);
    } catch (e) {
      setError(formatError(e));
    } finally {
      setExportBusy(false);
    }
  };

  const row = detail?.row;
  const manifest = detail?.manifest;
  const mods = manifest?.mods ?? [];

  const filteredBrowse = browseFilter
    ? browseItemsList.filter((i) =>
        i.name.toLowerCase().includes(browseFilter.toLowerCase()) ||
        i.id.toLowerCase().includes(browseFilter.toLowerCase())
      )
    : browseItemsList;

  if (loading) {
    return (
      <div className="space-y-6">
        <BackButton onBack={onBack} />
        <div className="rounded-xl p-6 border border-dashed border-border text-center text-muted-foreground">
          Loading instance…
        </div>
      </div>
    );
  }

  if (error && !row) {
    return (
      <div className="space-y-6">
        <BackButton onBack={onBack} />
        <div className="rounded-lg bg-destructive p-3 text-sm text-destructive-foreground">
          {error}
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      <BackButton onBack={onBack} />

      {/* Header */}
      <section className="rounded-xl border border-border bg-card p-6">
        <div className="flex items-start justify-between gap-4">
          <div>
            <h2 className="text-2xl font-bold">{row?.name}</h2>
            <p className="text-xs text-muted-foreground mt-1">
              MC {row?.minecraft_version} · {manifest?.loader} {manifest?.loader_version}
            </p>
            <p className="text-xs text-muted-foreground mt-1">
              {manifest?.is_locked ? '🔒 Locked' : '🔓 Unlocked'}
              {row?.last_launched_at && (
                <span className="ml-2">· Last launched {row.last_launched_at}</span>
              )}
            </p>
          </div>
          <div className="flex gap-2">
            <button
              onClick={() => {
                setPackInstallOpen(true);
                setPackIdInput('');
                setPackProgress(null);
                setError(null);
              }}
              className="rounded-lg border border-input bg-background hover:bg-accent px-3 py-1.5 text-sm font-medium"
            >
              📦 Install all mods from pack
            </button>
            <button
              disabled
              className="rounded-lg border border-input bg-background px-3 py-1.5 text-sm font-medium cursor-not-allowed text-muted-foreground"
              title="JVM settings edit — backend command not yet implemented"
            >
              ⚙️ Edit Settings (TODO)
            </button>
            <button
              onClick={handleImportPack}
              className="rounded-lg border border-input bg-background hover:bg-accent px-3 py-1.5 text-sm font-medium"
            >
              📥 Import Pack
            </button>
            <button
              onClick={() => handleExportPack('json')}
              disabled={exportBusy}
              className="rounded-lg border border-input bg-background hover:bg-accent px-3 py-1.5 text-sm font-medium disabled:opacity-50"
            >
              {exportBusy ? 'Exporting…' : 'Export as JSON'}
            </button>
            <button
              onClick={() => handleExportPack('mrpack')}
              disabled={exportBusy}
              className="rounded-lg border border-input bg-background hover:bg-accent px-3 py-1.5 text-sm font-medium disabled:opacity-50"
            >
              {exportBusy ? 'Exporting…' : 'Export as .mrpack'}
            </button>
          </div>
        </div>

        {/* Lock / Unlock / Revert controls (§6.5) */}
        <div className="mt-3 flex items-center gap-3 text-sm">
          <span className="text-muted-foreground">
            {manifest?.is_locked ? '🔒 Locked' : '🔓 Unlocked'}
          </span>
          {manifest?.is_locked ? (
            <button
              onClick={handleUnlock}
              className="rounded-lg border border-input bg-background hover:bg-accent px-3 py-1.5 text-sm font-medium"
            >
              🔓 Unlock
            </button>
          ) : (
            <>
              <button
                onClick={handleLock}
                className="rounded-lg border border-input bg-background hover:bg-accent px-3 py-1.5 text-sm font-medium"
              >
                🔒 Lock
              </button>
              <button
                onClick={handleRevert}
                className="rounded-lg border border-input bg-background hover:bg-accent px-3 py-1.5 text-sm font-medium"
              >
                ↩ Revert
              </button>
            </>
          )}
        </div>

        {error && (
          <div className="mt-4 rounded-lg bg-destructive p-3 text-sm text-destructive-foreground">
            {error}
          </div>
        )}
      </section>

      {/* Sub-sidebar tabs */}
      <div className="flex border-b border-border gap-0">
        {(['mods', 'snapshots', 'loadout-profiles', 'import', 'console'] as const).map((tab) => (
          <button
            key={tab}
            onClick={() => setActiveTab(tab)}
            className={[
              'px-4 py-2 text-sm font-medium border-b-2 transition-colors -mb-px',
              activeTab === tab
                ? 'border-primary text-foreground'
                : 'border-transparent text-muted-foreground hover:text-foreground',
            ].join(' ')}
          >
            {tab === 'mods' ? 'Mods' : tab === 'snapshots' ? 'Snapshots' : tab === 'loadout-profiles' ? 'Loadout Profiles' : tab === 'import' ? 'Import' : 'Console'}
          </button>
        ))}
        {advancedMode && (
          <>
            <button
              key="java-args"
              onClick={() => setActiveTab('java-args')}
              className={[
                'px-4 py-2 text-sm font-medium border-b-2 transition-colors -mb-px',
                activeTab === 'java-args'
                  ? 'border-primary text-foreground'
                  : 'border-transparent text-muted-foreground hover:text-foreground',
              ].join(' ')}
            >
              Java & Args
            </button>
            <button
              key="advanced"
              onClick={() => setActiveTab('advanced')}
              className={[
                'px-4 py-2 text-sm font-medium border-b-2 transition-colors -mb-px',
                activeTab === 'advanced'
                  ? 'border-primary text-foreground'
                  : 'border-transparent text-muted-foreground hover:text-foreground',
              ].join(' ')}
            >
              Advanced
            </button>
          </>
        )}
      </div>

      {activeTab === 'mods' && (
        <>
      {/* Pack install progress */}
      {packInstallOpen && (
        <section className="rounded-xl border border-border bg-card p-4 space-y-3">
          <div className="flex items-center justify-between">
            <h3 className="font-semibold text-sm">Install mods from pack</h3>
            {!packProgress && (
              <button
                onClick={() => {
                  setPackInstallOpen(false);
                  setPackIdInput('');
                  setError(null);
                }}
                className="text-xs text-muted-foreground hover:text-foreground"
              >
                Close
              </button>
            )}
          </div>

          {!packProgress ? (
            <div className="flex gap-2">
              <input
                type="text"
                value={packIdInput}
                onChange={(e) => setPackIdInput(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') handleInstallPackMods();
                }}
                placeholder="e.g. optimized-survival"
                className="flex-1 rounded-lg border border-input bg-background px-3 py-2 text-sm"
              />
              <button
                onClick={handleInstallPackMods}
                disabled={!packIdInput.trim()}
                className="rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
              >
                Start
              </button>
            </div>
          ) : (
            <>
              <p className="text-xs text-muted-foreground">
                Installing pack: {packIdInput} ({packProgress.length} mods)
              </p>
              <div className="space-y-1">
                {packProgress.map((p, idx) => {
                  const icon =
                    p.status === 'done'
                      ? '✓'
                      : p.status === 'failed'
                        ? '✗'
                        : p.status === 'installing'
                          ? '⏳'
                          : '○';
                  const statusText =
                    p.status === 'done'
                      ? 'installed'
                      : p.status === 'failed'
                        ? p.error ?? 'failed'
                        : p.status === 'installing'
                          ? 'installing…'
                          : 'pending';
                  const lineColor =
                    p.status === 'done'
                      ? 'text-green-600 dark:text-green-400'
                      : p.status === 'failed'
                        ? 'text-destructive'
                        : p.status === 'installing'
                          ? 'text-yellow-600 dark:text-yellow-400'
                          : 'text-muted-foreground';
                  return (
                    <div key={idx} className={`text-sm ${lineColor}`}>
                      <span className="inline-block w-5 text-center">{icon}</span>{' '}
                      <span className="font-medium">{p.modId}</span> — {statusText}
                    </div>
                  );
                })}
              </div>

              {/* Summary + Done */}
              {packProgress.every((p) => p.status === 'done' || p.status === 'failed') && (
                <div className="border-t border-border pt-3">
                  {(() => {
                    const done = packProgress.filter((p) => p.status === 'done').length;
                    const failed = packProgress.filter((p) => p.status === 'failed');
                    if (failed.length === 0) {
                      return <p className="text-sm text-green-600 dark:text-green-400">Installed {done} mod{done !== 1 ? 's' : ''} successfully.</p>;
                    }
                    return (
                      <>
                        <p className="text-sm text-yellow-600 dark:text-yellow-400">
                          Installed {done} of {packProgress.length} mods. {failed.length} failed:
                        </p>
                        <ul className="mt-1 text-xs text-destructive space-y-0.5">
                          {failed.map((f, idx) => (
                            <li key={idx}>• {f.modId}: {f.error}</li>
                          ))}
                        </ul>
                      </>
                    );
                  })()}
                  <button
                    onClick={handleDismissPackProgress}
                    className="mt-3 rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90"
                  >
                    Done
                  </button>
                </div>
              )}
            </>
          )}
        </section>
      )}

      {/* Status message */}
      {status && (
        <div className="rounded-lg bg-accent text-accent-foreground p-3 text-sm">
          {status}
        </div>
      )}

      {/* Mods list */}
      <section
        className="rounded-xl border border-border bg-card p-4"
        onDragOver={(e) => { e.preventDefault(); e.stopPropagation(); }}
        onDrop={handleDrop}
      >
        <div className="flex items-center justify-between mb-3">
          <h3 className="font-semibold text-sm">Installed Mods ({mods.length})</h3>
          <div className="flex gap-2">
            <button
              onClick={handleImportMod}
              className="rounded-lg border border-input bg-background hover:bg-accent px-3 py-1.5 text-sm font-medium"
            >
              📥 Import Mod
            </button>
          </div>
        </div>
        <p className="text-xs text-muted-foreground mb-3">
          Drag and drop a .jar mod, or a .mrpack / .agora-pack.json pack file, here to install it.
        </p>
        {mods.length === 0 ? (
          <p className="text-sm text-muted-foreground">No mods installed.</p>
        ) : (
          <div className="space-y-2">
            {mods.map((mod) => (
              <div key={mod.filename} className="flex items-center justify-between rounded-lg border border-border px-3 py-2 text-sm">
                <div className="min-w-0 flex-1">
                  <span className="font-medium truncate block">{mod.filename}</span>
                  <div className="text-xs text-muted-foreground flex gap-2 mt-0.5">
                    {mod.version && <span>v{mod.version}</span>}
                    <span className="rounded-full bg-brand-600/10 text-brand-600 dark:text-brand-400 px-1.5 py-0.5 text-[10px] uppercase">{mod.source}</span>
                    <span>Installed {mod.installed_at}</span>
                  </div>
                </div>
                <button
                  onClick={() => handleRemove(mod.filename)}
                  disabled={removeBusy === mod.filename}
                  className="ml-3 text-xs text-destructive hover:underline disabled:opacity-50 whitespace-nowrap"
                >
                  {removeBusy === mod.filename ? 'Removing…' : 'Remove'}
                </button>
              </div>
            ))}
          </div>
        )}
      </section>

      {/* Add mod section */}
      {!showAdd ? (
        <button
          onClick={() => {
            setShowAdd(true);
            setBrowseFilter('');
            setSelectedAddItem(null);
            setCandidates([]);
            setSelectedCandidate(null);
            setAddError(null);
            handleBrowse();
          }}
          className="rounded-lg border border-dashed border-border px-4 py-2 text-sm font-medium text-muted-foreground hover:bg-accent w-full"
        >
          + Add Mod
        </button>
      ) : (
        <section className="rounded-xl border border-border bg-card p-4 space-y-4">
          <div className="flex items-center justify-between">
            <h3 className="font-semibold text-sm">Add Mod</h3>
            <button onClick={() => setShowAdd(false)} className="text-xs text-muted-foreground hover:text-foreground">
              Close
            </button>
          </div>

          {addError && (
            <p className="text-sm text-destructive">{addError}</p>
          )}

          {/* Filters row */}
          <div className="flex flex-col sm:flex-row gap-2">
            <input
              value={browseFilter}
              onChange={(e) => setBrowseFilter(e.target.value)}
              placeholder="Search mods…"
              className="flex-1 rounded-lg border border-input bg-background px-3 py-2 text-sm"
            />
            <select
              value={browseContentType ?? ''}
              onChange={(e) => setBrowseContentType(e.target.value || null)}
              className="rounded-lg border border-input bg-background px-3 py-2 text-sm"
            >
              <option value="">All types</option>
              {CONTENT_TYPES.map((ct) => (
                <option key={ct} value={ct}>{ct}</option>
              ))}
            </select>
            <select
              value={browseSort}
              onChange={(e) => setBrowseSort(e.target.value as SortOption)}
              className="rounded-lg border border-input bg-background px-3 py-2 text-sm"
            >
              {SORTS.map((s) => (
                <option key={s.value} value={s.value}>{s.label}</option>
              ))}
            </select>
          </div>

          {categories.length > 0 && (
            <div className="flex flex-wrap gap-2">
              <button
                onClick={() => setBrowseCategory(null)}
                className={[
                  'px-3 py-1 rounded-full text-sm border transition-colors',
                  browseCategory === null
                    ? 'bg-primary text-primary-foreground border-primary'
                    : 'border-border hover:bg-accent',
                ].join(' ')}
              >
                All
              </button>
              {categories.map((c) => (
                <button
                  key={c.id}
                  onClick={() => setBrowseCategory(c.id)}
                  className={[
                    'px-3 py-1 rounded-full text-sm border transition-colors',
                    browseCategory === c.id
                      ? 'bg-primary text-primary-foreground border-primary'
                      : 'border-border hover:bg-accent',
                  ].join(' ')}
                >
                  {c.display_name}
                </button>
              ))}
            </div>
          )}

          {browseLoading ? (
            <p className="text-xs text-muted-foreground">Loading mods…</p>
          ) : filteredBrowse.length === 0 ? (
            <p className="text-sm text-muted-foreground">No mods found.</p>
          ) : (
            <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
              {filteredBrowse.map((item) => (
                <button
                  key={item.id}
                  onClick={() => handleSelectAddItem(item)}
                  className={`text-left rounded-lg border px-3 py-2 text-sm transition-colors ${
                    selectedAddItem?.id === item.id
                      ? 'border-primary bg-primary/10'
                      : 'border-border hover:bg-accent'
                  }`}
                >
                  <div className="flex items-start gap-2">
                    {item.icon_url && (
                      <img
                        src={item.icon_url}
                        alt={item.name}
                        className="h-10 w-10 rounded border object-contain border-border"
                      />
                    )}
                    <div className="min-w-0">
                      <span className="font-medium block truncate">{item.name}</span>
                      <span className="text-xs text-muted-foreground">
                        {item.content_type} · {item.download_strategy}
                      </span>
                    </div>
                  </div>
                </button>
              ))}
            </div>
          )}

          {/* Version picker */}
          {selectedAddItem && (
            <div className="border-t border-border pt-3">
              <p className="text-xs font-medium mb-2">
                Available versions for {selectedAddItem.name}
              </p>
              {candidates.length === 0 ? (
                <p className="text-xs text-muted-foreground">No versions available.</p>
              ) : (
                <div className="space-y-1 max-h-40 overflow-y-auto">
                  {[...candidates].sort((a, b) => {
                    const tier = (c: ModVersionCandidate) =>
                      c.version_compat === 'compatible' ? 0
                      : c.version_compat === 'major_match' ? 1
                      : 2;
                    return tier(a) - tier(b);
                  }).map((cand, idx) => (
                    <button
                      key={idx}
                      onClick={() => setSelectedCandidate(cand)}
                      className={`w-full text-left rounded-lg border px-3 py-2 text-sm transition-colors ${
                        selectedCandidate?.filename === cand.filename
                          ? 'border-primary bg-primary/10'
                          : 'border-border hover:bg-accent'
                      }`}
                    >
                      <div className="flex items-center justify-between">
                        <span className="font-medium">{cand.version}</span>
                        {cand.version_compat === 'compatible' ? (
                          <span className="text-xs text-green-600 dark:text-green-400">✓ compatible</span>
                        ) : cand.version_compat === 'major_match' ? (
                          <span className="text-xs text-yellow-600 dark:text-yellow-400">⚠ may not match your exact version</span>
                        ) : (
                          <span className="text-xs text-muted-foreground">may not match</span>
                        )}
                      </div>
                      <p className="text-xs text-muted-foreground mt-0.5 truncate">{cand.filename}</p>
                    </button>
                  ))}
                </div>
              )}

              {selectedCandidate && (
                <button
                  onClick={handleConfirmAdd}
                  disabled={adding}
                  className="mt-3 w-full rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
                >
                  {adding ? 'Installing…' : `Install ${selectedCandidate.filename}`}
                </button>
              )}
            </div>
          )}
        </section>
      )}

      {/* Dependency removal prompt */}
      {removePlanTarget && (
        <DependencyPrompt
          title="Remove mod and dependents"
          actionLabel="Remove selected"
          candidates={removePlanTarget.dependents.map((d) => ({
            key: d.mod_id,
            label: d.mod_id || d.filename,
            requirement: d.requirement,
            source: d.source,
          }))}
          onConfirm={handleRemoveConfirm}
          onCancel={() => setRemovePlanTarget(null)}
        />
      )}
        </>
      )}

      {activeTab === 'snapshots' && (
        <section className="rounded-xl border border-border bg-card p-4 space-y-4">
          <div className="flex items-center justify-between">
            <h3 className="font-semibold text-sm">Snapshots</h3>
            <div className="flex gap-2">
              <input
                type="text"
                value={snapshotLabelInput}
                onChange={(e) => setSnapshotLabelInput(e.target.value)}
                placeholder="Optional label…"
                className="rounded-lg border border-input bg-background px-3 py-1.5 text-sm w-48"
              />
              <button
                onClick={async () => {
                  setError(null);
                  try {
                    await createSnapshot(instanceId, snapshotLabelInput || undefined);
                    const result = await listSnapshots(instanceId);
                    setSnapshots(result);
                    setSnapshotLabelInput('');
                  } catch (e) {
                    setError(formatError(e));
                  }
                }}
                className="rounded-lg bg-primary px-4 py-1.5 text-sm font-medium text-primary-foreground hover:bg-primary/90 whitespace-nowrap"
              >
                Create Snapshot
              </button>
            </div>
          </div>

          {snapshots.length === 0 ? (
            <p className="text-sm text-muted-foreground">
              No snapshots yet. Create one to save a restore point.
            </p>
          ) : (
            <div className="space-y-2">
              {snapshots.map((snap) => (
                <div key={snap.id} className="flex items-center justify-between rounded-lg border border-border px-3 py-2 text-sm">
                  <div className="min-w-0 flex-1">
                    <span className="font-medium block">{snap.label ?? 'Unnamed'}</span>
                    <span className="text-xs text-muted-foreground">
                      {snap.created_at} · {snap.file_count} file{snap.file_count !== 1 ? 's' : ''}
                    </span>
                  </div>
                  <div className="flex gap-2 ml-3">
                    <button
                      onClick={async () => {
                        setSnapshotBusy(snap.id);
                        setError(null);
                        try {
                          await restoreSnapshot(instanceId, snap.id);
                          const result = await listSnapshots(instanceId);
                          setSnapshots(result);
                          setStatus('Snapshot restored.');
                        } catch (e) {
                          setError(formatError(e));
                        } finally {
                          setSnapshotBusy(null);
                        }
                      }}
                      disabled={snapshotBusy === snap.id}
                      className="text-xs text-foreground hover:underline disabled:opacity-50"
                    >
                      {snapshotBusy === snap.id ? 'Restoring…' : 'Restore'}
                    </button>
                    {confirmDeleteSnapshot === snap.id ? (
                      <div className="flex gap-1">
                        <button
                          onClick={async () => {
                            setSnapshotBusy(snap.id);
                            setError(null);
                            try {
                              await deleteSnapshot(instanceId, snap.id);
                              const result = await listSnapshots(instanceId);
                              setSnapshots(result);
                              setConfirmDeleteSnapshot(null);
                            } catch (e) {
                              setError(formatError(e));
                            } finally {
                              setSnapshotBusy(null);
                            }
                          }}
                          className="text-xs text-destructive font-medium"
                        >
                          Confirm
                        </button>
                        <button
                          onClick={() => setConfirmDeleteSnapshot(null)}
                          className="text-xs text-muted-foreground"
                        >
                          Cancel
                        </button>
                      </div>
                    ) : (
                      <button
                        onClick={() => setConfirmDeleteSnapshot(snap.id)}
                        className="text-xs text-destructive hover:underline"
                      >
                        Delete
                      </button>
                    )}
                  </div>
                </div>
              ))}
            </div>
          )}
        </section>
      )}

      {activeTab === 'loadout-profiles' && (
        <section className="rounded-xl border border-border bg-card p-4 space-y-4">
          <div className="flex items-center justify-between">
            <h3 className="font-semibold text-sm">Loadout Profiles</h3>
            <div className="flex gap-2">
              <input
                type="text"
                value={profileNameInput}
                onChange={(e) => setProfileNameInput(e.target.value)}
                placeholder="Profile name…"
                className="rounded-lg border border-input bg-background px-3 py-1.5 text-sm w-48"
              />
              <button
                onClick={async () => {
                  if (!profileNameInput.trim()) return;
                  setError(null);
                  try {
                    await createLoadoutProfile(instanceId, profileNameInput.trim());
                    const result = await listLoadoutProfiles(instanceId);
                    setProfiles(result);
                    setProfileNameInput('');
                    setStatus(`Profile "${profileNameInput.trim()}" created.`);
                  } catch (e) {
                    setError(formatError(e));
                  }
                }}
                className="rounded-lg bg-primary px-4 py-1.5 text-sm font-medium text-primary-foreground hover:bg-primary/90 whitespace-nowrap"
              >
                Create Profile
              </button>
            </div>
          </div>

          <button
            onClick={async () => {
              setError(null);
              try {
                await createLoadoutProfile(instanceId, `Current Setup ${new Date().toLocaleString()}`);
                const result = await listLoadoutProfiles(instanceId);
                setProfiles(result);
                setStatus('Current mod setup saved as profile.');
              } catch (e) {
                setError(formatError(e));
              }
            }}
            className="rounded-lg border border-input bg-background hover:bg-accent px-4 py-2 text-sm font-medium w-full"
          >
            + Save Current as Profile
          </button>

          {profiles.length === 0 ? (
            <p className="text-sm text-muted-foreground">
              No loadout profiles yet. Create one or save the current mod setup.
            </p>
          ) : (
            <div className="space-y-2">
              {profiles.map((prof) => (
                <div key={prof.name} className="flex items-center justify-between rounded-lg border border-border px-3 py-2 text-sm">
                  <div className="min-w-0 flex-1">
                    <span className="font-medium block">{prof.name}</span>
                    <span className="text-xs text-muted-foreground">{prof.enabled_mods.length} mod{prof.enabled_mods.length !== 1 ? 's' : ''}</span>
                  </div>
                  <div className="flex gap-2 ml-3">
                    <button
                      onClick={async () => {
                        setProfileBusy(prof.name);
                        setError(null);
                        try {
                          await applyLoadoutProfile(instanceId, prof.name);
                          const result = await listLoadoutProfiles(instanceId);
                          setProfiles(result);
                          setStatus(`Profile "${prof.name}" applied.`);
                        } catch (e) {
                          setError(formatError(e));
                        } finally {
                          setProfileBusy(null);
                        }
                      }}
                      disabled={profileBusy === prof.name}
                      className="text-xs text-foreground hover:underline disabled:opacity-50"
                    >
                      {profileBusy === prof.name ? 'Applying…' : 'Apply'}
                    </button>
                    {confirmDeleteProfile === prof.name ? (
                      <div className="flex gap-1">
                        <button
                          onClick={async () => {
                            setProfileBusy(prof.name);
                            setError(null);
                            try {
                              await deleteLoadoutProfile(instanceId, prof.name);
                              const result = await listLoadoutProfiles(instanceId);
                              setProfiles(result);
                              setConfirmDeleteProfile(null);
                            } catch (e) {
                              setError(formatError(e));
                            } finally {
                              setProfileBusy(null);
                            }
                          }}
                          className="text-xs text-destructive font-medium"
                        >
                          Confirm
                        </button>
                        <button
                          onClick={() => setConfirmDeleteProfile(null)}
                          className="text-xs text-muted-foreground"
                        >
                          Cancel
                        </button>
                      </div>
                    ) : (
                      <button
                        onClick={() => setConfirmDeleteProfile(prof.name)}
                        className="text-xs text-destructive hover:underline"
                      >
                        Delete
                      </button>
                    )}
                  </div>
                </div>
              ))}
            </div>
          )}
        </section>
      )}

      {activeTab === 'import' && (
        <section className="rounded-xl border border-border bg-card p-4 space-y-4">
          <h3 className="font-semibold text-sm">Import from file</h3>
          <p className="text-xs text-muted-foreground">
            Import a Modrinth pack (.mrpack) or a ZIP archive as a new instance.
          </p>
          <div className="flex items-center justify-between">
            <span className="text-sm">Symlink saves (recommended)</span>
            <button
              onClick={() => setSymlinkSaves(!symlinkSaves)}
              className={[
                'relative inline-flex h-5 w-9 items-center rounded-full transition-colors',
                symlinkSaves ? 'bg-primary' : 'bg-border',
              ].join(' ')}
            >
              <span
                className={[
                  'inline-block h-3.5 w-3.5 rounded-full bg-primary-foreground transition-transform',
                  symlinkSaves ? 'translate-x-[18px]' : 'translate-x-[3px]',
                ].join(' ')}
              />
            </button>
          </div>
          <button
            onClick={async () => {
              setImportBusy(true);
              setError(null);
              try {
                const path = await pickOpenFile('Import Instance', ['mrpack', 'zip']);
                if (path === null) { setImportBusy(false); return; }
                const result = await importInstance(path, symlinkSaves);
                if (onOpenInstanceEditor) {
                  onOpenInstanceEditor(result.instance_id);
                } else {
                  setStatus(`Imported "${result.name}" (MC ${result.minecraft_version}).`);
                }
              } catch (e) {
                setError(formatError(e));
              } finally {
                setImportBusy(false);
              }
            }}
            disabled={importBusy}
            className="rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50 w-full"
          >
            {importBusy ? 'Importing…' : 'Select File & Import'}
          </button>
        </section>
      )}

      {activeTab === 'console' && (
        <section className="rounded-xl border border-border bg-card p-4 space-y-3">
          <h3 className="font-semibold text-sm">Game Console</h3>
          <p className="text-xs text-muted-foreground">
            Live stdout/stderr from the launched Minecraft process. Logs stream here when the instance is running via Agora's direct launcher.
          </p>
          <ConsoleView instanceId={instanceId} className="mt-2" />
        </section>
      )}

      {activeTab === 'java-args' && (
        <section className="rounded-xl border border-border bg-card p-4 space-y-3">
          <h3 className="font-semibold text-sm">Java & Args</h3>
          <p className="text-xs text-muted-foreground">
            Configure per-instance Java path, JVM arguments, and GC profile. (Full GC architect UI coming soon — for now, settings are controlled via the Settings page.)
          </p>
        </section>
      )}

      {activeTab === 'advanced' && (
        <section className="rounded-xl border border-border bg-card p-4 space-y-3">
          <h3 className="font-semibold text-sm">Advanced</h3>
          <p className="text-xs text-muted-foreground">
            Instance-level advanced settings: custom launch commands, environment variables, wrapper scripts. (Coming soon.)
          </p>
        </section>
      )}
    </div>
  );
}

function BackButton({ onBack }: { onBack: () => void }) {
  return (
    <button
      onClick={onBack}
      className="rounded-lg border border-input bg-background hover:bg-accent px-3 py-1.5 text-sm font-medium"
    >
      ← Back
    </button>
  );
}
