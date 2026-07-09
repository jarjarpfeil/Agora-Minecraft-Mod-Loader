import { useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import {
  checkInstanceCrash,
  checkInstanceHealth,
  createInstance,
  deleteInstance,
  getSetting,
  killProcess,
  launchInstance,
  launchInstanceDirect,
  listInstances,
  listLoaderVersions,
  listManifestLoaders,
  listManifestMcVersions,
  formatError,
  type CreateInstanceRequest,
  type HealthReport,
  type InstanceRow,
  type LoaderVersionSummary,
} from '../lib/tauri';
import { ConsoleView } from '../components/ConsoleView';
import { CrashInvestigator } from '../components/CrashInvestigator';
import { HealthDialog } from '../components/HealthDialog';

export function Instances({ onEditInstance }: { onEditInstance: (id: string) => void }) {
  const [instances, setInstances] = useState<InstanceRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showCreate, setShowCreate] = useState(false);
  const [crashInvestigation, setCrashInvestigation] = useState<{
    instanceId: string;
    crashFilename: string | null;
    manualLogText: string | null;
  } | null>(null);

  // Direct launch state
  const [directLaunch, setDirectLaunch] = useState(false);
  const [runningPid, setRunningPid] = useState<number | null>(null);
  const [runningInstanceId, setRunningInstanceId] = useState<string | null>(null);

  const refresh = async () => {
    setLoading(true);
    setError(null);
    try {
      setInstances(await listInstances());
    } catch (e) {
      setError(formatError(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    refresh();
  }, []);

  // Reactive crash detection when the tab becomes visible.
  // Check the most recently launched instance for a crash report.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const lastLaunch = instances.find((i) => i.last_launched_at);
        if (!lastLaunch) return;
        const report = await checkInstanceCrash(lastLaunch.instance_id);
        if (!cancelled && report) {
          setCrashInvestigation({
            instanceId: lastLaunch.instance_id,
            crashFilename: report.filename,
            manualLogText: null,
          });
        }
      } catch {
        // Silently ignore — the user can still use manual troubleshooting.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [instances]);

  // Load launch mode setting on mount
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const mode = await getSetting('launch_mode');
        if (!cancelled) setDirectLaunch(mode === 'direct');
      } catch {
        // Default to delegation
      }
    })();
    return () => { cancelled = true; };
  }, []);

  // Listen for game-exited to clear running state
  useEffect(() => {
    const unlisten = listen<{ instance_id: string; exit_code: number }>('game-exited', (_e) => {
      setRunningPid(null);
      setRunningInstanceId(null);
    });
    return () => { unlisten.then(fn => fn()); };
  }, []);

  // State for the manual crash-log paste modal.
  const [pasteLog, setPasteLog] = useState<{ open: boolean; instanceId: string } | null>(null);

  const openCrashInvestigator = (instanceId: string) => {
    setPasteLog({ open: true, instanceId });
  };

  const submitPasteLog = (text: string) => {
    if (!pasteLog) return;
    setPasteLog(null);
    setCrashInvestigation({
      instanceId: pasteLog.instanceId,
      crashFilename: null,
      manualLogText: text || null,
    });
  };

  return (
    <div className="space-y-6">
      <section className="flex items-center justify-between">
        <div>
          <h2 className="text-2xl font-bold mb-2">My Instances</h2>
          <p className="text-muted-foreground">
            Isolated modpack profiles, custom instances, and launch history.
          </p>
        </div>
        <button
          onClick={() => setShowCreate(true)}
          className="rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90"
        >
          + Create Instance
        </button>
      </section>

      {error && (
        <div className="rounded-lg bg-destructive p-3 text-sm text-destructive-foreground">
          {error}
        </div>
      )}

      {loading ? (
        <div className="rounded-xl p-6 border border-dashed border-border text-center text-muted-foreground">
          Loading instances…
        </div>
      ) : instances.length === 0 ? (
        <div className="rounded-xl p-6 border border-dashed border-border text-center">
          <p className="text-muted-foreground">No instances yet.</p>
          <p className="text-sm text-muted-foreground mt-2">
            Create a custom instance to install a verified modloader and launch via the official Mojang launcher or the in-app direct launcher.
          </p>
        </div>
      ) : (
        <ul className="grid grid-cols-1 gap-4 md:grid-cols-2">
          {instances.map((instance) => (
            <InstanceCard
              key={instance.instance_id}
              instance={instance}
              onChanged={refresh}
              onEdit={() => onEditInstance(instance.instance_id)}
              onOpenCrashInvestigator={openCrashInvestigator}
              directLaunch={directLaunch}
              runningPid={runningPid}
              runningInstanceId={runningInstanceId}
              onRunningChanged={(id, pid) => {
                setRunningInstanceId(id);
                setRunningPid(pid);
              }}
            />
          ))}
        </ul>
      )}

      {showCreate && (
        <CreateInstanceDialog
          onClose={() => setShowCreate(false)}
          onCreated={() => {
            setShowCreate(false);
            refresh();
          }}
        />
      )}

      {crashInvestigation && (
        <CrashInvestigator
          instanceId={crashInvestigation.instanceId}
          crashFilename={crashInvestigation.crashFilename}
          manualLogText={crashInvestigation.manualLogText}
          onClose={() => setCrashInvestigation(null)}
        />
      )}

      {pasteLog && (
        <PasteLogModal
          onClose={() => setPasteLog(null)}
          onSubmit={(text) => submitPasteLog(text)}
        />
      )}
    </div>
  );
}

