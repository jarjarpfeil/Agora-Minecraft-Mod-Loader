import { useEffect, useState } from 'react';
import {
  browseItems,
  getSetting,
  listCategories,
  type CategoryInfo,
  type RegistryItem,
  type SortOption,
} from '../lib/tauri';

const SORTS: { label: string; value: SortOption }[] = [
  { label: 'Net Score', value: 'net_score' },
  { label: 'Trending', value: 'velocity' },
  { label: 'Newest', value: 'newest' },
  { label: 'Most Upvoted', value: 'most_upvoted' },
  { label: 'Most Downvoted', value: 'most_downvoted' },
];

const CONTENT_TYPES = ['mod', 'pack', 'shader', 'resourcepack', 'server', 'datapack', 'world'];

const LOADERS = ['fabric', 'quilt', 'neoforge', 'forge'];
const MC_VERSIONS = ['1.21.11', '1.21.10', '1.21.9'];

export function Browse({ onSelectMod, onOpenModrinth }: { onSelectMod?: (id: string) => void; onOpenModrinth?: () => void }) {
  const [items, setItems] = useState<RegistryItem[]>([]);
  const [categories, setCategories] = useState<CategoryInfo[]>([]);
  const [sort, setSort] = useState<SortOption>('net_score');
  const [category, setCategory] = useState<string | null>(null);
  const [contentType, setContentType] = useState<string | null>(null);
  const [mcVersion, setMcVersion] = useState<string | null>(null);
  const [loader, setLoader] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        if (!cancelled) setLoading(true);
        const cats = await listCategories();
        if (!cancelled) setCategories(cats);
      } catch (e) {
        if (!cancelled) setError(String(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        if (!cancelled) setLoading(true);
        // Re-read the modrinth_enabled setting on every refresh so toggling
        // it in Settings is reflected on the next browse without a remount.
        let modrinthEnabled = false;
        try {
          const m = await getSetting('modrinth_enabled');
          modrinthEnabled = m === true;
        } catch {
          // Setting read failure should not block browsing; default to safe
          // (Modrinth-hidden) behaviour.
          modrinthEnabled = false;
        }
        const result = await browseItems(
          contentType ?? undefined,
          category ?? undefined,
          sort,
          modrinthEnabled,
          mcVersion ?? undefined,
          loader ?? undefined,
        );
        if (!cancelled) setItems(result);
      } catch (e) {
        if (!cancelled) {
          setError(String(e));
          setItems([]);
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [sort, category, contentType, mcVersion, loader]);

  const filtered = query.trim()
    ? items.filter((item) =>
        item.name.toLowerCase().includes(query.toLowerCase()) ||
        item.id.toLowerCase().includes(query.toLowerCase())
      )
    : items;

  return (
    <div className="space-y-6">
      <section>
        <h2 className="text-2xl font-bold mb-2">Browse</h2>
        <p className="text-[rgb(var(--muted))]">
          Curated mods, packs, shaders, resource packs, and more.
        </p>
      </section>

      <div className="flex flex-col lg:flex-row gap-4">
        <input
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search items..."
          className="flex-1 rounded-lg border border-gray-300 dark:border-gray-600 bg-transparent px-4 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-brand-500"
        />
        <select
          value={contentType ?? ''}
          onChange={(e) => setContentType(e.target.value || null)}
          className="rounded-lg border border-gray-300 dark:border-gray-600 bg-transparent px-3 py-2 text-sm"
        >
          <option value="">All types</option>
          {CONTENT_TYPES.map((ct) => (
            <option key={ct} value={ct}>{ct}</option>
          ))}
        </select>
        <select
          value={mcVersion ?? ''}
          onChange={(e) => setMcVersion(e.target.value || null)}
          className="rounded-lg border border-gray-300 dark:border-gray-600 bg-transparent px-3 py-2 text-sm"
          title="Filter by Minecraft version"
        >
          <option value="">Any MC version</option>
          {MC_VERSIONS.map((v) => (
            <option key={v} value={v}>MC {v}</option>
          ))}
        </select>
        <select
          value={loader ?? ''}
          onChange={(e) => setLoader(e.target.value || null)}
          className="rounded-lg border border-gray-300 dark:border-gray-600 bg-transparent px-3 py-2 text-sm"
          title="Filter by modloader"
        >
          <option value="">Any loader</option>
          {LOADERS.map((l) => (
            <option key={l} value={l}>{l}</option>
          ))}
        </select>
        <select
          value={sort}
          onChange={(e) => setSort(e.target.value as SortOption)}
          className="rounded-lg border border-gray-300 dark:border-gray-600 bg-transparent px-3 py-2 text-sm"
        >
          {SORTS.map((s) => (
            <option key={s.value} value={s.value}>{s.label}</option>
          ))}
        </select>
      </div>

      {(mcVersion || loader) && (
        <div className="flex flex-wrap items-center gap-2 text-xs text-[rgb(var(--muted))]">
          <span>Active filters:</span>
          {mcVersion && (
            <button
              onClick={() => setMcVersion(null)}
              className="rounded-full border border-gray-300 dark:border-gray-600 px-2 py-0.5 hover:bg-gray-100 dark:hover:bg-gray-800"
            >
              MC {mcVersion} ✕
            </button>
          )}
          {loader && (
            <button
              onClick={() => setLoader(null)}
              className="rounded-full border border-gray-300 dark:border-gray-600 px-2 py-0.5 hover:bg-gray-100 dark:hover:bg-gray-800"
            >
              {loader} ✕
            </button>
          )}
        </div>
      )}

      {categories.length > 0 && (
        <div className="flex flex-wrap gap-2">
          <button
            onClick={() => setCategory(null)}
            className={[
              'px-3 py-1 rounded-full text-sm border transition-colors',
              category === null
                ? 'bg-brand-600 text-white border-brand-600'
                : 'border-gray-300 dark:border-gray-600 hover:bg-gray-100 dark:hover:bg-gray-800',
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
                  ? 'bg-brand-600 text-white border-brand-600'
                  : 'border-gray-300 dark:border-gray-600 hover:bg-gray-100 dark:hover:bg-gray-800',
              ].join(' ')}
            >
              {c.display_name}
            </button>
          ))}
        </div>
      )}

      {error && (
        <div className="rounded-lg border border-red-300 bg-red-50 p-3 text-sm text-red-700 dark:border-red-700 dark:bg-red-900/30 dark:text-red-200">
          {error}
        </div>
      )}

      {loading ? (
        <div className="rounded-xl p-6 border border-dashed border-gray-300 dark:border-gray-600 text-center text-[rgb(var(--muted))]">
          Loading items…
        </div>
      ) : filtered.length === 0 ? (
        <div className="rounded-xl p-6 border border-dashed border-gray-300 dark:border-gray-600 text-center">
          <p className="text-[rgb(var(--muted))]">No curated items to display.</p>
          {onOpenModrinth && (
            <button
              onClick={onOpenModrinth}
              className="mt-3 text-sm font-medium text-brand-600 hover:underline dark:text-brand-400"
            >
              Not finding what you need? Search all of Modrinth →
            </button>
          )}
        </div>
      ) : (
        <ul className="grid grid-cols-1 gap-4 md:grid-cols-2 lg:grid-cols-3">
          {filtered.map((item) => (
            <li
              key={item.id}
              className="rounded-xl border border-gray-200 dark:border-gray-700 surface p-4"
            >
              <div className="flex items-start gap-3">
                {item.icon_url && (
                  <img
                    src={item.icon_url}
                    alt={item.name}
                    className="h-12 w-12 rounded-lg border object-contain dark:border-gray-600"
                  />
                )}
                <div className="flex-1 min-w-0">
                  <h3 className="font-semibold truncate">{item.name}</h3>
                  <p className="text-xs text-[rgb(var(--muted))]">
                    {item.content_type} · {item.download_strategy}
                  </p>
                  <p className="text-xs text-[rgb(var(--muted))] mt-1">
                    ↑ {item.upvotes} · ↓ {item.downvotes} · net {item.net_score}
                  </p>
                </div>
              </div>
              <div className="mt-3">
                <button
                  onClick={() => onSelectMod?.(item.id)}
                  className="rounded-lg bg-brand-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-brand-700"
                >
                  View Details
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
