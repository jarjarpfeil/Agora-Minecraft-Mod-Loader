import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Check, ChevronDown, Leaf, List, LayoutGrid, Search } from 'lucide-react';
import {
  browseSearch,
  browseLoadMore,
  batchCheckCompat,
  forYouItems,
  formatError,
  getInstanceDetail,
  getSetting,
  listCategories,
  listInstances,
  listManifestLoaders,
  listManifestMcVersions,
  listModrinthCategories,
  type BrowseItemCached,
  type CategoryInfo,
  type RegistryItem,
  type SortOption,
  type ModrinthSearchResult,
  type ModrinthCategoryInfo,
  type InstanceDetail,
  type InstanceRow,
} from '../lib/tauri';
import { useRegistryState } from '../lib/useRegistryState';
import { RegistryStatusView } from '../components/registry-status-view';
import { agoraRepositoryUrl } from '../lib/brandConfig';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '../components/ui/dropdown-menu';

function useDebounce<T>(value: T, delay: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const id = setTimeout(() => setDebounced(value), delay);
    return () => clearTimeout(id);
  }, [value, delay]);
  return debounced;
}

const SORTS: { label: string; value: SortOption }[] = [
  { label: 'For You', value: 'for_you' },
  { label: 'Net Score', value: 'net_score' },
  { label: 'Trending', value: 'velocity' },
  { label: 'Newest', value: 'newest' },
  { label: 'Most Upvoted', value: 'most_upvoted' },
  { label: 'Most Downvoted', value: 'most_downvoted' },
];

const CONTENT_TYPES = ['mod', 'pack', 'shader', 'resourcepack', 'server', 'datapack', 'world'];

const MODRINTH_PROJECT_TYPES: Partial<Record<string, string>> = {
  mod: 'mod',
  pack: 'modpack',
  shader: 'shader',
  resourcepack: 'resourcepack',
  server: 'minecraft_java_server',
  datapack: 'datapack',
};

const SUPPORTED_MODRINTH_PROJECT_TYPES = new Set(Object.values(MODRINTH_PROJECT_TYPES));

function formatCategoryName(value: string): string {
  return value.replace(/[-_]/g, ' ').replace(/\b\w/g, (character) => character.toUpperCase());
}

function modrinthCategoryMatchesContentType(category: ModrinthCategoryInfo, contentType: string | null): boolean {
  if (contentType === null) return SUPPORTED_MODRINTH_PROJECT_TYPES.has(category.project_type);
  return MODRINTH_PROJECT_TYPES[contentType] === category.project_type;
}

function curatedCategoryMatchesContentType(category: CategoryInfo, contentType: string | null): boolean {
  return contentType === null || category.content_types.includes(contentType);
}

