import { useEffect, useState, useCallback } from 'react';
import { getSetting, setSetting } from '../lib/tauri';
import { Switch } from '../components/ui/switch';
import { Badge } from '../components/ui/badge';
import { cn } from '../lib/utils';

// ─── Configuration ───────────────────────────────────────────────────────────

interface Endpoint {
  key: string;
  name: string;
  hosts: string;
  purpose: string;
  note?: string;
  default: boolean;
  group?: string;
}

const ENDPOINTS: Endpoint[] = [
  {
    key: 'network_modrinth_enabled',
    name: 'Modrinth API requests',
    hosts: 'api.modrinth.com',
    purpose: 'Permit requests to api.modrinth.com for live search, project details, and version metadata. Disabling this does not remove metadata already included in the signed catalog.',
    default: true,
    group: 'Mod Discovery',
  },
  {
    key: 'network_modrinth_cdn_enabled',
    name: 'Modrinth CDN (downloads)',
    hosts: 'cdn.modrinth.com / resources.minecraftcraft.cc',
    purpose: 'Download mod files from the Modrinth content delivery network.',
    note: 'Auto-disabled when Modrinth Catalog API is off.',
    default: true,
    group: 'Mod Discovery',
  },
  {
    key: 'network_registry_sync_enabled',
    name: 'GitHub Releases (registry sync)',
    hosts: 'github.com / objects.githubusercontent.com',
    purpose: 'Check for and download signed registry.db updates.',
    default: true,
    group: 'Registry',
  },
  {
    key: 'network_github_oauth_enabled',
    name: 'GitHub OAuth (governance)',
    hosts: 'github.com/login',
    purpose: 'Authenticate for community governance features (triage, flagging).',
    note: 'Only active when governance features are used.',
    default: true,
    group: 'Governance',
  },
  {
    key: 'network_mojang_metadata_enabled',
    name: 'Mojang Metadata',
    hosts: 'piston-meta.mojang.com / launcher.mojang.com',
    purpose: 'Fetch the Minecraft version manifest and version-specific metadata JSON files.',
    note: 'Enabled by default; cached files remain usable if disabled.',
    default: true,
    group: 'Launch',
  },
  {
    key: 'network_mojang_content_enabled',
    name: 'Mojang Content',
    hosts: 'piston-data.mojang.com / libraries.minecraft.net / resources.download.minecraft.net',
    purpose: 'Download the Minecraft client JAR, official libraries, native binaries, and game assets.',
    note: 'Enabled by default; cached files remain usable if disabled.',
    default: true,
    group: 'Launch',
  },
  {
    key: 'network_loader_enabled',
    name: 'Modloader Metadata & Content',
    hosts: 'maven.fabricmc.net / maven.quiltmc.org / loader pinned hosts',
    purpose: 'Fetch Fabric/Quilt profile JSONs and Maven-hosted loader libraries.',
    note: 'Enabled by default; cached files remain usable if disabled.',
    default: true,
    group: 'Launch',
  },
  {
    key: 'network_msa_enabled',
    name: 'Minecraft Authentication',
    hosts: 'login.live.com / user.auth.xboxlive.com / api.minecraftservices.com',
    purpose: 'Authenticate with Microsoft/Xbox to launch Minecraft.',
    note: 'Only used when launching with direct sign-in (Phase 5).',
    default: true,
    group: 'Launch',
  },
  {
    key: 'network_adoptium_enabled',
    name: 'Adoptium (Java runtime)',
    hosts: 'api.adoptium.net',
    purpose: 'Auto-provision a missing Java runtime for your Minecraft version.',
    note: 'Only used when a required Java version is missing.',
    default: true,
    group: 'Runtime',
  },
];

const LOCKDOWN_KEY = 'network_lockdown_enabled';

// ─── Component ───────────────────────────────────────────────────────────────

