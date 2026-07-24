import { useEffect, useState, useRef } from 'react';
import { useAdvancedMode } from '../components/AdvancedModeContext';
import { ConsoleView } from '../components/ConsoleView';
import { InstallFlow } from '../components/InstallFlow';
import type { BatchInstallItem, InstallIntent } from '../lib/installFlow';
import {
  getInstanceDetail,
  getRegistryItem,
  fetchModrinthProject,
  enableInstanceMod,
  disableInstanceMod,
  exportInstancePack,
  formatError,
  importInstancePack,
  inspectJavaExecutable,
  pickOpenFile,
  importInstance,
  exportLockfile,
  verifyLockfile,
  repairLockfile,
  importLockfile,
  updateInstanceJava,
  updateInstanceJvm,
  computeGcArgs,
  browseItems,
  listModVersions,
  listPackMods,
  unlockInstance,
  lockInstance,
  renameInstance,
  revertInstance,
  listSnapshots,
  createSnapshot,
  restoreSnapshot,
  deleteSnapshot,
  detectDrift,
  listLoadoutProfiles,
  createLoadoutProfile,
  applyLoadoutProfile,
  deleteLoadoutProfile,
  openInstanceFolder,
  revealPath,
  type InstanceDetail,
  type InstanceManifest,
  type JavaRuntimeSummary,
  type GcProfile,
  type RegistryItem,
  type PackModRow,
  type InstalledMod,
  type Snapshot,
  type SnapshotDiff,
  type LoadoutProfile,
  type LockfileDriftReport,
} from '../lib/tauri';
import { Play } from 'lucide-react';

function installedModKey(mod: InstalledMod): string {
  return `${mod.filename}:${mod.sha256}`;
}

function installedModDetailId(mod: InstalledMod): string | null {
  return mod.registry_id || mod.modrinth_id || mod.mod_jar_id || null;
}

function installedModSourceLabel(source: string): string {
  const normalized = (source ?? '').trim().toLowerCase().replace(/[\s-]+/g, '_');
  if (normalized === 'modrinth_raw' || normalized === 'modrinth') return 'Modrinth';
  if (normalized === 'modrinth_pack') return 'Modrinth Pack';
  if (normalized.includes('github')) return 'GitHub Release';
  if (normalized === 'registry' || normalized === 'curated') return 'Agora Registry';
  if (normalized.includes('manual') || normalized === 'local') return 'Manual';
  return 'Other';
}

function formatInstalledAt(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  const weekday = new Intl.DateTimeFormat(undefined, { weekday: 'long' }).format(date);
  const month = date.getMonth() + 1;
  const day = date.getDate();
  const year = date.getFullYear();
  const hour24 = date.getHours();
  const hour = hour24 % 12 || 12;
  const minute = String(date.getMinutes()).padStart(2, '0');
  const meridiem = hour24 < 12 ? 'AM' : 'PM';
  return `${weekday}, ${month}/${day}/${year} ${hour}:${minute} ${meridiem}`;
}

const CONTENT_KEYS = ['mods', 'resourcepacks', 'shaders', 'datapacks', 'worlds'] as const;

function updateManifestEntryEnabled(
  manifest: InstanceManifest,
  filename: string,
  enabled: boolean,
): InstanceManifest {
  for (const key of CONTENT_KEYS) {
    if (!manifest[key].some((entry) => entry.filename === filename)) continue;
    return {
      ...manifest,
      [key]: manifest[key].map((entry) =>
        entry.filename === filename ? { ...entry, enabled } : entry,
      ),
    };
  }
  return manifest;
}

function installedModMetadataKey(mods: InstalledMod[] | undefined): string {
  return (mods ?? [])
    .map((mod) => `${installedModKey(mod)}:${installedModDetailId(mod) ?? ''}`)
    .join('|');
}

type GcMode = 'auto' | GcProfile;

function storedGcMode(value: string | undefined): GcMode {
  switch ((value ?? '').toLowerCase()) {
    case 'zgc':
    case 'low_latency':
      return 'low_latency';
    case 'manual':
      return 'manual';
    case 'g1gc':
      // Legacy rows used g1gc as the implicit default before Auto existed.
      return 'auto';
    case 'high_efficiency':
      return 'high_efficiency';
    default:
      return 'auto';
  }
}

function previewJavaMajor(version: string | undefined): number {
  const parts = (version ?? '').split('.');
  const first = Number(parts[0]);
  if (first >= 26) return 25;
  const minor = first === 1 ? Number(parts[1]) : first;
  const patch = first === 1 ? Number(parts[2]) : Number(parts[1]);
  if (minor >= 21 || (minor === 20 && patch >= 5)) return 21;
  if (minor >= 18) return 17;
  return 8;
}