function CuratedCategoryDropdown({
  categories,
  selectedCategory,
  onSelect,
  disabled,
}: {
  categories: CategoryInfo[];
  selectedCategory: string | null;
  onSelect: (category: string | null) => void;
  disabled: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState('');
  const selected = categories.find((category) => category.id === selectedCategory) ?? null;
  const normalizedSearch = search.trim().toLocaleLowerCase();
  const filteredCategories = normalizedSearch
    ? categories.filter((category) =>
        category.display_name.toLocaleLowerCase().includes(normalizedSearch)
        || category.id.toLocaleLowerCase().includes(normalizedSearch),
      )
    : categories;

  return (
    <DropdownMenu
      open={open}
      onOpenChange={(nextOpen) => {
        setOpen(nextOpen);
        if (!nextOpen) setSearch('');
      }}
    >
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          disabled={disabled}
          aria-label={selected ? `Curated category: ${selected.display_name}` : 'Curated categories'}
          className={[
            'inline-flex min-w-48 items-center justify-between gap-2 rounded-lg border px-3 py-2 text-sm transition-colors disabled:cursor-not-allowed disabled:opacity-50',
            selected
              ? 'border-primary bg-primary text-primary-foreground'
              : 'border-input bg-background hover:bg-accent',
          ].join(' ')}
        >
          <span className="truncate">{selected ? `Curated: ${selected.display_name}` : 'Curated categories'}</span>
          <ChevronDown aria-hidden className="h-4 w-4 shrink-0" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-72 p-2">
        <div
          className="relative mb-2"
          onKeyDown={(event) => event.stopPropagation()}
        >
          <Search aria-hidden className="pointer-events-none absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
          <input
            autoFocus
            type="search"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            placeholder="Search curated categories…"
            aria-label="Search curated categories"
            className="w-full rounded-md border border-input bg-background py-2 pl-8 pr-3 text-sm outline-none focus:ring-2 focus:ring-ring"
          />
        </div>
        <div className="max-h-64 overflow-y-auto">
          <DropdownMenuItem onSelect={() => onSelect(null)}>
            <span className="flex-1">All categories</span>
            {selectedCategory === null && <Check aria-hidden className="h-4 w-4" />}
          </DropdownMenuItem>
          {filteredCategories.map((category) => (
            <DropdownMenuItem key={category.id} onSelect={() => onSelect(category.id)}>
              <span className="flex-1 truncate">{category.display_name}</span>
              {category.id === selectedCategory && <Check aria-hidden className="h-4 w-4" />}
            </DropdownMenuItem>
          ))}
          {filteredCategories.length === 0 && (
            <p className="px-2 py-3 text-center text-xs text-muted-foreground">No curated categories found.</p>
          )}
        </div>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

type BrowseItem = BrowseItemCached;

interface ItemContext {
  instanceName: string;
  minecraftVersion: string;
  loader: string;
  compatibility: 'compatible' | 'major_match' | '';
  installed: boolean;
  updateAvailable: boolean;
  whyRecommended: string | null;
}

// NOTE: the modrinthResults branch is currently unused (For You sort passes []).
// Kept for future Modrinth_raw integration.
function mergeItems(
  registryItems: RegistryItem[],
  modrinthResults: ModrinthSearchResult[],
): BrowseItem[] {
  const registryByModrinthId = new Map<string, RegistryItem>();
  for (const ri of registryItems) {
    if (ri.modrinth_id) {
      registryByModrinthId.set(ri.modrinth_id, ri);
    }
  }

  const matchedRegistryIds = new Set<string>();
  const merged: BrowseItem[] = [];

  for (const mr of modrinthResults) {
    const matched = registryByModrinthId.get(mr.project_id);
    if (matched) {
      matchedRegistryIds.add(matched.id);
      merged.push({
        id: matched.id,
        source: 'curated',
        registryItem: matched,
        modrinthResult: mr,
        name: matched.name,
        iconUrl: matched.icon_url ?? mr.icon_url,
        description: matched.description ?? mr.description,
        contentType: matched.content_type,
      });
    } else {
      merged.push({
        id: mr.project_id,
        source: 'modrinth',
        registryItem: null,
        modrinthResult: mr,
        name: mr.title,
        iconUrl: mr.icon_url,
        description: mr.description,
        contentType: mr.project_type,
      });
    }
  }

  for (const ri of registryItems) {
    if (!matchedRegistryIds.has(ri.id)) {
      merged.push({
        id: ri.id,
        source: 'curated',
        registryItem: ri,
        modrinthResult: null,
        name: ri.name,
        iconUrl: ri.icon_url,
        description: ri.description,
        contentType: ri.content_type,
      });
    }
  }

  return merged;
}

// ---------------------------------------------------------------------------
// Deterministic query-key — stable string identity for the current filter set.
// ---------------------------------------------------------------------------
function computeQueryKey(params: {
  sort: SortOption;
  category: string | null;
  contentType: string | null;
  mcVersion: string | null;
  loader: string | null;
  query: string;
}): string {
  return JSON.stringify({
    sort: params.sort,
    category: params.category,
    contentType: params.contentType,
    mcVersion: params.mcVersion,
    loader: params.loader,
    query: params.query,
  });
}

// ---- Registry recovery shell (no hooks after this point) ----
function RegistryRecoveryShell({
  state,
  status,
  error,
  actions,
}: {
  state: import('../lib/useRegistryState').RegistryState;
  status: import('../lib/tauri').RegistryStatus | null;
  error: string | null;
  actions: import('../lib/useRegistryState').RegistryActions;
}) {
  return (
    <div className="space-y-6">
      <section>
        <h2 className="text-2xl font-bold mb-2">Browse</h2>
        <p className="text-muted-foreground">
          Curated mods, packs, shaders, resource packs, and more.
        </p>
      </section>
      <RegistryStatusView
        variant="fullscreen"
        state={state}
        status={status}
        error={error}
        actions={actions}
      />
    </div>
  );
}

export function Browse({ onSelectMod, initialInstanceId, initialContentType }: { onSelectMod?: (id: string) => void; initialInstanceId?: string; initialContentType?: string }) {
  // Registry availability — show recovery panel when missing.
  // This is the ONLY hook call in this component, so the hook count is stable.
  const registry = useRegistryState();

  // Bail out BEFORE any other hooks when the registry is unavailable.
  //
  // Conditions for showing the recovery shell:
  //   - State is exactly 'missing' (no cache, no loading)
  //   - State is 'loading' AND no cached database exists (registry download in
  //     progress but we have nothing to show)
  //   - State is 'unknown' with an error (status-read failed)
  //
  // Once the user has a cached database they can browse cached content, even
  // while an update downloads in the background ('loading' with cache).
  const showRecovery =
    registry.state === 'missing' ||
    (registry.state === 'loading' && !registry.hasCachedDb) ||
    (registry.state === 'unknown' && registry.error !== null);

  if (showRecovery) {
    return (
      <RegistryRecoveryShell
        state={registry.state}
        status={registry.status}
        error={registry.error}
        actions={registry.actions}
      />
    );
  }

  return <BrowseContent onSelectMod={onSelectMod} initialInstanceId={initialInstanceId} initialContentType={initialContentType} registryState={registry.state} registryStatus={registry.status} registryError={registry.error} registryActions={registry.actions} />;
}

function BrowseContent({
  onSelectMod,
  initialInstanceId,
  initialContentType,
  registryState: regState,
  registryStatus: regStatus,
  registryError: regError,
  registryActions: regActions,
}: {
  onSelectMod?: (id: string) => void;
  initialInstanceId?: string;
  initialContentType?: string;
  registryState: import('../lib/useRegistryState').RegistryState;
  registryStatus: import('../lib/tauri').RegistryStatus | null;
  registryError: string | null;
  registryActions: import('../lib/useRegistryState').RegistryActions;
}) {
  // ---- Filter state ----
  const [items, setItems] = useState<BrowseItemCached[]>([]);
  const [hasMore, setHasMore] = useState(false);
  const [categories, setCategories] = useState<CategoryInfo[]>([]);
  const [modrinthCategories, setModrinthCategories] = useState<ModrinthCategoryInfo[]>([]);
  const [sort, setSort] = useState<SortOption>(() => {
    try {
      const saved = localStorage.getItem('browse_sort');
      if (saved && SORTS.some((s) => s.value === saved)) return saved as SortOption;
    } catch { /* ignore */ }
    return 'net_score';
  });
  const [category, setCategory] = useState<string | null>(null);
  const [contentType, setContentType] = useState<string | null>(initialContentType ?? 'mod');
  const [mcVersion, setMcVersion] = useState<string | null>(null);
  const [loader, setLoader] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const debouncedQuery = useDebounce(query, 250);
  const [layout, setLayout] = useState<'list' | 'grid'>(() => {
    try {
      const saved = localStorage.getItem('browse_layout');
      if (saved === 'list' || saved === 'grid') return saved;
    } catch { /* ignore */ }
    return 'list';
  });

  // ---- Separate loading/error state for metadata vs search ----
  const [metaLoading, setMetaLoading] = useState(true);
  const [metaError, setMetaError] = useState<string | null>(null);
  const [searchLoading, setSearchLoading] = useState(true);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [loadMoreLoading, setLoadMoreLoading] = useState(false);
  const [loadMoreError, setLoadMoreError] = useState<string | null>(null);
  // Tracks the 0-indexed page displayed. Starts at 0; incremented after each
  // successful load-more. Reset to 0 on a new search.
  const [currentPage, setCurrentPage] = useState(0);

  // ---- Request generation counter ----
  const generationRef = useRef(0);
  const inFlightSearchRef = useRef<number | null>(null);
  const inFlightLoadMoreRef = useRef<number | null>(null);

  const [loaders, setLoaders] = useState<string[]>([]);
  const [mcVersions, setMcVersions] = useState<string[]>([]);
  const [instances, setInstances] = useState<InstanceRow[]>([]);
  const [activeInstanceId, setActiveInstanceId] = useState('');
  const [activeInstance, setActiveInstance] = useState<InstanceDetail | null>(null);
  const [compatibilityById, setCompatibilityById] = useState<Record<string, string>>({});
  const [contextLoading, setContextLoading] = useState(false);
  const [contextError, setContextError] = useState<string | null>(null);

  const visibleCuratedCategories = useMemo(
    () => categories.filter((item) => curatedCategoryMatchesContentType(item, contentType)),
    [categories, contentType],
  );
  const visibleModrinthCategories = useMemo(() => {
    const seen = new Set<string>();
    return modrinthCategories
      .filter((item) => modrinthCategoryMatchesContentType(item, contentType))
      .filter((item) => {
        if (seen.has(item.name)) return false;
        seen.add(item.name);
        return true;
      })
      .sort((left, right) => formatCategoryName(left.name).localeCompare(formatCategoryName(right.name)));
  }, [contentType, modrinthCategories]);

  useEffect(() => {
    let cancelled = false;
    void listInstances()
      .then((result) => { if (!cancelled) setInstances(result); })
      .catch((cause) => { if (!cancelled) setContextError(formatError(cause)); });
    return () => { cancelled = true; };
  }, []);

  const selectInstanceContext = async (instanceId: string) => {
    setActiveInstanceId(instanceId);
    setContextError(null);
    setActiveInstance(null);
    setCompatibilityById({});
    if (!instanceId) return;
    setContextLoading(true);
    try {
      const detail = await getInstanceDetail(instanceId);
      if (!detail) throw new Error('The selected instance no longer exists.');
      setActiveInstance(detail);
      setMcVersion(detail.row.minecraft_version);
      setLoader(detail.row.loader);
    } catch (cause) {
      setContextError(formatError(cause));
    } finally {
      setContextLoading(false);
    }
  };

  useEffect(() => {
    if (!initialInstanceId || activeInstanceId === initialInstanceId) return;
    if (!instances.some((instance) => instance.instance_id === initialInstanceId)) return;
    void selectInstanceContext(initialInstanceId);
  }, [activeInstanceId, initialInstanceId, instances]);

  useEffect(() => {
    if (!activeInstance || items.length === 0) {
      setCompatibilityById({});
      return;
    }
    let cancelled = false;
    const itemIds = [...new Set(items.map((item) => item.id))];
    void batchCheckCompat(activeInstance.row.instance_id, itemIds)
      .then((result) => {
        if (!cancelled) setCompatibilityById(result);
      })
      .catch((cause) => {
        if (!cancelled) setContextError(formatError(cause));
      });
    return () => { cancelled = true; };
  }, [activeInstance, items]);

  const handleSortChange = (next: SortOption) => {
    setSort(next);
    if (next === 'for_you') setCategory(null);
    try { localStorage.setItem('browse_sort', next); } catch { /* ignore */ }
  };

  // ---- Clear all filters ----
  const clearFilters = useCallback(() => {
    setCategory(null);
    setContentType(null);
    setMcVersion(activeInstance?.row.minecraft_version ?? null);
    setLoader(activeInstance?.row.loader ?? null);
    setQuery('');
    setSearchError(null);
    generationRef.current += 1;
  }, [activeInstance]);

  const hasActiveFilters = category !== null || contentType !== null || mcVersion !== null || loader !== null || query.trim() !== '';

  const contextFor = (item: BrowseItem): ItemContext | null => {
    if (!activeInstance) return null;
    const installed = activeInstance.manifest
      ? [
          ...activeInstance.manifest.mods,
          ...activeInstance.manifest.resourcepacks,
          ...activeInstance.manifest.shaders,
          ...activeInstance.manifest.datapacks,
          ...activeInstance.manifest.worlds,
        ].some((entry) =>
          entry.registry_id === item.id
          || entry.modrinth_id === item.id
          || entry.mod_jar_id === item.id
        )
      : false;
    return {
      instanceName: activeInstance.row.name,
      minecraftVersion: activeInstance.row.minecraft_version,
      loader: activeInstance.row.loader,
      compatibility: (compatibilityById[item.id] ?? '') as ItemContext['compatibility'],
      installed,
      updateAvailable: false,
      whyRecommended: sort === 'for_you'
        ? item.registryItem?.recommendation_reason
          ?? `Recommended by Agora's curated score for ${activeInstance.row.loader} ${activeInstance.row.minecraft_version}.`
        : null,
    };
  };

  // ---- Build a stable query key from active filter params ----
  const queryKey = useMemo(
    () =>
      computeQueryKey({
        sort,
        category,
        contentType,
        mcVersion,
        loader,
        query: debouncedQuery,
      }),
    [sort, category, contentType, mcVersion, loader, debouncedQuery],
  );

  // ---- Load category metadata — separate from search ----
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        if (!cancelled) setMetaLoading(true);
        setMetaError(null);
        const [curatedResult, modrinthEnabledResult] = await Promise.allSettled([
          listCategories(),
          getSetting('modrinth_enabled'),
        ]);
        if (cancelled) return;

        if (curatedResult.status === 'fulfilled') {
          setCategories(Array.isArray(curatedResult.value) ? curatedResult.value : []);
        } else {
          setMetaError(`Curated categories: ${formatError(curatedResult.reason)}`);
        }

        const modrinthEnabled = modrinthEnabledResult.status === 'fulfilled'
          && (modrinthEnabledResult.value === true || modrinthEnabledResult.value === 'true');
        if (modrinthEnabled) {
          try {
            const result = await listModrinthCategories();
            if (!cancelled) setModrinthCategories(Array.isArray(result) ? result : []);
          } catch (error) {
            if (!cancelled) {
              setMetaError((current) => [current, `Modrinth categories: ${formatError(error)}`].filter(Boolean).join(' '));
            }
          }
        } else {
          setModrinthCategories([]);
        }
      } catch (e) {
        if (!cancelled) setMetaError(formatError(e));
      } finally {
        if (!cancelled) setMetaLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (metaLoading || category === null) return;
    const isAvailable = visibleCuratedCategories.some((item) => item.id === category)
      || visibleModrinthCategories.some((item) => item.name === category);
    if (!isAvailable) setCategory(null);
  }, [category, metaLoading, visibleCuratedCategories, visibleModrinthCategories]);

  // ---- Load loaders and MC versions (static metadata) ----
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const [l, v] = await Promise.all([listManifestLoaders(), listManifestMcVersions()]);
        if (!cancelled) {
          setLoaders(l);
          setMcVersions(v);
        }
      } catch {
        // degraded behavior
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // ---- Main search effect — uses generation counter for stale-response protection ----
  useEffect(() => {
    const generation = ++generationRef.current;
    inFlightSearchRef.current = generation;
    setSearchError(null);
    setSearchLoading(true);
    // Reset items and pagination state immediately so stale results
    // and stuck load-more states are never visible.
    setItems([]);
    setHasMore(false);
    setLoadMoreLoading(false);
    setLoadMoreError(null);
    setCurrentPage(0);
    inFlightLoadMoreRef.current = null;

    let cancelled = false;

    (async () => {
      try {
        if (sort === 'for_you') {
          let modrinthEnabled = false;
          try {
            const m = await getSetting('modrinth_enabled');
            modrinthEnabled = m === true || m === 'true';
          } catch { /* default false */ }

          const registryItems = await forYouItems(
            modrinthEnabled,
            mcVersion ?? undefined,
            loader ?? undefined,
            undefined,
            undefined,
            debouncedQuery.trim() || undefined,
          );

          if (!cancelled && inFlightSearchRef.current === generation) {
            setItems(mergeItems(registryItems, []));
            setHasMore(false);
          }
        } else {
          const page = await browseSearch(
            queryKey,
            debouncedQuery.trim() || undefined,
            contentType ?? undefined,
            category ?? undefined,
            sort,
            mcVersion ?? undefined,
            loader ?? undefined,
          );

          if (!cancelled && inFlightSearchRef.current === generation) {
            setItems(page.items);
            setHasMore(page.hasMore);
          }
        }
      } catch (e) {
        if (!cancelled && inFlightSearchRef.current === generation) {
          setSearchError(formatError(e));
          setItems([]);
          setHasMore(false);
        }
      } finally {
        if (!cancelled && inFlightSearchRef.current === generation) {
          setSearchLoading(false);
          inFlightSearchRef.current = null;
        }
      }
    })();

    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [queryKey]);

  const loadNextPage = useCallback(() => {
    if (!hasMore || searchLoading || loadMoreLoading) return;
    const capturedGeneration = generationRef.current;
    inFlightLoadMoreRef.current = capturedGeneration;
    const nextPage = currentPage + 1;
    setLoadMoreLoading(true);
    setLoadMoreError(null);

    browseLoadMore(queryKey, nextPage)
      .then((page) => {
        if (capturedGeneration === generationRef.current) {
          setItems((prev) => {
            const existingKeys = new Set(prev.map((i) => `${i.id}:${i.source}`));
            const newItems = page.items.filter(
              (ni) => !existingKeys.has(`${ni.id}:${ni.source}`),
            );
            return newItems.length === 0 ? prev : [...prev, ...newItems];
          });
          setHasMore(page.hasMore);
          setCurrentPage(nextPage);
        }
      })
      .catch((e) => {
        if (
          capturedGeneration === generationRef.current
          && !(typeof e === 'object' && e !== null && 'code' in e && e.code === 'ERR_BROWSE_STALE')
        ) {
          setLoadMoreError(formatError(e));
        }
      })
      .finally(() => {
        if (capturedGeneration === generationRef.current) {
          setLoadMoreLoading(false);
          inFlightLoadMoreRef.current = null;
        }
      });
  }, [currentPage, hasMore, loadMoreLoading, queryKey, searchLoading]);

  // ---- Infinite scroll — captures generation at trigger time ----
  const sentinelRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const sentinel = sentinelRef.current;
    if (!sentinel || !hasMore || searchLoading || loadMoreError) return;

    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0]?.isIntersecting && hasMore && !loadMoreLoading && !loadMoreError) {
          loadNextPage();
        }
      },
      { rootMargin: '600px' },
    );
    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [hasMore, searchLoading, loadMoreLoading, loadMoreError, loadNextPage]);

  // ---- Render ----
  return (
    <div className="space-y-6">
      <section>
        <div className="flex items-center gap-3 mb-2">
          <h2 className="text-2xl font-bold">Browse</h2>
          {/* <span className="rounded-full bg-muted text-muted-foreground px-2 py-0.5 text-xs font-medium uppercase tracking-wide">
            Preview
          </span> */}
        </div>
        <p className="text-muted-foreground">
          Curated mods, packs, shaders, resource packs, and more.
        </p>
      </section>

      {/* Registry offline banner */}
      <RegistryStatusView
        variant="banner"
        state={regState}
        status={regStatus}
        error={regError}
        actions={regActions}
      />

      <section className="rounded-xl border border-border bg-card p-4">
        <label className="block text-sm font-semibold" htmlFor="browse-instance-context">
          Discover for an instance
        </label>
        <p className="mt-1 text-xs text-muted-foreground">
          Select an instance to apply its exact Minecraft version and loader and see installed/update labels.
        </p>
        <select
          id="browse-instance-context"
          value={activeInstanceId}
          onChange={(event) => { void selectInstanceContext(event.target.value); }}
          disabled={contextLoading}
          className="mt-3 w-full rounded-lg border border-input bg-background px-3 py-2 text-sm disabled:opacity-50"
        >
          <option value="">No instance context</option>
          {instances.map((instance) => (
            <option key={instance.instance_id} value={instance.instance_id}>
              {instance.name} — {instance.loader} · MC {instance.minecraft_version}
            </option>
          ))}
        </select>
        {contextLoading && <p className="mt-2 text-xs text-muted-foreground">Checking compatibility and updates…</p>}
        {contextError && <p className="mt-2 text-xs text-destructive">{contextError}</p>}
      </section>

      <section className="rounded-xl border border-border bg-card p-4">
        <div className="flex items-start gap-3">
          <Leaf aria-hidden className="mt-0.5 h-5 w-5 text-sea-blue" />
          <div className="flex-1">
            <p className="text-sm font-semibold text-foreground">
              Help grow the community registry
            </p>
            <p className="text-xs text-muted-foreground mt-1">
              Agora's curated catalog is built by the community, for the community.
              We're assembling links to our favorite mods — ideally through
              alternative hosting options like GitHub Releases rather than
              centralized platforms. Every contribution counts.
            </p>
            {agoraRepositoryUrl && (
              <a
                href={agoraRepositoryUrl}
                target="_blank"
                rel="noopener noreferrer"
                className="mt-2 inline-block text-xs font-medium text-primary hover:underline"
              >
                Contribute on GitHub ↗
              </a>
            )}
          </div>
        </div>
      </section>

      <div className="flex flex-col lg:flex-row gap-4">
        <input
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search mods, packs, and more…"
          className="flex-1 rounded-lg border border-input bg-background px-4 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-ring"
        />
        <div className="flex items-center gap-1 rounded-lg border border-input bg-background p-0.5">
          <button
            onClick={() => {
              setLayout('list');
              try { localStorage.setItem('browse_layout', 'list'); } catch { /* ignore */ }
            }}
            className={`rounded-md p-1.5 transition-colors ${layout === 'list' ? 'bg-accent text-accent-foreground' : 'text-muted-foreground hover:text-foreground'}`}
            title="List view"
          >
            <List size={18} />
          </button>
          <button
            onClick={() => {
              setLayout('grid');
              try { localStorage.setItem('browse_layout', 'grid'); } catch { /* ignore */ }
            }}
            className={`rounded-md p-1.5 transition-colors ${layout === 'grid' ? 'bg-accent text-accent-foreground' : 'text-muted-foreground hover:text-foreground'}`}
            title="Grid view"
          >
            <LayoutGrid size={18} />
          </button>
        </div>
        <select
          aria-label="Content type"
          value={contentType ?? ''}
          onChange={(e) => setContentType(e.target.value || null)}
          disabled={sort === 'for_you'}
          className="rounded-lg border border-input bg-background px-3 py-2 text-sm disabled:opacity-40 disabled:cursor-not-allowed"
        >
          <option value="">All types</option>
          {CONTENT_TYPES.map((ct) => (
            <option key={ct} value={ct}>{ct}</option>
          ))}
        </select>
        <select
          value={mcVersion ?? ''}
          onChange={(e) => setMcVersion(e.target.value || null)}
          className="rounded-lg border border-input bg-background px-3 py-2 text-sm"
          title="Filter by Minecraft version"
        >
          <option value="">Any MC version</option>
          {mcVersions.map((v) => (
            <option key={v} value={v}>MC {v}</option>
          ))}
        </select>
        <select
          value={loader ?? ''}
          onChange={(e) => setLoader(e.target.value || null)}
          className="rounded-lg border border-input bg-background px-3 py-2 text-sm"
          title="Filter by modloader"
        >
          <option value="">Any loader</option>
          {loaders.map((l) => (
            <option key={l} value={l}>{l}</option>
          ))}
        </select>
        <select
          value={sort}
          onChange={(e) => handleSortChange(e.target.value as SortOption)}
          className="rounded-lg border border-input bg-background px-3 py-2 text-sm"
        >
          {SORTS.map((s) => (
            <option key={s.value} value={s.value}>{s.label}</option>
          ))}
        </select>
      </div>

      {/* Active filter chips */}
      {(mcVersion || loader) && (
        <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
          <span>Active filters:</span>
          {mcVersion && (
            <button
              onClick={() => setMcVersion(null)}
              className="rounded-full border border-border px-2 py-0.5 hover:bg-accent"
            >
              MC {mcVersion} ✕
            </button>
          )}
          {loader && (
            <button
              onClick={() => setLoader(null)}
              className="rounded-full border border-border px-2 py-0.5 hover:bg-accent"
            >
              {loader} ✕
            </button>
          )}
        </div>
      )}

      {/* Category filters */}
      {sort !== 'for_you' && (
        <div className="space-y-3">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div>
              <p className="text-sm font-semibold">Modrinth categories</p>
              <p className="text-xs text-muted-foreground">
                {contentType ? `Categories for ${contentType} content.` : 'Combined categories for all content types.'}
              </p>
            </div>
            <CuratedCategoryDropdown
              categories={visibleCuratedCategories}
              selectedCategory={category}
              onSelect={setCategory}
              disabled={metaLoading || visibleCuratedCategories.length === 0}
            />
          </div>
          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              onClick={() => setCategory(null)}
              className={[
                'px-3 py-1 rounded-full text-sm border transition-colors',
                category === null
                  ? 'bg-primary text-primary-foreground border-primary'
                  : 'border-border hover:bg-accent',
              ].join(' ')}
            >
              All
            </button>
            {visibleModrinthCategories.map((item) => (
              <button
                type="button"
                key={item.name}
                onClick={() => setCategory(item.name)}
                className={[
                  'px-3 py-1 rounded-full text-sm border transition-colors',
                  category === item.name
                    ? 'bg-primary text-primary-foreground border-primary'
                    : 'border-border hover:bg-accent',
                ].join(' ')}
              >
                {formatCategoryName(item.name)}
              </button>
            ))}
            {!metaLoading && visibleModrinthCategories.length === 0 && (
              <span className="self-center text-xs text-muted-foreground">
                No Modrinth categories are available for this content type.
              </span>
            )}
          </div>
        </div>
      )}

      {/* Metadata error (categories) — separate from search error */}
      {metaError && (
        <div className="rounded-lg border border-destructive bg-destructive/10 p-3 text-xs text-destructive">
          Could not load categories: {metaError}
        </div>
      )}

      {/* Search error with Retry and Clear Filters */}
      {searchError && (
        <div className="rounded-lg border border-destructive bg-destructive/10 p-3 text-sm text-destructive">
          <p>{searchError}</p>
          <div className="mt-2 flex gap-2">
            <button
              onClick={() => {
                setSearchError(null);
                setSearchLoading(true);
                // Bump generation to re-trigger the search effect
                generationRef.current += 1;
                const newGen = generationRef.current;
                inFlightSearchRef.current = newGen;
                setItems([]);
                setHasMore(false);

                // Re-run the current query
                const run = async () => {
                  try {
                    if (sort === 'for_you') {
                      // simplified for_you retry
                      let modrinthEnabled = false;
                      try {
                        const m = await getSetting('modrinth_enabled');
                        modrinthEnabled = m === true || m === 'true';
                      } catch {}
                      const registryItems = await forYouItems(
                        modrinthEnabled,
                        mcVersion ?? undefined,
                        loader ?? undefined,
                        undefined,
                        undefined,
                        debouncedQuery.trim() || undefined,
                      );
                      if (inFlightSearchRef.current === newGen) {
                        setItems(mergeItems(registryItems, []));
                        setHasMore(false);
                      }
                    } else {
                      const page = await browseSearch(
                        queryKey,
                        debouncedQuery.trim() || undefined,
                        contentType ?? undefined,
                        category ?? undefined,
                        sort,
                        mcVersion ?? undefined,
                        loader ?? undefined,
                      );
                      if (inFlightSearchRef.current === newGen) {
                        setItems(page.items);
                        setHasMore(page.hasMore);
                      }
                    }
                  } catch (e) {
                    if (inFlightSearchRef.current === newGen) {
                      setSearchError(formatError(e));
                    }
                  } finally {
                    if (inFlightSearchRef.current === newGen) {
                      setSearchLoading(false);
                      inFlightSearchRef.current = null;
                    }
                  }
                };
                run();
              }}
              className="rounded-lg bg-primary px-3 py-1 text-xs font-medium text-primary-foreground hover:bg-primary/90"
            >
              Retry
            </button>
            {hasActiveFilters && (
              <button
                onClick={clearFilters}
                className="rounded-lg border border-border px-3 py-1 text-xs font-medium text-muted-foreground hover:bg-accent"
              >
                Clear Filters
              </button>
            )}
          </div>
        </div>
      )}

      {/* Loading or results */}
      {searchLoading ? (
        <div className="rounded-xl border border-dashed border-border bg-card p-6 text-center text-muted-foreground">
          Loading items…
        </div>
      ) : items.length === 0 ? (
        <div className="rounded-xl border border-dashed border-border bg-card p-6 text-center">
          <p className="text-muted-foreground">No items to display.</p>
          {hasActiveFilters && (
            <button
              onClick={clearFilters}
              className="mt-2 rounded-lg bg-primary px-3 py-1.5 text-sm font-medium text-primary-foreground hover:bg-primary/90"
            >
              Clear Filters
            </button>
          )}
        </div>
      ) : layout === 'grid' ? (
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
          {items.map((item) => (
            <GridCard key={item.id} item={item} context={contextFor(item)} onSelectMod={onSelectMod} />
          ))}
        </div>
      ) : (
        <ul className="grid grid-cols-1 gap-4 md:grid-cols-2 lg:grid-cols-3">
          {items.map((item) => (
            <ListCard key={item.id} item={item} context={contextFor(item)} onSelectMod={onSelectMod} />
          ))}
        </ul>
      )}

      {/* Infinite scroll sentinel */}
      {loadMoreError && (
        <div className="rounded-lg border border-destructive bg-destructive/10 p-3 text-sm text-destructive">
          <p>{loadMoreError}</p>
          <button
            onClick={loadNextPage}
            className="mt-2 rounded-lg bg-primary px-3 py-1 text-xs font-medium text-primary-foreground hover:bg-primary/90"
          >
            Retry loading more
          </button>
        </div>
      )}
      {hasMore && !searchLoading && (
        <div
          ref={sentinelRef}
          data-testid="browse-load-sentinel"
          className="py-6 text-center text-sm text-muted-foreground"
        >
          {loadMoreLoading ? 'Loading more…' : ''}
        </div>
      )}
      {!hasMore && items.length > 0 && (
        <p className="py-4 text-center text-xs text-muted-foreground">All results loaded</p>
      )}
    </div>
  );
}

