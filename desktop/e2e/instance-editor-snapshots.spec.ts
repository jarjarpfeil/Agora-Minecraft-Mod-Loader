import { test, expect, type Page } from '@playwright/test';

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const DEFAULT_SNAPSHOTS: Record<string, unknown>[] = [
  {
    id: 'snap-lkg-current',
    label: 'Before session 2026-07-12',
    created_at: '2026-07-12T09:00:00Z',
    file_count: 42,
    size_estimate: 2_500_000,
    is_lkg: true,
    is_current_lkg: true,
    is_pre_restore: false,
  },
  {
    id: 'snap-known-good',
    label: 'Checkpoint before update',
    created_at: '2026-07-10T14:00:00Z',
    file_count: 38,
    size_estimate: 2_100_000,
    is_lkg: true,
    is_current_lkg: false,
    is_pre_restore: false,
  },
  {
    id: 'snap-undo',
    label: 'Before restore 2026-07-11',
    created_at: '2026-07-11T08:00:00Z',
    file_count: 41,
    size_estimate: 2_400_000,
    is_lkg: false,
    is_current_lkg: false,
    is_pre_restore: true,
  },
  {
    id: 'snap-plain',
    label: 'Manual backup',
    created_at: '2026-07-09T16:00:00Z',
    file_count: 40,
    size_estimate: 2_300_000,
    is_lkg: false,
    is_current_lkg: false,
    is_pre_restore: false,
  },
];

const PLAIN_SNAPSHOTS: Record<string, unknown>[] = [
  {
    id: 'snap-plain',
    label: 'Manual backup',
    created_at: '2026-07-09T16:00:00Z',
    file_count: 40,
    size_estimate: 2_300_000,
    is_lkg: false,
    is_current_lkg: false,
    is_pre_restore: false,
  },
];

const DRIFT_RESULT: Record<string, unknown> = {
  fromId: 'snap-to-diff',
  toId: null,
  added: [
    { path: 'mods/new-mod.jar', oldSha256: null, newSha256: 'abc123', oldSize: null, newSize: 50000 },
  ],
  removed: [
    { path: 'mods/old-mod.jar', oldSha256: 'def456', newSha256: null, oldSize: 40000, newSize: null },
  ],
  modified: [
    { path: 'config/options.txt', oldSha256: 'oldhash', newSha256: 'newhash', oldSize: 200, newSize: 210 },
  ],
  unchangedCount: 37,
  totalFilesA: 40,
  totalFilesB: 41,
};

const INSTANCE_DETAIL: Record<string, unknown> = {
  row: {
    instance_id: 'test-instance',
    name: 'Test Instance',
    minecraft_version: '26.2',
    loader: 'fabric',
    loader_version: '0.16.9',
    is_modpack: false,
    is_locked: false,
    last_launched_at: '2026-07-12T10:00:00Z',
    jvm_memory_mb: 4096,
    jvm_gc: 'G1GC',
    jvm_custom_args: '',
    created_at: '2026-06-01T00:00:00Z',
  },
  manifest: {
    instance_id: 'test-instance',
    name: 'Test Instance',
    created_from_pack: null,
    minecraft_version: '26.2',
    loader: 'fabric',
    loader_version: '0.16.9',
    is_locked: false,
    mods: [],
    resourcepacks: [],
    shaders: [],
    datapacks: [],
    worlds: [],
    user_preferences: {},
  },
};

// ---------------------------------------------------------------------------
// Mock installer
// ---------------------------------------------------------------------------

interface SnapshotEditorMockOptions {
  snapshots?: Record<string, unknown>[];
  driftResult?: Record<string, unknown>;
  restoreReject?: boolean;
}

