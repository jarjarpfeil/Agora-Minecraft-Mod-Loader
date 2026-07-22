import { useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  Dialog,
  DialogContent,
  DialogTitle,
  DialogDescription,
} from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { cn } from '@/lib/utils';
import type { InstanceRow } from '@/lib/tauri';

type Tab = 'home' | 'browse' | 'instances' | 'governance' | 'ai' | 'settings';

interface CommandPaletteProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onNavigate: (tab: Tab, instanceId?: string) => void;
}

const SETTINGS_ITEMS: { label: string; tab: Tab; icon: string }[] = [
  { label: 'Home', tab: 'home', icon: '🏠' },
  { label: 'Browse', tab: 'browse', icon: '🔍' },
  { label: 'My Instances', tab: 'instances', icon: '📦' },
  { label: 'Community Governance', tab: 'governance', icon: '🗳️' },
  { label: 'Settings', tab: 'settings', icon: '⚙️' },
];

type ResultItem =
  | { __section: string }
  | { __type: 'instance'; instance_id: string; name: string; loader: string; loader_version: string; minecraft_version: string }
  | { __type: 'setting'; label: string; tab: Tab; icon: string };

export function CommandPalette({ open, onOpenChange, onNavigate }: CommandPaletteProps) {
  const [query, setQuery] = useState('');
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [instances, setInstances] = useState<InstanceRow[]>([]);
  const [instancesLoaded, setInstancesLoaded] = useState(false);
  const searchRef = useRef<HTMLInputElement>(null);
  const listboxRef = useRef<HTMLDivElement>(null);

  // Fetch instances once per open cycle
  useEffect(() => {
    if (open && !instancesLoaded) {
      invoke<InstanceRow[]>('list_instances')
        .then((data) => {
          setInstances(data);
          setInstancesLoaded(true);
        })
        .catch(() => {
          setInstances([]);
          setInstancesLoaded(true);
        });
    }
    if (!open) {
      setQuery('');
      setSelectedIndex(0);
      setInstancesLoaded(false);
    }
  }, [open]);

  // Auto-focus search input on open
  useEffect(() => {
    if (open) {
      requestAnimationFrame(() => searchRef.current?.focus());
    }
  }, [open]);

  // Build flattened results, then compute the list of actionable (non-section) indices.
  const results = useMemo(() => {
    const r: ResultItem[] = [];
    const filteredInstances = instances.filter((inst) =>
      query ? inst.name.toLowerCase().includes(query.toLowerCase()) : true,
    );
    const filteredSettings = SETTINGS_ITEMS.filter((item) =>
      query ? item.label.toLowerCase().includes(query.toLowerCase()) : true,
    );

    if (filteredInstances.length > 0) {
      r.push({ __section: 'Instances' });
      for (const inst of filteredInstances) {
        r.push({
          __type: 'instance',
          instance_id: inst.instance_id,
          name: inst.name,
          loader: inst.loader,
          loader_version: inst.loader_version,
          minecraft_version: inst.minecraft_version,
        });
      }
    }
    if (filteredSettings.length > 0) {
      r.push({ __section: 'Settings' });
      for (const s of filteredSettings) {
        r.push({ __type: 'setting', ...s });
      }
    }
    return r;
  }, [query, instances]);

  // Indices into `results` that point to actionable (non-section) items.
  const actionableIndices = useMemo(
    () =>
      results
        .map((item, i) => (isSection(item) ? -1 : i))
        .filter((i) => i >= 0),
    [results],
  );

  const actionableCount = actionableIndices.length;

  // Clamp selectedIndex to the actionable range, defaulting to the first
  // actionable item (index 0 in the actionable array).
  const safeSelectedIndex = (() => {
    if (actionableCount === 0) return -1;

    // If selectedIndex is out of bounds or points to a section, snap to
    // the first actionable item.
    if (
      selectedIndex < 0 ||
      selectedIndex >= results.length ||
      isSection(results[selectedIndex])
    ) {
      return actionableIndices[0];
    }
    return selectedIndex;
  })();

  // Keyboard navigation — only moves among actionable items.
  useEffect(() => {
    if (!open || actionableCount === 0) return;

    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setSelectedIndex((prev) => {
          const currPos = actionableIndices.indexOf(
            prev >= 0 && prev < results.length && !isSection(results[prev])
              ? prev
              : actionableIndices[0],
          );
          const nextPos = (currPos + 1) % actionableCount;
          return actionableIndices[nextPos];
        });
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        setSelectedIndex((prev) => {
          const currPos = actionableIndices.indexOf(
            prev >= 0 && prev < results.length && !isSection(results[prev])
              ? prev
              : actionableIndices[0],
          );
          const nextPos = (currPos - 1 + actionableCount) % actionableCount;
          return actionableIndices[nextPos];
        });
      } else if (e.key === 'Enter') {
        e.preventDefault();
        const idx = safeSelectedIndex;
        if (idx >= 0) {
          activateItem(idx);
        }
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, actionableIndices, actionableCount, safeSelectedIndex, results, onOpenChange, onNavigate]);

  const activateItem = (index: number) => {
    const item = results[index];
    if (!item || isSection(item)) return;

    if (item.__type === 'instance') {
      onOpenChange(false);
      onNavigate('instances', item.instance_id);
    } else if (item.__type === 'setting') {
      onOpenChange(false);
      onNavigate(item.tab);
    }
  };

  const isItemSelected = (index: number) => index === safeSelectedIndex;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl p-0 gap-0 overflow-hidden">
        <DialogTitle className="sr-only">Command Palette</DialogTitle>
        <DialogDescription className="sr-only">
          Search and navigate across instances, settings, and the catalog.
        </DialogDescription>

        <div className="flex items-center gap-3 px-4 border-b border-gray-200 dark:border-gray-700">
          <span className="text-[rgb(var(--muted))] text-lg" aria-hidden="true">⌕</span>
          <Input
            ref={searchRef}
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              // Reset to first actionable item on filter change.
              setSelectedIndex(actionableIndices.length > 0 ? actionableIndices[0] : 0);
            }}
            placeholder="Type a command or search…"
            className="border-0 shadow-none focus-visible:ring-0 focus-visible:ring-offset-0 h-12 text-base bg-transparent placeholder:text-[rgb(var(--muted))]"
          />
          <kbd className="ml-auto text-xs text-[rgb(var(--muted))] border border-gray-300 dark:border-gray-600 rounded px-1.5 py-0.5">
            ESC
          </kbd>
        </div>

        <div
          ref={listboxRef}
          role="listbox"
          aria-label="Command palette results"
          className="max-h-[60vh] overflow-y-auto p-2"
        >
          {results.length === 0 ? (
            <div className="text-center py-8 text-[rgb(var(--muted))] text-sm">
              No results found.
            </div>
          ) : (
            results.map((item, index) => {
              if (isSection(item)) {
                return (
                  <div
                    key={item.__section}
                    role="presentation"
                    className="px-3 py-1.5 text-xs font-semibold uppercase tracking-wider text-[rgb(var(--muted))]"
                  >
                    {item.__section}
                  </div>
                );
              }

              if (item.__type === 'instance') {
                return (
                  <button
                    key={item.instance_id}
                    role="option"
                    aria-selected={isItemSelected(index)}
                    onClick={() => activateItem(index)}
                    className={cn(
                      'w-full flex items-center gap-3 px-3 py-2.5 rounded-lg text-sm transition-colors text-left',
                      isItemSelected(index)
                        ? 'bg-accent text-accent-foreground'
                        : 'text-[rgb(var(--text))] hover:bg-gray-100 dark:hover:bg-gray-800',
                    )}
                  >
                    <span className="text-lg" aria-hidden="true">📦</span>
                    <div className="flex-1 min-w-0">
                      <div className="font-medium truncate">{item.name}</div>
                      <div className="text-xs text-[rgb(var(--muted))] truncate">
                        {item.loader} {item.loader_version} · MC {item.minecraft_version}
                      </div>
                    </div>
                  </button>
                );
              }

              if (item.__type === 'setting') {
                return (
                  <button
                    key={item.tab}
                    role="option"
                    aria-selected={isItemSelected(index)}
                    onClick={() => activateItem(index)}
                    className={cn(
                      'w-full flex items-center gap-3 px-3 py-2.5 rounded-lg text-sm transition-colors text-left',
                      isItemSelected(index)
                        ? 'bg-accent text-accent-foreground'
                        : 'text-[rgb(var(--text))] hover:bg-gray-100 dark:hover:bg-gray-800',
                    )}
                  >
                    <span className="text-lg" aria-hidden="true">{item.icon}</span>
                    <span className="font-medium">{item.label}</span>
                  </button>
                );
              }

              return null;
            })
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}

function isSection(item: ResultItem): item is { __section: string } {
  return '__section' in item;
}
