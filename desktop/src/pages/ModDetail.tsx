import { useEffect, useState } from 'react';
import { getRegistryItem, type RegistryItem } from '../lib/tauri';

type CompatibleVersionEntry = Record<string, unknown> | string;

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

export function ModDetail({ itemId, onBack }: { itemId: string; onBack: () => void }) {
  const [item, setItem] = useState<RegistryItem | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [installMessage, setInstallMessage] = useState<string | null>(null);

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
        if (!cancelled) setError(String(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [itemId]);

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

  const handleInstall = () => {
    setInstallMessage('Coming soon — actual mod download arrives in a future update.');
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
          </div>
        </div>

        <div className="mt-5 flex flex-wrap gap-2">
          <button
            onClick={handleInstall}
            className="rounded-lg bg-brand-600 px-4 py-2 text-sm font-medium text-white hover:bg-brand-700"
          >
            Install to Instance
          </button>
        </div>
        {installMessage && (
          <p className="mt-3 text-xs text-[rgb(var(--muted))]">{installMessage}</p>
        )}
      </section>

      {curatorNotes && (
        <section className="rounded-xl border border-gray-200 dark:border-gray-700 surface p-4">
          <h3 className="font-semibold text-sm mb-2">Curator Notes</h3>
          <p className="text-sm whitespace-pre-wrap text-[rgb(var(--muted))]">{curatorNotes}</p>
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
