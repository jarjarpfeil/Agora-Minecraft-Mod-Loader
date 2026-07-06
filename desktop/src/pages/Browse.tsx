import { useEffect, useState, useRef } from 'react';
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

export function Browse({ onSelectMod }: { onSelectMod?: (id: string) => void }) {
  const [items, setItems] = useState<BrowseItemCached[]>([]);
  const [hasMore, setHasMore] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
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
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [loaders, setLoaders] = useState<string[]>([]);
  const [mcVersions, setMcVersions] = useState<string[]>([]);

  const handleSortChange = (next: SortOption) => {
    setSort(next);
    if (next === 'for_you') setCategory(null);
    try { localStorage.setItem('browse_sort', next); } catch { /* ignore */ }
  };

  // --- Load categories ---
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        if (!cancelled) setLoading(true);
        const cats = await listCategories();
        if (!cancelled) setCategories(cats);
      } catch (e) {
        if (!cancelled) setError(formatError(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // --- Load loaders and MC versions ---
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

  // --- Main search effect ---
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        if (!cancelled) setLoading(true);

        if (sort === 'for_you') {
          let modrinthEnabled = false;
          try {
            const m = await getSetting('modrinth_enabled');
            modrinthEnabled = m === true || m === 'true';
          } catch { /* default false */ }
          const registryItems = await forYouItems(modrinthEnabled, mcVersion ?? undefined, loader ?? undefined, undefined, undefined);
          console.log('[BROWSE] forYouItems returned', registryItems.length, 'items');
          if (!cancelled) {
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
          console.log('[BROWSE] browseSearch returned', page.items.length, 'items, hasMore=', page.hasMore, 'total=', page.total);
          if (!cancelled) {
            setItems(page.items);
            setHasMore(page.hasMore);
          }
        }
      } catch (e) {
        console.error('[BROWSE] error:', e);
        if (!cancelled) {
          setError(formatError(e));
          setItems([]);
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [sort, category, contentType, mcVersion, loader, debouncedQuery]);

  // --- Infinite scroll ---
  const sentinelRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const sentinel = sentinelRef.current;
    if (!sentinel || !hasMore || loading) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0]?.isIntersecting && hasMore && !loadingMore) {
          setLoadingMore(true);
          browseLoadMore()
            .then((page) => {
              console.log('[BROWSE] loadMore returned', page.items.length, 'items');
              setItems((prev) => [...prev, ...page.items]);
              setHasMore(page.hasMore);
            })
            .catch((e) => console.error('[BROWSE] loadMore error:', e))
            .finally(() => setLoadingMore(false));
        }
      },
      { rootMargin: '600px' },
    );
    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [hasMore, loading, loadingMore]);

  // --- Filtered list ---
  const filtered = query.trim()
    ? items.filter((item) => {
        const searchable = `${item.name} ${item.registryItem?.id ?? ''} ${item.modrinthResult?.title ?? ''} ${item.modrinthResult?.author ?? ''}`;
        return searchable.toLowerCase().includes(query.toLowerCase());
      })
    : items;

  console.log('[RENDER] Browse: items=', filtered.length, 'loading=', loading);

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

      {error && (
        <div className="rounded-lg border border-destructive bg-destructive/10 p-3 text-sm text-destructive">
          {error}
        </div>
      )}

      {loading ? (
        <div className="rounded-xl p-6 border border-dashed border-border text-center text-muted-foreground">
          Loading items…
        </div>
      ) : filtered.length === 0 ? (
        <div className="rounded-xl p-6 border border-dashed border-border text-center">
          <p className="text-muted-foreground">No items to display.</p>
        </div>
      ) : layout === 'grid' ? (
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
          {filtered.map((item) => (
            <GridCard key={item.id} item={item} onSelectMod={onSelectMod} />
          ))}
        </div>
      ) : (
        <ul className="grid grid-cols-1 gap-4 md:grid-cols-2 lg:grid-cols-3">
          {filtered.map((item) => (
            <ListCard key={item.id} item={item} onSelectMod={onSelectMod} />
          ))}
        </ul>
      )}

      {/* Infinite scroll sentinel */}
      {hasMore && !loading && (
        <div ref={sentinelRef} className="py-6 text-center text-sm text-muted-foreground">
          {loadingMore ? 'Loading more…' : ''}
        </div>
      )}
      {!hasMore && filtered.length > 0 && (
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
