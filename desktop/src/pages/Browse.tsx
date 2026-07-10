import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { List, LayoutGrid } from 'lucide-react';
import {
  browseSearch,
  browseLoadMore,
  forYouItems,
  formatError,
  getSetting,
  listCategories,
  listManifestLoaders,
  listManifestMcVersions,
  type BrowseItemCached,
  type CategoryInfo,
  type RegistryItem,
  type SortOption,
  type ModrinthSearchResult,
} from '../lib/tauri';
import { useRegistryState } from '../lib/useRegistryState';
import { RegistryStatusView } from '../components/registry-status-view';

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

type BrowseItem = BrowseItemCached;

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

export function Browse({ onSelectMod }: { onSelectMod?: (id: string) => void }) {
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

  return <BrowseContent onSelectMod={onSelectMod} registryState={registry.state} registryStatus={registry.status} registryError={registry.error} registryActions={registry.actions} />;
}

function BrowseContent({
  onSelectMod,
  registryState: regState,
  registryStatus: regStatus,
  registryError: regError,
  registryActions: regActions,
}: {
  onSelectMod?: (id: string) => void;
  registryState: import('../lib/useRegistryState').RegistryState;
  registryStatus: import('../lib/tauri').RegistryStatus | null;
  registryError: string | null;
  registryActions: import('../lib/useRegistryState').RegistryActions;
}) {
  // ---- Filter state ----
  const [items, setItems] = useState<BrowseItemCached[]>([]);
  const [hasMore, setHasMore] = useState(false);
  const [categories, setCategories] = useState<CategoryInfo[]>([]);
  const [sort, setSort] = useState<SortOption>(() => {
    try {
      const saved = localStorage.getItem('browse_sort');
      if (saved && SORTS.some((s) => s.value === saved)) return saved as SortOption;
    } catch { /* ignore */ }
    return 'net_score';
  });
  const [category, setCategory] = useState<string | null>(null);
  const [contentType, setContentType] = useState<string | null>(null);
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
  const [, setMetaLoading] = useState(true);
  const [metaError, setMetaError] = useState<string | null>(null);
  const [searchLoading, setSearchLoading] = useState(true);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [loadMoreLoading, setLoadMoreLoading] = useState(false);
  // Tracks the 0-indexed page displayed. Starts at 0; incremented after each
  // successful load-more. Reset to 0 on a new search.
  const [currentPage, setCurrentPage] = useState(0);

  // ---- Request generation counter ----
  const generationRef = useRef(0);
  const inFlightSearchRef = useRef<number | null>(null);
  const inFlightLoadMoreRef = useRef<number | null>(null);

  const [loaders, setLoaders] = useState<string[]>([]);
  const [mcVersions, setMcVersions] = useState<string[]>([]);

  const handleSortChange = (next: SortOption) => {
    setSort(next);
    if (next === 'for_you') setCategory(null);
    try { localStorage.setItem('browse_sort', next); } catch { /* ignore */ }
  };

  // ---- Clear all filters ----
  const clearFilters = useCallback(() => {
    setCategory(null);
    setContentType(null);
    setMcVersion(null);
    setLoader(null);
    setQuery('');
    setSearchError(null);
    generationRef.current += 1;
  }, []);

  const hasActiveFilters = category !== null || contentType !== null || mcVersion !== null || loader !== null || query.trim() !== '';

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

  // ---- Load categories (metadata only — separate from search) ----
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        if (!cancelled) setMetaLoading(true);
        setMetaError(null);
        const cats = await listCategories();
        if (!cancelled) setCategories(cats);
      } catch (e) {
        if (!cancelled) setMetaError(formatError(e));
      } finally {
        if (!cancelled) setMetaLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [contentType]);

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
          );

          if (!cancelled && inFlightSearchRef.current === generation) {
            setItems(mergeItems(registryItems, []));
            setHasMore(false);
          }
        } else {
          const page = await browseSearch(
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

  // ---- Infinite scroll — captures generation at trigger time ----
  const sentinelRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const sentinel = sentinelRef.current;
    if (!sentinel || !hasMore || searchLoading) return;

    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0]?.isIntersecting && hasMore && !loadMoreLoading) {
          const capturedGeneration = generationRef.current;
          inFlightLoadMoreRef.current = capturedGeneration;
          const nextPage = currentPage + 1;
          setLoadMoreLoading(true);

          browseLoadMore(nextPage)
            .then((page) => {
              // Only apply if the captured generation is still current
              if (capturedGeneration === generationRef.current) {
                // Deduplicate by (id, source) composite key — the Rust cache
                // deduplicates stored items by id, but the response may include
                // items that the cache already had before dedup ran.
                setItems((prev) => {
                  const existingKeys = new Set(prev.map((i) => `${i.id}:${i.source}`));
                  const newItems = page.items.filter(
                    (ni) => !existingKeys.has(`${ni.id}:${ni.source}`),
                  );
                  if (newItems.length === 0) return prev;
                  return [...prev, ...newItems];
                });
                setHasMore(page.hasMore);
                setCurrentPage(nextPage);
              }
            })
            .catch(() => {
              // Silently ignore stale load-more errors; the next search
              // will reset pagination anyway.
            })
            .finally(() => {
              if (capturedGeneration === generationRef.current) {
                setLoadMoreLoading(false);
                inFlightLoadMoreRef.current = null;
              }
            });
        }
      },
      { rootMargin: '600px' },
    );
    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [hasMore, searchLoading, loadMoreLoading, currentPage]);

  // ---- Render ----
  return (
    <div className="space-y-6">
      <section>
        <div className="flex items-center gap-3 mb-2">
          <h2 className="text-2xl font-bold">Browse</h2>
          <span className="rounded-full bg-muted text-muted-foreground px-2 py-0.5 text-xs font-medium uppercase tracking-wide">
            Preview
          </span>
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
        <div className="flex items-start gap-3">
          <span aria-hidden className="text-xl mt-0.5">🌱</span>
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
            <a
              href="https://github.com/jarjarpfeil/Agora-Minecraft-Mod-Loader"
              target="_blank"
              rel="noopener noreferrer"
              className="mt-2 inline-block text-xs font-medium text-primary hover:underline"
            >
              Contribute on GitHub ↗
            </a>
          </div>
        </div>
      </section>

      <div className="flex flex-col lg:flex-row gap-4">
        <input
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search mods, packs, and more…"
          className="flex-1 rounded-lg border border-input bg-background px-4 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-brand-500"
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

      {/* Category chips */}
      {sort !== 'for_you' && categories.length > 0 && (
        <div className="flex flex-wrap gap-2">
          <button
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
          {categories.map((c) => (
            <button
              key={c.id}
              onClick={() => setCategory(c.id)}
              className={[
                'px-3 py-1 rounded-full text-sm border transition-colors',
                category === c.id
                  ? 'bg-primary text-primary-foreground border-primary'
                  : 'border-border hover:bg-accent',
              ].join(' ')}
            >
              {c.display_name}
            </button>
          ))}
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
                      const registryItems = await forYouItems(modrinthEnabled, mcVersion ?? undefined, loader ?? undefined);
                      if (inFlightSearchRef.current === newGen) {
                        setItems(mergeItems(registryItems, []));
                        setHasMore(false);
                      }
                    } else {
                      const page = await browseSearch(
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
        <div className="rounded-xl p-6 border border-dashed border-border text-center text-muted-foreground">
          Loading items…
        </div>
      ) : items.length === 0 ? (
        <div className="rounded-xl p-6 border border-dashed border-border text-center">
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
            <GridCard key={item.id} item={item} onSelectMod={onSelectMod} />
          ))}
        </div>
      ) : (
        <ul className="grid grid-cols-1 gap-4 md:grid-cols-2 lg:grid-cols-3">
          {items.map((item) => (
            <ListCard key={item.id} item={item} onSelectMod={onSelectMod} />
          ))}
        </ul>
      )}

      {/* Infinite scroll sentinel */}
      {hasMore && !searchLoading && (
        <div ref={sentinelRef} className="py-6 text-center text-sm text-muted-foreground">
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

function GridCard({ item, onSelectMod }: { item: BrowseItem; onSelectMod?: (id: string) => void }) {
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

function ListCard({ item, onSelectMod }: { item: BrowseItem; onSelectMod?: (id: string) => void }) {
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