function InstanceCard({
  instance,
  onChanged,
  onEdit,
  onOpenCrashInvestigator,
  directLaunch,
  runningPid,
  runningInstanceId,
  onRunningChanged,
}: {
  instance: InstanceRow;
  onChanged: () => void;
  onEdit: () => void;
  onOpenCrashInvestigator: (id: string) => void;
  directLaunch: boolean;
  runningPid: number | null;
  runningInstanceId: string | null;
  onRunningChanged: (instanceId: string | null, pid: number | null) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [healthReport, setHealthReport] = useState<HealthReport | null>(null);
  const [showHealth, setShowHealth] = useState(false);

  const isRunning = runningInstanceId === instance.instance_id;

  const launch = async () => {
    setBusy(true);
    setError(null);
    try {
      // Phase 4: pre-launch health scan
      const report = await checkInstanceHealth(instance.instance_id);
      const hasBlockers = report.blockers.length > 0;
      const hasWarnings = report.warnings.length > 0;
      if (hasBlockers || hasWarnings) {
        setHealthReport(report);
        setShowHealth(true);
        setBusy(false);
        return;
      }
      // All clear — launch
      if (directLaunch) {
        const pid = await launchInstanceDirect(instance.instance_id);
        onRunningChanged(instance.instance_id, pid);
      } else {
        await launchInstance(instance.instance_id);
      }
    } catch (e) {
      setError(formatError(e));
    } finally {
      setBusy(false);
    }
  };

  const kill = async () => {
    if (runningPid == null) return;
    try {
      await killProcess(runningPid);
      onRunningChanged(null, null);
    } catch (e) {
      setError(formatError(e));
    }
  };

  const handleLaunchFromHealth = () => {
    setShowHealth(false);
    setHealthReport(null);
  };

  const handleCloseHealth = () => {
    setShowHealth(false);
    setHealthReport(null);
  };

  const remove = async () => {
    if (!confirm(`Delete instance "${instance.name}"? This moves the folder to trash.`)) return;
    setBusy(true);
    setError(null);
    try {
      await deleteInstance(instance.instance_id);
      onChanged();
    } catch (e) {
      setError(formatError(e));
      setBusy(false);
    }
  };

  return (
    <>
    {showHealth && healthReport && (
      <HealthDialog
        instanceId={instance.instance_id}
        instanceName={instance.name}
        onClose={handleCloseHealth}
        onLaunch={handleLaunchFromHealth}
      />
    )}
    <li className="rounded-xl border border-border bg-card p-4">
      <div className="flex items-start justify-between gap-3">
        <div>
          <h3 className="font-semibold">{instance.name}</h3>
          <p className="text-xs text-muted-foreground">
            {instance.loader} {instance.loader_version} · MC {instance.minecraft_version}
          </p>
          <p className="text-xs text-muted-foreground mt-1">
            {isRunning ? (
              <span className="text-green-600 dark:text-green-400">● Running (PID {runningPid})</span>
            ) : instance.last_launched_at ? (
              `Last launched ${instance.last_launched_at}`
            ) : (
              'Never launched'
            )}
          </p>
        </div>
        <span className="text-xs uppercase tracking-wide text-muted-foreground">
          {instance.is_locked ? 'Locked' : 'Unlocked'}
        </span>
      </div>

      {error && (
        <p className="mt-2 text-xs text-destructive">{error}</p>
      )}

      <div className="mt-4 flex gap-2">
        {isRunning && directLaunch ? (
          <button
            onClick={kill}
            className="rounded-lg bg-destructive px-3 py-1.5 text-sm font-medium text-destructive-foreground hover:bg-destructive/90"
          >
            ■ Kill
          </button>
        ) : (
          <button
            onClick={launch}
            disabled={busy || (runningInstanceId != null && runningInstanceId !== instance.instance_id)}
            className="rounded-lg bg-primary px-3 py-1.5 text-sm font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
          >
            {busy ? 'Launching…' : directLaunch ? '▶ Launch (Direct)' : '▶ Launch'}
          </button>
        )}
        <button
          onClick={() => onOpenCrashInvestigator(instance.instance_id)}
          disabled={busy}
          className="rounded-lg border border-input bg-background px-3 py-1.5 text-sm font-medium hover:bg-accent disabled:opacity-50"
        >
          Troubleshoot Crash
        </button>
        <button
          onClick={onEdit}
          disabled={busy}
          className="rounded-lg border border-input bg-background px-3 py-1.5 text-sm font-medium hover:bg-accent disabled:opacity-50"
        >
          Edit
        </button>
        <button
          onClick={remove}
          disabled={busy}
          className="rounded-lg border border-input bg-background px-3 py-1.5 text-sm font-medium hover:bg-accent disabled:opacity-50"
        >
          Delete
        </button>
      </div>

      {isRunning && directLaunch && (
        <div className="mt-3">
          <ConsoleView instanceId={instance.instance_id} />
        </div>
      )}
    </li>
    </>
  );
}

function CreateInstanceDialog({
  onClose,
  onCreated,
}: {
  onClose: () => void;
  onCreated: () => void;
}) {
  const [name, setName] = useState('');
  const [mcVersion, setMcVersion] = useState('');
  const [loader, setLoader] = useState('fabric');
  const [loaderVersions, setLoaderVersions] = useState<LoaderVersionSummary[]>([]);
  const [loaderVersion, setLoaderVersion] = useState('');
  const [memoryMb, setMemoryMb] = useState(4096);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [progressMessage, setProgressMessage] = useState<string | null>(null);
  const [loaders, setLoaders] = useState<string[]>([]);
  const [mcVersions, setMcVersions] = useState<string[]>([]);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const versions = await listLoaderVersions(loader, mcVersion);
        if (cancelled) return;
        setLoaderVersions(versions);
        setLoaderVersion(versions[0]?.loader_version ?? '');
      } catch (e) {
        if (!cancelled) setError(formatError(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [loader, mcVersion]);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const [l, v] = await Promise.all([listManifestLoaders(), listManifestMcVersions()]);
        if (!cancelled) {
          setLoaders(l);
          setMcVersions(v);
          if (!mcVersion && v.length > 0) {
            setMcVersion(v[0]);
          }
        }
      } catch {
        // Fetch failure: dropdowns render empty — acceptable degraded behavior.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Re-filter MC versions when the loader changes.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      if (!loader) return;
      try {
        const filtered = await listManifestMcVersions(loader);
        if (cancelled) return;
        if (filtered.length > 0) {
          setMcVersions(filtered);
          if (!filtered.includes(mcVersion)) {
            setMcVersion(filtered[0]);
          }
        }
      } catch {
        // Fetch failure — keep existing list (graceful)
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [loader]);

  // Progress event listener during creation
  useEffect(() => {
    if (!busy) return;
    setProgressMessage('Starting…');
    const unlisten = listen<{ instance_id: string; stage: string; message: string }>('instance:create-progress', (e) => {
      setProgressMessage(e.payload.message);
    });
    return () => { unlisten.then(fn => fn()); };
  }, [busy]);

  const submit = async () => {
    setBusy(true);
    setError(null);
    setProgressMessage(null);
    try {
      const instanceId = name
        .toLowerCase()
        .replace(/[^a-z0-9-_]+/g, '-')
        .replace(/^-+|-+$/g, '');
      if (!instanceId) throw new Error('Enter a valid instance name.');
      if (!loaderVersion) throw new Error('No pinned loader version selected.');

      const request: CreateInstanceRequest = {
        name,
        instance_id: instanceId,
        minecraft_version: mcVersion,
        loader,
        loader_version: loaderVersion,
        jvm_memory_mb: memoryMb,
      };
      await createInstance(request);
      onCreated();
    } catch (e) {
      setError(formatError(e));
      setBusy(false);
    }
  };

  return (
    <div className="fixed inset-0 z-40 flex items-center justify-center bg-black/40 p-4">
      <div className="w-full max-w-lg rounded-2xl border border-border bg-background p-6 shadow-xl">
        <h3 className="text-lg font-bold mb-4">Create Custom Instance</h3>

        <div className="space-y-4">
          <label className="block">
            <span className="text-sm font-medium">Instance name</span>
            <input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="Optimized Survival"
              className="mt-1 w-full rounded-lg border border-input bg-background px-3 py-2 text-sm"
            />
          </label>

          <div className="grid grid-cols-2 gap-4">
            <label className="block">
              <span className="text-sm font-medium">Minecraft version</span>
              <select
                value={mcVersion}
                onChange={(e) => setMcVersion(e.target.value)}
                className="mt-1 w-full rounded-lg border border-input bg-background px-3 py-2 text-sm"
              >
                {mcVersions.map((v) => (
                  <option key={v} value={v}>
                    {v}
                  </option>
                ))}
              </select>
            </label>

            <label className="block">
              <span className="text-sm font-medium">Loader</span>
              <select
                value={loader}
                onChange={(e) => setLoader(e.target.value)}
                className="mt-1 w-full rounded-lg border border-input bg-background px-3 py-2 text-sm"
              >
                {loaders.map((l) => (
                  <option key={l} value={l}>
                    {l}
                  </option>
                ))}
              </select>
            </label>
          </div>

          <label className="block">
            <span className="text-sm font-medium">Loader version</span>
            <select
              value={loaderVersion}
              onChange={(e) => setLoaderVersion(e.target.value)}
              className="mt-1 w-full rounded-lg border border-input bg-background px-3 py-2 text-sm"
            >
              {loaderVersions.length === 0 && <option value="">No pinned versions</option>}
              {loaderVersions.map((v) => (
                <option key={v.loader_version} value={v.loader_version}>
                  {v.loader_version} ({v.file_type})
                </option>
              ))}
            </select>
          </label>

          <label className="block">
            <span className="text-sm font-medium">JVM memory: {memoryMb} MB</span>
            <input
              type="range"
              min={1024}
              max={16384}
              step={512}
              value={memoryMb}
              onChange={(e) => setMemoryMb(Number(e.target.value))}
              className="mt-1 w-full accent-brand-600"
            />
          </label>
        </div>

        {progressMessage && (
          <p className="mt-4 text-sm text-muted-foreground">{progressMessage}</p>
        )}

        {error && (
          <p className="mt-4 text-sm text-destructive">{error}</p>
        )}

        <div className="mt-6 flex justify-end gap-2">
          <button
            onClick={onClose}
            disabled={busy}
            className="rounded-lg border border-input bg-background px-4 py-2 text-sm font-medium hover:bg-accent"
          >
            Cancel
          </button>
          <button
            onClick={submit}
            disabled={busy}
            className="rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
          >
            {busy ? 'Creating…' : 'Create'}
          </button>
        </div>
      </div>
    </div>
  );
}

function PasteLogModal({
  onClose,
  onSubmit,
}: {
  onClose: () => void;
  onSubmit: (text: string) => void;
}) {
  const [text, setText] = useState('');

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4">
      <div className="w-full max-w-lg rounded-2xl border border-border bg-card p-6 shadow-xl">
        <h3 className="text-lg font-bold mb-4">Paste Crash Log</h3>
        <textarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder="Paste your crash log or latest.log contents here…"
          className="w-full h-48 rounded-lg border border-gray-300 dark:border-gray-600 bg-transparent px-3 py-2 text-sm font-mono resize-y"
        />
        <div className="mt-6 flex justify-end gap-2">
          <button
            onClick={onClose}
            className="rounded-lg border border-gray-300 dark:border-gray-600 px-4 py-2 text-sm font-medium hover:bg-gray-100 dark:hover:bg-gray-800"
          >
            Cancel
          </button>
          <button
            onClick={() => onSubmit(text)}
            className="rounded-lg bg-brand-600 px-4 py-2 text-sm font-medium text-white hover:bg-brand-700"
          >
            Investigate
          </button>
        </div>
      </div>
    </div>
  );
}
