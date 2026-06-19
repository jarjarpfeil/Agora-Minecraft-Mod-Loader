import { useEffect, useRef, useState } from 'react';
import {
  searchModrinth,
  listModrinthCategories,
  listModrinthLoaders,
  listModrinthGameVersions,
  listRawModrinthVersions,
  installRawModrinth,
  listInstances,
  formatError,
  type ModrinthSearchResult,
  type ModrinthSearchParams,
  type ModrinthSort,
  type ModrinthCategoryInfo,
  type ModrinthLoaderInfo,
  type ModrinthGameVersionInfo,
  type RawModrinthVersionCandidate,
  type InstanceRow,
} from '../lib/tauri';

const PAGE_SIZE = 20;

/// The four primary modloaders shown by default in the Loaders filter.
/// Additional loaders (e.g. rift, liteloader) are hidden behind an expand.
const PRIMARY_LOADERS = ['fabric', 'quilt', 'neoforge', 'forge'];

const SORTS: { label: string; value: ModrinthSort }[] = [
  { label: 'Relevance', value: 'relevance' },
  { label: 'Most Downloads', value: 'downloads' },
  { label: 'Most Followers', value: 'follows' },
  { label: 'Newest', value: 'newest' },
  { label: 'Recently Updated', value: 'updated' },
];

/**
 * Raw Modrinth tab (§6.3): live, uncurated search against the full Modrinth
 * mod catalog with category/loader/version facets, sort options, and offset
 * pagination (infinite scroll). Every downloaded file is SHA-1 verified against
 * the hash published by Modrinth's API before being written to the instance.
 *
 * This view is only reachable when the Modrinth integration toggle is on.
 */
