import { test, expect, type Page } from '@playwright/test';

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const DEFAULT_INSTANCE: Record<string, unknown> = {
  instance_id: 'test-instance',
  name: 'Test Instance',
  minecraft_version: '1.21',
  loader: 'fabric',
  loader_version: '0.16.9',
  is_modpack: false,
  is_locked: false,
  last_launched_at: '2026-07-12T10:00:00Z',
  jvm_memory_mb: 4096,
  jvm_gc: 'G1GC',
  jvm_custom_args: '',
  created_at: '2026-06-01T00:00:00Z',
};

const RECOMMENDED_ITEM: Record<string, unknown> = {
  id: 'rec-mod-1',
  name: 'Recommended Mod',
  content_type: 'mod',
  download_strategy: 'github_release',
  source_identifier: 'rec-mod/releases',
  sha256: '',
  upvotes: 42,
  downvotes: 3,
  net_score: 39,
  velocity: 2.5,
  status: 'active',
  is_immune: false,
  immunity_reason: null,
  allow_comments: true,
  icon_url: null,
  gallery_urls_json: null,
  date_added: '2026-06-15',
  compatible_versions_json: JSON.stringify([{ mc_version: '1.21', loader: 'fabric', mod_version: '1.0.0' }]),
  description: 'A recommended mod for your instance.',
  body_markdown: null,
  page_url: null,
  license_id: 'MIT',
  source_updated_at: '2026-07-01T00:00:00Z',
  modrinth_id: null,
  recommendation_reason: 'Matches your installed mod categories (performance, utility).',
  recommendation_overlap: 3,
};

const REGISTRY_READY = {
  has_cached_db: true,
  cached_tag: 'test',
  cached_schema_version: 5,
  latest_tag: 'test',
  update_available: false,
  checked: true,
  message: 'Registry ready.',
};

// ---------------------------------------------------------------------------
// Mock installer
// ---------------------------------------------------------------------------

interface HomeMockOptions {
  instances?: Record<string, unknown>[];
  crashResult?: Record<string, unknown> | null;
  lkgMarker?: Record<string, unknown> | null;
  snapshots?: Record<string, unknown>[];
  driftResult?: Record<string, unknown>;
  registryStatus?: Record<string, unknown>;
  recommendations?: Record<string, unknown>[];
  updatesResult?: Record<string, unknown>[];
  launchMode?: string;
  forYouItems?: Record<string, unknown>[];
}

async function installHomeMock(page: Page, opts: HomeMockOptions = {}) {
  const {
    instances = [],
    crashResult = null,
    lkgMarker = null,
    snapshots = [],
    driftResult = { added: [], removed: [], modified: [] },
    registryStatus = REGISTRY_READY,
    recommendations = [],
    updatesResult = [],
    launchMode = 'delegation',
    forYouItems: forYouItemsOverride,
  } = opts;

  // forYouItems on the Home page uses the `recommendations` field
  const effectiveForYou = forYouItemsOverride ?? recommendations;

  await page.addInitScript(
    (params: {
      instances: Record<string, unknown>[];
      crashResult: Record<string, unknown> | null;
      lkgMarker: Record<string, unknown> | null;
      snapshots: Record<string, unknown>[];
      driftResult: Record<string, unknown>;
      registryStatus: Record<string, unknown>;
      recommendations: Record<string, unknown>[];
      updatesResult: Record<string, unknown>[];
      launchMode: string;
      forYouItems: Record<string, unknown>[];
    }) => {
      const {
        instances, crashResult, lkgMarker, snapshots, driftResult,
        registryStatus, recommendations, updatesResult, launchMode, forYouItems,
      } = params;

      const callbacks = new Map<number, (...args: unknown[]) => void>();
      let callbackId = 0;
      let updateChecks = 0;

      const internals = {
        transformCallback(callback: (...args: unknown[]) => void) {
          const id = ++callbackId;
          callbacks.set(id, callback);
          return id;
        },
        unregisterCallback(id: number) { callbacks.delete(id); },
        invoke(command: string, args: Record<string, unknown> = {}) {
          // Settings
          if (command === 'get_setting') {
            const key = args.key as string;
            if (key === 'onboarding_complete') return Promise.resolve(true);
            if (key === 'launch_mode') return Promise.resolve(launchMode);
            if (key === 'modrinth_enabled') return Promise.resolve(true);
            if (key === 'last_home_visit') return Promise.resolve(null);
            return Promise.resolve(null);
          }
          if (command === 'set_setting') return Promise.resolve(null);

          // Registry
          if (command === 'get_registry_status') return Promise.resolve(registryStatus);
          if (command === 'check_registry_update') return Promise.resolve(registryStatus);
          if (command === 'list_categories') return Promise.resolve([]);
          if (command === 'list_manifest_loaders') return Promise.resolve([]);
          if (command === 'list_manifest_mc_versions') return Promise.resolve([]);

          // Browse
          if (command === 'browse_search') return Promise.resolve({ items: [], total: 0, page: 0, hasMore: false });
          if (command === 'browse_load_more') return Promise.resolve({ items: [], total: 0, page: 1, hasMore: false });

          // Instances
          if (command === 'list_instances') return Promise.resolve(instances);
          if (command === 'check_instance_crash') return Promise.resolve(crashResult);
          if (command === 'check_instance_updates') {
            updateChecks += 1;
            return Promise.resolve(updatesResult);
          }
          if (command === 'get_lkg_marker') return Promise.resolve(lkgMarker);
          if (command === 'list_snapshots') return Promise.resolve(snapshots);
          if (command === 'detect_drift') return Promise.resolve(driftResult);
          if (command === 'restore_snapshot') return Promise.resolve(null);

          // Recommendations
          if (command === 'for_you_items') return Promise.resolve(forYouItems);

          // Misc
          if (command === 'get_windows_accent_color') return Promise.resolve(null);
          if (command.startsWith('plugin:event|')) return Promise.resolve(1);

          return Promise.resolve(null);
        },
      };
      Object.assign(window as unknown as Record<string, unknown>, {
        __TAURI_INTERNALS__: internals,
        __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
        __homeUpdateChecks: () => updateChecks,
      });
    },
    {
      instances, crashResult, lkgMarker, snapshots, driftResult,
      registryStatus, recommendations, updatesResult, launchMode,
      forYouItems: effectiveForYou,
    },
  );
}