export function InstanceEditor({ instanceId, onBack, onOpenInstanceEditor, onOpenModDetail, onOpenBrowseForInstance, onLaunch }: { instanceId: string; onBack: () => void; onOpenInstanceEditor?: (instanceId: string) => void; onOpenModDetail?: (itemId: string) => void; onOpenBrowseForInstance?: (instanceId: string, contentType?: string) => void; onLaunch?: (instanceId: string) => Promise<boolean> }) {
  const [detail, setDetail] = useState<InstanceDetail | null>(null);
  const [modDisplayNames, setModDisplayNames] = useState<Record<string, string>>({});
  const modDisplayNameCache = useRef<Map<string, string | null>>(new Map());
  const modDisplayNameRequests = useRef<Map<string, Promise<string | null>>>(new Map());
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [exportBusy, setExportBusy] = useState(false);

  const { advancedMode } = useAdvancedMode();

  // Sub-sidebar active tab
  const [activeTab, setActiveTab] = useState<'mods' | 'resourcepacks' | 'shaders' | 'datapacks' | 'snapshots' | 'loadout-profiles' | 'import' | 'export' | 'console' | 'java-args'>('mods');

  // Snapshots state (Phase 6)
  const [snapshots, setSnapshots] = useState<Snapshot[]>([]);
  const [snapshotLabelInput, setSnapshotLabelInput] = useState('');
  const [snapshotBusy, setSnapshotBusy] = useState<string | null>(null);
  const [confirmDeleteSnapshot, setConfirmDeleteSnapshot] = useState<string | null>(null);
  const [snapshotDiff, setSnapshotDiff] = useState<{ snapshotId: string; diff: SnapshotDiff } | null>(null);

  // Loadout profiles state (Phase 6)
  const [profiles, setProfiles] = useState<LoadoutProfile[]>([]);
  const [profileNameInput, setProfileNameInput] = useState('');
  const [profileBusy, setProfileBusy] = useState<string | null>(null);
  const [confirmDeleteProfile, setConfirmDeleteProfile] = useState<string | null>(null);

  // Import state (Phase 6)
  const [importBusy, setImportBusy] = useState(false);
  const [symlinkSaves, setSymlinkSaves] = useState(true);
  const [lockfileText, setLockfileText] = useState('');
  const [lockfileBusy, setLockfileBusy] = useState<'export' | 'verify' | 'repair' | 'clone' | 'copy' | null>(null);
  const [lockfileReport, setLockfileReport] = useState<LockfileDriftReport | null>(null);
  const [lockfileNotice, setLockfileNotice] = useState<string | null>(null);

  // Java & Args state
  const [instanceJavaPath, setInstanceJavaPath] = useState('');
  const [instanceJavaArgs, setInstanceJavaArgs] = useState('');
  const [instanceJvmMemory, setInstanceJvmMemory] = useState(4096);
  const [instanceGcMode, setInstanceGcMode] = useState<GcMode>('auto');
  const [instanceAlwaysPreTouch, setInstanceAlwaysPreTouch] = useState(true);
  const [gcPreview, setGcPreview] = useState<Awaited<ReturnType<typeof computeGcArgs>> | null>(null);
  const [gcPreviewLoading, setGcPreviewLoading] = useState(false);
  const [instanceJavaInspected, setInstanceJavaInspected] = useState<JavaRuntimeSummary | null>(null);
  const [instanceJavaInspectError, setInstanceJavaInspectError] = useState<string | null>(null);
  const [instanceJavaAllowOverride, setInstanceJavaAllowOverride] = useState(false);
  const [instanceJavaSaving, setInstanceJavaSaving] = useState(false);
  const [playBusy, setPlayBusy] = useState(false);



  const [canonicalOperation, setCanonicalOperation] = useState<{
    intent: InstallIntent;
    instanceName: string;
  } | null>(null);

  // Pack install state
  const [packInstallOpen, setPackInstallOpen] = useState(false);
  const [packIdInput, setPackIdInput] = useState('');
  const [packProgress, setPackProgress] = useState<
    { modId: string; status: 'pending' | 'installing' | 'done' | 'failed'; error?: string }[] | null
  >(null);
  const [availablePacks, setAvailablePacks] = useState<RegistryItem[]>([]);
  const [packDropdownOpen, setPackDropdownOpen] = useState(false);
  const packDropdownRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!packInstallOpen) return;
    let cancelled = false;
    browseItems('pack').then((packs) => {
      if (!cancelled) setAvailablePacks(packs);
    }).catch(() => {});
    return () => { cancelled = true; };
  }, [packInstallOpen]);

  // Close pack dropdown on outside click
  useEffect(() => {
    if (!packDropdownOpen) return;
    const handler = (e: MouseEvent) => {
      if (packDropdownRef.current && !packDropdownRef.current.contains(e.target as Node)) {
        setPackDropdownOpen(false);
      }
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [packDropdownOpen]);

  useEffect(() => {
    setModDisplayNames({});
    let cancelled = false;
    (async () => {
      try {
        const result = await getInstanceDetail(instanceId);
        if (!cancelled) {
          setDetail(result);
          setInstanceJavaPath(result?.row?.java_path ?? '');
          setInstanceJavaArgs(result?.row?.jvm_custom_args ?? '');
          setInstanceJvmMemory(result?.row?.jvm_memory_mb ?? 4096);
          setInstanceGcMode(storedGcMode(result?.row?.jvm_gc));
          setInstanceAlwaysPreTouch(result?.row?.jvm_always_pre_touch ?? true);
          setInstanceJavaAllowOverride(result?.row?.java_incompatible_override ?? false);
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

  useEffect(() => {
    if (activeTab !== 'java-args' || !detail?.row) return;
    let cancelled = false;
    setGcPreviewLoading(true);
    computeGcArgs(
      instanceJavaInspected?.version ?? previewJavaMajor(detail.row.minecraft_version),
      instanceJvmMemory,
      instanceGcMode === 'manual' ? instanceJavaArgs : '',
      instanceGcMode,
      instanceAlwaysPreTouch,
    ).then((result) => {
      if (!cancelled) setGcPreview(result);
    }).catch(() => {
      if (!cancelled) setGcPreview(null);
    }).finally(() => {
      if (!cancelled) setGcPreviewLoading(false);
    });
    return () => { cancelled = true; };
  }, [activeTab, detail?.row, instanceGcMode, instanceJvmMemory, instanceJavaArgs, instanceAlwaysPreTouch, instanceJavaInspected?.version]);

  const modMetadataKey = installedModMetadataKey(detail?.manifest?.mods);

  useEffect(() => {
    const installedMods = detail?.manifest?.mods;
    if (!installedMods) {
      return;
    }

    let cancelled = false;

    const resolveDisplayName = async (mod: InstalledMod): Promise<string | null> => {
      const identity = installedModDetailId(mod);
      if (!identity) return null;
      if (modDisplayNameCache.current.has(identity)) {
        return modDisplayNameCache.current.get(identity) ?? null;
      }

      const pending = modDisplayNameRequests.current.get(identity);
      if (pending) return pending;

      const request = (async () => {
        try {
          if (mod.registry_id || !mod.modrinth_id) {
            const item = await getRegistryItem(identity);
            return item?.name ?? null;
          }
          const project = await fetchModrinthProject(mod.modrinth_id);
          return project.title || null;
        } catch {
          return null;
        }
      })()
        .then((name) => {
          modDisplayNameCache.current.set(identity, name);
          return name;
        })
        .finally(() => {
          modDisplayNameRequests.current.delete(identity);
        });
      modDisplayNameRequests.current.set(identity, request);
      return request;
    };

    void Promise.all(installedMods.map(async (mod) => {
      const name = await resolveDisplayName(mod);
      if (name) {
        return name ? [installedModKey(mod), name] as const : null;
      }
      return null;
    })).then((results) => {
      if (cancelled) return;
      setModDisplayNames((previous) => ({
        ...previous,
        ...Object.fromEntries(results.filter((result): result is readonly [string, string] => result !== null)),
      }));
    });

    return () => { cancelled = true; };
  }, [modMetadataKey]);

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

  const beginCanonicalOperation = (action: InstallIntent['action']) => {
    setCanonicalOperation({
      instanceName: detail?.row.name ?? instanceId,
      intent: {
        action,
        targetInstance: instanceId,
        optionalDeps: { type: 'prompt' },
        requestedBy: 'interactive',
        overrides: {
          allowReplace: false,
          skipHealthScan: false,
          forceConflictResolution: {},
        },
      },
    });
  };

  const handleRemove = (filename: string) => {
    if (!confirm(`Review a safe removal plan for "${filename}"?`)) return;
    setError(null);
    beginCanonicalOperation({ type: 'remove', filename });
  };

  const handleToggleMod = async (mod: InstalledMod) => {
    setError(null);
    try {
      const enabled = !mod.enabled;
      if (mod.enabled) {
        await disableInstanceMod(instanceId, mod.filename);
      } else {
        await enableInstanceMod(instanceId, mod.filename);
      }
      setDetail((current) => {
        if (!current?.manifest) return current;
        return {
          ...current,
          manifest: updateManifestEntryEnabled(current.manifest, mod.filename, enabled),
        };
      });
    } catch (e) {
      setError(formatError(e));
    }
  };

  const handleInstallPackMods = async () => {
    if (!packIdInput.trim()) return;
    const packId = packIdInput.trim();
    setError(null);

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

    setPackProgress(mods.map((mod) => ({ modId: mod.mod_id, status: 'pending' as const })));
    const items: BatchInstallItem[] = [];
    let resolutionFailed = false;

    for (let index = 0; index < mods.length; index += 1) {
      const mod = mods[index];
      setPackProgress((previous) =>
        previous?.map((progress, current) =>
          current === index ? { ...progress, status: 'installing' as const } : progress
        ) ?? previous
      );
      try {
        const page = await listModVersions(instanceId, mod.mod_id);
        const candidate =
          page.items.find((version) => version.version_compat === 'compatible')
          ?? page.items.find((version) => version.version_compat === 'major_match')
          ?? page.items[0];
        if (!candidate) throw new Error('No compatible verified version is available.');
        items.push({
          sourceType: 'curated',
          itemId: mod.mod_id,
          candidateVersion: candidate.version,
        });
        setPackProgress((previous) =>
          previous?.map((progress, current) =>
            current === index ? { ...progress, status: 'done' as const } : progress
          ) ?? previous
        );
      } catch (e) {
        resolutionFailed = true;
        setPackProgress((previous) =>
          previous?.map((progress, current) =>
            current === index
              ? { ...progress, status: 'failed' as const, error: formatError(e) }
              : progress
          ) ?? previous
        );
      }
    }

    if (resolutionFailed) {
      setError('The pack plan could not be resolved completely. No instance files were changed.');
      return;
    }

    setPackProgress(null);
    setPackInstallOpen(false);
    beginCanonicalOperation({ type: 'batch-install', items });
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

  const handleRename = async () => {
    const newName = window.prompt('Rename instance', row?.name ?? '');
    if (!newName || newName.trim() === '' || newName.trim() === row?.name) return;
    setError(null);
    try {
      await renameInstance(instanceId, newName.trim());
      await refreshDetail();
      setStatus(`Renamed to "${newName.trim()}".`);
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
    beginCanonicalOperation({
      type: 'install',
      sourceType: 'manual',
      itemId: path.split(/[\\/]/).pop() ?? 'manual-mod',
      candidateVersion: path,
    });
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
        beginCanonicalOperation({
          type: 'install',
          sourceType: 'manual',
          itemId: file.name,
          candidateVersion: filePath,
        });
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
      await revealPath(path);
    } catch (e) {
      setError(formatError(e));
    } finally {
      setExportBusy(false);
    }
  };

  const requireLockfileText = () => {
    if (lockfileText.trim()) return lockfileText;
    setError('Export this instance or paste a lockfile before continuing.');
    return null;
  };

  const handleExportLockfile = async () => {
    setLockfileBusy('export');
    setError(null);
    setLockfileNotice(null);
    setLockfileReport(null);
    try {
      const lockfile = await exportLockfile(instanceId);
      setLockfileText(JSON.stringify(lockfile, null, 2));
      const artifacts = Array.isArray(lockfile.artifacts) ? lockfile.artifacts : [];
      const unresolved = artifacts.filter((artifact) => {
        if (!artifact || typeof artifact !== 'object') return false;
        return Boolean((artifact as Record<string, unknown>).unresolvedReason);
      }).length;
      setLockfileNotice(
        unresolved === 0
          ? 'Canonical lockfile exported. It contains hashes and settings, never private config contents.'
          : `Lockfile exported with ${unresolved} unreproducible artifact${unresolved === 1 ? '' : 's'} clearly marked. Verification still works, but clone and repair will refuse substitution.`,
      );
    } catch (cause) {
      setError(formatError(cause));
    } finally {
      setLockfileBusy(null);
    }
  };

  const handleCopyLockfile = async () => {
    const text = requireLockfileText();
    if (!text) return;
    setLockfileBusy('copy');
    setError(null);
    try {
      await navigator.clipboard.writeText(text);
      setLockfileNotice('Lockfile copied to the clipboard.');
    } catch (cause) {
      setError(`Could not copy the lockfile: ${formatError(cause)}`);
    } finally {
      setLockfileBusy(null);
    }
  };

  const handleVerifyLockfile = async () => {
    const text = requireLockfileText();
    if (!text) return;
    setLockfileBusy('verify');
    setError(null);
    setLockfileNotice(null);
    try {
      const report = await verifyLockfile(instanceId, text);
      setLockfileReport(report);
      setLockfileNotice(
        report.status === 'in-sync'
          ? 'This instance exactly matches the lockfile artifacts and tracked config hash.'
          : `${report.differences.length} difference${report.differences.length === 1 ? '' : 's'} found. Review them before repairing.`,
      );
    } catch (cause) {
      setLockfileReport(null);
      setError(formatError(cause));
    } finally {
      setLockfileBusy(null);
    }
  };

  const handleRepairLockfile = async () => {
    const text = requireLockfileText();
    if (!text) return;
    if (!window.confirm(
      'Repair this instance to the pasted lockfile? Agora will create one recovery snapshot, download exact hashes, and remove managed artifacts that are not in the lockfile. Private config contents cannot be repaired because lockfiles never contain them.',
    )) return;

    setLockfileBusy('repair');
    setError(null);
    setLockfileNotice(null);
    try {
      const outcome = await repairLockfile(instanceId, text);
      if (outcome.type === 'success') {
        await refreshDetail();
        const report = await verifyLockfile(instanceId, text);
        setLockfileReport(report);
        setLockfileNotice(
          report.status === 'in-sync'
            ? 'Repair completed and the instance now matches the lockfile.'
            : 'Artifact repair completed. Remaining differences cannot be reproduced from this privacy-preserving lockfile (usually private config changes).',
        );
      } else if (outcome.type === 'health-rollback') {
        setLockfileReport(null);
        setError('Repair introduced a health blocker, so Agora restored the recovery snapshot.');
      } else if (outcome.type === 'cancelled') {
        setLockfileReport(null);
        setLockfileNotice(
          outcome.rollbackPerformed
            ? 'Repair was cancelled and the recovery snapshot was restored.'
            : 'Repair was cancelled before the instance changed.',
        );
      } else {
        setLockfileReport(null);
        setError(
          outcome.rollbackPerformed
            ? `${outcome.error} The recovery snapshot was restored.`
            : outcome.error,
        );
      }
    } catch (cause) {
      setLockfileReport(null);
      setError(formatError(cause));
    } finally {
      setLockfileBusy(null);
    }
  };

  const handleCloneLockfile = async () => {
    const text = requireLockfileText();
    if (!text) return;
    setLockfileBusy('clone');
    setError(null);
    setLockfileNotice(null);
    try {
      const newInstanceId = await importLockfile(text);
      if (onOpenInstanceEditor) {
        onOpenInstanceEditor(newInstanceId);
      } else {
        setLockfileNotice(`Reproduced the lockfile as new instance "${newInstanceId}".`);
      }
    } catch (cause) {
      setError(formatError(cause));
    } finally {
      setLockfileBusy(null);
    }
  };

  const row = detail?.row;
  const manifest = detail?.manifest;
  const mods = manifest?.mods ?? [];

  const handleOpenInstalledMod = (mod: InstalledMod) => {
    const itemId = mod.registry_id || mod.modrinth_id || mod.mod_jar_id;
    if (itemId) onOpenModDetail?.(itemId);
  };

  if (loading) {
    return (
      <div className="space-y-6">
        <BackButton onBack={onBack} />
        <div className="rounded-xl border border-dashed border-border bg-card p-6 text-center text-muted-foreground">
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
        <div className="flex flex-col gap-4 xl:flex-row xl:items-start xl:justify-between">
          <div>
            <div className="flex flex-wrap items-center gap-3">
              <h2 className="text-2xl font-bold">
                {row?.name}
                {' '}
                <button
                  onClick={handleRename}
                  className="text-xs text-muted-foreground hover:text-foreground underline"
                >
                  Rename
                </button>
              </h2>
              <button
                type="button"
                onClick={async () => {
                  if (!onLaunch || playBusy) return;
                  setPlayBusy(true);
                  setError(null);
                  try {
                    await onLaunch(instanceId);
                  } catch (cause) {
                    setError(formatError(cause));
                  } finally {
                    setPlayBusy(false);
                  }
                }}
                disabled={!onLaunch || playBusy}
                className="inline-flex items-center gap-2 rounded-lg bg-primary px-5 py-3 text-base font-semibold text-primary-foreground shadow-sm hover:bg-primary/90 disabled:cursor-not-allowed disabled:opacity-50"
                aria-label={`Play ${row?.name ?? 'instance'}`}
              >
                <Play className="h-5 w-5 fill-current" aria-hidden="true" />
                {playBusy ? 'Starting…' : 'Play'}
              </button>
            </div>
            <p className="text-xs text-muted-foreground mt-1">
              MC {row?.minecraft_version} · {manifest?.loader} {manifest?.loader_version}
            </p>
            <div className="mt-2 flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
              <span>{row?.is_locked ? '🔒 Locked' : '🔓 Unlocked'}</span>
              {row?.is_locked ? (
                <button
                  onClick={handleUnlock}
                  className="rounded-lg border border-input bg-background px-2.5 py-1 text-xs font-medium hover:bg-accent"
                >
                  Unlock
                </button>
              ) : (
                <>
                  <button
                    onClick={handleLock}
                    className="rounded-lg border border-input bg-background px-2.5 py-1 text-xs font-medium hover:bg-accent"
                  >
                    Lock
                  </button>
                  <button
                    onClick={handleRevert}
                    className="rounded-lg border border-input bg-background px-2.5 py-1 text-xs font-medium hover:bg-accent"
                  >
                    Revert
                  </button>
                </>
              )}
              {row?.last_launched_at && (
                <span className="ml-2">· Last launched {row.last_launched_at}</span>
              )}
            </div>
          </div>
          <div className="flex flex-wrap gap-2">
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
              onClick={handleImportPack}
              className="rounded-lg border border-input bg-background hover:bg-accent px-3 py-1.5 text-sm font-medium"
            >
              📥 Import Pack
            </button>
            <button
              onClick={() => openInstanceFolder(instanceId)}
              className="rounded-lg border border-input bg-background hover:bg-accent px-3 py-1.5 text-sm font-medium"
              title="Open instance folder in file explorer"
            >
              📂 Open in Folder
            </button>
          </div>
        </div>

        {error && (
          <div className="mt-4 rounded-lg bg-destructive p-3 text-sm text-destructive-foreground">
            {error}
          </div>
        )}
        {status && (
          <div className="mt-4 rounded-lg bg-accent text-accent-foreground p-3 text-sm">
            {status}
          </div>
        )}
      </section>

      {/* Sub-sidebar tabs */}
      <div className="flex border-b border-border gap-0">
        {(['mods', 'resourcepacks', 'shaders', 'datapacks', 'snapshots', 'loadout-profiles', 'import', 'export', 'console'] as const).map((tab) => (
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
            {tab === 'mods' ? `Mods (${mods.length})` : tab === 'resourcepacks' ? `Resource Packs (${manifest?.resourcepacks?.length ?? 0})` : tab === 'shaders' ? `Shaders (${manifest?.shaders?.length ?? 0})` : tab === 'datapacks' ? `Data Packs (${manifest?.datapacks?.length ?? 0})` : tab === 'snapshots' ? 'Snapshots' : tab === 'loadout-profiles' ? 'Loadout Profiles' : tab === 'import' ? 'Import' : tab === 'export' ? 'Export' : 'Console'}
          </button>
        ))}
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
            <div className="flex gap-2 relative" ref={packDropdownRef}>
              <div className="flex-1 relative">
                <input
                  type="text"
                  value={packIdInput}
                  onChange={(e) => {
                    setPackIdInput(e.target.value);
                    setPackDropdownOpen(true);
                  }}
                  onFocus={() => setPackDropdownOpen(true)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') handleInstallPackMods();
                    if (e.key === 'Escape') setPackDropdownOpen(false);
                  }}
                  placeholder="Search packs…"
                  className="w-full rounded-lg border border-input bg-background px-3 py-2 text-sm"
                />
                {packDropdownOpen && availablePacks.length > 0 && (
                  <div className="absolute z-50 mt-1 w-full rounded-lg border border-border bg-card shadow-lg max-h-48 overflow-y-auto">
                    {availablePacks
                      .filter((p) =>
                        !packIdInput ||
                        p.id.toLowerCase().includes(packIdInput.toLowerCase()) ||
                        p.name.toLowerCase().includes(packIdInput.toLowerCase())
                      )
                      .slice(0, 50)
                      .map((p) => (
                        <button
                          key={p.id}
                          onClick={() => {
                            setPackIdInput(p.id);
                            setPackDropdownOpen(false);
                          }}
                          className="w-full text-left px-3 py-2 text-sm hover:bg-accent border-b border-border last:border-b-0"
                        >
                          <span className="font-medium">{p.name}</span>
                          <span className="text-muted-foreground ml-2 text-xs">({p.id})</span>
                        </button>
                      ))}
                  </div>
                )}
              </div>
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
              disabled={!!row?.is_locked}
              className="rounded-lg border border-input bg-background hover:bg-accent px-3 py-1.5 text-sm font-medium disabled:opacity-50 disabled:cursor-not-allowed"
              title={row?.is_locked ? 'Unlock the instance to add mods.' : undefined}
            >
              📥 Import Mod
            </button>
          </div>
        </div>
        {row?.is_locked ? (
          <p className="text-xs text-muted-foreground mb-3">
            Instance is locked. Unlock it to add or remove mods.
          </p>
        ) : (
          <p className="text-xs text-muted-foreground mb-3">
            Drag and drop a .jar mod, or a .mrpack / .agora-pack.json pack file, here to install it.
          </p>
        )}
        {mods.length === 0 ? (
          <p className="text-sm text-muted-foreground">No mods installed.</p>
        ) : (
          <div className="space-y-2">
            {mods.map((mod) => (
              <div
                key={mod.filename}
                className={`group flex items-center justify-between rounded-lg border border-border px-3 py-2 text-sm transition-colors ${
                  installedModDetailId(mod)
                    ? 'cursor-pointer hover:border-primary/50 hover:bg-accent/60'
                    : ''
                } ${!mod.enabled ? 'opacity-50' : ''}`}
              >
                <button
                  type="button"
                  onClick={() => handleOpenInstalledMod(mod)}
                  disabled={!installedModDetailId(mod)}
                  className="min-w-0 flex-1 text-left disabled:cursor-default enabled:cursor-pointer"
                  title={installedModDetailId(mod) ? 'View mod details' : 'Mod details unavailable'}
                >
                  <span className={`font-medium truncate block group-hover:text-primary ${!mod.enabled ? 'line-through' : ''}`}>
                    {modDisplayNames[installedModKey(mod)] ?? mod.filename}
                  </span>
                  <span className="text-xs text-muted-foreground flex flex-wrap items-center gap-x-2 gap-y-0.5 mt-0.5">
                    <span className="truncate">{mod.filename}</span>
                    <span className="rounded-full bg-primary/10 text-primary px-1.5 py-0.5 text-[10px]">{installedModSourceLabel(mod.source)}</span>
                    <span>Installed {formatInstalledAt(mod.installed_at)}</span>
                    {!mod.enabled && <span className="text-yellow-600 dark:text-yellow-400 font-medium">disabled</span>}
                    {installedModDetailId(mod) && (
                      <span className="text-primary font-medium">View details</span>
                    )}
                  </span>
                </button>
                {!row?.is_locked && (
                  <button
                    onClick={() => handleToggleMod(mod)}
                    className="ml-2 text-xs text-foreground hover:text-primary whitespace-nowrap"
                    title={mod.enabled ? 'Disable mod' : 'Enable mod'}
                  >
                    {mod.enabled ? '🔌 Disable' : '🔌 Enable'}
                  </button>
                )}
                <button
                  onClick={() => handleRemove(mod.filename)}
                  disabled={!!row?.is_locked}
                  className="ml-2 text-xs text-destructive hover:underline disabled:opacity-50 whitespace-nowrap"
                  title={row?.is_locked ? 'Unlock the instance to remove mods.' : undefined}
                >
                  {row?.is_locked ? '🔒' : 'Remove'}
                </button>
              </div>
            ))}
          </div>
        )}
      </section>

      <button
        onClick={() => onOpenBrowseForInstance?.(instanceId)}
        disabled={!!row?.is_locked}
        className="rounded-lg border border-dashed border-border px-4 py-2 text-sm font-medium text-muted-foreground hover:bg-accent disabled:opacity-50 disabled:cursor-not-allowed w-full"
        title={row?.is_locked ? 'Unlock the instance to add mods.' : undefined}
      >
        {row?.is_locked ? '🔒 Instance Locked' : '+ Add Mod'}
      </button>

        </>
      )}

      {activeTab === 'resourcepacks' && (
        <section className="rounded-xl border border-border bg-card p-4">
          <h3 className="font-semibold text-sm mb-3">Resource Packs ({(manifest?.resourcepacks ?? []).length})</h3>
          {(manifest?.resourcepacks ?? []).length === 0 ? (
            <p className="text-sm text-muted-foreground">No resource packs installed.</p>
          ) : (
            <div className="space-y-2">
              {(manifest?.resourcepacks ?? []).map((rp) => (
                <div key={rp.filename} className={`flex items-center justify-between rounded-lg border border-border px-3 py-2 text-sm ${!rp.enabled ? 'opacity-50' : ''}`}>
                  <div className="min-w-0 flex-1">
                    <span className={`font-medium truncate block ${!rp.enabled ? 'line-through' : ''}`}>{rp.filename}</span>
                    <div className="text-xs text-muted-foreground mt-0.5">
                      {rp.version && <span>v{rp.version}</span>}
                      {!rp.enabled && <span className="ml-2 text-yellow-600 dark:text-yellow-400 font-medium">disabled</span>}
                    </div>
                  </div>
                  {!row?.is_locked && (
                    <button
                      onClick={() => handleToggleMod(rp)}
                      className="ml-2 text-xs text-foreground hover:text-primary whitespace-nowrap"
                    >
                      {rp.enabled ? '🔌 Disable' : '🔌 Enable'}
                    </button>
                  )}
                  <button
                    onClick={() => handleRemove(rp.filename)}
                    disabled={!!row?.is_locked}
                    className="ml-2 text-xs text-destructive hover:underline disabled:opacity-50 whitespace-nowrap"
                    title={row?.is_locked ? 'Unlock the instance to remove content.' : undefined}
                  >
                    {row?.is_locked ? '🔒' : 'Remove'}
                  </button>
                </div>
              ))}
            </div>
          )}
          <button
            onClick={() => onOpenBrowseForInstance?.(instanceId, 'resourcepack')}
            disabled={!!row?.is_locked}
            className="mt-3 rounded-lg border border-dashed border-border px-4 py-2 text-sm font-medium text-muted-foreground hover:bg-accent disabled:opacity-50 disabled:cursor-not-allowed w-full"
            title={row?.is_locked ? 'Unlock the instance to add resource packs.' : undefined}
          >
            {row?.is_locked ? '🔒 Instance Locked' : '+ Add Resource Pack'}
          </button>
        </section>
      )}

      {activeTab === 'shaders' && (
        <section className="rounded-xl border border-border bg-card p-4">
          <h3 className="font-semibold text-sm mb-3">Shaders ({(manifest?.shaders ?? []).length})</h3>
          {(manifest?.shaders ?? []).length === 0 ? (
            <p className="text-sm text-muted-foreground">No shaders installed.</p>
          ) : (
            <div className="space-y-2">
              {(manifest?.shaders ?? []).map((s) => (
                <div key={s.filename} className={`flex items-center justify-between rounded-lg border border-border px-3 py-2 text-sm ${!s.enabled ? 'opacity-50' : ''}`}>
                  <div className="min-w-0 flex-1">
                    <span className={`font-medium truncate block ${!s.enabled ? 'line-through' : ''}`}>{s.filename}</span>
                    <div className="text-xs text-muted-foreground mt-0.5">
                      {s.version && <span>v{s.version}</span>}
                      {!s.enabled && <span className="ml-2 text-yellow-600 dark:text-yellow-400 font-medium">disabled</span>}
                    </div>
                  </div>
                  {!row?.is_locked && (
                    <button
                      onClick={() => handleToggleMod(s)}
                      className="ml-2 text-xs text-foreground hover:text-primary whitespace-nowrap"
                    >
                      {s.enabled ? '🔌 Disable' : '🔌 Enable'}
                    </button>
                  )}
                  <button
                    onClick={() => handleRemove(s.filename)}
                    disabled={!!row?.is_locked}
                    className="ml-2 text-xs text-destructive hover:underline disabled:opacity-50 whitespace-nowrap"
                    title={row?.is_locked ? 'Unlock the instance to remove content.' : undefined}
                  >
                    {row?.is_locked ? '🔒' : 'Remove'}
                  </button>
                </div>
              ))}
            </div>
          )}
          <button
            onClick={() => onOpenBrowseForInstance?.(instanceId, 'shader')}
            disabled={!!row?.is_locked}
            className="mt-3 rounded-lg border border-dashed border-border px-4 py-2 text-sm font-medium text-muted-foreground hover:bg-accent disabled:opacity-50 disabled:cursor-not-allowed w-full"
            title={row?.is_locked ? 'Unlock the instance to add shaders.' : undefined}
          >
            {row?.is_locked ? '🔒 Instance Locked' : '+ Add Shader'}
          </button>
        </section>
      )}

      {activeTab === 'datapacks' && (
        <section className="rounded-xl border border-border bg-card p-4">
          <h3 className="font-semibold text-sm mb-3">Data Packs ({(manifest?.datapacks ?? []).length})</h3>
          {(manifest?.datapacks ?? []).length === 0 ? (
            <p className="text-sm text-muted-foreground">No data packs installed.</p>
          ) : (
            <div className="space-y-2">
              {(manifest?.datapacks ?? []).map((dp) => (
                <div key={dp.filename} className={`flex items-center justify-between rounded-lg border border-border px-3 py-2 text-sm ${!dp.enabled ? 'opacity-50' : ''}`}>
                  <div className="min-w-0 flex-1">
                    <span className={`font-medium truncate block ${!dp.enabled ? 'line-through' : ''}`}>{dp.filename}</span>
                    <div className="text-xs text-muted-foreground mt-0.5">
                      {dp.version && <span>v{dp.version}</span>}
                      {!dp.enabled && <span className="ml-2 text-yellow-600 dark:text-yellow-400 font-medium">disabled</span>}
                    </div>
                  </div>
                  {!row?.is_locked && (
                    <button
                      onClick={() => handleToggleMod(dp)}
                      className="ml-2 text-xs text-foreground hover:text-primary whitespace-nowrap"
                    >
                      {dp.enabled ? '🔌 Disable' : '🔌 Enable'}
                    </button>
                  )}
                  <button
                    onClick={() => handleRemove(dp.filename)}
                    disabled={!!row?.is_locked}
                    className="ml-2 text-xs text-destructive hover:underline disabled:opacity-50 whitespace-nowrap"
                    title={row?.is_locked ? 'Unlock the instance to remove content.' : undefined}
                  >
                    {row?.is_locked ? '🔒' : 'Remove'}
                  </button>
                </div>
              ))}
            </div>
          )}
          <button
            onClick={() => onOpenBrowseForInstance?.(instanceId, 'datapack')}
            disabled={!!row?.is_locked}
            className="mt-3 rounded-lg border border-dashed border-border px-4 py-2 text-sm font-medium text-muted-foreground hover:bg-accent disabled:opacity-50 disabled:cursor-not-allowed w-full"
            title={row?.is_locked ? 'Unlock the instance to add data packs.' : undefined}
          >
            {row?.is_locked ? '🔒 Instance Locked' : '+ Add Data Pack'}
          </button>
        </section>
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
                    const label = snapshotLabelInput.trim() || `Snapshot ${new Date().toLocaleString()}`;
                    await createSnapshot(instanceId, label);
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
                    <span className="font-medium flex items-center gap-2">
                      <span>{snap.label}</span>
                      {snap.is_current_lkg && (
                        <span className="rounded-full bg-green-500/10 px-2 py-0.5 text-[10px] text-green-700 dark:text-green-300">Current LKG</span>
                      )}
                      {!snap.is_current_lkg && snap.is_lkg && (
                        <span className="rounded-full bg-primary/10 px-2 py-0.5 text-[10px] text-primary">Known good</span>
                      )}
                      {snap.is_pre_restore && (
                        <span className="rounded-full bg-amber-500/10 px-2 py-0.5 text-[10px] text-amber-700 dark:text-amber-300">Undo restore</span>
                      )}
                    </span>
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
                          const diff = await detectDrift(instanceId, snap.id);
                          setSnapshotDiff({ snapshotId: snap.id, diff });
                        } catch (e) {
                          setError(formatError(e));
                        } finally {
                          setSnapshotBusy(null);
                        }
                      }}
                      disabled={snapshotBusy === snap.id}
                      className="text-xs text-primary hover:underline disabled:opacity-50"
                    >
                      Show diff
                    </button>
                    <button
                      onClick={async () => {
                        setSnapshotBusy(snap.id);
                        setError(null);
                        try {
                          await restoreSnapshot(instanceId, snap.id);
                          const result = await listSnapshots(instanceId);
                          setSnapshots(result);
                          setDetail(await getInstanceDetail(instanceId));
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
              {snapshotDiff && (
                <div className="rounded-lg border border-border bg-muted/30 p-3 text-xs space-y-2">
                  <div className="flex items-center justify-between gap-3">
                    <p className="font-semibold">Changes since snapshot</p>
                    <button onClick={() => setSnapshotDiff(null)} className="text-muted-foreground hover:text-foreground">Close</button>
                  </div>
                  <p className="text-muted-foreground">
                    +{snapshotDiff.diff.added.length} added · -{snapshotDiff.diff.removed.length} removed · ~{snapshotDiff.diff.modified.length} modified · {snapshotDiff.diff.unchangedCount} unchanged
                  </p>
                  {[
                    ...snapshotDiff.diff.added.map((entry) => ({ ...entry, marker: '+', label: 'Added' })),
                    ...snapshotDiff.diff.removed.map((entry) => ({ ...entry, marker: '-', label: 'Removed' })),
                    ...snapshotDiff.diff.modified.map((entry) => ({ ...entry, marker: '~', label: 'Modified' })),
                  ].length > 0 ? (
                    <ul className="max-h-48 overflow-y-auto space-y-1 font-mono">
                      {[
                        ...snapshotDiff.diff.added.map((entry) => ({ ...entry, marker: '+', label: 'Added' })),
                        ...snapshotDiff.diff.removed.map((entry) => ({ ...entry, marker: '-', label: 'Removed' })),
                        ...snapshotDiff.diff.modified.map((entry) => ({ ...entry, marker: '~', label: 'Modified' })),
                      ].map((entry) => (
                        <li key={`${entry.marker}:${entry.path}`} className="break-all">
                          <span className="font-semibold">{entry.marker}</span> {entry.path} <span className="font-sans text-muted-foreground">({entry.label})</span>
                        </li>
                      ))}
                    </ul>
                  ) : (
                    <p className="text-green-700 dark:text-green-300">The current instance matches this snapshot exactly.</p>
                  )}
                </div>
              )}
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
                  setError(null);
                  try {
                    const name = profileNameInput.trim() || `Current Setup ${new Date().toLocaleString()}`;
                    await createLoadoutProfile(instanceId, name);
                    const result = await listLoadoutProfiles(instanceId);
                    setProfiles(result);
                    setProfileNameInput('');
                    setStatus(`Profile "${name}" created.`);
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

          {profiles.length === 0 ? (
            <p className="text-sm text-muted-foreground">
              No loadout profiles yet. Enter a name and click Create Profile.
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
                          setDetail(await getInstanceDetail(instanceId));
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

      {activeTab === 'export' && (
        <section className="space-y-4">
          <div>
            <h3 className="font-semibold text-sm">Export Instance</h3>
            <p className="text-xs text-muted-foreground mt-1">
              Choose how to share or back up this instance. Each format serves a different
              purpose — pick the one that matches your goal.
            </p>
          </div>

          {/* Card 1: mrpack (Recommended) */}
          <div className="rounded-xl border border-border bg-card p-4 space-y-3">
            <div className="flex items-start justify-between gap-2">
              <h4 className="font-semibold text-sm">Export as Modrinth Pack (.mrpack)</h4>
              <span className="rounded-full bg-primary/10 text-primary text-[10px] font-semibold px-2 py-0.5 shrink-0">
                Recommended
              </span>
            </div>
            <p className="text-xs text-muted-foreground">
              The industry-standard format for sharing Minecraft modpacks. Compatible with
              Modrinth, Prism Launcher, and other launchers. Contains mod references, config
              files, and overrides — not the mod files themselves.
            </p>
            <p className="text-xs text-muted-foreground">
              <span className="font-medium text-foreground">Best for:</span> sharing your
              modpack with other launchers or publishing to Modrinth.
            </p>
            <button
              onClick={() => handleExportPack('mrpack')}
              disabled={exportBusy}
              className="rounded-lg bg-primary px-3 py-1.5 text-sm font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
            >
              {exportBusy ? 'Exporting…' : 'Export .mrpack'}
            </button>
          </div>

          {/* Card 2: Agora JSON pack */}
          <div className="rounded-xl border border-border bg-card p-4 space-y-3">
            <div className="flex items-start justify-between gap-2">
              <h4 className="font-semibold text-sm">Export as Agora Pack (.json)</h4>
              <span className="rounded-full bg-muted text-muted-foreground text-[10px] font-semibold px-2 py-0.5 shrink-0">
                Agora native
              </span>
            </div>
            <p className="text-xs text-muted-foreground">
              Agora's native pack format. Contains the full mod list with exact versions and
              source references. Reimport into any Agora instance to recreate this loadout.
            </p>
            <p className="text-xs text-muted-foreground">
              <span className="font-medium text-foreground">Best for:</span> backing up your
              mod selection or sharing with other Agora users.
            </p>
            <button
              onClick={() => handleExportPack('json')}
              disabled={exportBusy}
              className="rounded-lg border border-input bg-background hover:bg-accent px-3 py-1.5 text-sm font-medium disabled:opacity-50"
            >
              {exportBusy ? 'Exporting…' : 'Export agora-pack.json'}
            </button>
          </div>

          {/* Card 3: Lockfile (Advanced) */}
          <div className="rounded-xl border border-border bg-card p-4 space-y-3">
            <div className="flex items-start justify-between gap-2">
              <h4 className="font-semibold text-sm">Export Reproduction Lockfile</h4>
              <span className="rounded-full bg-muted text-muted-foreground text-[10px] font-semibold px-2 py-0.5 shrink-0">
                Advanced
              </span>
            </div>
            <p className="text-xs text-muted-foreground">
              A privacy-preserving lockfile recording SHA-256 hashes, exact download sources,
              mod versions, and all settings. Any installation with the same lockfile reproduces
              identical artifacts. Private config contents are never included.
            </p>
            <p className="text-xs text-muted-foreground">
              <span className="font-medium text-foreground">Best for:</span> forensic
              reproduction, drift detection, and bit-identical cloning.
            </p>

            <div className="flex flex-wrap gap-2">
              <button
                onClick={() => void handleExportLockfile()}
                disabled={lockfileBusy !== null}
                className="rounded-lg border border-input bg-background hover:bg-accent px-3 py-1.5 text-sm font-medium disabled:opacity-50"
              >
                {lockfileBusy === 'export' ? 'Exporting…' : 'Export Lockfile'}
              </button>
              {lockfileText.trim() && (
                <>
                  <button
                    onClick={() => void handleCopyLockfile()}
                    disabled={lockfileBusy !== null}
                    className="rounded-lg border border-input bg-background hover:bg-accent px-3 py-1.5 text-sm font-medium disabled:opacity-50"
                  >
                    {lockfileBusy === 'copy' ? 'Copying…' : 'Copy'}
                  </button>
                  <button
                    onClick={() => {
                      setLockfileText('');
                      setLockfileReport(null);
                      setLockfileNotice(null);
                      setError(null);
                    }}
                    disabled={lockfileBusy !== null}
                    className="rounded-lg border border-input bg-background hover:bg-accent px-3 py-1.5 text-sm font-medium disabled:opacity-50"
                  >
                    Clear
                  </button>
                </>
              )}
            </div>

            <textarea
              value={lockfileText}
              onChange={(event) => {
                setLockfileText(event.target.value);
                setLockfileReport(null);
                setLockfileNotice(null);
                setError(null);
              }}
              rows={12}
              aria-label="Instance lockfile JSON"
              placeholder="Export this instance or paste an Agora lockfile JSON here…"
              className="w-full rounded-lg border border-input bg-background p-3 text-xs font-mono resize-y"
            />

            {lockfileText.trim() ? (
              <div className="flex flex-wrap gap-2">
                <button
                  onClick={() => void handleVerifyLockfile()}
                  disabled={lockfileBusy !== null}
                  className="rounded-lg border border-input bg-background hover:bg-accent px-3 py-1.5 text-sm font-medium disabled:opacity-50"
                >
                  {lockfileBusy === 'verify' ? 'Verifying…' : 'Verify'}
                </button>
                <button
                  onClick={() => void handleRepairLockfile()}
                  disabled={lockfileBusy !== null || Boolean(row?.is_locked)}
                  title={row?.is_locked ? 'Unlock this instance before repairing drift.' : undefined}
                  className="rounded-lg border border-input bg-background hover:bg-accent px-3 py-1.5 text-sm font-medium disabled:opacity-50"
                >
                  {lockfileBusy === 'repair' ? 'Repairing…' : 'Repair'}
                </button>
                <button
                  onClick={() => void handleCloneLockfile()}
                  disabled={lockfileBusy !== null}
                  className="rounded-lg border border-input bg-background hover:bg-accent px-3 py-1.5 text-sm font-medium disabled:opacity-50"
                >
                  {lockfileBusy === 'clone' ? 'Cloning…' : 'Clone'}
                </button>
              </div>
            ) : (
              <div className="rounded-lg border border-dashed border-border bg-muted p-4 text-center text-xs text-muted-foreground">
                Export this instance or paste a received lockfile to verify, repair, or clone it.
              </div>
            )}

            {lockfileNotice && (
              <div className="rounded-lg bg-accent/20 p-3 text-xs text-muted-foreground">{lockfileNotice}</div>
            )}

            {lockfileReport && (
              <div className="rounded-lg border border-border bg-background p-3 space-y-1 text-xs">
                <p className="font-medium">
                  {lockfileReport.status === 'in-sync' ? 'In sync' : 'Drift detected'}
                </p>
                {lockfileReport.differences?.map((diff, idx) => (
                  <p key={idx} className="text-muted-foreground">
                    {diff.path}: {diff.kind} {diff.expectedSha256 && `(expected ${diff.expectedSha256})`} {diff.actualSha256 && `(got ${diff.actualSha256})`}
                  </p>
                ))}
              </div>
            )}
          </div>
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
        <section className="rounded-xl border border-border bg-card p-4 space-y-4">
          <h3 className="font-semibold text-sm">Java & Args</h3>
          <p className="text-xs text-muted-foreground">
            Configure per-instance Java runtime path. By default Agora auto-selects the exact
            major version required by the instance's Minecraft version. Override only when you
            need a specific Java distribution for this instance.
          </p>

          {/* Per-instance Java path */}
          <div className="space-y-2">
            <label className="text-sm font-medium">Java executable path</label>
            <p className="text-xs text-muted-foreground">
              Leave empty to use the global default (from Settings) or auto-detection.
            </p>
            <div className="flex gap-2">
              <input
                value={instanceJavaPath}
                onChange={(e) => {
                  setInstanceJavaPath(e.target.value);
                  setInstanceJavaInspected(null);
                  setInstanceJavaInspectError(null);
                }}
                placeholder="Auto (global default)"
                className="flex-1 rounded-lg border border-input bg-background px-3 py-2 text-sm"
              />
              <button
                onClick={async () => {
                  setInstanceJavaInspectError(null);
                  setInstanceJavaInspected(null);
                  try {
                    const chosen = await pickOpenFile('Select Java executable', ['exe', 'java']);
                    if (chosen) {
                      setInstanceJavaPath(chosen);
                      const info = await inspectJavaExecutable(chosen);
                      setInstanceJavaInspected(info);
                    }
                  } catch (e) {
                    setInstanceJavaInspectError(formatError(e));
                  }
                }}
                className="rounded-lg border border-input px-3 py-2 text-sm font-medium hover:bg-accent"
              >
                Browse…
              </button>
            </div>

            {/* Inspect result */}
            {instanceJavaInspected && (
              <div className="rounded-lg bg-muted px-3 py-2 space-y-1">
                <p className="text-xs text-green-600 dark:text-green-400">Java {instanceJavaInspected.version} detected</p>
                <p className="text-xs text-muted-foreground">
                  {instanceJavaInspected.version_string} · {instanceJavaInspected.arch ?? 'unknown arch'}
                </p>
                <p className="text-xs text-muted-foreground">
                  Source: <span className="font-medium">{instanceJavaInspected.source}</span>
                </p>
              </div>
            )}
            {instanceJavaInspectError && (
              <p className="text-xs text-destructive">{instanceJavaInspectError}</p>
            )}

            <div className="rounded-lg border border-border bg-background p-4 space-y-4">
              <div>
                <h4 className="text-sm font-semibold">Automatic JVM tuning</h4>
                <p className="mt-1 text-xs text-muted-foreground">
                  Agora keeps heap size, Java compatibility, and garbage collection flags together so you do not need to hand-build JVM arguments.
                </p>
              </div>

              <label className="block space-y-2">
                <span className="flex items-center justify-between text-sm font-medium">
                  <span>Memory allocation</span>
                  <span className="tabular-nums text-primary">
                    {instanceJvmMemory >= 1024 ? `${(instanceJvmMemory / 1024).toFixed(instanceJvmMemory % 1024 === 0 ? 0 : 1)} GB` : `${instanceJvmMemory} MB`}
                  </span>
                </span>
                <input
                  aria-label="Memory allocation"
                  type="range"
                  min={2048}
                  max={32768}
                  step={512}
                  value={instanceJvmMemory}
                  onChange={(e) => setInstanceJvmMemory(Number(e.target.value))}
                  className="w-full accent-primary"
                />
                <span className="flex justify-between text-[11px] text-muted-foreground">
                  <span>2 GB</span>
                  <span>32 GB maximum</span>
                </span>
              </label>

              <label className="flex items-center justify-between gap-3 rounded-lg border border-border px-3 py-2">
                <span>
                  <span className="block text-sm font-medium">Choose the best GC automatically</span>
                  <span className="block text-xs text-muted-foreground">Uses Generational ZGC on Java 21+ and tuned G1GC on older Java.</span>
                </span>
                <input
                  aria-label="Automatic GC selection"
                  type="checkbox"
                  checked={instanceGcMode === 'auto'}
                  onChange={(e) => setInstanceGcMode(e.target.checked ? 'auto' : 'high_efficiency')}
                  className="h-5 w-5 shrink-0 accent-primary"
                />
              </label>

              {instanceGcMode !== 'auto' && (
                <label className="block space-y-2">
                  <span className="text-sm font-medium">Garbage collector</span>
                  <select
                    aria-label="Garbage collector"
                    value={instanceGcMode}
                    onChange={(e) => setInstanceGcMode(e.target.value as GcMode)}
                    className="w-full rounded-lg border border-input bg-background px-3 py-2 text-sm"
                  >
                    <option value="high_efficiency">G1GC · high efficiency</option>
                    <option value="low_latency">ZGC · low latency (Java 15+)</option>
                    {advancedMode && <option value="manual">Manual flags</option>}
                  </select>
                </label>
              )}

              <label className="flex items-center justify-between gap-3 rounded-lg border border-border px-3 py-2">
                <span>
                  <span className="block text-sm font-medium">Pre-touch allocated memory</span>
                  <span className="block text-xs text-muted-foreground">Reduces in-game stutter at the cost of a longer startup. Recommended for G1GC.</span>
                </span>
                <input
                  aria-label="Pre-touch allocated memory"
                  type="checkbox"
                  checked={instanceAlwaysPreTouch}
                  onChange={(e) => setInstanceAlwaysPreTouch(e.target.checked)}
                  className="h-5 w-5 shrink-0 accent-primary"
                />
              </label>

              <div className="rounded-lg bg-muted px-3 py-2 text-xs">
                <div className="flex items-center justify-between gap-2">
                  <span className="font-medium">Launch preview</span>
                  {gcPreviewLoading && <span className="text-muted-foreground">Updating…</span>}
                </div>
                <p className="mt-1 text-muted-foreground">
                  Selected: {instanceGcMode === 'auto' ? 'Auto' : instanceGcMode === 'manual' ? 'Manual' : instanceGcMode === 'low_latency' ? 'ZGC low latency' : 'G1GC high efficiency'}
                </p>
                {gcPreview ? (
                  <>
                    <p className="mt-1 text-muted-foreground">
                      {gcPreview.profile === 'low_latency' ? 'Generational ZGC' : gcPreview.profile === 'high_efficiency' ? 'Tuned G1GC' : 'Manual JVM flags'} · {gcPreview.heap_mb >= 1024 ? `${(gcPreview.heap_mb / 1024).toFixed(1)} GB effective heap` : `${gcPreview.heap_mb} MB effective heap`}
                    </p>
                    <code className="mt-2 block max-h-20 overflow-auto whitespace-pre-wrap break-words font-mono text-[11px] text-foreground/80">{gcPreview.jvm_args}</code>
                    {gcPreview.heap_mb !== instanceJvmMemory && (
                      <p className="mt-1 text-amber-700 dark:text-amber-400">The launch-time safety limit adjusted the heap to leave room for the OS.</p>
                    )}
                  </>
                ) : (
                  <p className="mt-1 text-muted-foreground">Launch preview unavailable until the backend responds.</p>
                )}
              </div>

              {/* Manual flags remain available without making them the default workflow. */}
              {advancedMode && instanceGcMode === 'manual' && (
                <div className="space-y-2">
                  <label htmlFor="instance-java-args" className="text-sm font-medium">Additional JVM flags</label>
                  <p className="text-xs text-muted-foreground">
                    Advanced flags are appended after Agora&apos;s managed memory settings. Do not include classpath or native-library flags.
                  </p>
                  <textarea
                    id="instance-java-args"
                    value={instanceJavaArgs}
                    onChange={(e) => setInstanceJavaArgs(e.target.value)}
                    rows={4}
                    placeholder="-Xss1M -Dsome.setting=true"
                    className="w-full rounded-lg border border-input bg-background px-3 py-2 text-sm font-mono resize-y"
                  />
                </div>
              )}

            {/* Allow incompatible override (Advanced Mode only) */}
            {advancedMode && (
              <div className="space-y-2">
                <label className="flex items-center gap-2">
                  <input
                    type="checkbox"
                    checked={instanceJavaAllowOverride}
                    onChange={(e) => setInstanceJavaAllowOverride(e.target.checked)}
                    className="h-4 w-4 accent-primary"
                  />
                  <span className="text-sm">
                    Allow this Java version even when Minecraft requests a different version (⚠ advanced users only)
                  </span>
                </label>
                {instanceJavaAllowOverride && (
                  <div className="rounded-lg border border-amber-500/30 bg-amber-500/5 px-3 py-2">
                    <p className="text-xs text-amber-700 dark:text-amber-400 font-medium">⚠ Compatibility warning</p>
                    <p className="text-xs text-amber-600/80 dark:text-amber-400/80 mt-0.5">
                      Using an incompatible Java version may cause crashes or unexpected behavior.
                      Only enable this if you understand the risks and have verified that the
                      selected Java runtime works with this Minecraft version.
                    </p>
                  </div>
                )}
              </div>
            )}

            {/* Save / Clear buttons */}
            <div className="flex gap-2">
              <button
                onClick={async () => {
                  setInstanceJavaSaving(true);
                  setInstanceJavaInspectError(null);
                  try {
                    // Validate path if non-empty
                    if (instanceJavaPath.trim()) {
                      await inspectJavaExecutable(instanceJavaPath.trim());
                    }
                    await updateInstanceJava(
                      instanceId,
                      instanceJavaPath.trim() || null,
                      instanceJavaAllowOverride,
                    );
                    await updateInstanceJvm(
                      instanceId,
                      instanceJvmMemory,
                      instanceGcMode,
                      instanceAlwaysPreTouch,
                      instanceJavaArgs.trim(),
                    );
                    setStatus('Java settings saved.');
                    // Refresh to update the displayed detail
                    const fresh = await getInstanceDetail(instanceId);
                    setDetail(fresh);
                  } catch (e) {
                    setInstanceJavaInspectError(formatError(e));
                  } finally {
                    setInstanceJavaSaving(false);
                  }
                }}
                disabled={instanceJavaSaving}
                className="rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
              >
                {instanceJavaSaving ? 'Saving…' : 'Save'}
              </button>
              <button
                onClick={async () => {
                  setInstanceJavaPath('');
                  setInstanceJavaArgs('');
                  setInstanceJavaInspected(null);
                  setInstanceJavaInspectError(null);
                  try {
                     await updateInstanceJava(instanceId, null, false);
                     await updateInstanceJvm(
                       instanceId,
                       instanceJvmMemory,
                       instanceGcMode,
                       instanceAlwaysPreTouch,
                       '',
                     );
                    setStatus('Java settings cleared.');
                    const fresh = await getInstanceDetail(instanceId);
                    setDetail(fresh);
                  } catch (e) {
                    setInstanceJavaInspectError(formatError(e));
                  }
                }}
                className="rounded-lg border border-input px-4 py-2 text-sm font-medium hover:bg-accent"
              >
                Clear
              </button>
            </div>
          </div>
          </div>
        </section>
      )}

      {canonicalOperation && (
        <InstallFlow
          open
          intent={canonicalOperation.intent}
          instanceName={canonicalOperation.instanceName}
          onOpenInstance={onOpenInstanceEditor}
          onClose={() => {
            setCanonicalOperation(null);
            void getInstanceDetail(instanceId).then(setDetail).catch((cause) => setError(formatError(cause)));
          }}
        />
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