function CuratedBadge() {
  return (
    <span className="shrink-0 rounded-full bg-amber-500/15 text-amber-600 dark:text-amber-400 px-2 py-0.5 text-[10px] font-medium uppercase tracking-wide">
      Curated
    </span>
  );
}

function ContextLabels({ item, context }: { item: BrowseItem; context: ItemContext | null }) {
  return (
    <div className="flex flex-wrap gap-1 text-[10px]">
      <span className="rounded-full border border-border px-2 py-0.5 text-muted-foreground">
        Source: {item.source === 'curated' ? 'Agora registry' : 'Modrinth'}
      </span>
      {context?.compatibility === 'compatible' && (
        <span className="rounded-full bg-green-500/10 px-2 py-0.5 text-green-700 dark:text-green-300">
          Compatible with {context.instanceName} · {context.loader} · MC {context.minecraftVersion}
        </span>
      )}
      {context?.compatibility === 'major_match' && (
        <span className="rounded-full bg-amber-500/10 px-2 py-0.5 text-amber-700 dark:text-amber-300">
          May work with {context.instanceName} · same major Minecraft version
        </span>
      )}
      {context?.installed && (
        <span className="rounded-full bg-primary/10 px-2 py-0.5 text-primary">Installed</span>
      )}
      {context?.updateAvailable && (
        <span className="rounded-full bg-amber-500/10 px-2 py-0.5 text-amber-700 dark:text-amber-300">Update available</span>
      )}
    </div>
  );
}

