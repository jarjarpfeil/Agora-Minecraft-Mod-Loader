import { useEffect, useState } from 'react';
import { DependencyPrompt } from '../components/DependencyPrompt';
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
  type InstanceDetail,
  type RegistryItem,
  type CategoryInfo,
  type ModVersionCandidate,
  type SortOption,
  type PackModRow,
  type DependentInfo,
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
      setCandidates(vers);
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
        const candidate = candidates.find((c) => c.is_compatible) ?? candidates[0];
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
        <div className="rounded-xl p-6 border border-dashed border-gray-300 dark:border-gray-600 text-center text-[rgb(var(--muted))]">
          Loading instance…
        </div>
      </div>
    );
  }

  if (error && !row) {
    return (
      <div className="space-y-6">
        <BackButton onBack={onBack} />
        <div className="rounded-lg border border-red-300 bg-red-50 p-3 text-sm text-red-700 dark:border-red-700 dark:bg-red-900/30 dark:text-red-200">
          {error}
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      <BackButton onBack={onBack} />

      {/* Header */}
      <section className="rounded-xl border border-gray-200 dark:border-gray-700 surface p-6">
        <div className="flex items-start justify-between gap-4">
          <div>
            <h2 className="text-2xl font-bold">{row?.name}</h2>
            <p className="text-xs text-[rgb(var(--muted))] mt-1">
              MC {row?.minecraft_version} · {manifest?.loader} {manifest?.loader_version}
            </p>
            <p className="text-xs text-[rgb(var(--muted))] mt-1">
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
              className="rounded-lg border border-gray-300 dark:border-gray-600 px-3 py-1.5 text-sm font-medium text-[rgb(var(--muted))] hover:bg-gray-50 dark:hover:bg-gray-800"
            >
              📦 Install all mods from pack
            </button>
            <button
              disabled
              className="rounded-lg border border-gray-300 dark:border-gray-600 px-3 py-1.5 text-sm font-medium text-[rgb(var(--muted))] cursor-not-allowed"
              title="JVM settings edit — backend command not yet implemented"
            >
              ⚙️ Edit Settings (TODO)
            </button>
            <button
              onClick={handleImportPack}
              className="rounded-lg border border-gray-300 dark:border-gray-600 px-3 py-1.5 text-sm font-medium text-[rgb(var(--muted))] hover:bg-gray-50 dark:hover:bg-gray-800"
            >
              📥 Import Pack
            </button>
            <button
              onClick={() => handleExportPack('json')}
              disabled={exportBusy}
              className="rounded-lg border border-gray-300 dark:border-gray-600 px-3 py-1.5 text-sm font-medium text-[rgb(var(--muted))] hover:bg-gray-50 dark:hover:bg-gray-800 disabled:opacity-50"
            >
              {exportBusy ? 'Exporting…' : 'Export as JSON'}
            </button>
            <button
              onClick={() => handleExportPack('mrpack')}
              disabled={exportBusy}
              className="rounded-lg border border-gray-300 dark:border-gray-600 px-3 py-1.5 text-sm font-medium text-[rgb(var(--muted))] hover:bg-gray-50 dark:hover:bg-gray-800 disabled:opacity-50"
            >
              {exportBusy ? 'Exporting…' : 'Export as .mrpack'}
            </button>
          </div>
        </div>

        {/* Lock / Unlock / Revert controls (§6.5) */}
        <div className="mt-3 flex items-center gap-3 text-sm">
          <span className="text-[rgb(var(--muted))]">
            {manifest?.is_locked ? '🔒 Locked' : '🔓 Unlocked'}
          </span>
          {manifest?.is_locked ? (
            <button
              onClick={handleUnlock}
              className="rounded-lg border border-gray-300 dark:border-gray-600 px-3 py-1.5 text-sm font-medium text-[rgb(var(--muted))] hover:bg-gray-50 dark:hover:bg-gray-800"
            >
              🔓 Unlock
            </button>
          ) : (
            <>
              <button
                onClick={handleLock}
                className="rounded-lg border border-gray-300 dark:border-gray-600 px-3 py-1.5 text-sm font-medium text-[rgb(var(--muted))] hover:bg-gray-50 dark:hover:bg-gray-800"
              >
                🔒 Lock
              </button>
              <button
                onClick={handleRevert}
                className="rounded-lg border border-gray-300 dark:border-gray-600 px-3 py-1.5 text-sm font-medium text-[rgb(var(--muted))] hover:bg-gray-50 dark:hover:bg-gray-800"
              >
                ↩ Revert
              </button>
            </>
          )}
        </div>

        {error && (
          <div className="mt-4 rounded-lg border border-red-300 bg-red-50 p-3 text-sm text-red-700 dark:border-red-700 dark:bg-red-900/30 dark:text-red-200">
            {error}
          </div>
        )}
      </section>

      {/* Pack install progress */}
      {packInstallOpen && (
        <section className="rounded-xl border border-gray-200 dark:border-gray-700 surface p-4 space-y-3">
          <div className="flex items-center justify-between">
            <h3 className="font-semibold text-sm">Install mods from pack</h3>
            {!packProgress && (
              <button
                onClick={() => {
                  setPackInstallOpen(false);
                  setPackIdInput('');
                  setError(null);
                }}
                className="text-xs text-[rgb(var(--muted))] hover:text-[rgb(var(--foreground))]"
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
                className="flex-1 rounded-lg border border-gray-300 dark:border-gray-600 bg-transparent px-3 py-2 text-sm"
              />
              <button
                onClick={handleInstallPackMods}
                disabled={!packIdInput.trim()}
                className="rounded-lg bg-brand-600 px-4 py-2 text-sm font-medium text-white hover:bg-brand-700 disabled:opacity-50"
              >
                Start
              </button>
            </div>
          ) : (
            <>
              <p className="text-xs text-[rgb(var(--muted))]">
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
                        ? 'text-red-600 dark:text-red-400'
                        : p.status === 'installing'
                          ? 'text-yellow-600 dark:text-yellow-400'
                          : 'text-[rgb(var(--muted))]';
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
                <div className="border-t border-gray-200 dark:border-gray-700 pt-3">
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
                        <ul className="mt-1 text-xs text-red-600 dark:text-red-400 space-y-0.5">
                          {failed.map((f, idx) => (
                            <li key={idx}>• {f.modId}: {f.error}</li>
                          ))}
                        </ul>
                      </>
                    );
                  })()}
                  <button
                    onClick={handleDismissPackProgress}
                    className="mt-3 rounded-lg bg-brand-600 px-4 py-2 text-sm font-medium text-white hover:bg-brand-700"
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
        <div className="rounded-lg border border-green-300 bg-green-50 p-3 text-sm text-green-700 dark:border-green-700 dark:bg-green-900/30 dark:text-green-200">
          {status}
        </div>
      )}

      {/* Mods list */}
      <section
        className="rounded-xl border border-gray-200 dark:border-gray-700 surface p-4"
        onDragOver={(e) => { e.preventDefault(); e.stopPropagation(); }}
        onDrop={handleDrop}
      >
        <div className="flex items-center justify-between mb-3">
          <h3 className="font-semibold text-sm">Installed Mods ({mods.length})</h3>
          <div className="flex gap-2">
            <button
              onClick={handleImportMod}
              className="rounded-lg border border-gray-300 dark:border-gray-600 px-3 py-1.5 text-sm font-medium text-[rgb(var(--muted))] hover:bg-gray-50 dark:hover:bg-gray-800"
            >
              📥 Import Mod
            </button>
          </div>
        </div>
        <p className="text-xs text-[rgb(var(--muted))] mb-3">
          Drag and drop a .jar mod, or a .mrpack / .agora-pack.json pack file, here to install it.
        </p>
        {mods.length === 0 ? (
          <p className="text-sm text-[rgb(var(--muted))]">No mods installed.</p>
        ) : (
          <div className="space-y-2">
            {mods.map((mod) => (
              <div key={mod.filename} className="flex items-center justify-between rounded-lg border border-gray-200 dark:border-gray-700 px-3 py-2 text-sm">
                <div className="min-w-0 flex-1">
                  <span className="font-medium truncate block">{mod.filename}</span>
                  <div className="text-xs text-[rgb(var(--muted))] flex gap-2 mt-0.5">
                    {mod.version && <span>v{mod.version}</span>}
                    <span className="rounded-full bg-brand-600/10 text-brand-600 dark:text-brand-400 px-1.5 py-0.5 text-[10px] uppercase">{mod.source}</span>
                    <span>Installed {mod.installed_at}</span>
                  </div>
                </div>
                <button
                  onClick={() => handleRemove(mod.filename)}
                  disabled={removeBusy === mod.filename}
                  className="ml-3 text-xs text-red-600 dark:text-red-400 hover:underline disabled:opacity-50 whitespace-nowrap"
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
          className="rounded-lg border border-dashed border-gray-300 dark:border-gray-600 px-4 py-2 text-sm font-medium text-[rgb(var(--muted))] hover:bg-gray-50 dark:hover:bg-gray-800 w-full"
        >
          + Add Mod
        </button>
      ) : (
        <section className="rounded-xl border border-gray-200 dark:border-gray-700 surface p-4 space-y-4">
          <div className="flex items-center justify-between">
            <h3 className="font-semibold text-sm">Add Mod</h3>
            <button onClick={() => setShowAdd(false)} className="text-xs text-[rgb(var(--muted))] hover:text-[rgb(var(--foreground))]">
              Close
            </button>
          </div>

          {addError && (
            <p className="text-sm text-red-600 dark:text-red-300">{addError}</p>
          )}

          {/* Filters row */}
          <div className="flex flex-col sm:flex-row gap-2">
            <input
              value={browseFilter}
              onChange={(e) => setBrowseFilter(e.target.value)}
              placeholder="Search mods…"
              className="flex-1 rounded-lg border border-gray-300 dark:border-gray-600 bg-transparent px-3 py-2 text-sm"
            />
            <select
              value={browseContentType ?? ''}
              onChange={(e) => setBrowseContentType(e.target.value || null)}
              className="rounded-lg border border-gray-300 dark:border-gray-600 bg-transparent px-3 py-2 text-sm"
            >
              <option value="">All types</option>
              {CONTENT_TYPES.map((ct) => (
                <option key={ct} value={ct}>{ct}</option>
              ))}
            </select>
            <select
              value={browseSort}
              onChange={(e) => setBrowseSort(e.target.value as SortOption)}
              className="rounded-lg border border-gray-300 dark:border-gray-600 bg-transparent px-3 py-2 text-sm"
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
                    ? 'bg-brand-600 text-white border-brand-600'
                    : 'border-gray-300 dark:border-gray-600 hover:bg-gray-100 dark:hover:bg-gray-800',
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
                      ? 'bg-brand-600 text-white border-brand-600'
                      : 'border-gray-300 dark:border-gray-600 hover:bg-gray-100 dark:hover:bg-gray-800',
                  ].join(' ')}
                >
                  {c.display_name}
                </button>
              ))}
            </div>
          )}

          {browseLoading ? (
            <p className="text-xs text-[rgb(var(--muted))]">Loading mods…</p>
          ) : filteredBrowse.length === 0 ? (
            <p className="text-sm text-[rgb(var(--muted))]">No mods found.</p>
          ) : (
            <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
              {filteredBrowse.map((item) => (
                <button
                  key={item.id}
                  onClick={() => handleSelectAddItem(item)}
                  className={`text-left rounded-lg border px-3 py-2 text-sm transition-colors ${
                    selectedAddItem?.id === item.id
                      ? 'border-brand-500 bg-brand-50 dark:bg-brand-900/20'
                      : 'border-gray-200 dark:border-gray-700 hover:bg-gray-50 dark:hover:bg-gray-800'
                  }`}
                >
                  <div className="flex items-start gap-2">
                    {item.icon_url && (
                      <img
                        src={item.icon_url}
                        alt={item.name}
                        className="h-10 w-10 rounded border object-contain dark:border-gray-600"
                      />
                    )}
                    <div className="min-w-0">
                      <span className="font-medium block truncate">{item.name}</span>
                      <span className="text-xs text-[rgb(var(--muted))]">
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
            <div className="border-t border-gray-200 dark:border-gray-700 pt-3">
              <p className="text-xs font-medium mb-2">
                Available versions for {selectedAddItem.name}
              </p>
              {candidates.length === 0 ? (
                <p className="text-xs text-[rgb(var(--muted))]">No versions available.</p>
              ) : (
                <div className="space-y-1 max-h-40 overflow-y-auto">
                  {candidates.map((cand, idx) => (
                    <button
                      key={idx}
                      onClick={() => setSelectedCandidate(cand)}
                      className={`w-full text-left rounded-lg border px-3 py-2 text-sm transition-colors ${
                        selectedCandidate?.filename === cand.filename
                          ? 'border-brand-500 bg-brand-50 dark:bg-brand-900/20'
                          : 'border-gray-200 dark:border-gray-700 hover:bg-gray-50 dark:hover:bg-gray-800'
                      }`}
                    >
                      <div className="flex items-center justify-between">
                        <span className="font-medium">{cand.version}</span>
                        {cand.is_compatible ? (
                          <span className="text-xs text-green-600 dark:text-green-400">✓ compatible</span>
                        ) : (
                          <span className="text-xs text-[rgb(var(--muted))]">may not match</span>
                        )}
                      </div>
                      <p className="text-xs text-[rgb(var(--muted))] mt-0.5 truncate">{cand.filename}</p>
                    </button>
                  ))}
                </div>
              )}

              {selectedCandidate && (
                <button
                  onClick={handleConfirmAdd}
                  disabled={adding}
                  className="mt-3 w-full rounded-lg bg-brand-600 px-4 py-2 text-sm font-medium text-white hover:bg-brand-700 disabled:opacity-50"
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
    </div>
  );
}

function BackButton({ onBack }: { onBack: () => void }) {
  return (
    <button
      onClick={onBack}
      className="rounded-lg border border-gray-300 dark:border-gray-600 px-3 py-1.5 text-sm font-medium hover:bg-gray-100 dark:hover:bg-gray-800"
    >
      ← Back
    </button>
  );
}