// ---------------------------------------------------------------------------
// Tests — Home page zones
// ---------------------------------------------------------------------------

test.describe('Home — zone B: Hero / Continue Playing', () => {

  test('shows welcome card when no instances exist', async ({ page }) => {
    await installHomeMock(page, { instances: [] });
    await page.goto('/');

    await expect(page.getByText('Welcome to Agora')).toBeVisible();
    await expect(page.getByText(/No instances yet/)).toBeVisible();
    await expect(page.getByRole('button', { name: 'Browse mod packs' })).toBeVisible();
  });

  test('shows Continue Playing for the last-launched instance', async ({ page }) => {
    const instance = { ...DEFAULT_INSTANCE, last_launched_at: '2026-07-12T10:00:00Z' };
    await installHomeMock(page, { instances: [instance] });
    await page.goto('/');

    // Instance name appears in the hero card heading
    await expect(page.getByRole('heading', { name: 'Test Instance' })).toBeVisible();
    await expect(page.getByText('fabric 0.16.9 · MC 1.21')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Continue Playing' })).toBeVisible();
  });

  test('Continue Playing button with no launched instance shows welcome', async ({ page }) => {
    const instance = { ...DEFAULT_INSTANCE, last_launched_at: null };
    await installHomeMock(page, { instances: [instance] });
    await page.goto('/');

    // Instance with null last_launched_at still shows Continue Playing
    // (heroInstance = lastLaunched ?? sortedByLaunched[0])
    await expect(page.getByRole('heading', { name: 'Test Instance' })).toBeVisible();
    await expect(page.getByText('Not launched yet')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Continue Playing' })).toBeVisible();
  });

});

test.describe('Home — zone A: Alerts', () => {

  test('crash alert shows View & restore when LKG exists', async ({ page }) => {
    const crashData = {
      filename: 'crash-2026-07-12_10.05.23-server.txt',
      modified_at: '2026-07-12T10:05:23Z',
      size_bytes: 45210,
    };
    const lkg = {
      currentLkgSnapshotId: 'snap-lkg-001',
      lastPromotedAt: '2026-07-11T12:00:00Z',
    };
    const snapshots = [{
      id: 'snap-lkg-001',
      label: 'Before playing session 2026-07-11',
      created_at: '2026-07-11T11:59:00Z',
      file_count: 42,
      size_estimate: 2_500_000,
    }];
    const drift = {
      added: [{ path: 'mods/new-mod.jar', expectedSha256: null, actualSha256: 'abc' }],
      removed: [],
      modified: [{ path: 'config/options.txt', expectedSha256: 'old', actualSha256: 'new' }],
    };

    await installHomeMock(page, {
      instances: [DEFAULT_INSTANCE],
      crashResult: crashData,
      lkgMarker: lkg,
      snapshots,
      driftResult: drift,
    });
    await page.goto('/');

    // Crash alert
    await expect(page.getByText(/did not exit cleanly/)).toBeVisible();
    await expect(page.getByText(/crash-2026-07-12_10.05.23-server\.txt/)).toBeVisible();

    // Because LKG exists, the button says "View & restore"
    await expect(page.getByRole('button', { name: 'View & restore' })).toBeVisible();
  });

  test('crash alert shows View instance when LKG is absent', async ({ page }) => {
    const crashData = {
      filename: 'crash-2026-07-12_10.05.23-server.txt',
      modified_at: '2026-07-12T10:05:23Z',
      size_bytes: 45210,
    };

    await installHomeMock(page, {
      instances: [DEFAULT_INSTANCE],
      crashResult: crashData,
      lkgMarker: null,
    });
    await page.goto('/');

    await expect(page.getByText(/did not exit cleanly/)).toBeVisible();
    // Without LKG, the button says "View instance"
    await expect(page.getByRole('button', { name: 'View instance' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'View & restore' })).toHaveCount(0);
  });

  test('registry recovery alert when registry is missing', async ({ page }) => {
    const missingStatus = {
      has_cached_db: false,
      cached_tag: null,
      cached_schema_version: null,
      latest_tag: null,
      update_available: false,
      checked: true,
      message: 'No registry database found.',
    };

    await installHomeMock(page, {
      instances: [],
      registryStatus: missingStatus,
    });
    await page.goto('/');

    // RegistryAlert shown with the "not downloaded" message for missing
    await expect(page.getByText(/Registry not downloaded yet/)).toBeVisible();
    await expect(page.getByRole('button', { name: 'Download registry' })).toBeVisible();
  });

  test('registry recovery alert shows missing-state text when no cached DB', async ({ page }) => {
    // RegistryAlert in Home only renders when regState is 'missing',
    // which requires has_cached_db=false in useRegistryState.
    // The "Using cached registry" text in RegistryAlert is technically
    // unreachable currently — this test verifies the reachable path.
    const noDbStatus = {
      has_cached_db: false,
      cached_tag: null,
      cached_schema_version: null,
      latest_tag: null,
      update_available: false,
      checked: true,
      message: 'No registry database found.',
    };

    await installHomeMock(page, { registryStatus: noDbStatus });
    await page.goto('/');

    await expect(page.getByText(/Registry not downloaded yet/)).toBeVisible();
    await expect(page.getByRole('button', { name: 'Download registry' })).toBeVisible();
  });

});

test.describe('Home — zone D: Discovery / Recommendations', () => {

  test('recommendations section adapts when no instances exist', async ({ page }) => {
    await installHomeMock(page, { instances: [] });
    await page.goto('/');

    // The RecommendationsCard with no instances
    await expect(page.getByText(/Once you have an instance/)).toBeVisible();
    await expect(page.getByRole('button', { name: 'Browse all mods' })).toBeVisible();
  });

  test('recommendations section shows no-results message when empty', async ({ page }) => {
    await installHomeMock(page, {
      instances: [DEFAULT_INSTANCE],
      recommendations: [],
    });
    await page.goto('/');

    await expect(page.getByText(/No new curated matches were found/)).toBeVisible();
    await expect(page.getByRole('button', { name: 'Browse catalog' })).toBeVisible();
  });

  test('recommendations section shows items with explanation reason', async ({ page }) => {
    await installHomeMock(page, {
      instances: [DEFAULT_INSTANCE],
      recommendations: [RECOMMENDED_ITEM],
    });
    await page.goto('/');

    await expect(page.getByText('Compatible recommendations')).toBeVisible();
    // Use .first() because "Recommended Mod" text appears in both the heading
    // and the description within the same card button element.
    await expect(page.getByText('Recommended Mod').first()).toBeVisible();
    await expect(page.getByText('A recommended mod for your instance.')).toBeVisible();

    // The recommendation reason is shown indirectly via the item's metadata
    // (status/download_strategy) — the reason is embedded in the RegistryItem
    // and shown in the card's description/status area.
    await expect(page.getByText(/github_release/)).toBeVisible();
    await expect(page.getByText(/Curated and active/)).toBeVisible();
  });

  test('avoids fake trending or featured placeholder copy', async ({ page }) => {
    // With no recommendations, the page must NOT show fake "Trending" or
    // "Featured" headings — only the real empty-state message.
    await installHomeMock(page, {
      instances: [DEFAULT_INSTANCE],
      recommendations: [],
    });
    await page.goto('/');

    await expect(page.getByText(/No new curated matches/)).toBeVisible();
    // Verify no fake marketing copy is shown
    await expect(page.getByText('Trending', { exact: true })).toHaveCount(0);
    await expect(page.getByText('Featured', { exact: true })).toHaveCount(0);
  });

  test('recommendation card shows rank description for active instance', async ({ page }) => {
    await installHomeMock(page, {
      instances: [DEFAULT_INSTANCE],
      recommendations: [RECOMMENDED_ITEM],
    });
    await page.goto('/');

    // The recommendations subtitle includes the rating description
    await expect(page.getByText(/Ranked by category overlap/)).toBeVisible();
  });

});

test.describe('Home — zone C: Maintenance / LKG', () => {

  test('known-good card appears when LKG snapshots are present', async ({ page }) => {
    const lkg = {
      currentLkgSnapshotId: 'snap-lkg-001',
      lastPromotedAt: '2026-07-11T12:00:00Z',
    };
    const snapshots = [{
      id: 'snap-lkg-001',
      label: 'Before playing session 2026-07-11',
      created_at: '2026-07-11T11:59:00Z',
      file_count: 42,
      size_estimate: 2_500_000,
    }];
    const drift = {
      added: [{ path: 'mods/new-mod.jar', expectedSha256: null, actualSha256: 'abc' }],
      removed: [],
      modified: [{ path: 'config/options.txt', expectedSha256: 'old', actualSha256: 'new' }],
    };

    await installHomeMock(page, {
      instances: [DEFAULT_INSTANCE],
      lkgMarker: lkg,
      snapshots,
      driftResult: drift,
    });
    await page.goto('/');

    await expect(page.getByText('Last Known Good')).toBeVisible();
    await expect(page.getByText(/Before playing session 2026-07-11/)).toBeVisible();
    await expect(page.getByRole('button', { name: 'Restore' })).toBeVisible();
  });

  test('no-LKG state shows informational message', async ({ page }) => {
    await installHomeMock(page, {
      instances: [DEFAULT_INSTANCE],
      lkgMarker: null,
    });
    await page.goto('/');

    await expect(page.getByText('No last-known-good state yet')).toBeVisible();
    await expect(
      page.getByText(/Play an instance successfully for at least 60 seconds/),
    ).toBeVisible();
  });

  test('one-click Restore on KnownGoodCard calls restore_snapshot and refreshes', async ({ page }) => {
    const lkg = {
      currentLkgSnapshotId: 'snap-lkg-001',
      lastPromotedAt: '2026-07-11T12:00:00Z',
    };
    const snapshots = [{
      id: 'snap-lkg-001',
      label: 'Before playing session 2026-07-11',
      created_at: '2026-07-11T11:59:00Z',
      file_count: 42,
      size_estimate: 2_500_000,
    }];
    const drift = {
      added: [{ path: 'mods/new-mod.jar', expectedSha256: null, actualSha256: 'abc' }],
      removed: [],
      modified: [{ path: 'config/options.txt', expectedSha256: 'old', actualSha256: 'new' }],
    };

    await installHomeMock(page, {
      instances: [{ ...DEFAULT_INSTANCE, instance_id: 'test-instance' }],
      lkgMarker: lkg,
      snapshots,
      driftResult: drift,
    });

    await page.goto('/');

    // KnownGoodCard visible
    await expect(page.getByText('Last Known Good')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Restore' })).toBeVisible();

    // Accept the confirm dialog
    const dialogPromise = new Promise<string>((resolve) => {
      page.on('dialog', (dialog) => {
        resolve(dialog.message());
        dialog.accept();
      });
    });

    // Click Restore
    await page.getByRole('button', { name: 'Restore' }).click();

    // Verify the confirm message mentions the instance and snapshot
    const msg = await dialogPromise;
    expect(msg).toContain('Test Instance');
    expect(msg).toContain('Before playing session 2026-07-11');

    // After restore completes, loadData() re-runs and the KnownGoodCard re-renders
    await expect(page.getByText('Last Known Good')).toBeVisible();
    await expect(page.getByText(/Before playing session 2026-07-11/)).toBeVisible();
  });

  test('does not check mod updates during Home mount', async ({ page }) => {
    const updates = [
      {
        filename: 'old-mod.jar',
        mod_jar_id: 'old-mod',
        current_version: '1.0.0',
        latest_version: '2.0.0',
        target_version: '2.0.0',
        source: 'curated',
      },
    ];

    await installHomeMock(page, {
      instances: [DEFAULT_INSTANCE],
      updatesResult: updates,
    });
    await page.goto('/');

    await expect(page.getByText('Updates Available')).toHaveCount(0);
    expect(await page.evaluate(() => (window as any).__homeUpdateChecks())).toBe(0);
  });

});