async function installSnapshotEditorMock(page: Page, opts: SnapshotEditorMockOptions = {}) {
  const {
    snapshots = DEFAULT_SNAPSHOTS,
    driftResult = DRIFT_RESULT,
    restoreReject = false,
  } = opts;

  await page.addInitScript(
    (params: {
      snapshots: Record<string, unknown>[];
      driftResult: Record<string, unknown>;
      restoreReject: boolean;
      detail: Record<string, unknown>;
    }) => {
      const { snapshots, driftResult, restoreReject, detail } = params;

      const callbacks = new Map<number, (...args: unknown[]) => void>();
      let callbackId = 0;
      const commandCalls: Record<string, number> = {};
      const commandArgs: Record<string, Record<string, unknown>> = {};

      const internals = {
        transformCallback(callback: (...args: unknown[]) => void) {
          const id = ++callbackId;
          callbacks.set(id, callback);
          return id;
        },
        unregisterCallback(id: number) { callbacks.delete(id); },
        invoke(command: string, args: Record<string, unknown> = {}) {
          commandCalls[command] = (commandCalls[command] ?? 0) + 1;
          commandArgs[command] = args;

          if (command === 'get_setting') {
            const key = args.key as string;
            if (key === 'onboarding_complete') return Promise.resolve(true);
             if (key === 'launch_mode') return Promise.resolve('delegation');
             if (key === 'modrinth_enabled') return Promise.resolve(true);
             if (key === 'advanced_mode') return Promise.resolve('true');
             return Promise.resolve(null);
          }
          if (command === 'set_setting') return Promise.resolve(null);
          if (command === 'get_windows_accent_color') return Promise.resolve(null);
          if (command === 'get_registry_status') {
            return Promise.resolve({
              has_cached_db: true,
              cached_tag: 'test',
              cached_schema_version: 5,
              latest_tag: 'test',
              update_available: false,
              checked: true,
              message: 'Registry ready.',
            });
          }
          if (command.startsWith('plugin:event|')) return Promise.resolve(1);
           if (command === 'get_instance_detail') return Promise.resolve(detail);
           if (command === 'compute_gc_args') {
             const profile = args.gcMode === 'auto'
               ? ((args.javaVersion as number) >= 21 ? 'low_latency' : 'high_efficiency')
               : args.gcMode;
             return Promise.resolve({
               profile,
               jvm_args: `${profile} ${args.alwaysPreTouch ? 'AlwaysPreTouch' : 'NoPreTouch'} ${args.manualArgs ?? ''}`,
               heap_mb: args.requestedHeapMb,
               total_ram_mb: 32768,
               cpu_threads: 8,
               recommended: args.gcMode === 'auto',
             });
           }
           if (command === 'list_categories') return Promise.resolve([]);
          if (command === 'list_snapshots') return Promise.resolve(snapshots);
          if (command === 'detect_drift') return Promise.resolve(driftResult);
          if (command === 'restore_snapshot') {
            return restoreReject
              ? Promise.reject(new Error('Snapshot restore failed: disk I/O error'))
              : Promise.resolve(null);
          }
          if (command === 'list_instances') return Promise.resolve([]);
          if (command === 'check_registry_update') return Promise.resolve(null);
          if (command === 'list_manifest_loaders') return Promise.resolve([]);
          if (command === 'list_manifest_mc_versions') return Promise.resolve([]);
          if (command === 'for_you_items') return Promise.resolve([]);

          return Promise.resolve(null);
        },
      };
      Object.assign(window as unknown as Record<string, unknown>, {
        __TAURI_INTERNALS__: internals,
        __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
        __commandCalls: commandCalls,
        __commandArgs: commandArgs,
      });
    },
    { snapshots, driftResult, restoreReject, detail: INSTANCE_DETAIL },
  );
}

// ---------------------------------------------------------------------------
// Navigation helpers
// ---------------------------------------------------------------------------

async function navigateToInstanceEditor(page: Page) {
  await page.addInitScript(() => {
    window.history.replaceState({ __agora: { type: 'instance-detail', instanceId: 'test-instance' } }, '');
  });
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Test Instance' })).toBeVisible();
}

