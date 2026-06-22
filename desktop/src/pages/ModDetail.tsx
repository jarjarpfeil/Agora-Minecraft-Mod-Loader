import { useEffect, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import rehypeRaw from 'rehype-raw';
import rehypeSanitize from 'rehype-sanitize';
import { defaultSchema, type Schema } from 'hast-util-sanitize';
import {
  getRegistryItem,
  listInstances,
  listModVersions,
  installModVersion,
  listLoaderVersions,
  listManifestLoaders,
  listManifestMcVersions,
  createInstance,
  formatError,
  type RegistryItem,
  type InstanceRow,
  type ModVersionCandidate,
  type CreateInstanceRequest,
} from '../lib/tauri';

type CompatibleVersionEntry = Record<string, unknown> | string;

// Allowlist schema for rendering community/upstream markdown (Modrinth body).
// Built on rehype-sanitize's default (already strips <script>, on* handlers,
// javascript:/data: URLs, <iframe>). Additionally allows richer structural tags
// (details/summary, tables) for formatting; drops `style` (blocks CSS-based UI
// overlay) and `className` (blocks Tailwind-class UI-deception injection);
// restricts href/src to https only. Satisfies AGENTS.md: no
// dangerouslySetInnerHTML — unsafe nodes are stripped from the tree pre-render.
//
// MIRRORS web/src/components/MarkdownRenderer.tsx SANITIZE_SCHEMA — there is no
// shared monorepo package, so keep both in sync when tightening this allowlist.
const SANITIZE_SCHEMA: Schema = {
  ...defaultSchema,
  tagNames: [
    ...(defaultSchema.tagNames ?? []),
    'details', 'summary', 'section', 'article', 'header', 'footer', 'aside',
    'figure', 'figcaption', 'mark', 'abbr', 'kbd', 'var', 'samp',
    'table', 'thead', 'tbody', 'tfoot', 'tr', 'th', 'td', 'caption', 'colgroup', 'col',
    'blockquote', 'hr', 'br', 'wbr',
  ],
  attributes: {
    ...defaultSchema.attributes,
    a: [...(defaultSchema.attributes?.a ?? []), 'title'],
    img: [...(defaultSchema.attributes?.img ?? []), 'alt', 'title', 'loading'],
    th: ['align'], td: ['align'], col: ['span'], colgroup: ['span'],
    details: ['open'],
  },
  protocols: {
    ...defaultSchema.protocols,
    href: ['https'],
    src: ['https'],
    cite: ['https'],
    poster: ['https'],
  },
};

function parseCompatibleVersions(json: string | null): CompatibleVersionEntry[] {
  if (!json) return [];
  try {
    const parsed = JSON.parse(json);
    if (Array.isArray(parsed)) {
      return parsed.filter((entry): entry is CompatibleVersionEntry =>
        typeof entry === 'object' && entry !== null ? true : typeof entry === 'string',
      );
    }
    if (parsed && typeof parsed === 'object') {
      return [parsed as CompatibleVersionEntry];
    }
    return [];
  } catch {
    return [];
  }
}

function renderVersionEntry(entry: CompatibleVersionEntry): string {
  if (typeof entry === 'string') return entry;
  const fields = ['mc_version', 'minecraft_version', 'loader', 'loader_version', 'version', 'game_version'];
  const parts: string[] = [];
  for (const field of fields) {
    const value = (entry as Record<string, unknown>)[field];
    if (value != null && value !== '') parts.push(`${field}: ${String(value)}`);
  }
  if (parts.length > 0) return parts.join(' · ');
  return JSON.stringify(entry);
}

type CuratorNotesRegistryItem = RegistryItem & { curator_notes?: string | null };

export function ModDetail({ itemId, onBack, onOpenInstanceEditor }: { itemId: string; onBack: () => void; onOpenInstanceEditor?: (instanceId: string) => void }) {
  const [item, setItem] = useState<RegistryItem | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Pack-as-instance state
  const [showPackCreate, setShowPackCreate] = useState(false);

  // Install flow state
  const [showInstallFlow, setShowInstallFlow] = useState(false);
  const [instances, setInstances] = useState<InstanceRow[]>([]);
  const [instancesLoading, setInstancesLoading] = useState(false);
  const [selectedInstanceId, setSelectedInstanceId] = useState<string | null>(null);
  const [candidates, setCandidates] = useState<ModVersionCandidate[]>([]);
  const [selectedCandidate, setSelectedCandidate] = useState<ModVersionCandidate | null>(null);
  const [phase, setPhase] = useState<'idle' | 'loadingVersions' | 'pickingVersion' | 'installing' | 'done' | 'error'>('idle');
  const [installMsg, setInstallMsg] = useState<string | null>(null);

  // Inline create-instance state (inside install flow)
  const [showCreateInline, setShowCreateInline] = useState(false);
  const [createName, setCreateName] = useState('');
  const [createMcVersion, setCreateMcVersion] = useState('');
  const [createAvailableLoaders, setCreateAvailableLoaders] = useState<string[]>([]);
  const [createAvailableMcVersions, setCreateAvailableMcVersions] = useState<string[]>([]);
  const [createLoader, setCreateLoader] = useState('fabric');
  const [createLoaderVersions, setCreateLoaderVersions] = useState<import('../lib/tauri').LoaderVersionSummary[]>([]);
  const [createLoaderVersion, setCreateLoaderVersion] = useState('');
  const [createBusy, setCreateBusy] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        if (!cancelled) setLoading(true);
        const result = await getRegistryItem(itemId);
        if (!cancelled) {
          setItem(result);
          if (!result) setError('Mod not found in the registry.');
        }
      } catch (e) {
        if (!cancelled) setError(formatError(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [itemId]);

  // Inline create: reload loader versions when loader or mcVersion changes
  useEffect(() => {
    if (!showCreateInline) return;
    let cancelled = false;
    (async () => {
      try {
        const versions = await listLoaderVersions(createLoader, createMcVersion);
        if (cancelled) return;
        setCreateLoaderVersions(versions);
        setCreateLoaderVersion(versions[0]?.loader_version ?? '');
      } catch (e) {
        if (!cancelled) setCreateError(formatError(e));
      }
    })();
    return () => { cancelled = true; };
  }, [showCreateInline, createLoader, createMcVersion]);

  // Fetch available manifest loaders and MC versions once on mount
  useEffect(() => {
    let cancelled = false;
    Promise.all([listManifestLoaders(), listManifestMcVersions()]).then(
      ([loaders, versions]) => {
        if (cancelled) return;
        setCreateAvailableLoaders(loaders);
        setCreateAvailableMcVersions(versions);
        if (!createMcVersion && versions.length > 0) {
          setCreateMcVersion(versions[0]);
        }
      },
    );
    return () => { cancelled = true; };
  }, []);

  if (loading) {
    return (
      <div className="space-y-6">
        <BackButton onBack={onBack} />
        <div className="rounded-xl p-6 border border-dashed border-gray-300 dark:border-gray-600 text-center text-[rgb(var(--muted))]">
          Loading mod…
        </div>
      </div>
    );
  }

  if (error || !item) {
    return (
      <div className="space-y-6">
        <BackButton onBack={onBack} />
        <div className="rounded-lg border border-red-300 bg-red-50 p-3 text-sm text-red-700 dark:border-red-700 dark:bg-red-900/30 dark:text-red-200">
          {error ?? 'Mod not found.'}
        </div>
      </div>
    );
  }

  const curatorNotes = (item as CuratorNotesRegistryItem).curator_notes ?? null;
  const compatibleVersions = parseCompatibleVersions(item.compatible_versions_json);
  const showIcon = item.icon_url != null && item.icon_url.startsWith('https://');
  const velocityLabel =
    item.velocity > 0 ? `▲ ${item.velocity.toFixed(1)}` : item.velocity < 0 ? `▼ ${item.velocity.toFixed(1)}` : '0.0';

  const handleInstall = async () => {
    setShowInstallFlow(true);
    setPhase('idle');
    setInstallMsg(null);
    setSelectedInstanceId(null);
    setCandidates([]);
    setSelectedCandidate(null);
    setInstances([]);
    setInstancesLoading(true);
    try {
      const all = await listInstances();
      setInstances(all);
    } catch (e) {
      setPhase('error');
      setInstallMsg(formatError(e));
    } finally {
      setInstancesLoading(false);
    }
  };

  const handlePickVersion = async () => {
    if (!selectedInstanceId) return;
    setPhase('loadingVersions');
    setCandidates([]);
    setSelectedCandidate(null);
    setInstallMsg(null);
    try {
      const vers = await listModVersions(selectedInstanceId, itemId);
      setCandidates(vers);
      setPhase('pickingVersion');
    } catch (e) {
      setPhase('error');
      setInstallMsg(formatError(e));
    }
  };

  const handleConfirmInstall = async () => {
    if (!selectedInstanceId || !selectedCandidate) return;
    setPhase('installing');
    setInstallMsg(null);
    try {
      await installModVersion(selectedInstanceId, itemId, selectedCandidate);
      setPhase('done');
      setInstallMsg(`Installed ${selectedCandidate.filename} to ${instances.find((i) => i.instance_id === selectedInstanceId)?.name ?? selectedInstanceId}.`);
    } catch (e) {
      setPhase('error');
      setInstallMsg(formatError(e));
    }
  };

  const handleCloseInstallFlow = () => {
    setShowInstallFlow(false);
    setPhase('idle');
    setInstallMsg(null);
    setSelectedInstanceId(null);
    setCandidates([]);
    setSelectedCandidate(null);
  };

  // Inline create: submit handler
  const handleCreateInstance = async () => {
    setCreateBusy(true);
    setCreateError(null);
    try {
      const instanceId = createName
        .toLowerCase()
        .replace(/[^a-z0-9-_]+/g, '-')
        .replace(/^-+|-+$/g, '');
      if (!instanceId) throw new Error('Enter a valid instance name.');
      if (!createLoaderVersion) throw new Error('No pinned loader version selected.');

      const request: CreateInstanceRequest = {
        name: createName || instanceId,
        instance_id: instanceId,
        minecraft_version: createMcVersion,
        loader: createLoader,
        loader_version: createLoaderVersion,
        jvm_memory_mb: 4096,
      };
      const result = await createInstance(request);
      // Refresh the instances list
      const all = await listInstances();
      setInstances(all);
      setSelectedInstanceId(result.instance_id);
      setShowCreateInline(false);
      setCreateName('');
      setCreateLoaderVersion('');
      setCreateLoaderVersions([]);
      setCreateError(null);
    } catch (e) {
      setCreateError(formatError(e));
    } finally {
      setCreateBusy(false);
    }
  };

  return (
    <div className="space-y-6">
      <BackButton onBack={onBack} />

      {item.is_immune && (
        <div
          className="rounded-lg border px-4 py-3 text-sm"
          style={{
            backgroundColor: 'rgba(70, 130, 180, 0.12)',
            borderColor: 'rgba(70, 130, 180, 0.6)',
            color: 'rgb(70, 130, 180)',
          }}
        >
          <div className="flex items-center gap-2 font-semibold">
            <span aria-hidden>🛡️</span>
            <span>Immunity Shield Active</span>
          </div>
          {item.immunity_reason && (
            <p className="mt-1 text-xs opacity-90">{item.immunity_reason}</p>
          )}
        </div>
      )}

      <section className="rounded-xl border border-gray-200 dark:border-gray-700 surface p-6">
        <div className="flex items-start gap-4">
          {showIcon && (
            <img
              src={item.icon_url as string}
              alt={item.name}
              className="h-16 w-16 rounded-lg border object-contain dark:border-gray-600"
            />
          )}
          <div className="flex-1 min-w-0">
            <div className="flex items-center gap-2 flex-wrap">
              <h2 className="text-2xl font-bold break-words">{item.name}</h2>
              <span className="rounded-full bg-brand-600 px-2 py-0.5 text-xs font-medium uppercase tracking-wide text-white">
                {item.content_type}
              </span>
              <span className="rounded-full border border-gray-300 dark:border-gray-600 px-2 py-0.5 text-xs text-[rgb(var(--muted))]">
                {item.download_strategy}
              </span>
              {item.status && item.status !== 'active' && (
                <span className="rounded-full border border-gray-300 dark:border-gray-600 px-2 py-0.5 text-xs text-[rgb(var(--muted))]">
                  {item.status}
                </span>
              )}
            </div>
            <p className="text-xs text-[rgb(var(--muted))] mt-1 break-all">
              {item.source_identifier}
            </p>
            <p className="text-xs text-[rgb(var(--muted))] mt-2">
              ↑ {item.upvotes} · ↓ {item.downvotes} · net {item.net_score} · velocity {velocityLabel}
            </p>
            {item.date_added && (
              <p className="text-xs text-[rgb(var(--muted))] mt-1">
                Added {item.date_added}
              </p>
            )}
            {item.description && (
              <p className="text-sm text-[rgb(var(--foreground))] mt-3">{item.description}</p>
            )}
            {(item.license_id || item.source_updated_at || item.page_url) && (
              <p className="text-xs text-[rgb(var(--muted))] mt-2 flex flex-wrap gap-x-3 gap-y-1">
                {item.license_id && <span>License: {item.license_id}</span>}
                {item.source_updated_at && <span>Source updated {item.source_updated_at.slice(0, 10)}</span>}
                {item.page_url && (
                  <a
                    href={item.page_url}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="text-brand-600 hover:underline dark:text-brand-400"
                  >
                    View on Modrinth ↗
                  </a>
                )}
              </p>
            )}
          </div>
        </div>

        <div className="mt-5 flex flex-wrap gap-2">
          {item.content_type === 'pack' ? (
            <button
              onClick={() => setShowPackCreate(true)}
              className="rounded-lg bg-brand-600 px-4 py-2 text-sm font-medium text-white hover:bg-brand-700"
            >
              Create Instance from Pack
            </button>
          ) : (
            <button
              onClick={handleInstall}
              className="rounded-lg bg-brand-600 px-4 py-2 text-sm font-medium text-white hover:bg-brand-700"
            >
              Install to Instance
            </button>
          )}
        </div>
        {showInstallFlow && (
          <section className="mt-4 rounded-xl border border-gray-200 dark:border-gray-700 surface p-4 space-y-4">
            <div className="flex items-center justify-between">
              <h3 className="font-semibold text-sm">Install to Instance</h3>
              <button
                onClick={handleCloseInstallFlow}
                className="text-xs text-[rgb(var(--muted))] hover:text-[rgb(var(--foreground))]"
              >
                Close
              </button>
            </div>

            {phase === 'error' && installMsg && (
              <p className="text-sm text-red-600 dark:text-red-300">{installMsg}</p>
            )}

            {/* Step 1: Instance picker */}
            {phase === 'idle' && (
              instancesLoading ? (
                <div className="text-center py-2">
                  <p className="text-sm text-[rgb(var(--muted))]">Loading instances…</p>
                </div>
              ) : (
                <div>
                  <label className="block text-xs font-medium mb-1">Select instance</label>
                  <select
                    value={selectedInstanceId ?? ''}
                    onChange={(e) => setSelectedInstanceId(e.target.value || null)}
                    className="w-full rounded-lg border border-gray-300 dark:border-gray-600 bg-transparent px-3 py-2 text-sm"
                  >
                    <option value="">Choose an instance…</option>
                    {instances.map((inst) => (
                      <option key={inst.instance_id} value={inst.instance_id}>
                        {inst.name} ({inst.loader} {inst.loader_version} · MC {inst.minecraft_version})
                      </option>
                    ))}
                  </select>
                  <button
                    onClick={handlePickVersion}
                    disabled={!selectedInstanceId}
                    className="mt-3 rounded-lg bg-brand-600 px-4 py-2 text-sm font-medium text-white hover:bg-brand-700 disabled:opacity-50"
                  >
                    Next: Choose Version
                  </button>
                  <button
                    onClick={() => setShowCreateInline(true)}
                    className="mt-2 block text-xs text-brand-600 hover:underline dark:text-brand-400"
                  >
                    + Create new instance
                  </button>
                  {showCreateInline && (
                    <div className="mt-3 space-y-3 rounded-lg border border-gray-200 dark:border-gray-700 p-3">
                      <p className="text-xs font-medium">Create new instance</p>
                      <label className="block">
                        <span className="text-xs">Instance name</span>
                        <input
                          value={createName}
                          onChange={(e) => setCreateName(e.target.value)}
                          placeholder="My Instance"
                          className="mt-1 w-full rounded-lg border border-gray-300 dark:border-gray-600 bg-transparent px-3 py-2 text-sm"
                        />
                      </label>
                      <div className="grid grid-cols-2 gap-3">
                        <label className="block">
                          <span className="text-xs">Minecraft version</span>
                          <select
                            value={createMcVersion}
                            onChange={(e) => setCreateMcVersion(e.target.value)}
                            className="mt-1 w-full rounded-lg border border-gray-300 dark:border-gray-600 bg-transparent px-3 py-2 text-sm"
                          >
                            {createAvailableMcVersions.map((v) => (
                              <option key={v} value={v}>{v}</option>
                            ))}
                          </select>
                        </label>
                        <label className="block">
                          <span className="text-xs">Loader</span>
                          <select
                            value={createLoader}
                            onChange={(e) => setCreateLoader(e.target.value)}
                            className="mt-1 w-full rounded-lg border border-gray-300 dark:border-gray-600 bg-transparent px-3 py-2 text-sm"
                          >
                            {createAvailableLoaders.map((l) => (
                              <option key={l} value={l}>{l}</option>
                            ))}
                          </select>
                        </label>
                      </div>
                      <label className="block">
                        <span className="text-xs">Loader version</span>
                        <select
                          value={createLoaderVersion}
                          onChange={(e) => setCreateLoaderVersion(e.target.value)}
                          className="mt-1 w-full rounded-lg border border-gray-300 dark:border-gray-600 bg-transparent px-3 py-2 text-sm"
                        >
                          {createLoaderVersions.length === 0 && <option value="">Loading…</option>}
                          {createLoaderVersions.map((v) => (
                            <option key={v.loader_version} value={v.loader_version}>
                              {v.loader_version} ({v.file_type})
                            </option>
                          ))}
                        </select>
                      </label>
                      {createError && (
                        <p className="text-xs text-red-600 dark:text-red-300">{createError}</p>
                      )}
                      <div className="flex gap-2">
                        <button
                          onClick={() => {
                            setShowCreateInline(false);
                            setCreateError(null);
                          }}
                          disabled={createBusy}
                          className="rounded-lg border border-gray-300 dark:border-gray-600 px-3 py-1.5 text-xs font-medium hover:bg-gray-100 dark:hover:bg-gray-800"
                        >
                          Cancel
                        </button>
                        <button
                          onClick={handleCreateInstance}
                          disabled={createBusy}
                          className="rounded-lg bg-brand-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-brand-700 disabled:opacity-50"
                        >
                          {createBusy ? 'Creating…' : 'Create'}
                        </button>
                      </div>
                    </div>
                  )}
                </div>
              )
            )}

            {/* Step 2: Version picker */}
            {selectedInstanceId && phase !== 'idle' && phase !== 'installing' && phase !== 'done' && (
              <div>
                <p className="text-xs font-medium mb-2">Available versions</p>
                {phase === 'loadingVersions' ? (
                  <div className="text-center py-4">
                    <svg className="animate-spin h-5 w-5 mx-auto text-[rgb(var(--muted))]" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
                      <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                      <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z" />
                    </svg>
                    <p className="text-xs text-[rgb(var(--muted))] mt-2">Loading versions…</p>
                  </div>
                ) : (
                  <ul className="space-y-2 max-h-48 overflow-y-auto">
                    {candidates.map((cand, idx) => (
                      <li
                        key={idx}
                        className={`rounded-lg border px-3 py-2 text-sm cursor-pointer transition-colors ${
                          selectedCandidate?.filename === cand.filename
                            ? 'border-brand-500 bg-brand-50 dark:bg-brand-900/20'
                            : 'border-gray-200 dark:border-gray-700 hover:bg-gray-50 dark:hover:bg-gray-800'
                        }`}
                        onClick={() => setSelectedCandidate(cand)}
                      >
                        <div className="flex items-center justify-between">
                          <span className="font-medium">{cand.version}</span>
                          {cand.is_compatible ? (
                            <span className="text-xs text-green-600 dark:text-green-400">✓ compatible</span>
                          ) : (
                            <span className="text-xs text-[rgb(var(--muted))]">may not match your instance</span>
                          )}
                        </div>
                        <p className="text-xs text-[rgb(var(--muted))] mt-0.5 truncate">{cand.filename}</p>
                        <p className="text-xs text-[rgb(var(--muted))] mt-0.5">
                          {[cand.mc_version, cand.loader].filter(Boolean).join(' · ')}
                          {cand.release_date ? ` · ${cand.release_date}` : ''}
                        </p>
                      </li>
                    ))}
                  </ul>
                )}
                {selectedCandidate && (
                  <button
                    onClick={handleConfirmInstall}
                    className="mt-3 rounded-lg bg-brand-600 px-4 py-2 text-sm font-medium text-white hover:bg-brand-700"
                  >
                    Install {selectedCandidate.filename}
                  </button>
                )}
              </div>
            )}

            {/* Empty versions */}
            {selectedInstanceId && candidates.length === 0 && phase === 'pickingVersion' && (
              <p className="text-sm text-[rgb(var(--muted))]">No compatible versions found.</p>
            )}

            {/* Step 3: Installing */}
            {phase === 'installing' && (
              <div className="text-center py-4">
                <svg className="animate-spin h-5 w-5 mx-auto text-[rgb(var(--muted))]" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
                  <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                  <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z" />
                </svg>
                <p className="text-xs text-[rgb(var(--muted))] mt-2">Downloading &amp; verifying…</p>
              </div>
            )}

            {/* Step 3: Done */}
            {phase === 'done' && installMsg && (
              <p className="text-sm text-green-600 dark:text-green-400">{installMsg}</p>
            )}
          </section>
        )}

        {/* Pack-create dialog */}
        {showPackCreate && (
          <PackCreateDialog
            packName={item.name}
            onCancel={() => setShowPackCreate(false)}
            onCreated={(newInstanceId) => {
              setShowPackCreate(false);
              onOpenInstanceEditor?.(newInstanceId);
            }}
          />
        )}
      </section>

      {curatorNotes && (
        <section className="rounded-xl border border-gray-200 dark:border-gray-700 surface p-4">
          <h3 className="font-semibold text-sm mb-2">Curator Notes</h3>
          <p className="text-sm whitespace-pre-wrap text-[rgb(var(--muted))]">{curatorNotes}</p>
        </section>
      )}

      {item.body_markdown && (
        <section className="rounded-xl border border-gray-200 dark:border-gray-700 surface p-4">
          <div className="flex items-center justify-between mb-3">
            <h3 className="font-semibold text-sm">About</h3>
            <span className="text-[10px] uppercase tracking-wide text-[rgb(var(--muted))]">
              Source: upstream
            </span>
          </div>
          {/*
            body_markdown is community-authored content baked into the signed
            registry.db by the nightly compiler. It is rendered with
            react-markdown + rehype-raw + rehype-sanitize: rehype-raw parses
            upstream HTML (e.g. Modrinth <details>/<summary>, tables) into the
            hast tree, then rehype-sanitize strips <script>, on* handlers,
            javascript:/data: URLs, <iframe>, and `style` attributes via an
            allowlist BEFORE React renders — so no unsafe nodes ever reach the
            DOM. This satisfies the AGENTS.md prohibition on
            dangerouslySetInnerHTML for community content. Links open in a new
            tab with safe rel attributes.
          */}
          <div className="prose prose-sm dark:prose-invert max-w-none text-[rgb(var(--foreground))]">
            <ReactMarkdown
              rehypePlugins={[[rehypeRaw, { passThrough: ['html'] }], [rehypeSanitize, SANITIZE_SCHEMA]]}
              components={{
                a: ({ node, ...props }) => (
                  <a {...props} target="_blank" rel="noopener noreferrer" />
                ),
                img: ({ node, ...props }) => (
                  <img {...props} loading="lazy" className="max-w-full h-auto rounded-lg" />
                ),
              }}
            >
              {item.body_markdown}
            </ReactMarkdown>
          </div>
        </section>
      )}

      <section className="rounded-xl border border-gray-200 dark:border-gray-700 surface p-4">
        <h3 className="font-semibold text-sm mb-3">Compatible Versions</h3>
        {compatibleVersions.length === 0 ? (
          <p className="text-sm text-[rgb(var(--muted))]">No compatible version information available.</p>
        ) : (
          <ul className="space-y-1.5 text-sm">
            {compatibleVersions.map((entry, index) => (
              <li
                key={index}
                className="rounded-md border border-gray-200 dark:border-gray-700 px-3 py-1.5 break-words"
              >
                {renderVersionEntry(entry)}
              </li>
            ))}
          </ul>
        )}
      </section>

      <section className="rounded-xl border border-gray-200 dark:border-gray-700 surface p-4">
        <h3 className="font-semibold text-sm mb-3">Reviews</h3>
        {item.allow_comments ? (
          <p className="text-sm text-[rgb(var(--muted))]">
            Community reviews will appear here.
          </p>
        ) : (
          <p className="text-sm text-[rgb(var(--muted))]">
            Reviews are disabled for this mod.
          </p>
        )}
      </section>
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

function PackCreateDialog({
  packName,
  onCancel,
  onCreated,
}: {
  packName: string;
  onCancel: () => void;
  onCreated: (instanceId: string) => void;
}) {
  const [name, setName] = useState(packName);
  const [mcVersion, setMcVersion] = useState('');
  const [availableLoaders, setAvailableLoaders] = useState<string[]>([]);
  const [availableMcVersions, setAvailableMcVersions] = useState<string[]>([]);
  const [loader, setLoader] = useState('fabric');
  const [loaderVersions, setLoaderVersions] = useState<import('../lib/tauri').LoaderVersionSummary[]>([]);
  const [loaderVersion, setLoaderVersion] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

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
    return () => { cancelled = true; };
  }, [loader, mcVersion]);

  // Fetch available manifest loaders and MC versions once on mount
  useEffect(() => {
    let cancelled = false;
    Promise.all([listManifestLoaders(), listManifestMcVersions()]).then(
      ([loaders, versions]) => {
        if (cancelled) return;
        setAvailableLoaders(loaders);
        setAvailableMcVersions(versions);
        if (!mcVersion && versions.length > 0) {
          setMcVersion(versions[0]);
        }
      },
    );
    return () => { cancelled = true; };
  }, []);

  const submit = async () => {
    setBusy(true);
    setError(null);
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
        jvm_memory_mb: 4096,
      };
      const result = await createInstance(request);
      onCreated(result.instance_id);
    } catch (e) {
      setError(formatError(e));
      setBusy(false);
    }
  };

  return (
    <div className="fixed inset-0 z-40 flex items-center justify-center bg-black/40 p-4">
      <div className="w-full max-w-lg rounded-2xl border border-gray-200 dark:border-gray-700 surface p-6 shadow-xl">
        <h3 className="text-lg font-bold mb-4">Create Instance from Pack: {packName}</h3>

        <div className="space-y-4">
          <label className="block">
            <span className="text-sm font-medium">Instance name</span>
            <input
              value={name}
              onChange={(e) => setName(e.target.value)}
              className="mt-1 w-full rounded-lg border border-gray-300 dark:border-gray-600 bg-transparent px-3 py-2 text-sm"
            />
          </label>

          <div className="grid grid-cols-2 gap-4">
            <label className="block">
              <span className="text-sm font-medium">Minecraft version</span>
              <select
                value={mcVersion}
                onChange={(e) => setMcVersion(e.target.value)}
                className="mt-1 w-full rounded-lg border border-gray-300 dark:border-gray-600 bg-transparent px-3 py-2 text-sm"
              >
                {availableMcVersions.map((v) => (
                  <option key={v} value={v}>{v}</option>
                ))}
              </select>
            </label>

            <label className="block">
              <span className="text-sm font-medium">Loader</span>
              <select
                value={loader}
                onChange={(e) => setLoader(e.target.value)}
                className="mt-1 w-full rounded-lg border border-gray-300 dark:border-gray-600 bg-transparent px-3 py-2 text-sm"
              >
                {availableLoaders.map((l) => (
                  <option key={l} value={l}>{l}</option>
                ))}
              </select>
            </label>
          </div>

          <label className="block">
            <span className="text-sm font-medium">Loader version</span>
            <select
              value={loaderVersion}
              onChange={(e) => setLoaderVersion(e.target.value)}
              className="mt-1 w-full rounded-lg border border-gray-300 dark:border-gray-600 bg-transparent px-3 py-2 text-sm"
            >
              {loaderVersions.length === 0 && <option value="">No pinned versions</option>}
              {loaderVersions.map((v) => (
                <option key={v.loader_version} value={v.loader_version}>
                  {v.loader_version} ({v.file_type})
                </option>
              ))}
            </select>
          </label>

          <p className="text-xs text-[rgb(var(--muted))]">
            The pack's mods will not auto-install. Open the instance editor to install them individually.
          </p>
        </div>

        {error && (
          <p className="mt-4 text-sm text-red-600 dark:text-red-300">{error}</p>
        )}

        <div className="mt-6 flex justify-end gap-2">
          <button
            onClick={onCancel}
            disabled={busy}
            className="rounded-lg border border-gray-300 dark:border-gray-600 px-4 py-2 text-sm font-medium hover:bg-gray-100 dark:hover:bg-gray-800"
          >
            Cancel
          </button>
          <button
            onClick={submit}
            disabled={busy}
            className="rounded-lg bg-brand-600 px-4 py-2 text-sm font-medium text-white hover:bg-brand-700 disabled:opacity-50"
          >
            {busy ? 'Creating…' : 'Create'}
          </button>
        </div>
      </div>
    </div>
  );
}