export function ModrinthRaw({ onOpenInstanceEditor }: { onOpenInstanceEditor?: (id: string) => void }) {
  const [query, setQuery] = useState('');
  const [sort, setSort] = useState<ModrinthSort>('relevance');
  const [selectedCats, setSelectedCats] = useState<string[]>([]);
  const [selectedLoaders, setSelectedLoaders] = useState<string[]>([]);
  const [selectedVersions, setSelectedVersions] = useState<string[]>([]);

  const [results, setResults] = useState<ModrinthSearchResult[]>([]);
  const [totalHits, setTotalHits] = useState(0);
  const [nextOffset, setNextOffset] = useState(0);
  const [loading, setLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [categories, setCategories] = useState<ModrinthCategoryInfo[]>([]);
  const [loaders, setLoaders] = useState<ModrinthLoaderInfo[]>([]);
  const [gameVersions, setGameVersions] = useState<ModrinthGameVersionInfo[]>([]);
  const [filtersOpen, setFiltersOpen] = useState(true);
  const [showAllLoaders, setShowAllLoaders] = useState(false);

  const [activeProject, setActiveProject] = useState<ModrinthSearchResult | null>(null);

  const reqIdRef = useRef(0);

  // Load tag metadata once for filter UI.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const [cats, lds, gvs] = await Promise.all([
          listModrinthCategories(),
          listModrinthLoaders(),
          listModrinthGameVersions(),
        ]);
        if (cancelled) return;
        setCategories(cats);
        setLoaders(lds);
        // Game versions come newest-first from Modrinth; keep as-is.
        setGameVersions(gvs);
      } catch (e) {
        if (!cancelled) setError(formatError(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Group categories by their header for the filter panel.
  const categoryGroups = groupCategoriesByHeader(categories);

  // Build the search params object from current filter state.
  const buildParams = (offset: number): ModrinthSearchParams => ({
    query: query.trim() || undefined,
    sort,
    categories: selectedCats.length ? selectedCats : undefined,
    loaders: selectedLoaders.length ? selectedLoaders : undefined,
    game_versions: selectedVersions.length ? selectedVersions : undefined,
    offset,
    limit: PAGE_SIZE,
  });

  // Fetch a single page and either replace or append results.
  const fetchPage = async (offset: number, append: boolean) => {
    const id = ++reqIdRef.current;
    if (append) {
      setLoadingMore(true);
    } else {
      setLoading(true);
      setError(null);
    }
    try {
      const page = await searchModrinth(buildParams(offset));
      if (id !== reqIdRef.current) return; // stale response
      setResults((prev) => (append ? [...prev, ...page.results] : page.results));
      setTotalHits(page.total_hits);
      const newOffset = offset + page.results.length;
      setNextOffset(page.results.length < PAGE_SIZE ? -1 : newOffset);
    } catch (e) {
      if (id === reqIdRef.current) {
        setError(formatError(e));
        if (!append) setResults([]);
      }
    } finally {
      if (id === reqIdRef.current) {
        setLoading(false);
        setLoadingMore(false);
      }
    }
  };

  // Re-search from scratch whenever filters / sort / query change.
  const [searchKey, setSearchKey] = useState(0);
  const runSearch = () => {
    setSearchKey((k) => k + 1);
  };

  useEffect(() => {
    void fetchPage(0, false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchKey, sort, selectedCats, selectedLoaders, selectedVersions]);

  const onSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    runSearch();
  };

  const loadMore = () => {
    if (loadingMore || nextOffset <= 0) return;
    void fetchPage(nextOffset, true);
  };

  // Infinite scroll sentinel.
  const sentinelRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const el = sentinelRef.current;
    if (!el) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0]?.isIntersecting) loadMore();
      },
      { rootMargin: '600px' },
    );
    observer.observe(el);
    return () => observer.disconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [nextOffset, loadingMore, results.length]);

  const toggle = (
    list: string[],
    setList: (v: string[]) => void,
    value: string,
  ) => {
    setList(list.includes(value) ? list.filter((x) => x !== value) : [...list, value]);
  };

  const clearFilters = () => {
    setSelectedCats([]);
    setSelectedLoaders([]);
    setSelectedVersions([]);
  };

  const activeFilterCount =
    selectedCats.length + selectedLoaders.length + selectedVersions.length;

  if (activeProject) {
    return (
      <ModrinthProjectDetail
        project={activeProject}
        onBack={() => setActiveProject(null)}
        onOpenInstanceEditor={onOpenInstanceEditor}
      />
    );
  }

  const shownCount = results.length;
  const hasMore = nextOffset > 0;

  return (
    <div className="space-y-6">
      <section>
        <h2 className="text-2xl font-bold mb-2">Modrinth</h2>
        <p className="text-[rgb(var(--muted))]">
          Search all of Modrinth directly. Files are SHA-1 verified before install.
        </p>
      </section>

      <RawModrinthBanner />

      <form onSubmit={onSubmit} className="flex gap-2">
        <input
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search mods on Modrinth…"
          className="flex-1 rounded-lg border border-gray-300 dark:border-gray-600 bg-transparent px-4 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-brand-500"
        />
        <button
          type="submit"
          disabled={loading}
          className="rounded-lg bg-brand-600 px-4 py-2 text-sm font-medium text-white hover:bg-brand-700 disabled:opacity-50"
        >
          {loading ? 'Searching…' : 'Search'}
        </button>
        <select
          value={sort}
          onChange={(e) => setSort(e.target.value as ModrinthSort)}
          className="rounded-lg border border-gray-300 dark:border-gray-600 bg-transparent px-3 py-2 text-sm"
        >
          {SORTS.map((s) => (
            <option key={s.value} value={s.value}>{s.label}</option>
          ))}
        </select>
        <button
          type="button"
          onClick={() => setFiltersOpen((v) => !v)}
          className="rounded-lg border border-gray-300 dark:border-gray-600 px-3 py-2 text-sm font-medium hover:bg-gray-100 dark:hover:bg-gray-800"
        >
          {filtersOpen ? 'Hide filters' : 'Filters'}
          {activeFilterCount > 0 && (
            <span className="ml-2 inline-flex items-center justify-center rounded-full bg-brand-600 px-1.5 py-0.5 text-[10px] text-white">
              {activeFilterCount}
            </span>
          )}
        </button>
      </form>

      <div className="flex flex-col lg:flex-row gap-6">
        {filtersOpen && (
          <aside className="lg:w-64 flex-shrink-0 space-y-5 lg:max-h-[70vh] lg:overflow-y-auto lg:pr-2">
            {activeFilterCount > 0 && (
              <button
                onClick={clearFilters}
                className="text-xs font-medium text-brand-600 hover:underline dark:text-brand-400"
              >
                Clear all filters ({activeFilterCount})
              </button>
            )}

            <FilterGroup title="Loaders">
              <LoadersFilter
                loaders={loaders}
                selected={selectedLoaders}
                showAll={showAllLoaders}
                onToggleShowAll={() => setShowAllLoaders((v) => !v)}
                onToggle={(v) => toggle(selectedLoaders, setSelectedLoaders, v)}
              />
            </FilterGroup>

            {categoryGroups.map((group) => (
              <FilterGroup key={group.header || 'Other'} title={group.header || 'Other'}>
                {group.items.map((c) => (
                  <FilterCheckbox
                    key={c.name}
                    label={c.name}
                    checked={selectedCats.includes(c.name)}
                    onChange={() => toggle(selectedCats, setSelectedCats, c.name)}
                  />
                ))}
              </FilterGroup>
            ))}

            <FilterGroup title="Game Versions">
              <GameVersionFilter
                versions={gameVersions}
                selected={selectedVersions}
                onToggle={(v) => toggle(selectedVersions, setSelectedVersions, v)}
              />
            </FilterGroup>
          </aside>
        )}

        <div className="flex-1 min-w-0 space-y-4">
          {error && (
            <div className="rounded-lg border border-red-300 bg-red-50 p-3 text-sm text-red-700 dark:border-red-700 dark:bg-red-900/30 dark:text-red-200">
              {error}
            </div>
          )}

          <div className="flex items-center justify-between text-xs text-[rgb(var(--muted))]">
            <span>
              {loading
                ? 'Searching…'
                : `${shownCount.toLocaleString()} of ${totalHits.toLocaleString()} results`}
            </span>
          </div>

          {loading && results.length === 0 ? (
            <div className="rounded-xl p-6 border border-dashed border-gray-300 dark:border-gray-600 text-center text-[rgb(var(--muted))]">
              Searching Modrinth…
            </div>
          ) : results.length === 0 ? (
            <div className="rounded-xl p-6 border border-dashed border-gray-300 dark:border-gray-600 text-center">
              <p className="text-[rgb(var(--muted))]">
                No results. Try a different query or relax your filters.
              </p>
            </div>
          ) : (
            <>
              <ul className="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3">
                {results.map((r) => (
                  <ModrinthCard key={r.project_id} result={r} onView={() => setActiveProject(r)} />
                ))}
              </ul>

              {(hasMore || loadingMore) && (
                <div ref={sentinelRef} className="flex justify-center py-6">
                  <button
                    onClick={loadMore}
                    disabled={loadingMore || !hasMore}
                    className="rounded-lg border border-gray-300 dark:border-gray-600 px-4 py-2 text-sm font-medium hover:bg-gray-100 dark:hover:bg-gray-800 disabled:opacity-50"
                  >
                    {loadingMore ? 'Loading more…' : 'Load more'}
                  </button>
                </div>
              )}
              {!hasMore && results.length > 0 && (
                <p className="text-center text-xs text-[rgb(var(--muted))]">
                  End of results.
                </p>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );
}

function ModrinthCard({ result, onView }: { result: ModrinthSearchResult; onView: () => void }) {
  return (
    <li className="rounded-xl border border-gray-200 dark:border-gray-700 surface p-4 flex flex-col">
      <div className="flex items-start gap-3">
        {result.icon_url ? (
          <img
            src={result.icon_url}
            alt={result.title}
            className="h-12 w-12 rounded-lg border object-contain dark:border-gray-600"
          />
        ) : (
          <div className="h-12 w-12 rounded-lg border border-dashed border-gray-300 dark:border-gray-600 flex items-center justify-center text-xs text-[rgb(var(--muted))]">
            ?
          </div>
        )}
        <div className="flex-1 min-w-0">
          <h3 className="font-semibold truncate">{result.title}</h3>
          <p className="text-xs text-[rgb(var(--muted))] truncate">by {result.author}</p>
          <p className="text-xs text-[rgb(var(--muted))] mt-1">
            ↓ {result.downloads.toLocaleString()} · ★ {result.follows.toLocaleString()}
          </p>
        </div>
      </div>
      {result.description && (
        <p className="text-xs text-[rgb(var(--muted))] mt-2 line-clamp-2">{result.description}</p>
      )}
      {result.versions.length > 0 && (
        <p className="text-[10px] text-[rgb(var(--muted))] mt-2">
          MC: {result.versions.slice(0, 4).join(', ')}
          {result.versions.length > 4 ? ` +${result.versions.length - 4}` : ''}
        </p>
      )}
      {result.categories.length > 0 && (
        <div className="mt-2 flex flex-wrap gap-1">
          {result.categories.slice(0, 5).map((c) => (
            <span
              key={c}
              className="rounded-full border border-gray-300 dark:border-gray-600 px-2 py-0.5 text-[10px] text-[rgb(var(--muted))]"
            >
              {c}
            </span>
          ))}
        </div>
      )}
      <div className="mt-3 pt-3 border-t border-gray-100 dark:border-gray-800">
        <button
          onClick={onView}
          className="rounded-lg bg-brand-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-brand-700"
        >
          View Versions
        </button>
      </div>
    </li>
  );
}

function LoadersFilter({
  loaders,
  selected,
  showAll,
  onToggleShowAll,
  onToggle,
}: {
  loaders: ModrinthLoaderInfo[];
  selected: string[];
  showAll: boolean;
  onToggleShowAll: () => void;
  onToggle: (v: string) => void;
}) {
  const primary: ModrinthLoaderInfo[] = [];
  const rest: ModrinthLoaderInfo[] = [];
  for (const l of loaders) {
    if (PRIMARY_LOADERS.includes(l.name)) primary.push(l);
    else rest.push(l);
  }
  // Keep the primary four in their canonical order.
  primary.sort(
    (a, b) => PRIMARY_LOADERS.indexOf(a.name) - PRIMARY_LOADERS.indexOf(b.name),
  );
  rest.sort((a, b) => a.name.localeCompare(b.name));

  // If a non-primary loader is selected, force-show all so the user can
  // see (and deselect) their active pick.
  const hasSelectedExtra = selected.some((s) => !PRIMARY_LOADERS.includes(s));
  const expanded = showAll || hasSelectedExtra;
  const displayed = expanded ? [...primary, ...rest] : primary;
  const hiddenCount = rest.length;

  return (
    <div className="space-y-1.5">
      {displayed.map((l) => (
        <FilterCheckbox
          key={l.name}
          label={l.name}
          checked={selected.includes(l.name)}
          onChange={() => onToggle(l.name)}
        />
      ))}
      {displayed.length === 0 && (
        <p className="text-xs text-[rgb(var(--muted))]">No loaders.</p>
      )}
      {hiddenCount > 0 && (
        <button
          onClick={onToggleShowAll}
          className="text-xs font-medium text-brand-600 hover:underline dark:text-brand-400"
        >
          {expanded ? 'Show main loaders only' : `Show all (${hiddenCount} more)`}
        </button>
      )}
    </div>
  );
}


function FilterGroup({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div>
      <h4 className="text-xs font-semibold uppercase tracking-wide text-[rgb(var(--muted))] mb-2">
        {title}
      </h4>
      <div className="space-y-1.5">{children}</div>
    </div>
  );
}

function FilterCheckbox({
  label,
  checked,
  onChange,
}: {
  label: string;
  checked: boolean;
  onChange: () => void;
}) {
  return (
    <label className="flex items-center gap-2 cursor-pointer text-sm capitalize">
      <input
        type="checkbox"
        checked={checked}
        onChange={onChange}
        className="h-4 w-4 accent-brand-600"
      />
      <span className="truncate">{label}</span>
    </label>
  );
}

function GameVersionFilter({
  versions,
  selected,
  onToggle,
}: {
  versions: ModrinthGameVersionInfo[];
  selected: string[];
  onToggle: (v: string) => void;
}) {
  const [showAll, setShowAll] = useState(false);
  const [typeFilter, setTypeFilter] = useState<'all' | 'release' | 'snapshot' | 'beta' | 'alpha'>('all');

  const filtered = versions.filter((v) => typeFilter === 'all' || v.version_type === typeFilter);
  const majorOnly = showAll ? filtered : filtered.filter((v) => v.major);
  const displayed = majorOnly.slice(0, 30);

  return (
    <div className="space-y-2">
      <select
        value={typeFilter}
        onChange={(e) => setTypeFilter(e.target.value as typeof typeFilter)}
        className="w-full rounded-lg border border-gray-300 dark:border-gray-600 bg-transparent px-2 py-1 text-xs"
      >
        <option value="all">All types</option>
        <option value="release">Releases</option>
        <option value="snapshot">Snapshots</option>
        <option value="beta">Betas</option>
        <option value="alpha">Alphas</option>
      </select>
      <div className="space-y-1.5 max-h-56 overflow-y-auto pr-1">
        {displayed.map((v) => (
          <FilterCheckbox
            key={v.version}
            label={v.version}
            checked={selected.includes(v.version)}
            onChange={() => onToggle(v.version)}
          />
        ))}
        {displayed.length === 0 && (
          <p className="text-xs text-[rgb(var(--muted))]">No versions.</p>
        )}
      </div>
      {filtered.length > displayed.length || !showAll ? (
        <button
          onClick={() => setShowAll((v) => !v)}
          className="text-xs font-medium text-brand-600 hover:underline dark:text-brand-400"
        >
          {showAll ? 'Show major only' : `Show all (${filtered.length})`}
        </button>
      ) : null}
    </div>
  );
}

function groupCategoriesByHeader(cats: ModrinthCategoryInfo[]) {
  const map = new Map<string, ModrinthCategoryInfo[]>();
  for (const c of cats) {
    const header = c.header || 'Other';
    if (!map.has(header)) map.set(header, []);
    map.get(header)!.push(c);
  }
  return Array.from(map.entries())
    .map(([header, items]) => ({ header, items: items.sort((a, b) => a.name.localeCompare(b.name)) }))
    .sort((a, b) => a.header.localeCompare(b.header));
}

function RawModrinthBanner() {
  return (
    <div
      className="rounded-lg border px-4 py-3 text-sm"
      style={{
        backgroundColor: 'rgba(217, 119, 6, 0.12)',
        borderColor: 'rgba(217, 119, 6, 0.6)',
        color: 'rgb(217, 119, 6)',
      }}
    >
      <div className="flex items-center gap-2 font-semibold">
        <span aria-hidden>⚠️</span>
        <span>Uncurated Content</span>
      </div>
      <p className="mt-1 text-xs opacity-90">
        These mods are uncurated by the Agora community. Download at your own discretion. Files
        are integrity-checked against Modrinth&apos;s published SHA-1 hashes before install.
      </p>
    </div>
  );
}

function ModrinthProjectDetail({
  project,
  onBack,
  onOpenInstanceEditor,
}: {
  project: ModrinthSearchResult;
  onBack: () => void;
  onOpenInstanceEditor?: (id: string) => void;
}) {
  const [instances, setInstances] = useState<InstanceRow[]>([]);
  const [instancesLoading, setInstancesLoading] = useState(true);
  const [selectedInstanceId, setSelectedInstanceId] = useState<string | null>(null);
  const [candidates, setCandidates] = useState<RawModrinthVersionCandidate[]>([]);
  const [versionsLoading, setVersionsLoading] = useState(false);
  const [selectedCandidate, setSelectedCandidate] = useState<RawModrinthVersionCandidate | null>(null);
  const [phase, setPhase] = useState<'idle' | 'loadingVersions' | 'installing' | 'done' | 'error'>('idle');
  const [statusMsg, setStatusMsg] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      setInstancesLoading(true);
      try {
        const all = await listInstances();
        if (!cancelled) setInstances(all);
      } catch (e) {
        if (!cancelled) {
          setPhase('error');
          setStatusMsg(formatError(e));
        }
      } finally {
        if (!cancelled) setInstancesLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // When an instance is selected, fetch versions scoped to that instance.
  useEffect(() => {
    if (!selectedInstanceId) {
      setCandidates([]);
      setSelectedCandidate(null);
      return;
    }
    let cancelled = false;
    (async () => {
      setVersionsLoading(true);
      setCandidates([]);
      setSelectedCandidate(null);
      setStatusMsg(null);
      try {
        const vers = await listRawModrinthVersions(selectedInstanceId, project.project_id);
        if (!cancelled) {
          setCandidates(vers);
          setPhase('idle');
        }
      } catch (e) {
        if (!cancelled) {
          setPhase('error');
          setStatusMsg(formatError(e));
        }
      } finally {
        if (!cancelled) setVersionsLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [selectedInstanceId, project.project_id]);

  const confirmInstall = async () => {
    if (!selectedInstanceId || !selectedCandidate) return;
    setPhase('installing');
    setStatusMsg(null);
    try {
      const installed = await installRawModrinth(
        selectedInstanceId,
        project.project_id,
        selectedCandidate,
      );
      setPhase('done');
      setStatusMsg(
        `Installed ${installed.filename} to ${instances.find((i) => i.instance_id === selectedInstanceId)?.name ?? selectedInstanceId}.`,
      );
    } catch (e) {
      setPhase('error');
      setStatusMsg(formatError(e));
    }
  };

  return (
    <div className="space-y-6">
      <button
        onClick={onBack}
        className="rounded-lg border border-gray-300 dark:border-gray-600 px-3 py-1.5 text-sm font-medium hover:bg-gray-100 dark:hover:bg-gray-800"
      >
        ← Back to search
      </button>

      <RawModrinthBanner />

      <section className="rounded-xl border border-gray-200 dark:border-gray-700 surface p-6">
        <div className="flex items-start gap-4">
          {project.icon_url && (
            <img
              src={project.icon_url}
              alt={project.title}
              className="h-16 w-16 rounded-lg border object-contain dark:border-gray-600"
            />
          )}
          <div className="flex-1 min-w-0">
            <h2 className="text-2xl font-bold break-words">{project.title}</h2>
            <p className="text-xs text-[rgb(var(--muted))] mt-1">by {project.author || 'unknown'}</p>
            <p className="text-xs text-[rgb(var(--muted))] mt-2">
              ↓ {project.downloads.toLocaleString()} downloads · ★ {project.follows.toLocaleString()} followers
            </p>
            {project.description && (
              <p className="text-sm text-[rgb(var(--muted))] mt-3">{project.description}</p>
            )}
            {project.categories.length > 0 && (
              <div className="mt-2 flex flex-wrap gap-1">
                {project.categories.map((c) => (
                  <span
                    key={c}
                    className="rounded-full border border-gray-300 dark:border-gray-600 px-2 py-0.5 text-xs text-[rgb(var(--muted))]"
                  >
                    {c}
                  </span>
                ))}
              </div>
            )}
          </div>
        </div>
      </section>

      <section className="rounded-xl border border-gray-200 dark:border-gray-700 surface p-4 space-y-4">
        <h3 className="font-semibold text-sm">Install to Instance</h3>

        {phase === 'error' && statusMsg && (
          <p className="text-sm text-red-600 dark:text-red-300">{statusMsg}</p>
        )}

        {instancesLoading ? (
          <p className="text-sm text-[rgb(var(--muted))]">Loading instances…</p>
        ) : instances.length === 0 ? (
          <p className="text-sm text-[rgb(var(--muted))]">
            You need an instance first. Create one in My Instances, then come back here.
          </p>
        ) : (
          <>
            <label className="block">
              <span className="text-xs font-medium">Select instance</span>
              <select
                value={selectedInstanceId ?? ''}
                onChange={(e) => setSelectedInstanceId(e.target.value || null)}
                className="mt-1 w-full rounded-lg border border-gray-300 dark:border-gray-600 bg-transparent px-3 py-2 text-sm"
              >
                <option value="">Choose an instance…</option>
                {instances.map((inst) => (
                  <option key={inst.instance_id} value={inst.instance_id}>
                    {inst.name} ({inst.loader} · MC {inst.minecraft_version})
                  </option>
                ))}
              </select>
            </label>

            {selectedInstanceId && (
              versionsLoading ? (
                <div className="text-center py-4">
                  <svg className="animate-spin h-5 w-5 mx-auto text-[rgb(var(--muted))]" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
                    <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                    <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z" />
                  </svg>
                  <p className="text-xs text-[rgb(var(--muted))] mt-2">Loading versions from Modrinth…</p>
                </div>
              ) : candidates.length === 0 ? (
                <p className="text-sm text-[rgb(var(--muted))]">
                  No versions compatible with this instance&apos;s Minecraft version and loader.
                </p>
              ) : (
                <div>
                  <p className="text-xs font-medium mb-2">Available versions (SHA-1 verified on install)</p>
                  <ul className="space-y-2 max-h-64 overflow-y-auto">
                    {candidates.map((cand) => {
                      const isSelected = selectedCandidate?.version_id === cand.version_id;
                      return (
                        <li
                          key={cand.version_id}
                          className={`rounded-lg border px-3 py-2 text-sm cursor-pointer transition-colors ${
                            isSelected
                              ? 'border-brand-500 bg-brand-50 dark:bg-brand-900/20'
                              : 'border-gray-200 dark:border-gray-700 hover:bg-gray-50 dark:hover:bg-gray-800'
                          }`}
                          onClick={() => setSelectedCandidate(cand)}
                        >
                          <div className="flex items-center justify-between gap-2">
                            <span className="font-medium truncate">{cand.version}</span>
                            {cand.primary && (
                              <span className="text-[10px] uppercase tracking-wide text-[rgb(var(--muted))]">primary</span>
                            )}
                          </div>
                          <p className="text-xs text-[rgb(var(--muted))] mt-0.5 truncate">{cand.filename}</p>
                          <p className="text-xs text-[rgb(var(--muted))] mt-0.5">
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
                            <p className="text-[10px] text-red-600 dark:text-red-400 mt-0.5">
                              No SHA-1 published — install refused
                            </p>
                          )}
                        </li>
                      );
                    })}
                  </ul>
                  {selectedCandidate && (
                    <button
                      onClick={confirmInstall}
                      disabled={phase === 'installing' || !selectedCandidate.sha1}
                      className="mt-3 rounded-lg bg-brand-600 px-4 py-2 text-sm font-medium text-white hover:bg-brand-700 disabled:opacity-50"
                    >
                      {phase === 'installing' ? 'Installing…' : `Install ${selectedCandidate.filename}`}
                    </button>
                  )}
                </div>
              )
            )}

            {phase === 'installing' && (
              <div className="text-center py-4">
                <svg className="animate-spin h-5 w-5 mx-auto text-[rgb(var(--muted))]" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
                  <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                  <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z" />
                </svg>
                <p className="text-xs text-[rgb(var(--muted))] mt-2">Downloading &amp; SHA-1 verifying…</p>
              </div>
            )}

            {phase === 'done' && statusMsg && (
              <div className="space-y-2">
                <p className="text-sm text-green-600 dark:text-green-400">{statusMsg}</p>
                {selectedInstanceId && onOpenInstanceEditor && (
                  <button
                    onClick={() => onOpenInstanceEditor(selectedInstanceId)}
                    className="rounded-lg border border-gray-300 dark:border-gray-600 px-3 py-1.5 text-xs font-medium hover:bg-gray-100 dark:hover:bg-gray-800"
                  >
                    Open instance editor
                  </button>
                )}
              </div>
            )}
          </>
        )}
      </section>
    </div>
  );
}