function GridCard({ item, context, onSelectMod }: { item: BrowseItem; context: ItemContext | null; onSelectMod?: (id: string) => void }) {
  return (
    <div className="rounded-xl border border-border bg-card overflow-hidden flex flex-col">
      {item.iconUrl && (
        <div className="aspect-video bg-muted flex items-center justify-center p-4">
          <img
            src={item.iconUrl}
            alt={item.name}
            className="h-full w-full object-contain"
          />
        </div>
      )}
      <div className="flex flex-col gap-2 p-4 flex-1">
        <div className="flex items-center gap-2">
          <h3 className="font-semibold truncate">{item.name}</h3>
          {item.source === 'curated' && <CuratedBadge />}
        </div>
        <ContextLabels item={item} context={context} />
        {context?.whyRecommended && (
          <p className="text-[10px] text-muted-foreground">Why: {context.whyRecommended}</p>
        )}
        {item.registryItem ? (
          <>
            <p className="text-xs text-muted-foreground">
              {item.registryItem.content_type} · {item.registryItem.download_strategy}
            </p>
            <p className="text-xs text-muted-foreground">
              ↑ {item.registryItem.upvotes} · ↓ {item.registryItem.downvotes}
            </p>
          </>
        ) : item.modrinthResult ? (
          <>
            <p className="text-xs text-muted-foreground">by {item.modrinthResult.author}</p>
            <p className="text-xs text-muted-foreground">
              ↓ {item.modrinthResult.downloads.toLocaleString()} · ★ {item.modrinthResult.follows.toLocaleString()}
            </p>
          </>
        ) : null}
        {item.description && (
          <p className="text-xs text-muted-foreground line-clamp-2">{item.description}</p>
        )}
        {item.modrinthResult && item.modrinthResult.versions.length > 0 && (
          <p className="text-[10px] text-muted-foreground">
            MC: {item.modrinthResult.versions.slice(0, 3).join(', ')}
            {item.modrinthResult.versions.length > 3 ? ` +${item.modrinthResult.versions.length - 3}` : ''}
          </p>
        )}
        <div className="mt-auto pt-2">
          <button
            onClick={() => onSelectMod?.(item.id)}
            className="w-full rounded-lg bg-primary px-3 py-1.5 text-sm font-medium text-primary-foreground hover:bg-primary/90"
          >
            View Details
          </button>
          {item.modrinthResult && (
            <a
              href={`https://modrinth.com/${item.modrinthResult.project_type}/${item.modrinthResult.slug}`}
              target="_blank"
              rel="noopener noreferrer"
              className="block mt-1 text-[10px] text-muted-foreground hover:text-foreground text-center"
            >
              View on Modrinth ↗
            </a>
          )}
        </div>
      </div>
    </div>
  );
}

