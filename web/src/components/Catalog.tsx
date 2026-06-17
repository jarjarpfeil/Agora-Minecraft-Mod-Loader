'use client';

import { useMemo, useState } from 'react';
import Link from 'next/link';
import { RegistryItem } from '@/lib/db';

interface CatalogProps {
  items: RegistryItem[];
  typeLabel: string;
  typePath: string;
}

export function Catalog({ items, typeLabel, typePath }: CatalogProps) {
  const [query, setQuery] = useState('');

  const filtered = useMemo(() => {
    const q = query.toLowerCase().trim();
    if (!q) return items;
    return items.filter((item) => {
      const text = `${item.name} ${item.curator_note} ${item.categories.join(' ')}`.toLowerCase();
      return text.includes(q);
    });
  }, [query, items]);

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-3xl font-bold">{typeLabel}</h1>
        <p className="text-gray-600 dark:text-gray-400">
          {items.length} curated {typeLabel.toLowerCase()}.
        </p>
      </div>

      <div>
        <input
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={`Search ${typeLabel.toLowerCase()}...`}
          className="w-full rounded-lg border bg-white px-4 py-2 dark:border-gray-700 dark:bg-gray-800"
        />
      </div>

      {filtered.length === 0 ? (
        <p className="text-gray-600 dark:text-gray-400">No results match your search.</p>
      ) : (
        <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          {filtered.map((item) => (
            <Link
              key={item.id}
              href={`${typePath}/${item.id}`}
              className="flex flex-col rounded-xl border bg-white p-5 shadow-sm transition hover:shadow-md dark:border-gray-700 dark:bg-gray-800"
            >
              <div className="flex items-start justify-between gap-2">
                <h2 className="font-semibold">{item.name}</h2>
                <span className="shrink-0 rounded-full bg-indigo-100 px-2 py-0.5 text-xs font-medium text-indigo-700 dark:bg-indigo-900 dark:text-indigo-200">
                  {item.net_score}
                </span>
              </div>
              <p className="mt-2 line-clamp-3 flex-1 text-sm text-gray-600 dark:text-gray-400">
                {item.curator_note}
              </p>
              {item.categories.length > 0 && (
                <div className="mt-4 flex flex-wrap gap-2">
                  {item.categories.slice(0, 4).map((cat) => (
                    <span
                      key={cat}
                      className="rounded-md bg-gray-100 px-2 py-0.5 text-xs text-gray-700 dark:bg-gray-700 dark:text-gray-300"
                    >
                      {cat}
                    </span>
                  ))}
                </div>
              )}
            </Link>
          ))}
        </div>
      )}
    </div>
  );
}