export function Privacy() {
  const [endpointStates, setEndpointStates] = useState<Record<string, boolean>>({});
  const [lockdown, setLockdown] = useState(false);
  const [online, setOnline] = useState(typeof navigator !== 'undefined' ? navigator.onLine : true);
  const [loading, setLoading] = useState(true);

  // Load all per-endpoint settings + lockdown on mount
  useEffect(() => {
    let cancelled = false;
    (async () => {
      const keys = ENDPOINTS.map((e) => e.key);
      const results = await Promise.all(keys.map((k) => getSetting(k)));
      if (cancelled) return;

      const states: Record<string, boolean> = {};
      keys.forEach((k, i) => {
        const v = results[i];
        // Accept both JSON booleans and legacy JSON string "true"/"false".
        states[k] = typeof v === 'boolean'
          ? v
          : typeof v === 'string'
            ? v === 'true'
            : ENDPOINTS[i].default;
      });
      setEndpointStates(states);

      // Load lockdown
      const lockdownVal = await getSetting(LOCKDOWN_KEY);
      if (!cancelled) {
        setLockdown(typeof lockdownVal === 'boolean' ? lockdownVal : false);
      }
      setLoading(false);
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Sync individual toggles when lockdown state changes
  useEffect(() => {
    if (!lockdown) return;
    setEndpointStates(() => {
      const next: Record<string, boolean> = {};
      ENDPOINTS.forEach((ep) => {
        next[ep.key] = false;
      });
      return next;
    });
  }, [lockdown]);

  // Online/offline indicator
  useEffect(() => {
    const onOnline = () => setOnline(true);
    const onOffline = () => setOnline(false);
    window.addEventListener('online', onOnline);
    window.addEventListener('offline', onOffline);
    return () => {
      window.removeEventListener('online', onOnline);
      window.removeEventListener('offline', onOffline);
    };
  }, []);

  const persistToggle = useCallback(async (key: string, value: boolean) => {
    try {
      await setSetting(key, value);
    } catch {
      // Revert UI on failure — backend didn't accept the change
    }
  }, []);

  const handleToggle = useCallback(
    async (key: string, value: boolean) => {
      setEndpointStates((prev) => ({ ...prev, [key]: value }));

      // If turning Modrinth off, also disable CDN
      if (key === 'network_modrinth_enabled' && !value) {
        setEndpointStates((prev) => ({ ...prev, network_modrinth_cdn_enabled: false }));
        await Promise.all([
          persistToggle(key, value),
          persistToggle('network_modrinth_cdn_enabled', false),
        ]);
      } else {
        await persistToggle(key, value);
      }
    },
    [persistToggle],
  );

  const handleLockdown = useCallback(
    async (value: boolean) => {
      setLockdown(value);
      await persistToggle(LOCKDOWN_KEY, value);
    },
    [persistToggle],
  );

  // Group endpoints by their group field
  const grouped = ENDPOINTS.reduce<Record<string, Endpoint[]>>((acc, ep) => {
    const g = ep.group || 'Other';
    if (!acc[g]) acc[g] = [];
    acc[g].push(ep);
    return acc;
  }, {});

  if (loading) {
    return (
      <div className="rounded-xl border border-border bg-card p-4 space-y-3">
        <h3 className="font-semibold">Privacy &amp; Transparency</h3>
        <p className="text-xs text-muted-foreground">Loading network settings…</p>
      </div>
    );
  }

  return (
    <div className="rounded-xl border border-border bg-card p-4 space-y-5">
      {/* Header row */}
      <div className="flex items-start justify-between gap-4">
        <div className="space-y-1">
          <h3 className="font-semibold">Privacy &amp; Transparency</h3>
          <p className="text-xs text-muted-foreground max-w-prose leading-relaxed">
            Agora makes zero automated telemetry calls. Below is every network endpoint the app can reach, each independently toggleable. Turn them all off with Lockdown to run fully offline against your cached catalog and installed mods.
          </p>
        </div>
        <Badge
          variant="outline"
          className={cn(
            'shrink-0 text-[10px] font-semibold px-2 py-0.5 gap-1',
            online
              ? 'border-green-500/40 text-green-600 dark:text-green-400'
              : 'border-amber-500/40 text-amber-600 dark:text-amber-400',
          )}
        >
          <span
            className={cn(
              'inline-block h-1.5 w-1.5 rounded-full',
              online ? 'bg-green-500' : 'bg-amber-500',
            )}
          />
          {online ? 'Online' : 'Offline'}
        </Badge>
      </div>

      {/* Lockdown toggle */}
      <div className="rounded-lg bg-muted px-3 py-2.5 space-y-1">
        <label className="flex items-center justify-between">
          <div>
            <span className="text-sm font-medium">Lockdown Mode</span>
            <p className="text-xs text-muted-foreground mt-0.5">
              Disable all external network calls. Falls back to cached catalog + installed mods.
            </p>
          </div>
          <Switch
            checked={lockdown}
            onCheckedChange={handleLockdown}
            className={cn(
              lockdown && '[&_[data-state=checked]]:bg-red-600',
            )}
          />
        </label>
      </div>

      {/* Network-call list */}
      <div className="space-y-4">
        {Object.entries(grouped).map(([groupName, endpoints]) => (
          <div key={groupName} className="space-y-2">
            <h4 className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
              {groupName}
            </h4>
            <div className="divide-y divide-border">
              {endpoints.map((ep) => (
                <div
                  key={ep.key}
                  className={cn(
                    'flex items-start justify-between gap-4 py-3',
                    lockdown && 'opacity-50',
                  )}
                >
                  <div className="flex-1 min-w-0 space-y-1">
                    <div className="flex items-center gap-2">
                      <span className="text-sm font-medium">{ep.name}</span>
                      {ep.note && (
                        <Badge variant="secondary" className="text-[10px] font-normal px-1.5 py-0 h-4">
                          Note
                        </Badge>
                      )}
                    </div>
                    <p className="text-xs text-muted-foreground">{ep.purpose}</p>
                    <p className="text-[10px] text-muted-foreground font-mono truncate">
                      {ep.hosts}
                    </p>
                    {ep.note && (
                      <p className="text-[10px] text-muted-foreground italic">{ep.note}</p>
                    )}
                  </div>
                  <Switch
                    checked={endpointStates[ep.key] ?? ep.default}
                    onCheckedChange={(value) => handleToggle(ep.key, value)}
                    disabled={lockdown}
                  />
                </div>
              ))}
            </div>
          </div>
        ))}
      </div>

      {/* Footer note */}
      <p className="text-[10px] text-muted-foreground pt-1 border-t border-border">
        These toggles persist the UI preference. Actual network enforcement is handled by the Rust backend.
      </p>
    </div>
  );
}