async function openSnapshotsTab(page: Page) {
  await page.getByRole('button', { name: 'Snapshots' }).click();
  // Wait for the snapshot section heading to appear
  await expect(page.getByRole('heading', { name: 'Snapshots' })).toBeVisible();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

test.describe('InstanceEditor — Snapshots tab: badges', () => {

  test('shows Current LKG badge on the current known-good snapshot', async ({ page }) => {
    await installSnapshotEditorMock(page, { snapshots: DEFAULT_SNAPSHOTS });
    await navigateToInstanceEditor(page);
    await openSnapshotsTab(page);

    // The snapshot with is_current_lkg=true shows "Current LKG"
    const currentLkgRow = page.getByText('Before session 2026-07-12').locator('..');
    await expect(currentLkgRow.getByText('Current LKG')).toBeVisible();
  });

  test('shows Known good badge on LKG snapshots that are not current', async ({ page }) => {
    await installSnapshotEditorMock(page, { snapshots: DEFAULT_SNAPSHOTS });
    await navigateToInstanceEditor(page);
    await openSnapshotsTab(page);

    // The snapshot with is_lkg=true but is_current_lkg=false shows "Known good"
    const knownGoodRow = page.getByText('Checkpoint before update').locator('..');
    await expect(knownGoodRow.getByText('Known good')).toBeVisible();
  });

  test('shows Undo restore badge on pre-restore snapshots', async ({ page }) => {
    await installSnapshotEditorMock(page, { snapshots: DEFAULT_SNAPSHOTS });
    await navigateToInstanceEditor(page);
    await openSnapshotsTab(page);

    // The snapshot with is_pre_restore=true shows "Undo restore"
    const undoRow = page.getByText('Before restore 2026-07-11').locator('..');
    await expect(undoRow.getByText('Undo restore')).toBeVisible();
  });

  test('snapshot without any badge has no badge text', async ({ page }) => {
    await installSnapshotEditorMock(page, { snapshots: DEFAULT_SNAPSHOTS });
    await navigateToInstanceEditor(page);
    await openSnapshotsTab(page);

    // Plain snapshot has neither LKG nor restore badges
    const plainRow = page.getByText('Manual backup').locator('..');
    await expect(plainRow.getByText('Current LKG')).toHaveCount(0);
    await expect(plainRow.getByText('Known good')).toHaveCount(0);
    await expect(plainRow.getByText('Undo restore')).toHaveCount(0);
  });

});

test('JVM preview follows GC and pre-touch controls', async ({ page }) => {
  await installSnapshotEditorMock(page);
  await navigateToInstanceEditor(page);

  await page.getByRole('button', { name: 'Java & Args' }).click();
  await expect(page.getByText(/^Generational ZGC ·/)).toBeVisible();
  await expect(page.getByRole('checkbox', { name: 'Automatic GC selection' })).toBeChecked();

  await page.getByRole('checkbox', { name: 'Automatic GC selection' }).uncheck();
  await expect(page.getByText(/^Tuned G1GC ·/)).toBeVisible();
  await page.getByRole('checkbox', { name: 'Pre-touch allocated memory' }).uncheck();
  await expect.poll(async () => page.evaluate(() => (
    (window as unknown as { __commandArgs: Record<string, Record<string, unknown>> }).__commandArgs
      .compute_gc_args?.alwaysPreTouch
  ))).toBe(false);
  await expect(page.getByText('NoPreTouch')).toBeVisible();

  await page.getByRole('checkbox', { name: 'Automatic GC selection' }).uncheck();
  await page.getByRole('combobox', { name: 'Garbage collector' }).selectOption('manual');
  await page.getByLabel('Additional JVM flags').fill('-Xss1M');
  await expect(page.getByText(/^Manual JVM flags ·/)).toBeVisible();
  await expect(page.locator('code').filter({ hasText: '-Xss1M' })).toBeVisible();
});

test.describe('InstanceEditor — Snapshots tab: diff rendering', () => {

  test('shows diff summary with added, removed, modified, unchanged counts', async ({ page }) => {
    await installSnapshotEditorMock(page, {
      snapshots: DEFAULT_SNAPSHOTS,
      driftResult: DRIFT_RESULT,
    });
    await navigateToInstanceEditor(page);
    await openSnapshotsTab(page);

    // Click "Show diff" on the first snapshot
    await page.getByRole('button', { name: 'Show diff' }).first().click();

    // Diff summary appears with counts
    await expect(page.getByText(/\+1 added/)).toBeVisible();
    await expect(page.getByText(/-1 removed/)).toBeVisible();
    await expect(page.getByText(/~1 modified/)).toBeVisible();
    await expect(page.getByText(/37 unchanged/)).toBeVisible();
  });

  test('shows added, removed, and modified paths in diff list', async ({ page }) => {
    await installSnapshotEditorMock(page, {
      snapshots: DEFAULT_SNAPSHOTS,
      driftResult: DRIFT_RESULT,
    });
    await navigateToInstanceEditor(page);
    await openSnapshotsTab(page);

    await page.getByRole('button', { name: 'Show diff' }).first().click();

    // Added path
    await expect(page.getByText('mods/new-mod.jar')).toBeVisible();
    await expect(page.getByText('(Added)')).toBeVisible();

    // Removed path
    await expect(page.getByText('mods/old-mod.jar')).toBeVisible();
    await expect(page.getByText('(Removed)')).toBeVisible();

    // Modified path
    await expect(page.getByText('config/options.txt')).toBeVisible();
    await expect(page.getByText('(Modified)')).toBeVisible();
  });

  test('shows unchanged message when diff is empty', async ({ page }) => {
    const emptyDiff = {
      fromId: 'snap-plain',
      toId: null,
      added: [],
      removed: [],
      modified: [],
      unchangedCount: 40,
      totalFilesA: 40,
      totalFilesB: 40,
    };

    await installSnapshotEditorMock(page, {
      snapshots: PLAIN_SNAPSHOTS,
      driftResult: emptyDiff,
    });
    await navigateToInstanceEditor(page);
    await openSnapshotsTab(page);

    await page.getByRole('button', { name: 'Show diff' }).first().click();

    // The inline "exact match" message
    await expect(page.getByText('The current instance matches this snapshot exactly.')).toBeVisible();
  });

  test('diff section closes when clicking Close', async ({ page }) => {
    await installSnapshotEditorMock(page, {
      snapshots: DEFAULT_SNAPSHOTS,
      driftResult: DRIFT_RESULT,
    });
    await navigateToInstanceEditor(page);
    await openSnapshotsTab(page);

    await page.getByRole('button', { name: 'Show diff' }).first().click();

    // Verify diff is visible
    await expect(page.getByText(/\+1 added/)).toBeVisible();

    // Close the diff
    await page.getByRole('button', { name: 'Close' }).click();

    // Diff should be gone
    await expect(page.getByText(/\+1 added/)).toHaveCount(0);
  });

});

test.describe('InstanceEditor — Snapshots tab: restore flow', () => {

  test('restore calls restore_snapshot and refreshes snapshots', async ({ page }) => {
    await installSnapshotEditorMock(page, {
      snapshots: DEFAULT_SNAPSHOTS,
      restoreReject: false,
    });
    await navigateToInstanceEditor(page);
    await openSnapshotsTab(page);

    // Click first Restore button
    await page.getByRole('button', { name: 'Restore' }).first().click();

    // Verify restore_snapshot was called via tracking
    const wasCalled = await page.evaluate(() => {
      const calls = (window as any).__commandCalls as Record<string, number>;
      return (calls['restore_snapshot'] ?? 0) >= 1;
    });
    expect(wasCalled).toBe(true);

    // Verify list_snapshots was called again after restore (refresh)
    const listSnapshotCalls = await page.evaluate(() => {
      const calls = (window as any).__commandCalls as Record<string, number>;
      return calls['list_snapshots'] ?? 0;
    });
    expect(listSnapshotCalls).toBeGreaterThanOrEqual(2);

    // Verify get_instance_detail was called to refresh the detail
    const detailCalls = await page.evaluate(() => {
      const calls = (window as any).__commandCalls as Record<string, number>;
      return calls['get_instance_detail'] ?? 0;
    });
    expect(detailCalls).toBeGreaterThanOrEqual(2);

    // No error message is shown after successful restore
    await expect(page.getByText('Snapshot restore failed')).toHaveCount(0);

    // Snapshots section heading is still visible after refresh
    await expect(page.getByRole('heading', { name: 'Snapshots' })).toBeVisible();
  });

  test('restore failure shows error and remains recoverable', async ({ page }) => {
    await installSnapshotEditorMock(page, {
      snapshots: DEFAULT_SNAPSHOTS,
      restoreReject: true,
    });
    await navigateToInstanceEditor(page);
    await openSnapshotsTab(page);

    // Click first Restore button
    await page.getByRole('button', { name: 'Restore' }).first().click();

    // Error should be displayed
    await expect(page.getByText('Snapshot restore failed: disk I/O error')).toBeVisible();

    // UI is still interactive — Restore buttons are still present and enabled
    await expect(page.getByRole('button', { name: 'Restore' }).first()).toBeEnabled();

    // The instance heading is still visible (component is still functional)
    await expect(page.getByRole('heading', { name: 'Test Instance' })).toBeVisible();
  });

});

test.describe('InstanceEditor — Snapshots tab: empty state', () => {

  test('shows empty message when no snapshots exist', async ({ page }) => {
    await installSnapshotEditorMock(page, { snapshots: [] });
    await navigateToInstanceEditor(page);
    await openSnapshotsTab(page);

    await expect(page.getByText('No snapshots yet. Create one to save a restore point.')).toBeVisible();
  });

});