function ListCard({ item, context, onSelectMod }: { item: BrowseItem; context: ItemContext | null; onSelectMod?: (id: string) => void }) {
  return (
    <li className="rounded-xl border border-border bg-card p-4">
      <div className="flex items-start gap-3">
        {item.iconUrl ? (
          <img
            src={item.iconUrl}
            alt={item.name}
            className="h-12 w-12 rounded-lg border object-contain border-border"
          />
        ) : (
          <div className="h-12 w-12 rounded-lg border border-dashed border-border flex items-center justify-center text-xs text-muted-foreground">
            ?
          </div>
        )}
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2">
            <h3 className="font-semibold truncate">{item.name}</h3>
            {item.source === 'curated' && <CuratedBadge />}
          </div>
          <ContextLabels item={item} context={context} />
          {context?.whyRecommended && (
            <p className="mt-1 text-[10px] text-muted-foreground">Why: {context.whyRecommended}</p>
          )}
          {item.registryItem ? (
            <>
              <p className="text-xs text-muted-foreground">
                {item.registryItem.content_type} · {item.registryItem.download_strategy}
              </p>
              <p className="text-xs text-muted-foreground mt-1">
                ↑ {item.registryItem.upvotes} · ↓ {item.registryItem.downvotes} · net {item.registryItem.net_score}
              </p>
            </>
          ) : item.modrinthResult ? (
            <>
              <p className="text-xs text-muted-foreground">by {item.modrinthResult.author}</p>
              <p className="text-xs text-muted-foreground mt-1">
                ↓ {item.modrinthResult.downloads.toLocaleString()} · ★ {item.modrinthResult.follows.toLocaleString()}
              </p>
            </>
          ) : null}
          {item.modrinthResult && item.modrinthResult.versions.length > 0 && (
            <p className="text-[10px] text-muted-foreground mt-2">
              MC: {item.modrinthResult.versions.slice(0, 4).join(', ')}
              {item.modrinthResult.versions.length > 4 ? ` +${item.modrinthResult.versions.length - 4}` : ''}
            </p>
          )}
        </div>
      </div>
      <div className="mt-3">
        <button
          onClick={() => onSelectMod?.(item.id)}
          className="rounded-lg bg-primary px-3 py-1.5 text-sm font-medium text-primary-foreground hover:bg-primary/90"
        >
          View Details
        </button>
        {item.modrinthResult && (
          <a
            href={`https://modrinth.com/${item.modrinthResult.project_type}/${item.modrinthResult.slug}`}
            target="_blank"
            rel="noopener noreferrer"
            className="ml-2 text-[10px] text-muted-foreground hover:text-foreground"
          >
            View on Modrinth ↗
          </a>
        )}
      </div>
    </li>
  );
}
