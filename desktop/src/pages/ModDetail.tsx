import { useEffect, useState, useRef, useCallback } from 'react';
import { listen } from '@tauri-apps/api/event';
import ReactMarkdown from 'react-markdown';
import rehypeRaw from 'rehype-raw';
import rehypeSanitize from 'rehype-sanitize';
import { defaultSchema, type Schema } from 'hast-util-sanitize';
import {
  formatError,
  getAuthStatus,
  getCuratedAnnotation,
  getFlagRateLimit,
  getGithubProfile,
  getRegistryItem,
  importModrinthPackByUrl,
  isModrinthEnabled,
  listInstances,
  listLoaderVersions,
  listManifestLoaders,
  listManifestMcVersions,
  listModReviews,
  listModVersions,
  listModVersionsLoadMore,
  listPackMods,
  listRawModrinthVersions,
  flagReview,
  createInstance,
  fetchModrinthProject,
  type CreateInstanceRequest,
  type CuratedAnnotation,
  type FlagRateLimit,
  type InstanceRow,
  type ModReview,
  type ModrinthProjectFull,
  type ModVersionCandidate,
  type PackModRow,
  type RegistryItem,
  type RawModrinthVersionCandidate,
} from '../lib/tauri';
import { InstallFlow } from '../components/InstallFlow';
import type { BatchInstallItem, InstallIntent, SourceType } from '../lib/installFlow';


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
  const [modrinthCandidates, setModrinthCandidates] = useState<RawModrinthVersionCandidate[]>([]);
  const [selectedCandidate, setSelectedCandidate] = useState<ModVersionCandidate | null>(null);
  const [selectedModrinthCandidate, setSelectedModrinthCandidate] = useState<RawModrinthVersionCandidate | null>(null);
  const [phase, setPhase] = useState<'idle' | 'loadingVersions' | 'pickingVersion' | 'installing' | 'done' | 'error'>('idle');
  const [installMsg, setInstallMsg] = useState<string | null>(null);
  const [canonicalInstall, setCanonicalInstall] = useState<{
    intent: InstallIntent;
    instanceName: string;
  } | null>(null);

  // Version pagination state
  const [versionPage, setVersionPage] = useState(1);
  const [hasMoreVersions, setHasMoreVersions] = useState(false);
  const [loadingMoreVersions, setLoadingMoreVersions] = useState(false);
  const versionSentinelRef = useRef<HTMLDivElement>(null);

  // Reviews state
  const [reviews, setReviews] = useState<ModReview[]>([]);
  const [reviewsLoading, setReviewsLoading] = useState(false);
  const [authed, setAuthed] = useState<boolean | null>(null);
  const [profile, setProfile] = useState<import('../lib/tauri').GithubProfile | null>(null);
  const [rateLimit, setRateLimit] = useState<FlagRateLimit | null>(null);
  const [flaggingId, setFlaggingId] = useState<number | null>(null);
  const [flagResult, setFlagResult] = useState<string | null>(null);
  const [flagError, setFlagError] = useState<string | null>(null);
  const governanceLoadedForRef = useRef<string | null>(null);

  // Tab state
  const [activeTab, setActiveTab] = useState<'description' | 'gallery' | 'versions' | 'agora'>('description');

  // Versions tab: live Modrinth version list + selected version detail
  const [modrinthVersions, setModrinthVersions] = useState<RawModrinthVersionCandidate[]>([]);
  const [versionsLoading, setVersionsLoading] = useState(false);
  const [versionsError, setVersionsError] = useState<string | null>(null);
  const [modrinthEnabled, setModrinthEnabled] = useState<boolean | null>(null);
  const [selectedVersion, setSelectedVersion] = useState<RawModrinthVersionCandidate | null>(null);
  // Versions tab install: instance picker
  const [versionsTabInstances, setVersionsTabInstances] = useState<InstanceRow[]>([]);
  const [versionsTabInstanceId, setVersionsTabInstanceId] = useState<string | null>(null);

  // Versions tab: GitHub release version list (for mods without modrinth_id)
  const [githubTabVersions, setGithubTabVersions] = useState<ModVersionCandidate[]>([]);
  const [githubTabLoading, setGithubTabLoading] = useState(false);
  const [githubTabError, setGithubTabError] = useState<string | null>(null);
  const [githubTabHasMore, setGithubTabHasMore] = useState(false);
  const [githubTabPage, setGithubTabPage] = useState(1);
  const [githubTabLoadingMore, setGithubTabLoadingMore] = useState(false);
  const githubTabSentinelRef = useRef<HTMLDivElement>(null);
  const [selectedGithubTabVersion, setSelectedGithubTabVersion] = useState<ModVersionCandidate | null>(null);

  // Phase 7: curated annotation overlay for Modrinth-linked projects
  const [curatedAnnotation, setCuratedAnnotation] = useState<CuratedAnnotation | null>(null);

  // Full Modrinth project data (primary source when modrinth_id exists)
  const [modrinthProject, setModrinthProject] = useState<ModrinthProjectFull | null>(null);
  const [_modrinthProjectLoading, setModrinthProjectLoading] = useState(false);
  const [_modrinthProjectError, setModrinthProjectError] = useState<string | null>(null);

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
        setError(null);
        // Try registry first
        const result = await getRegistryItem(itemId);
        if (!cancelled) {
          if (result) {
            setItem(result);
          } else {
            // Not in registry — try as a Modrinth project ID
            const project = await fetchModrinthProject(itemId);
            if (!cancelled) {
              if (project) {
                setModrinthProject(project);
                // Build a synthetic RegistryItem from Modrinth data so existing rendering works
                setItem({
                  id: project.id,
                  name: project.title,
                  content_type: project.project_type === 'modpack' ? 'pack' : project.project_type,
                  download_strategy: 'modrinth_id',
                  source_identifier: project.id,
                  sha256: '',
                  upvotes: 0,
                  downvotes: 0,
                  net_score: 0,
                  velocity: 0,
                  status: 'active',
                  is_immune: false,
                  immunity_reason: null,
                  allow_comments: true,
                  icon_url: project.icon_url,
                  gallery_urls_json: project.gallery_urls.length > 0 ? JSON.stringify(project.gallery_urls) : null,
                  date_added: null,
                  compatible_versions_json: null,
                  description: project.description,
                  body_markdown: project.body,
                  page_url: project.page_url,
                  license_id: project.license_id,
                  source_updated_at: project.source_updated_at,
                  modrinth_id: project.id,
                } as any);
              } else {
                setError('Mod not found.');
              }
            }
          }
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

  // When createLoader changes, re-fetch MC versions filtered by that loader
  // and intersect with the mod's compatible_versions_json if available.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      if (!createLoader) return;
      try {
        const filtered = await listManifestMcVersions(createLoader);
        if (cancelled) return;

        // Also intersect with the mod's compatible_versions if available
        let result = filtered;
        if (item?.compatible_versions_json) {
          try {
            const compat = JSON.parse(item.compatible_versions_json) as Array<{
              mc_version: string;
              loader: string;
              mod_version: string;
            }>;
            const modMcVersions = new Set(
              compat
                .filter((c) => c.loader.toLowerCase() === createLoader.toLowerCase())
                .map((c) => c.mc_version),
            );
            if (modMcVersions.size > 0) {
              result = result.filter((v) => modMcVersions.has(v));
            }
          } catch {
            // compatible_versions_json parse failure — skip the mod-compat filter
          }
        }

        // Fallback: if filtered results are empty, show the full loader-filtered list
        if (result.length === 0) {
          result = filtered;
        }

        setCreateAvailableMcVersions(result);
        if (result.length > 0 && !result.includes(createMcVersion)) {
          setCreateMcVersion(result[0]);
        }
      } catch {
        // Fetch failure — keep existing list (graceful)
      }
    })();
    return () => { cancelled = true; };
  }, [createLoader, item?.compatible_versions_json, createMcVersion]);

  // Validate GitHub only when governance/review controls become visible.
  useEffect(() => {
    if (activeTab !== 'agora' || governanceLoadedForRef.current === itemId) return;
    governanceLoadedForRef.current = itemId;
    let cancelled = false;
    (async () => {
      try {
        const [auth, prof, rl] = await Promise.all([
          getAuthStatus(),
          getGithubProfile(),
          getFlagRateLimit(),
        ]);
        if (cancelled) return;
        setAuthed(auth);
        setProfile(prof);
        setRateLimit(rl);
      } catch (e) {
        if (!cancelled) setFlagError(formatError(e));
      }
    })();
    return () => { cancelled = true; };
  }, [activeTab, itemId]);

  // Load reviews when item is available
  useEffect(() => {
    if (!item) return;
    let cancelled = false;
    (async () => {
      if (!cancelled) setReviewsLoading(true);
      try {
        const revs = await listModReviews(item.id);
        if (!cancelled) setReviews(revs);
      } catch (e) {
        if (!cancelled) setFlagError(formatError(e));
      } finally {
        if (!cancelled) setReviewsLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, [item]);

  // Fetch curated annotation for ALL items (modrinth_id or registry id).
  useEffect(() => {
    if (!item) return;
    let cancelled = false;
    (async () => {
      try {
        const key = item.modrinth_id || item.id;
        const ann = await getCuratedAnnotation(key);
        if (!cancelled) setCuratedAnnotation(ann);
      } catch {
        // Annotation fetch failure is non-fatal.
      }
    })();
    return () => { cancelled = true; };
  }, [item]);

  // Fetch full Modrinth project data when the item has a modrinth_id.
  useEffect(() => {
    if (!item?.modrinth_id) return;
    let cancelled = false;
    (async () => {
      try {
        const enabled = await isModrinthEnabled();
        if (cancelled || !enabled) return;
        setModrinthProjectLoading(true);
        const project = await fetchModrinthProject(item.modrinth_id!);
        if (cancelled) return;
        setModrinthProject(project);
      } catch (e) {
        if (!cancelled) setModrinthProjectError(formatError(e));
      } finally {
        if (!cancelled) setModrinthProjectLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, [item]);

  // Fetch Modrinth versions for the Versions tab (with changelogs).
  // Only when the mod has a modrinth_id and Modrinth is enabled.
  useEffect(() => {
    if (!item?.modrinth_id) return;
    let cancelled = false;
    (async () => {
      try {
        const enabled = await isModrinthEnabled();
        if (cancelled) return;
        setModrinthEnabled(enabled);
        if (!enabled) {
          if (item?.download_strategy !== 'github_release') {
            setVersionsError('Enable Modrinth integration in Settings to view live versions.');
          }
          return;
        }
        setVersionsLoading(true);
        setVersionsError(null);
        const versions = await listRawModrinthVersions(null, item.modrinth_id!);
        if (cancelled) return;
        setModrinthVersions(versions);
      } catch (e) {
        if (!cancelled) setVersionsError(formatError(e));
      } finally {
        if (!cancelled) setVersionsLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, [item]);

  // Fetch instances for the versions tab install picker.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const all = await listInstances();
        if (!cancelled) setVersionsTabInstances(all);
      } catch {
        // Silent — the picker will simply be empty.
      }
    })();
    return () => { cancelled = true; };
  }, []);

  // ═══════════════════════════════════════════════════════════════
  // Hooks (useCallback, useEffect) MUST be called every render.
  // They are placed here — BEFORE any early returns — to keep hook
  // order stable across all render paths.
  // ═══════════════════════════════════════════════════════════════

  // item may still be null during loading renders; fall back to false.
  const isModrinthInstall = !!(item?.modrinth_id && modrinthProject);

  const loadMoreVersions = useCallback(async () => {
    if (!selectedInstanceId || loadingMoreVersions || !hasMoreVersions) return;
    if (isModrinthInstall) return;
    setLoadingMoreVersions(true);
    try {
      const nextPage = await listModVersionsLoadMore(selectedInstanceId, itemId, versionPage);
      setCandidates((prev) => [...prev, ...nextPage.items]);
      setHasMoreVersions(nextPage.hasMore);
      setVersionPage((prev) => prev + 1);
    } catch {
      // Silently stop loading on error
    } finally {
      setLoadingMoreVersions(false);
    }
  }, [selectedInstanceId, itemId, versionPage, loadingMoreVersions, hasMoreVersions, isModrinthInstall]);

  // Infinite scroll for version picker
  useEffect(() => {
    const sentinel = versionSentinelRef.current;
    if (!sentinel || !hasMoreVersions || loadingMoreVersions || isModrinthInstall) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0]?.isIntersecting && hasMoreVersions && !loadingMoreVersions) {
          loadMoreVersions();
        }
      },
      { rootMargin: '400px' },
    );
    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [hasMoreVersions, loadingMoreVersions, isModrinthInstall, loadMoreVersions]);

  // --- Versions tab: GitHub release fetching (for mods without modrinth_id) ---
  // Fetches real GitHub release versions instead of showing the static
  // compatible_versions_json "guess" from the registry.
  const loadMoreGithubTabVersions = useCallback(async () => {
    if (githubTabLoadingMore || !githubTabHasMore) return;
    setGithubTabLoadingMore(true);
    try {
      const nextPage = await listModVersionsLoadMore(null, itemId, githubTabPage);
      setGithubTabVersions((prev) => [...prev, ...nextPage.items]);
      setGithubTabHasMore(nextPage.hasMore);
      setGithubTabPage((prev) => prev + 1);
    } catch {
      // Silently stop loading on error
    } finally {
      setGithubTabLoadingMore(false);
    }
  }, [itemId, githubTabPage, githubTabLoadingMore, githubTabHasMore]);

  useEffect(() => {
    if (activeTab !== 'versions' || !item) return;
    // When the mod has a modrinth_id, only skip GitHub versions if Modrinth
    // IS enabled, or if the mod doesn't use github_release as its strategy.
    if (item.modrinth_id && (modrinthEnabled !== false || item.download_strategy !== 'github_release')) return;
    let cancelled = false;
    (async () => {
      setGithubTabLoading(true);
      setGithubTabError(null);
      setGithubTabVersions([]);
      setGithubTabHasMore(false);
      setGithubTabPage(1);
      setSelectedGithubTabVersion(null);
      try {
        const page = await listModVersions(null, itemId);
        if (cancelled) return;
        setGithubTabVersions(page.items);
        setGithubTabHasMore(page.hasMore);
      } catch (e) {
        if (!cancelled) setGithubTabError(formatError(e));
      } finally {
        if (!cancelled) setGithubTabLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, [activeTab, item, itemId, modrinthEnabled]);

  // Infinite scroll for GitHub tab versions
  useEffect(() => {
    const sentinel = githubTabSentinelRef.current;
    if (!sentinel || !githubTabHasMore || githubTabLoadingMore) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0]?.isIntersecting && githubTabHasMore && !githubTabLoadingMore) {
          loadMoreGithubTabVersions();
        }
      },
      { rootMargin: '400px' },
    );
    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [githubTabHasMore, githubTabLoadingMore, loadMoreGithubTabVersions]);

  if (loading) {
    return (
      <div className="space-y-6">
        <BackButton onBack={onBack} />
        <div className="rounded-xl border border-dashed border-border bg-card p-6 text-center text-muted-foreground">
          Loading mod…
        </div>
      </div>
    );
  }

  if (error || !item) {
    return (
      <div className="space-y-6">
        <BackButton onBack={onBack} />
        <div className="rounded-lg border border-destructive bg-destructive/10 p-3 text-sm dark:text-destructive">
          {error ?? 'Mod not found.'}
        </div>
      </div>
    );
  }

  const curatorNotes = (item as CuratorNotesRegistryItem).curator_notes ?? null;
  const galleryUrls: string[] = (() => {
    if (!item.gallery_urls_json) return [];
    try {
      const parsed = JSON.parse(item.gallery_urls_json);
      if (Array.isArray(parsed)) {
        return parsed.filter((url): url is string => typeof url === 'string' && url.startsWith('https://'));
      }
      return [];
    } catch {
      return [];
    }
  })();
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
    setModrinthCandidates([]);
    setSelectedCandidate(null);
    setSelectedModrinthCandidate(null);
    setInstallMsg(null);
    setVersionPage(1);
    setHasMoreVersions(false);
    setLoadingMoreVersions(false);
    try {
      if (isModrinthInstall) {
        const vers = await listRawModrinthVersions(selectedInstanceId, item.modrinth_id!, item.content_type);
        setModrinthCandidates(vers);
      } else {
        const page = await listModVersions(selectedInstanceId, itemId);
        setCandidates(page.items);
        setHasMoreVersions(page.hasMore);
      }
      setPhase('pickingVersion');
    } catch (e) {
      setPhase('error');
      setInstallMsg(formatError(e));
    }
  };

  const openCanonicalInstall = (
    instanceId: string,
    sourceType: SourceType,
    sourceItemId: string,
    candidateVersion: string,
  ) => {
    const instance = instances.find((candidate) => candidate.instance_id === instanceId)
      ?? versionsTabInstances.find((candidate) => candidate.instance_id === instanceId);
    setCanonicalInstall({
      instanceName: instance?.name ?? instanceId,
      intent: {
        action: {
          type: 'install',
          sourceType,
          itemId: sourceItemId,
          candidateVersion,
        },
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
    setShowInstallFlow(false);
  };

  const handleConfirmInstall = () => {
    if (!selectedInstanceId) return;
    if (isModrinthInstall && selectedModrinthCandidate && item.modrinth_id) {
      openCanonicalInstall(
        selectedInstanceId,
        'modrinth',
        item.modrinth_id,
        selectedModrinthCandidate.version_id,
      );
      return;
    }
    if (selectedCandidate) {
      openCanonicalInstall(selectedInstanceId, 'curated', itemId, selectedCandidate.version);
    }
  };

  const handleCloseInstallFlow = () => {
    setShowInstallFlow(false);
    setPhase('idle');
    setInstallMsg(null);
    setSelectedInstanceId(null);
    setCandidates([]);
    setModrinthCandidates([]);
    setSelectedCandidate(null);
    setSelectedModrinthCandidate(null);
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

  // Flag handler
  const handleFlagReview = async (review: ModReview) => {
    if (!authed) return;
    if (!window.confirm(
      `Flag this review?\n\nAuthor: ${review.author ?? 'Anonymous'}\nText: ${review.text.slice(0, 200)}${review.text.length > 200 ? '…' : ''}`
    )) {
      return;
    }
    setFlaggingId(review.issue_number);
    setFlagResult(null);
    setFlagError(null);
    try {
      const login = profile?.login ?? '';
      const url = await flagReview({
        modId: item.id,
        modName: item.name,
        issueNumber: review.issue_number,
        author: review.author ?? 'Anonymous',
        quotedText: review.text,
        reporterLogin: login,
      });
      if (url.startsWith('https://')) {
        window.open(url, '_blank');
      }
      setFlagResult(url);
    } catch (e) {
      setFlagError(formatError(e));
    } finally {
      setFlaggingId(null);
    }
  };

  // Versions tab uses the same canonical plan and executor as the primary action.
  const handleInstallVersionFromTab = () => {
    if (!versionsTabInstanceId) return;
    if (selectedGithubTabVersion) {
      openCanonicalInstall(
        versionsTabInstanceId,
        'curated',
        itemId,
        selectedGithubTabVersion.version,
      );
      return;
    }
    if (selectedVersion && item.modrinth_id) {
      openCanonicalInstall(
        versionsTabInstanceId,
        'modrinth',
        item.modrinth_id,
        selectedVersion.version_id,
      );
    }
  };

  const hasModrinthId = !!item.modrinth_id;
  const canShowModrinthVersions = hasModrinthId && modrinthEnabled !== false;
  const canShowGithubVersions = !hasModrinthId || (hasModrinthId && modrinthEnabled === false && item.download_strategy === 'github_release');

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

      <section className="rounded-xl border border-border bg-card p-6">
        <div className="flex items-start gap-4">
          {(modrinthProject?.icon_url || showIcon) && (
            <img
              src={modrinthProject?.icon_url ?? (item.icon_url as string)}
              alt={modrinthProject?.title ?? item.name}
              className="h-16 w-16 rounded-lg border object-contain dark:border-border"
            />
          )}
          <div className="flex-1 min-w-0">
            <div className="flex items-center gap-2 flex-wrap">
              <h2 className="text-2xl font-bold break-words">
                {modrinthProject?.title ?? item.name}
              </h2>
              {modrinthProject && (
                <span className="rounded-full bg-primary px-2 py-0.5 text-xs font-medium uppercase tracking-wide text-primary-foreground">
                  {item.content_type}
                </span>
              )}
              {curatedAnnotation && (
                <span className="rounded-full border border-amber-500 bg-amber-50 px-2 py-0.5 text-xs font-semibold uppercase tracking-wide text-amber-800 dark:bg-amber-900/30 dark:text-amber-300 dark:border-amber-600">
                  Agora Curated
                </span>
              )}
              {!modrinthProject && (
                <span className="rounded-full border border-border px-2 py-0.5 text-xs text-muted-foreground">
                  {item.download_strategy}
                </span>
              )}
              {item.status && item.status !== 'active' && (
                <span className="rounded-full border border-border px-2 py-0.5 text-xs text-muted-foreground">
                  {item.status}
                </span>
              )}
            </div>
            <p className="text-xs text-muted-foreground mt-1 break-all">
              {modrinthProject ? item.id : item.source_identifier}
            </p>
            {modrinthProject?.description && (
              <p className="text-sm text-foreground mt-2">{modrinthProject.description}</p>
            )}
            {!modrinthProject && item.description && (
              <p className="text-sm text-foreground mt-2">{item.description}</p>
            )}
            <p className="text-xs text-muted-foreground mt-3">
              ↑ {item.upvotes} · ↓ {item.downvotes} · net {item.net_score} · velocity {velocityLabel}
            </p>
            {item.date_added && (
              <p className="text-xs text-muted-foreground mt-1">
                Added {item.date_added}
              </p>
            )}
            <p className="text-xs text-muted-foreground mt-2 flex flex-wrap gap-x-3 gap-y-1">
              {modrinthProject?.license_id ? <span>License: {modrinthProject.license_id}</span> : item.license_id ? <span>License: {item.license_id}</span> : null}
              {modrinthProject?.source_updated_at ? <span>Updated {modrinthProject.source_updated_at.slice(0, 10)}</span> : item.source_updated_at ? <span>Source updated {item.source_updated_at.slice(0, 10)}</span> : null}
              {(modrinthProject?.page_url || item.page_url) && (
                <a
                  href={modrinthProject?.page_url ?? item.page_url!}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="text-primary hover:underline dark:text-primary"
                >
                  View on Modrinth ↗
                </a>
              )}
            </p>
          </div>
        </div>

        <div className="mt-5 flex flex-wrap gap-2">
          {item.content_type === 'pack' ? (
            <button
              onClick={() => setShowPackCreate(true)}
              className="rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90"
            >
              Create Instance from Pack
            </button>
          ) : (
            <button
              onClick={handleInstall}
              className="rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90"
            >
              Install to Instance
            </button>
          )}
        </div>
        {showInstallFlow && (
          <section className="mt-4 rounded-xl border border-border bg-card p-4 space-y-4">
            <div className="flex items-center justify-between">
              <h3 className="font-semibold text-sm">Install to Instance</h3>
              <button
                onClick={handleCloseInstallFlow}
                className="text-xs text-muted-foreground hover:text-foreground"
              >
                Close
              </button>
            </div>

            {phase === 'error' && installMsg && (
              <p className="text-sm text-destructive">{installMsg}</p>
            )}

            {/* Step 1: Instance picker */}
            {phase === 'idle' && (
              instancesLoading ? (
                <div className="text-center py-2">
                  <p className="text-sm text-muted-foreground">Loading instances…</p>
                </div>
              ) : (
                <div>
                  <label className="block text-xs font-medium mb-1">Select instance</label>
                  <select
                    value={selectedInstanceId ?? ''}
                    onChange={(e) => setSelectedInstanceId(e.target.value || null)}
                    className="w-full rounded-lg border border-border bg-background px-3 py-2 text-sm"
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
                    className="mt-3 rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
                  >
                    Next: Choose Version
                  </button>
                  <button
                    onClick={() => setShowCreateInline(true)}
                    className="mt-2 block text-xs text-primary hover:underline dark:text-primary"
                  >
                    + Create new instance
                  </button>
                  {showCreateInline && (
                    <div className="mt-3 space-y-3 rounded-lg border border-border bg-muted p-3">
                      <p className="text-xs font-medium">Create new instance</p>
                      <label className="block">
                        <span className="text-xs">Instance name</span>
                        <input
                          value={createName}
                          onChange={(e) => setCreateName(e.target.value)}
                          placeholder="My Instance"
                          className="mt-1 w-full rounded-lg border border-border bg-background px-3 py-2 text-sm"
                        />
                      </label>
                      <div className="grid grid-cols-2 gap-3">
                        <label className="block">
                          <span className="text-xs">Minecraft version</span>
                          <select
                            value={createMcVersion}
                            onChange={(e) => setCreateMcVersion(e.target.value)}
                            className="mt-1 w-full rounded-lg border border-border bg-background px-3 py-2 text-sm"
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
                            className="mt-1 w-full rounded-lg border border-border bg-background px-3 py-2 text-sm"
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
                          className="mt-1 w-full rounded-lg border border-border bg-background px-3 py-2 text-sm"
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
                        <p className="text-xs text-destructive">{createError}</p>
                      )}
                      <div className="flex gap-2">
                        <button
                          onClick={() => {
                            setShowCreateInline(false);
                            setCreateError(null);
                          }}
                          disabled={createBusy}
                          className="rounded-lg border border-border px-3 py-1.5 text-xs font-medium hover:bg-accent"
                        >
                          Cancel
                        </button>
                        <button
                          onClick={handleCreateInstance}
                          disabled={createBusy}
                          className="rounded-lg bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
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
                    <svg className="animate-spin h-5 w-5 mx-auto text-muted-foreground" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
                      <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                      <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z" />
                    </svg>
                    <p className="text-xs text-muted-foreground mt-2">Loading versions…</p>
                  </div>
                ) : isModrinthInstall ? (
                  <ul className="space-y-2 max-h-80 overflow-y-auto">
                    {modrinthCandidates.map((cand) => (
                      <li
                        key={cand.version_id}
                        className={`rounded-lg border px-3 py-2 text-sm cursor-pointer transition-colors ${
                          selectedModrinthCandidate?.version_id === cand.version_id
                            ? 'border-primary bg-card/50 dark:bg-card/20'
                            : 'border-border hover:bg-accent'
                        }`}
                        onClick={() => setSelectedModrinthCandidate(cand)}
                      >
                        <div className="flex items-center justify-between gap-2">
                          <span className="font-medium truncate">{cand.version}</span>
                          {cand.primary && (
                            <span className="text-[10px] uppercase tracking-wide text-muted-foreground">primary</span>
                          )}
                        </div>
                        <p className="text-xs text-muted-foreground mt-0.5 truncate">{cand.filename}</p>
                        <p className="text-xs text-muted-foreground mt-0.5">
                          {cand.mc_versions.join(', ') || '—'}
                          {' · '}
                          {cand.loaders.join(', ') || '—'}
                          {cand.release_date ? ` · ${cand.release_date.slice(0, 10)}` : ''}
                        </p>
                        {cand.sha1 ? (
                          <p className="text-[10px] text-green-600 dark:text-green-400 mt-0.5">
                            SHA-1: {cand.sha1.slice(0, 12)}…
                          </p>
                        ) : (
                          <p className="text-[10px] text-destructive mt-0.5">
                            No SHA-1 published — install refused
                          </p>
                        )}
                      </li>
                    ))}
                  </ul>
                ) : (
                  <ul className="space-y-2 max-h-80 overflow-y-auto">
                    {candidates.map((cand, idx) => (
                      <li
                        key={idx}
                        className={`rounded-lg border px-3 py-2 text-sm cursor-pointer transition-colors ${
                          selectedCandidate?.filename === cand.filename
                            ? 'border-primary bg-card/50 dark:bg-card/20'
                            : 'border-border hover:bg-accent'
                        }`}
                        onClick={() => setSelectedCandidate(cand)}
                      >
                        <div className="flex items-center justify-between">
                          <span className="font-medium">{cand.version}</span>
                          {cand.version_compat === 'compatible' ? (
                            <span className="text-xs text-green-600 dark:text-green-400">✓ compatible</span>
                          ) : cand.version_compat === 'major_match' ? (
                            <span className="text-xs text-yellow-600 dark:text-yellow-400">⚠ may not match your exact version</span>
                          ) : (
                            <span className="text-xs text-muted-foreground">may not match your instance</span>
                          )}
                        </div>
                        <p className="text-xs text-muted-foreground mt-0.5 truncate">{cand.filename}</p>
                        <p className="text-xs text-muted-foreground mt-0.5">
                          {[cand.mc_version, cand.loader].filter(Boolean).join(' · ')}
                          {cand.release_date ? ` · ${cand.release_date}` : ''}
                        </p>
                      </li>
                    ))}
                  </ul>
                )}
                {hasMoreVersions && !isModrinthInstall && (
                  <div ref={versionSentinelRef} className="py-3 text-center text-xs text-muted-foreground">
                    {loadingMoreVersions ? 'Loading more versions…' : ''}
                  </div>
                )}
                {(selectedCandidate || selectedModrinthCandidate) && (
                  <button
                    onClick={handleConfirmInstall}
                    disabled={!!(isModrinthInstall && selectedModrinthCandidate && !selectedModrinthCandidate.sha1)}
                    className="mt-3 rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
                  >
                    Install {(selectedCandidate ?? selectedModrinthCandidate)!.filename}
                  </button>
                )}
              </div>
            )}

            {/* Empty versions */}
            {selectedInstanceId && phase === 'pickingVersion' && (
              isModrinthInstall ? (
                modrinthCandidates.length === 0 && (
                  <p className="text-sm text-muted-foreground">No compatible versions found.</p>
                )
              ) : (
                candidates.length === 0 && (
                  <p className="text-sm text-muted-foreground">No compatible versions found.</p>
                )
              )
            )}

            {/* Step 3: Installing */}
            {phase === 'installing' && (
              <div className="text-center py-4">
                <svg className="animate-spin h-5 w-5 mx-auto text-muted-foreground" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
                  <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                  <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z" />
                </svg>
                <p className="text-xs text-muted-foreground mt-2">Downloading &amp; verifying…</p>
              </div>
            )}

            {/* Step 3: Done */}
            {phase === 'done' && installMsg && (
              <p className="text-sm text-green-600 dark:text-green-400">{installMsg}</p>
            )}
          </section>
        )}

        {canonicalInstall && (
          <InstallFlow
            open
            intent={canonicalInstall.intent}
            instanceName={canonicalInstall.instanceName}
            onOpenInstance={onOpenInstanceEditor}
            onClose={() => setCanonicalInstall(null)}
          />
        )}

        {/* Pack-create dialog */}
        {showPackCreate && (
          <PackCreateDialog
            item={item}
            onCancel={() => setShowPackCreate(false)}
            onCreated={(newInstanceId) => {
              setShowPackCreate(false);
              onOpenInstanceEditor?.(newInstanceId);
            }}
          />
        )}
      </section>

      {/* Tab bar */}
      <div className="flex gap-1 border-b border-border">
        {([
          { key: 'description' as const, label: 'About' },
          { key: 'gallery' as const, label: 'Gallery' },
          { key: 'versions' as const, label: 'Versions' },
          ...(curatedAnnotation ? [{ key: 'agora' as const, label: 'Agora' }] : []),
        ] as const).map((tab) => (
          <button
            key={tab.key}
            onClick={() => setActiveTab(tab.key)}
            className={`px-4 py-2 text-sm font-medium border-b-2 transition-colors ${
              activeTab === tab.key
                ? 'border-primary text-primary'
                : 'border-transparent text-muted-foreground hover:text-foreground hover:border-border'
            } ${tab.key === 'agora' ? 'text-amber-700 dark:text-amber-400' : ''}`}
          >
            {tab.label}
          </button>
        ))}
      </div>

      {/* Tab content */}
      {activeTab === 'description' && (
        <section className="rounded-xl border border-border bg-card p-4 space-y-4">
          {modrinthProject && (
            <>
              {/* Modrinth body markdown */}
              {modrinthProject.body ? (
                <div>
                  <div className="flex items-center justify-between mb-3">
                    <h3 className="font-semibold text-sm">About</h3>
                    <span className="text-[10px] uppercase tracking-wide text-muted-foreground">
                      Source: Modrinth
                    </span>
                  </div>
                  <div className="prose prose-sm dark:prose-invert max-w-none text-foreground">
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
                      {modrinthProject.body}
                    </ReactMarkdown>
                  </div>
                </div>
              ) : (
                <p className="text-sm text-muted-foreground">No description available from Modrinth.</p>
              )}
            </>
          )}
          {!modrinthProject && (
            <>
              {/* Registry body markdown */}
              {item.body_markdown ? (
                <div>
                  <div className="flex items-center justify-between mb-3">
                    <h3 className="font-semibold text-sm">About</h3>
                    <span className="text-[10px] uppercase tracking-wide text-muted-foreground">
                      Source: upstream
                    </span>
                  </div>
                  <div className="prose prose-sm dark:prose-invert max-w-none text-foreground">
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
                </div>
              ) : item.description ? (
                <div>
                  <h3 className="font-semibold text-sm mb-2">About</h3>
                  <p className="text-sm text-foreground whitespace-pre-wrap">{item.description}</p>
                </div>
              ) : (
                <p className="text-sm text-muted-foreground">No information available.</p>
              )}
            </>
          )}
        </section>
      )}

      {activeTab === 'gallery' && (
        <section className="rounded-xl border border-border bg-card p-4 space-y-3">
          <h3 className="font-semibold text-sm">Gallery</h3>
          {((modrinthProject && modrinthProject.gallery_urls.length > 0) || galleryUrls.length > 0) ? (
            <div className="grid grid-cols-2 gap-3">
              {(modrinthProject ? modrinthProject.gallery_urls : galleryUrls).map((url, index) => (
                <img
                  key={index}
                  src={url}
                  alt={`${item.name} screenshot ${index + 1}`}
                  className="rounded-lg border border-border w-full h-48 object-cover"
                  loading="lazy"
                />
              ))}
            </div>
          ) : (
            <p className="text-sm text-muted-foreground">No gallery images available.</p>
          )}
        </section>
      )}

      {activeTab === 'versions' && (
        <section className="rounded-xl border border-border bg-card p-4">
          <h3 className="font-semibold text-sm mb-3">Versions</h3>

          {canShowModrinthVersions ? (
            versionsLoading ? (
              <p className="text-sm text-muted-foreground">Loading versions…</p>
            ) : versionsError ? (
              <p className="text-sm text-muted-foreground">{versionsError}</p>
            ) : modrinthVersions.length === 0 ? (
              <p className="text-sm text-muted-foreground">No versions published.</p>
            ) : (
              <div className="flex flex-col lg:flex-row gap-4">
                {/* Versions table */}
                <div className="flex-1 overflow-x-auto">
                  <table className="w-full text-sm border-collapse">
                    <thead>
                      <tr className="border-b border-border text-left text-xs text-muted-foreground">
                        <th className="py-2 pr-3 font-medium">Version</th>
                        <th className="py-2 pr-3 font-medium">MC Versions</th>
                        <th className="py-2 pr-3 font-medium">Loaders</th>
                        <th className="py-2 pr-3 font-medium">Released</th>
                      </tr>
                    </thead>
                    <tbody>
                      {modrinthVersions.map((v) => {
                        const isSelected = selectedVersion?.version_id === v.version_id;
                        return (
                          <tr
                            key={v.version_id}
                            onClick={() => {
                              setSelectedVersion(v);
                              setSelectedGithubTabVersion(null);
                            }}
                            className={`cursor-pointer border-b border-border/50 transition-colors ${
                              isSelected
                                ? 'bg-accent'
                                : 'hover:bg-accent'
                            }`}
                          >
                            <td className="py-2 pr-3 font-medium break-all">{v.name || v.version}</td>
                            <td className="py-2 pr-3 text-xs text-muted-foreground">{v.mc_versions.join(', ') || '—'}</td>
                            <td className="py-2 pr-3 text-xs text-muted-foreground">{v.loaders.join(', ') || '—'}</td>
                            <td className="py-2 pr-3 text-xs text-muted-foreground">{v.release_date ? v.release_date.slice(0, 10) : '—'}</td>
                          </tr>
                        );
                      })}
                    </tbody>
                  </table>
                </div>

                {/* Selected version detail panel */}
                {selectedVersion && (
                  <div className="space-y-3 rounded-lg border border-border bg-muted p-3 lg:w-80 lg:flex-shrink-0">
                    <div>
                      <p className="text-xs text-muted-foreground">Selected version</p>
                      <p className="font-semibold text-sm break-all">{selectedVersion.name || selectedVersion.version}</p>
                    </div>
                    <div className="text-xs text-muted-foreground space-y-1">
                      <p>Version: {selectedVersion.version}</p>
                      <p className="break-all">File: {selectedVersion.filename}</p>
                      <p>MC: {selectedVersion.mc_versions.join(', ') || '—'}</p>
                      <p>Loaders: {selectedVersion.loaders.join(', ') || '—'}</p>
                      {selectedVersion.release_date && (
                        <p>Released: {selectedVersion.release_date.slice(0, 10)}</p>
                      )}
                    </div>
                    {selectedVersion.changelog && (
                      <div>
                        <p className="text-xs font-medium mb-1">Changelog</p>
                        <div className="prose prose-sm dark:prose-invert max-w-none max-h-48 overflow-y-auto text-foreground text-xs">
                          <ReactMarkdown
                            rehypePlugins={[[rehypeRaw, { passThrough: ['html'] }], [rehypeSanitize, SANITIZE_SCHEMA]]}
                            components={{
                              a: ({ node, ...props }) => (
                                <a {...props} target="_blank" rel="noopener noreferrer" />
                              ),
                            }}
                          >
                            {selectedVersion.changelog}
                          </ReactMarkdown>
                        </div>
                      </div>
                    )}

                    {/* Install controls */}
                    <div className="pt-2 border-t border-border">
                      <label className="block text-xs font-medium mb-1">Install to instance</label>
                      <select
                        value={versionsTabInstanceId ?? ''}
                        onChange={(e) => setVersionsTabInstanceId(e.target.value || null)}
                        className="w-full rounded-lg border border-border bg-background px-2 py-1.5 text-xs mb-2"
                      >
                        <option value="">Choose an instance…</option>
                        {versionsTabInstances.map((inst) => (
                          <option key={inst.instance_id} value={inst.instance_id}>
                            {inst.name} ({inst.loader} · MC {inst.minecraft_version})
                          </option>
                        ))}
                      </select>
                      <button
                        onClick={handleInstallVersionFromTab}
                        disabled={!versionsTabInstanceId}
                        className="w-full rounded-lg bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
                      >
                        Review install plan
                      </button>
                    </div>
                  </div>
                )}
              </div>
            )
          ) : canShowGithubVersions ? (
            // Fallback to GitHub releases (no modrinth_id, or modrinth disabled + github_release strategy)
            githubTabLoading ? (
              <p className="text-sm text-muted-foreground">Loading versions…</p>
            ) : githubTabError ? (
              <p className="text-sm text-muted-foreground">{githubTabError}</p>
            ) : githubTabVersions.length === 0 ? (
              <p className="text-sm text-muted-foreground">No versions published.</p>
            ) : (
              <div className="flex flex-col lg:flex-row gap-4">
                {/* GitHub versions table */}
                <div className="flex-1 overflow-x-auto">
                  <table className="w-full text-sm border-collapse">
                    <thead>
                      <tr className="border-b border-border text-left text-xs text-muted-foreground">
                        <th className="py-2 pr-3 font-medium">Version</th>
                        <th className="py-2 pr-3 font-medium">MC Version</th>
                        <th className="py-2 pr-3 font-medium">Loader</th>
                        <th className="py-2 pr-3 font-medium">Released</th>
                      </tr>
                    </thead>
                    <tbody>
                      {githubTabVersions.map((v, idx) => {
                        const isSelected = selectedGithubTabVersion?.filename === v.filename
                          && selectedGithubTabVersion?.version === v.version;
                        return (
                          <tr
                            key={`${v.version}-${idx}`}
                            onClick={() => {
                              setSelectedGithubTabVersion(v);
                              setSelectedVersion(null);
                            }}
                            className={`cursor-pointer border-b border-border/50 transition-colors ${
                              isSelected
                                ? 'bg-accent'
                                : 'hover:bg-accent'
                            }`}
                          >
                            <td className="py-2 pr-3 font-medium break-all">{v.version}</td>
                            <td className="py-2 pr-3 text-xs text-muted-foreground">{v.mc_version || '—'}</td>
                            <td className="py-2 pr-3 text-xs text-muted-foreground">{v.loader || '—'}</td>
                            <td className="py-2 pr-3 text-xs text-muted-foreground">{v.release_date ? v.release_date.slice(0, 10) : '—'}</td>
                          </tr>
                        );
                      })}
                    </tbody>
                  </table>
                  {githubTabHasMore && (
                    <div ref={githubTabSentinelRef} className="py-3 text-center text-xs text-muted-foreground">
                      {githubTabLoadingMore ? 'Loading more versions…' : ''}
                    </div>
                  )}
                </div>

                {/* Selected version detail panel */}
                {selectedGithubTabVersion && (
                  <div className="space-y-3 rounded-lg border border-border bg-muted p-3 lg:w-80 lg:flex-shrink-0">
                    <div>
                      <p className="text-xs text-muted-foreground">Selected version</p>
                      <p className="font-semibold text-sm break-all">{selectedGithubTabVersion.version}</p>
                    </div>
                    <div className="text-xs text-muted-foreground space-y-1">
                      <p className="break-all">File: {selectedGithubTabVersion.filename}</p>
                      <p>MC: {selectedGithubTabVersion.mc_version || '—'}</p>
                      <p>Loader: {selectedGithubTabVersion.loader || '—'}</p>
                      {selectedGithubTabVersion.release_date && (
                        <p>Released: {selectedGithubTabVersion.release_date.slice(0, 10)}</p>
                      )}
                    </div>

                    {/* Install controls */}
                    <div className="pt-2 border-t border-border">
                      <label className="block text-xs font-medium mb-1">Install to instance</label>
                      <select
                        value={versionsTabInstanceId ?? ''}
                        onChange={(e) => setVersionsTabInstanceId(e.target.value || null)}
                        className="w-full rounded-lg border border-border bg-background px-2 py-1.5 text-xs mb-2"
                      >
                        <option value="">Choose an instance…</option>
                        {versionsTabInstances.map((inst) => (
                          <option key={inst.instance_id} value={inst.instance_id}>
                            {inst.name} ({inst.loader} · MC {inst.minecraft_version})
                          </option>
                        ))}
                      </select>
                      <button
                        onClick={handleInstallVersionFromTab}
                        disabled={!versionsTabInstanceId}
                        className="w-full rounded-lg bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
                      >
                        Review install plan
                      </button>
                    </div>
                  </div>
                )}
              </div>
            )
          ) : versionsError ? (
            <p className="text-sm text-muted-foreground">{versionsError}</p>
          ) : null}
        </section>
      )}

      {activeTab === 'agora' && (
        <section className="rounded-xl border border-amber-200/50 bg-card p-4 space-y-5 dark:border-amber-800/30">
          {/* Curator Note */}
          {curatedAnnotation && (
            <div>
              <h3 className="font-semibold text-sm flex items-center gap-2 mb-2">
                <span className="rounded-full bg-amber-500 px-2 py-0.5 text-[10px] font-bold uppercase tracking-wide text-white">
                  Agora Curated
                </span>
                {curatedAnnotation.net_score != null && (
                  <span className="text-xs text-muted-foreground">
                    net score: {curatedAnnotation.net_score.toFixed(1)}
                  </span>
                )}
              </h3>
              {curatedAnnotation.curator_note && (
                <p className="text-sm whitespace-pre-wrap text-foreground bg-amber-50/50 dark:bg-amber-900/10 rounded-lg border border-amber-200/50 dark:border-amber-800/30 p-3">
                  {curatedAnnotation.curator_note}
                </p>
              )}
              {curatedAnnotation.base_categories.length > 0 && (
                <div className="flex flex-wrap gap-1.5 mt-2">
                  {curatedAnnotation.base_categories.map((cat) => (
                    <span
                      key={cat}
                      className="rounded-full border border-amber-300/50 dark:border-amber-700/50 px-2 py-0.5 text-xs text-muted-foreground"
                    >
                      {cat}
                    </span>
                  ))}
                </div>
              )}
            </div>
          )}

          {curatorNotes && (
            <div>
              <h3 className="font-semibold text-sm mb-2">Registry Curator Notes</h3>
              <p className="text-sm whitespace-pre-wrap text-muted-foreground">{curatorNotes}</p>
            </div>
          )}

          <div>
            <h3 className="font-semibold text-sm mb-2">Known Conflicts</h3>
            <p className="text-sm text-muted-foreground">
              Agora checks curated conflicts and the target instance&apos;s dependency graph in the install plan before any download begins.
            </p>
          </div>

          {/* Curated score */}
          <div>
            <h3 className="font-semibold text-sm mb-2">Curated Score</h3>
            <p className="text-xs text-muted-foreground">
              ↑ {item.upvotes} · ↓ {item.downvotes} · net {item.net_score} · velocity {velocityLabel}
            </p>
          </div>

          {/* Reviews */}
          <div className="pt-2 border-t border-border">
            <h3 className="font-semibold text-sm mb-3">Community Reviews</h3>
            {item.allow_comments ? (
              !authed ? (
                <>
                  <p className="text-sm text-muted-foreground mb-2">
                    Reviews require GitHub authentication.
                  </p>
                  <p className="text-xs text-muted-foreground">
                    Sign in to flag reviews.
                  </p>
                </>
              ) : reviewsLoading ? (
                <div className="text-center py-2">
                  <p className="text-sm text-muted-foreground">Loading reviews…</p>
                </div>
              ) : reviews.length === 0 ? (
                <p className="text-sm text-muted-foreground">
                  No community reviews yet.
                </p>
              ) : (
                <>
                  {flagResult && (
                    <p className="text-sm text-green-600 dark:text-green-400 mb-3">
                      Flag submitted.{' '}
                      <a
                        href={flagResult}
                        onClick={(e) => {
                          if (!flagResult.startsWith('https://')) {
                            e.preventDefault();
                          }
                        }}
                        target="_blank"
                        rel="noopener noreferrer"
                        className="underline"
                      >
                        View admin alert ↗
                      </a>
                    </p>
                  )}
                  {flagError && (
                    <p className="text-sm text-destructive mb-3">
                      {flagError}
                    </p>
                  )}
                  <ul className="space-y-3">
                    {reviews.map((review) => {
                      const rl = rateLimit;
                      const disabledFlag = rl && !rl.can_flag;
                      const resetTime = disabledFlag
                        ? new Date(rl.reset_hour_at_unix * 1000).toLocaleString()
                        : '';
                      return (
                        <li
                          key={review.issue_number}
                          className="rounded-lg border border-border px-3 py-2"
                        >
                          <div className="flex items-center justify-between gap-2">
                            <span className="text-xs font-medium text-muted-foreground">
                              {review.author ?? 'Anonymous'}
                            </span>
                            {review.created_at && (
                              <span className="text-xs text-muted-foreground">
                                {new Date(review.created_at).toLocaleString()}
                              </span>
                            )}
                            <button
                              onClick={() => handleFlagReview(review)}
                              disabled={disabledFlag || flaggingId === review.issue_number}
                              title={disabledFlag ? `Flag limit reached — resets at ${resetTime}` : ''}
                              className="text-xs text-muted-foreground hover:text-destructive disabled:opacity-40 disabled:cursor-not-allowed"
                            >
                              🚩 Flag
                            </button>
                          </div>
                          <p className="text-sm mt-1 whitespace-pre-wrap text-foreground">
                            {review.text}
                          </p>
                        </li>
                      );
                    })}
                  </ul>
                </>
              )
            ) : (
              <p className="text-sm text-muted-foreground">
                Reviews are disabled for this mod.
              </p>
            )}
          </div>
        </section>
      )}
    </div>
  );
}

function BackButton({ onBack }: { onBack: () => void }) {
  return (
    <button
      onClick={onBack}
      className="rounded-lg border border-border px-3 py-1.5 text-sm font-medium hover:bg-accent"
    >
      ← Back
    </button>
  );
}

type PackInstallModProgress = {
  modId: string;
  status: 'pending' | 'installing' | 'done' | 'failed';
  error?: string;
};

type PackInstallProgressEvent = {
  operationId: string;
  phase: string;
  message: string;
  progress?: number | null;
  step?: number | null;
  totalSteps?: number | null;
  bytesDownloaded?: number | null;
  bytesTotal?: number | null;
};

function formatPackBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

function packProgressLabel(phase: string): string {
  switch (phase) {
    case 'downloading': return 'Downloading pack and mod files';
    case 'extracting': return 'Extracting pack files';
    case 'verifying': return 'Verifying pack metadata';
    case 'installing': return 'Installing modloader';
    case 'health-scan': return 'Checking pack health';
    case 'snapshotting': return 'Creating recovery snapshot';
    case 'done': return 'Finishing installation';
    default: return 'Preparing installation';
  }
}

const isModrinthPack = (item: RegistryItem): boolean =>
  item.download_strategy === 'modrinth_id' || !!item.modrinth_id;

function PackCreateDialog({
  item,
  onCancel,
  onCreated,
}: {
  item: RegistryItem;
  onCancel: () => void;
  onCreated: (instanceId: string) => void;
}) {
  const isModrinth = isModrinthPack(item);
  const packName = item.name;
  const [name, setName] = useState(packName);
  const [mcVersion, setMcVersion] = useState('');
  const [availableLoaders, setAvailableLoaders] = useState<string[]>([]);
  const [availableMcVersions, setAvailableMcVersions] = useState<string[]>([]);
  const [loader, setLoader] = useState('fabric');
  const [loaderVersions, setLoaderVersions] = useState<import('../lib/tauri').LoaderVersionSummary[]>([]);
  const [loaderVersion, setLoaderVersion] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [installPhase, setInstallPhase] = useState<'form' | 'installing' | 'done'>('form');
  const [modProgress, setModProgress] = useState<PackInstallModProgress[]>([]);
  const [createdInstanceId, setCreatedInstanceId] = useState<string | null>(null);
  const [canonicalInstall, setCanonicalInstall] = useState<InstallIntent | null>(null);
  const [packInstallProgress, setPackInstallProgress] = useState<PackInstallProgressEvent | null>(null);

  useEffect(() => {
    if (!isModrinth) return;
    let disposed = false;
    const unlisten = listen<PackInstallProgressEvent>('pack-install-progress', (event) => {
      if (!disposed) setPackInstallProgress(event.payload);
    });
    return () => {
      disposed = true;
      void unlisten.then((remove) => remove());
    };
  }, [isModrinth]);

  // Modrinth pack version selection
  const [modrinthVersions, setModrinthVersions] = useState<RawModrinthVersionCandidate[]>([]);
  const [selectedVersionIdx, setSelectedVersionIdx] = useState(-1);

  // Fetch Modrinth versions on mount
  useEffect(() => {
    if (!isModrinth) return;
    let cancelled = false;
    (async () => {
      try {
        const projectId = item.modrinth_id || item.id;
        const versions = await listRawModrinthVersions(null, projectId, 'modpack');
        if (cancelled) return;
        setModrinthVersions(versions);
        if (versions.length > 0) setSelectedVersionIdx(0);
      } catch (e) {
        if (!cancelled) setError(formatError(e));
      }
    })();
    return () => { cancelled = true; };
  }, [isModrinth, item.modrinth_id, item.id]);

  // When Modrinth version changes, update MC version + loader presets
  useEffect(() => {
    if (!isModrinth || selectedVersionIdx < 0) return;
    const ver = modrinthVersions[selectedVersionIdx];
    if (!ver) return;
    if (ver.mc_versions.length > 0) setMcVersion(ver.mc_versions[0]);
    // Pick first known loader from version data
    const knownLoader = ver.loaders.find((l) =>
      ['fabric', 'quilt', 'forge', 'neoforge'].includes(l)
    );
    if (knownLoader) setLoader(knownLoader);
  }, [isModrinth, selectedVersionIdx, modrinthVersions]);

  // Loader version fetcher
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

  useEffect(() => {
    let cancelled = false;
    (async () => {
      if (!loader) return;
      try {
        const filtered = await listManifestMcVersions(loader);
        if (cancelled) return;
        setAvailableMcVersions(filtered.length > 0 ? filtered : availableMcVersions);
        if (filtered.length > 0 && !filtered.includes(mcVersion)) {
          setMcVersion(filtered[0]);
        }
      } catch {
      }
    })();
    return () => { cancelled = true; };
  }, [loader]);

  const submitCurated = async (instanceId: string) => {
    const mods: PackModRow[] = await listPackMods(item.id);
    if (mods.length === 0) {
      throw new Error('No mods found for this pack in the registry.');
    }
    setModProgress(mods.map((mod) => ({ modId: mod.mod_id, status: 'pending' as const })));
    const items: BatchInstallItem[] = [];

    for (let index = 0; index < mods.length; index += 1) {
      const mod = mods[index];
      setModProgress((previous) =>
        previous.map((progress, current) =>
          current === index ? { ...progress, status: 'installing' as const } : progress
        )
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
        setModProgress((previous) =>
          previous.map((progress, current) =>
            current === index ? { ...progress, status: 'done' as const } : progress
          )
        );
      } catch (cause) {
        setModProgress((previous) =>
          previous.map((progress, current) =>
            current === index
              ? { ...progress, status: 'failed' as const, error: formatError(cause) }
              : progress
          )
        );
        throw new Error(
          `Could not resolve every pack item. No pack files were installed: ${formatError(cause)}`,
        );
      }
    }

    setCanonicalInstall({
      action: { type: 'batch-install', items },
      targetInstance: instanceId,
      optionalDeps: { type: 'prompt' },
      requestedBy: 'interactive',
      overrides: {
        allowReplace: false,
        skipHealthScan: false,
        forceConflictResolution: {},
      },
    });
  };

  const submitModrinth = async () => {
    if (selectedVersionIdx < 0) {
      throw new Error('Select a pack version first.');
    }
    const ver = modrinthVersions[selectedVersionIdx];
    if (!ver.download_url) {
      throw new Error('Selected version has no downloadable file.');
    }
    const newId = await importModrinthPackByUrl(ver.download_url);
    setCreatedInstanceId(newId);
  };

  const submit = async () => {
    setBusy(true);
    setError(null);
    setPackInstallProgress(null);
    try {
      setInstallPhase('installing');

      if (isModrinth) {
        await submitModrinth();
        setInstallPhase('done');
      } else {
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
        const createdId = result.instance_id;
        setCreatedInstanceId(createdId);
        await submitCurated(createdId);
      }
    } catch (e) {
      setError(formatError(e));
    } finally {
      setBusy(false);
    }
  };

  const handleDone = () => {
    if (createdInstanceId) {
      onCreated(createdInstanceId);
    }
  };

  const selectedModrinthVer = selectedVersionIdx >= 0 ? modrinthVersions[selectedVersionIdx] : null;

  return (
    <div className="fixed inset-0 z-40 flex items-center justify-center bg-black/40 p-4">
      <div className="w-full max-w-lg rounded-2xl border border-border bg-card p-6 shadow-xl">
        {installPhase === 'form' && (
          <>
            <h3 className="text-lg font-bold mb-4">
              {isModrinth ? 'Install Modrinth Pack' : 'Create Instance from Pack'}: {packName}
            </h3>
            <div className="space-y-4">
              {isModrinth && modrinthVersions.length > 0 && (
                <label className="block">
                  <span className="text-sm font-medium">Pack Version</span>
                  <select
                    value={selectedVersionIdx}
                    onChange={(e) => setSelectedVersionIdx(Number(e.target.value))}
                    className="mt-1 w-full rounded-lg border border-border bg-background px-3 py-2 text-sm"
                  >
                    {modrinthVersions.map((v, idx) => (
                      <option key={v.version_id} value={idx}>
                        {v.name || v.version} {v.mc_versions.length > 0 ? `(MC ${v.mc_versions.join(', ')})` : ''}
                      </option>
                    ))}
                  </select>
                  {selectedModrinthVer && selectedModrinthVer.loaders.length > 0 && (
                    <p className="text-xs text-muted-foreground mt-1">
                      Loader: {selectedModrinthVer.loaders.join(', ')}
                    </p>
                  )}
                </label>
              )}

              {!isModrinth && (
                <>
                  <label className="block">
                    <span className="text-sm font-medium">Instance name</span>
                    <input
                      value={name}
                      onChange={(e) => setName(e.target.value)}
                      className="mt-1 w-full rounded-lg border border-border bg-background px-3 py-2 text-sm"
                    />
                  </label>

                  <div className="grid grid-cols-2 gap-4">
                    <label className="block">
                      <span className="text-sm font-medium">Minecraft version</span>
                      <select
                        value={mcVersion}
                        onChange={(e) => setMcVersion(e.target.value)}
                        className="mt-1 w-full rounded-lg border border-border bg-background px-3 py-2 text-sm"
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
                        className="mt-1 w-full rounded-lg border border-border bg-background px-3 py-2 text-sm"
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
                      className="mt-1 w-full rounded-lg border border-border bg-background px-3 py-2 text-sm"
                    >
                      {loaderVersions.length === 0 && <option value="">No pinned versions</option>}
                      {loaderVersions.map((v) => (
                        <option key={v.loader_version} value={v.loader_version}>
                          {v.loader_version} ({v.file_type})
                        </option>
                      ))}
                    </select>
                  </label>
                </>
              )}
            </div>

            {error && (
              <p className="mt-4 text-sm text-destructive">{error}</p>
            )}

            <div className="mt-6 flex justify-end gap-2">
              <button
                onClick={onCancel}
                disabled={busy}
                className="rounded-lg border border-border px-4 py-2 text-sm font-medium hover:bg-accent"
              >
                Cancel
              </button>
              <button
                onClick={submit}
                disabled={busy}
                className="rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
              >
                {busy ? 'Installing…' : isModrinth ? 'Install' : 'Create'}
              </button>
            </div>
          </>
        )}

        {installPhase === 'installing' && !error && (
          <div className="py-8 text-center">
            <div className="flex items-center justify-center gap-3">
              <div className="h-5 w-5 animate-spin rounded-full border-2 border-primary border-t-transparent" />
              <p className="text-lg font-medium">
                {isModrinth
                  ? packProgressLabel(packInstallProgress?.phase ?? 'resolving')
                  : 'Installing pack mods…'}
              </p>
            </div>
            {isModrinth && packInstallProgress && (() => {
              const bytesDownloaded = packInstallProgress.bytesDownloaded ?? 0;
              const bytesTotal = packInstallProgress.bytesTotal ?? 0;
              const hasBytes = bytesTotal > 0;
              const step = packInstallProgress.step ?? 0;
              const totalSteps = packInstallProgress.totalSteps ?? 0;
              const hasSteps = totalSteps > 0;
              const rawProgress = packInstallProgress.progress
                ?? (hasBytes ? bytesDownloaded / bytesTotal : hasSteps ? step / totalSteps : null);
              const percent = rawProgress === null
                ? null
                : Math.max(0, Math.min(100, Math.round(rawProgress * 100)));
              return (
                <div className="mt-5 space-y-2 text-left" aria-live="polite">
                  <div
                    className="h-2.5 overflow-hidden rounded-full bg-muted"
                    role="progressbar"
                    aria-label="Pack installation progress"
                    aria-valuemin={0}
                    aria-valuemax={100}
                    aria-valuenow={percent ?? undefined}
                  >
                    <div
                      className={percent === null ? 'h-full w-1/3 animate-pulse rounded-full bg-primary' : 'h-full rounded-full bg-primary transition-all duration-300'}
                      style={percent === null ? undefined : { width: `${percent}%` }}
                    />
                  </div>
                  <div className="flex justify-between text-xs text-muted-foreground">
                    <span>{percent === null ? 'Working…' : `${percent}%`}</span>
                    {hasBytes ? (
                      <span>{formatPackBytes(bytesDownloaded)} / {formatPackBytes(bytesTotal)}</span>
                    ) : hasSteps ? (
                      <span>File {Math.min(step, totalSteps)} of {totalSteps}</span>
                    ) : null}
                  </div>
                  <p className="truncate text-sm font-medium" title={packInstallProgress.message}>
                    {packInstallProgress.message}
                  </p>
                </div>
              );
            })()}
            {modProgress.length > 0 && (
              <div className="mt-4 space-y-1 max-h-64 overflow-y-auto text-left">
                {modProgress.map((p, idx) => {
                  const icon =
                    p.status === 'done' ? '✓'
                    : p.status === 'failed' ? '✗'
                    : p.status === 'installing' ? '⏳'
                    : '○';
                  const statusText =
                    p.status === 'done' ? 'installed'
                    : p.status === 'failed' ? p.error ?? 'failed'
                    : p.status === 'installing' ? 'installing…'
                    : 'pending';
                  const lineColor =
                    p.status === 'done' ? 'text-green-600 dark:text-green-400'
                    : p.status === 'failed' ? 'text-destructive'
                    : p.status === 'installing' ? 'text-yellow-600 dark:text-yellow-400'
                    : 'text-muted-foreground';
                  return (
                    <div key={idx} className={`text-sm ${lineColor}`}>
                      <span className="inline-block w-5 text-center">{icon}</span>{' '}
                      <span className="font-medium">{p.modId}</span> — {statusText}
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        )}

        {installPhase === 'installing' && error && (
          <div className="py-8 text-center">
            <p className="text-lg font-medium text-destructive">Installation Failed</p>
            <p className="text-sm text-muted-foreground mt-2">{error}</p>
            <button
              onClick={() => setInstallPhase('form')}
              className="mt-4 rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90"
            >
              Back
            </button>
          </div>
        )}

        {installPhase === 'done' && (
          <>
            <h3 className="text-lg font-bold mb-4">Installation Complete: {packName}</h3>
            {modProgress.length > 0 ? (
              (() => {
                const done = modProgress.filter((p) => p.status === 'done').length;
                const failed = modProgress.filter((p) => p.status === 'failed');
                if (failed.length === 0) {
                  return <p className="text-sm text-green-600 dark:text-green-400">Installed {done} mod{done !== 1 ? 's' : ''} successfully.</p>;
                }
                return (
                  <>
                    <p className="text-sm text-yellow-600 dark:text-yellow-400">
                      Installed {done} of {modProgress.length} mods. {failed.length} failed:
                    </p>
                    <ul className="mt-1 text-xs text-destructive space-y-0.5">
                      {failed.map((f, idx) => (
                        <li key={idx}>• {f.modId}: {f.error}</li>
                      ))}
                    </ul>
                  </>
                );
              })()
            ) : (
              <p className="text-sm text-green-600 dark:text-green-400">Pack installed successfully.</p>
            )}

            {error && (
              <p className="mt-4 text-sm text-destructive">{error}</p>
            )}

            <div className="mt-6 flex justify-end gap-2">
              <button
                onClick={handleDone}
                className="rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90"
              >
                Open Instance Editor
              </button>
            </div>
          </>
        )}
      </div>
      {canonicalInstall && createdInstanceId && (
        <InstallFlow
          open
          intent={canonicalInstall}
          instanceName={name || createdInstanceId}
          onOpenInstance={onCreated}
          onClose={() => {
            setCanonicalInstall(null);
            onCreated(createdInstanceId);
          }}
        />
      )}
    </div>
  );
}
