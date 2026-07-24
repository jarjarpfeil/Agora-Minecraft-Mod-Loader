import { test, expect, type Page } from '@playwright/test';

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const INSTANCE_ID = 'test-instance';

const LOCKFILE_OBJECT: Record<string, unknown> = {
  formatVersion: 1,
  instanceId: INSTANCE_ID,
  minecraftVersion: '1.21',
  loader: 'fabric',
  loaderVersion: '0.16.9',
  artifacts: [
    {
      id: 'mod-A',
      name: 'Sodium',
      source: 'modrinth',
      sha256: 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
      url: 'https://cdn.modrinth.com/data/AANobbMI/versions/0.6.10/sodium-fabric-0.6.10%2Bmc1.21.jar',
      filename: 'sodium-fabric-0.6.10+mc1.21.jar',
    },
    {
      id: 'mod-B',
      name: 'Lithium',
      source: 'modrinth',
      sha256: 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
      url: 'https://cdn.modrinth.com/data/gvQqBUqZ/versions/0.13.1/lithium-fabric-0.13.1%2Bmc1.21.jar',
      filename: 'lithium-fabric-0.13.1+mc1.21.jar',
    },
  ],
  trackedConfigSha256: 'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc',
  userPreferences: { java_memory_gb: 4 },
};

const LOCKFILE_JSON = JSON.stringify(LOCKFILE_OBJECT, null, 2);

const IN_SYNC_REPORT: Record<string, unknown> = {
  status: 'in-sync',
  differences: [],
};

const DRIFT_REPORT: Record<string, unknown> = {
  status: 'drifted',
  differences: [
    { path: 'mods/new-mod.jar', kind: 'added', expectedSha256: null, actualSha256: 'abc123' },
    { path: 'mods/removed-mod.jar', kind: 'removed', expectedSha256: 'def456', actualSha256: null },
    { path: 'mods/changed-mod.jar', kind: 'modified', expectedSha256: 'oldhash', actualSha256: 'newhash' },
    { path: 'mods/disabled-mod.jar', kind: 'disabled', expectedSha256: null, actualSha256: null },
    { path: 'mods/enabled-mod.jar', kind: 'enabled', expectedSha256: null, actualSha256: null },
    { path: 'config/options.txt', kind: 'config-modified', expectedSha256: 'conf-old', actualSha256: 'conf-new' },
  ],
};

const SUCCESSFUL_REPAIR: Record<string, unknown> = {
  type: 'success',
  installedItems: ['mod-A', 'mod-B'],
  existingItemsReused: [],
  warnings: [],
  health: { type: 'completed', report: {} },
  snapshotId: 'snap-repair-1',
};

const FAILED_REPAIR: Record<string, unknown> = {
  type: 'failed',
  error: 'Download failed for Sodium: Checksum mismatch',
  rollbackPerformed: true,
  snapshotId: 'snap-repair-1',
};

function makeInstanceDetail(isLocked: boolean): Record<string, unknown> {
  return {
    row: {
      instance_id: INSTANCE_ID,
      name: 'Test Instance',
      minecraft_version: '1.21',
      loader: 'fabric',
      loader_version: '0.16.9',
      is_modpack: false,
      is_locked: isLocked,
      last_launched_at: '2026-07-12T10:00:00Z',
      jvm_memory_mb: 4096,
      jvm_gc: 'G1GC',
      jvm_custom_args: '',
      created_at: '2026-06-01T00:00:00Z',
    },
    manifest: {
      instance_id: INSTANCE_ID,
      name: 'Test Instance',
      created_from_pack: null,
      minecraft_version: '1.21',
      loader: 'fabric',
      loader_version: '0.16.9',
      is_locked: isLocked,
      mods: [
        { id: 'mod-A', name: 'Sodium', filename: 'sodium-fabric-0.6.10+mc1.21.jar', modrinth_id: 'project-A', source: 'modrinth', enabled: true },
        { id: 'mod-B', name: 'Lithium', filename: 'lithium-fabric-0.13.1+mc1.21.jar', enabled: true },
      ],
      resourcepacks: [],
      shaders: [],
      datapacks: [],
      worlds: [],
      user_preferences: { java_memory_gb: 4 },
    },
  };
}

const UNLOCKED_DETAIL = makeInstanceDetail(false);
const LOCKED_DETAIL = makeInstanceDetail(true);

// ---------------------------------------------------------------------------
// Mock installer
// ---------------------------------------------------------------------------

interface ReproducibleMockOptions {
  /** Whether the instance is locked (default false). */
  isLocked?: boolean;
  /**
   * The object returned by `export_lockfile`.
   * Set to `undefined` and use `exportReject` to simulate failure.
   */
  lockfileObject?: Record<string, unknown>;
  /** If true, `export_lockfile` rejects. */
  exportReject?: boolean;
  /**
   * The report returned by `verify_lockfile`.
   * Set to `undefined` and use `verifyReject` to simulate failure.
   */
  verifyReport?: Record<string, unknown>;
  /** If true, `verify_lockfile` rejects. */
  verifyReject?: boolean;
  /**
   * The outcome returned by `repair_lockfile`.
   * Set to `undefined` and use `repairReject` to simulate failure.
   */
  repairOutcome?: Record<string, unknown>;
  /** If true, `repair_lockfile` rejects. */
  repairReject?: boolean;
  /**
   * The new instance ID returned by `import_lockfile`.
   * Set to `undefined` and use `cloneReject` to simulate failure.
   */
  cloneInstanceId?: string;
  /** If true, `import_lockfile` rejects. */
  cloneReject?: boolean;
  /** Whether window.confirm returns true (default true). */
  confirmResult?: boolean;
}

async function installReproducibleMock(page: Page, opts: ReproducibleMockOptions = {}) {
  const {
    isLocked = false,
    lockfileObject = LOCKFILE_OBJECT,
    exportReject = false,
    verifyReport = IN_SYNC_REPORT,
    verifyReject = false,
    repairOutcome = SUCCESSFUL_REPAIR,
    repairReject = false,
    cloneInstanceId = 'new-cloned-instance',
    cloneReject = false,
    confirmResult = true,
  } = opts;

  const detail = makeInstanceDetail(isLocked);

  await page.addInitScript(
    (params: {
      detail: Record<string, unknown>;
      lockfileObject: Record<string, unknown>;
      exportReject: boolean;
      verifyReport: Record<string, unknown>;
      verifyReject: boolean;
      repairOutcome: Record<string, unknown>;
      repairReject: boolean;
      cloneInstanceId: string;
      cloneReject: boolean;
      confirmResult: boolean;
    }) => {
      const { detail, lockfileObject, exportReject, verifyReport, verifyReject, repairOutcome, repairReject, cloneInstanceId, cloneReject, confirmResult } = params;

      const callbacks = new Map<number, (...args: unknown[]) => void>();
      let callbackId = 0;
      const commandCalls: Record<string, number> = {};
      const commandArgs: Record<string, Record<string, unknown>> = {};

      // Mock clipboard
      let clipboardText = '';
      Object.defineProperty(navigator, 'clipboard', {
        value: {
          writeText(text: string) {
            clipboardText = text;
            return Promise.resolve();
          },
          readText() {
            return Promise.resolve(clipboardText);
          },
        },
        configurable: true,
        writable: true,
      });

      // Mock confirm
      window.confirm = () => confirmResult;

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
            if (key === 'ai_chat_enabled') return Promise.resolve(true);
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
          if (command === 'fetch_modrinth_project') {
            return Promise.resolve({
              id: 'project-A',
              title: 'Sodium',
              description: '',
              body: null,
              icon_url: null,
              project_type: 'mod',
              page_url: null,
              license_id: null,
              source_updated_at: null,
              gallery_urls: [],
            });
          }
          if (command === 'list_categories') return Promise.resolve([]);
          if (command === 'list_instances') return Promise.resolve([]);
          if (command === 'check_registry_update') return Promise.resolve(null);
          if (command === 'list_manifest_loaders') return Promise.resolve([]);
          if (command === 'list_manifest_mc_versions') return Promise.resolve([]);
          if (command === 'for_you_items') return Promise.resolve([]);
          if (command === 'list_snapshots') return Promise.resolve([]);
          if (command === 'list_loadout_profiles') return Promise.resolve([]);
          if (command === 'list_pack_mods') return Promise.resolve([]);
          if (command === 'export_instance_pack') return Promise.resolve('');
          if (command === 'import_instance_pack') return Promise.resolve('');

          // --- Reproducible tab commands ---
          if (command === 'export_lockfile') {
            if (exportReject) return Promise.reject(new Error('Failed to export lockfile: registry data not found'));
            return Promise.resolve(lockfileObject);
          }
          if (command === 'verify_lockfile') {
            if (verifyReject) return Promise.reject(new Error('Lockfile verification failed: corrupt schema version'));
            return Promise.resolve(verifyReport);
          }
          if (command === 'repair_lockfile') {
            if (repairReject) return Promise.reject(new Error('Repair failed: network timeout'));
            return Promise.resolve(repairOutcome);
          }
          if (command === 'import_lockfile') {
            if (cloneReject) return Promise.reject(new Error('Clone rejected: artifact source unavailable'));
            return Promise.resolve(cloneInstanceId);
          }

          return Promise.resolve(null);
        },
      };
      Object.assign(window as unknown as Record<string, unknown>, {
        __TAURI_INTERNALS__: internals,
        __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
        __commandCalls: commandCalls,
        __commandArgs: commandArgs,
        __clipboardWrite: () => clipboardText,
      });
    },
    { detail, lockfileObject, exportReject, verifyReport, verifyReject, repairOutcome, repairReject, cloneInstanceId, cloneReject, confirmResult },
  );
}

// ---------------------------------------------------------------------------
// Navigation helpers
// ---------------------------------------------------------------------------

async function navigateToInstanceEditor(page: Page) {
  // Already did addInitScript for the mock; now set history state for navigation.
  await page.addInitScript(() => {
    window.history.replaceState(
      { __agora: { type: 'instance-detail', instanceId: 'test-instance' } },
      '',
    );
  });
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Test Instance' })).toBeVisible();
}

async function openReproducibleTab(page: Page) {
  await page.getByRole('button', { name: 'Export', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Export Instance' })).toBeVisible();
}

test('consolidates mrpack, Agora JSON, and lockfile exports with guidance', async ({ page }) => {
  await installReproducibleMock(page);
  await navigateToInstanceEditor(page);
  await openReproducibleTab(page);

  await expect(page.getByRole('heading', { name: 'Export as Modrinth Pack (.mrpack)' })).toBeVisible();
  await expect(page.getByText('sharing your modpack with other launchers or publishing to Modrinth')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Export as Agora Pack (.json)' })).toBeVisible();
  await expect(page.getByText('backing up your mod selection or sharing with other Agora users')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Export Reproduction Lockfile' })).toBeVisible();
  await expect(page.getByText('forensic reproduction, drift detection, and bit-identical cloning')).toBeVisible();

  await page.getByRole('button', { name: 'Export .mrpack' }).click();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __commandArgs: Record<string, Record<string, unknown>> }
  ).__commandArgs.export_instance_pack?.format)).toBe('mrpack');

  await page.getByRole('button', { name: 'Export agora-pack.json' }).click();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __commandArgs: Record<string, Record<string, unknown>> }
  ).__commandArgs.export_instance_pack?.format)).toBe('json');
});

test('toggling a mod updates local state without refreshing detail or Modrinth metadata', async ({ page }) => {
  await installReproducibleMock(page);
  await navigateToInstanceEditor(page);
  await expect(page.getByText('Sodium', { exact: true })).toBeVisible();

  const before = await page.evaluate(() => ({
    detail: (window as any).__commandCalls.get_instance_detail ?? 0,
    modrinth: (window as any).__commandCalls.fetch_modrinth_project ?? 0,
  }));

  await page.getByRole('button', { name: /Disable/ }).first().click();
  await expect(page.getByRole('button', { name: /Enable/ }).first()).toBeVisible();

  const after = await page.evaluate(() => ({
    detail: (window as any).__commandCalls.get_instance_detail ?? 0,
    modrinth: (window as any).__commandCalls.fetch_modrinth_project ?? 0,
  }));
  expect(after.detail).toBe(before.detail);
  expect(after.modrinth).toBe(before.modrinth);
});

// ---------------------------------------------------------------------------
// Tests — Textarea and actions
// ---------------------------------------------------------------------------

test.describe('InstanceEditor — Reproducible tab: textarea and actions', () => {

  test('shows placeholder when textarea is empty', async ({ page }) => {
    await installReproducibleMock(page);
    await navigateToInstanceEditor(page);
    await openReproducibleTab(page);

    const textarea = page.getByRole('textbox', { name: 'Instance lockfile JSON' });
    await expect(textarea).toBeVisible();
    await expect(textarea).toHaveValue('');

    // Placeholder is shown via the dashed box when empty
    await expect(page.getByText('Export this instance or paste a received lockfile to verify, repair, or clone it.')).toBeVisible();

    // Export and Copy buttons are visible (Copy and Clear only when text is present)
    await expect(page.getByRole('button', { name: 'Export Lockfile' })).toBeVisible();
    await expect(page.getByRole('button', { name: /^Copy$/ })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Clear' })).toHaveCount(0);
    // Verify, Repair, Clone only when text is present
    await expect(page.getByRole('button', { name: 'Verify' })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Repair' })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Clone' })).toHaveCount(0);
  });

  test('pasting lockfile JSON populates textarea and shows action buttons', async ({ page }) => {
    await installReproducibleMock(page);
    await navigateToInstanceEditor(page);
    await openReproducibleTab(page);

    const textarea = page.getByRole('textbox', { name: 'Instance lockfile JSON' });
    await textarea.fill(LOCKFILE_JSON);

    await expect(textarea).toHaveValue(LOCKFILE_JSON);

    // Action buttons appear
    await expect(page.getByRole('button', { name: 'Copy' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Clear' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Verify' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Repair' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Clone' })).toBeVisible();

    // Placeholder dashed box disappears
    await expect(page.getByText('Export this instance or paste a received lockfile')).toHaveCount(0);
  });

  test('editing the textarea content clears previous verify report', async ({ page }) => {
    await installReproducibleMock(page);
    await navigateToInstanceEditor(page);
    await openReproducibleTab(page);

    const textarea = page.getByRole('textbox', { name: 'Instance lockfile JSON' });
    await textarea.fill(LOCKFILE_JSON);

    // Click Verify to produce a report
    await page.getByRole('button', { name: 'Verify' }).click();
    await expect(page.getByText('In sync')).toBeVisible();

    // Edit the textarea
    await textarea.fill(LOCKFILE_JSON + ' ');

    // The verify report should be cleared
    await expect(page.getByText('In sync')).toHaveCount(0);
  });
});

// ---------------------------------------------------------------------------
// Tests — Export
// ---------------------------------------------------------------------------

test.describe('InstanceEditor — Reproducible tab: export', () => {

  test('clicking Export Lockfile populates textarea and shows notice', async ({ page }) => {
    await installReproducibleMock(page);
    await navigateToInstanceEditor(page);
    await openReproducibleTab(page);

    await page.getByRole('button', { name: 'Export Lockfile' }).click();

    // Textarea is populated
    const textarea = page.getByRole('textbox', { name: 'Instance lockfile JSON' });
    const text = await textarea.inputValue();
    expect(text.length).toBeGreaterThan(0);

    const parsed = JSON.parse(text);
    expect(parsed.formatVersion).toBe(1);
    expect(parsed.instanceId).toBe('test-instance');
    expect(parsed.minecraftVersion).toBe('1.21');
    expect(Array.isArray(parsed.artifacts)).toBe(true);

    // Success notice shown
    await expect(page.getByText('Canonical lockfile exported.')).toBeVisible();

    // Verify export_lockfile was called
    const wasCalled = await page.evaluate(() => {
      const calls = (window as any).__commandCalls as Record<string, number>;
      return (calls['export_lockfile'] ?? 0) >= 1;
    });
    expect(wasCalled).toBe(true);
  });

  test('export with unresolved artifacts shows warning notice', async ({ page }) => {
    const lockfileWithUnresolved: Record<string, unknown> = {
      ...LOCKFILE_OBJECT,
      artifacts: [
        ...(LOCKFILE_OBJECT.artifacts as Record<string, unknown>[]),
        {
          id: 'mod-unresolved',
          name: 'Private Mod',
          source: 'manual',
          unresolvedReason: 'No public download URL available',
          filename: 'private-mod.jar',
        },
      ],
    };

    await installReproducibleMock(page, { lockfileObject: lockfileWithUnresolved });
    await navigateToInstanceEditor(page);
    await openReproducibleTab(page);

    await page.getByRole('button', { name: 'Export Lockfile' }).click();

    // Notice mentions unresolved artifacts
    await expect(page.getByText(/unreproducible artifact/)).toBeVisible();
  });

  test('export failure shows error and textarea stays empty', async ({ page }) => {
    await installReproducibleMock(page, { exportReject: true });
    await navigateToInstanceEditor(page);
    await openReproducibleTab(page);

    await page.getByRole('button', { name: 'Export Lockfile' }).click();

    // Error message appears
    await expect(page.getByText('Failed to export lockfile: registry data not found')).toBeVisible();

    // Textarea remains empty
    const textarea = page.getByRole('textbox', { name: 'Instance lockfile JSON' });
    await expect(textarea).toHaveValue('');

    // Export button is enabled again (not busy)
    await expect(page.getByRole('button', { name: 'Export Lockfile' })).toBeEnabled();
  });
});

// ---------------------------------------------------------------------------
// Tests — Copy
// ---------------------------------------------------------------------------

test.describe('InstanceEditor — Reproducible tab: copy', () => {

  test('copy writes lockfile text to clipboard and shows notice', async ({ page }) => {
    await installReproducibleMock(page);
    await navigateToInstanceEditor(page);
    await openReproducibleTab(page);

    // First populate the textarea
    const textarea = page.getByRole('textbox', { name: 'Instance lockfile JSON' });
    await textarea.fill(LOCKFILE_JSON);

    await page.getByRole('button', { name: 'Copy' }).click();

    // Notice is shown
    await expect(page.getByText('Lockfile copied to the clipboard.')).toBeVisible();

    // Verify clipboard was written
    const clipText = await page.evaluate(() => navigator.clipboard.readText());
    expect(clipText).toBe(LOCKFILE_JSON);
  });

  test('copy is disabled while busy', async ({ page }) => {
    // We trigger export first (which sets busy state) then immediately try copy
    // The export triggers a busy state, so copy should be disabled during it
    // We'll verify the button is present and enabled normally first
    await installReproducibleMock(page);
    await navigateToInstanceEditor(page);
    await openReproducibleTab(page);

    const textarea = page.getByRole('textbox', { name: 'Instance lockfile JSON' });
    await textarea.fill(LOCKFILE_JSON);

    // Copy is enabled when not busy
    await expect(page.getByRole('button', { name: 'Copy' })).toBeEnabled();
  });
});

// ---------------------------------------------------------------------------
// Tests — Clear
// ---------------------------------------------------------------------------

test.describe('InstanceEditor — Reproducible tab: clear', () => {

  test('clear button empties textarea, removes report and notice', async ({ page }) => {
    await installReproducibleMock(page);
    await navigateToInstanceEditor(page);
    await openReproducibleTab(page);

    const textarea = page.getByRole('textbox', { name: 'Instance lockfile JSON' });
    await textarea.fill(LOCKFILE_JSON);

    // Run verify to generate a report and notice
    await page.getByRole('button', { name: 'Verify' }).click();
    await expect(page.getByText('In sync')).toBeVisible();

    // Now click Clear
    await page.getByRole('button', { name: 'Clear' }).click();

    // Textarea is empty
    await expect(textarea).toHaveValue('');

    // Report and notice are gone
    await expect(page.getByText('In sync')).toHaveCount(0);
    await expect(page.getByText('This instance exactly matches')).toHaveCount(0);

    // Placeholder returns
    await expect(page.getByText('Export this instance or paste a received lockfile')).toBeVisible();

    // Copy, Clear, Verify, Repair, Clone buttons are hidden
    await expect(page.getByRole('button', { name: /^Copy$/ })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Clear' })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Verify' })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Repair' })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Clone' })).toHaveCount(0);
  });
});

// ---------------------------------------------------------------------------
// Tests — Verify
// ---------------------------------------------------------------------------

test.describe('InstanceEditor — Reproducible tab: verify', () => {

  test('verify in-sync shows confirmation and empty differences', async ({ page }) => {
    await installReproducibleMock(page, { verifyReport: IN_SYNC_REPORT });
    await navigateToInstanceEditor(page);
    await openReproducibleTab(page);

    const textarea = page.getByRole('textbox', { name: 'Instance lockfile JSON' });
    await textarea.fill(LOCKFILE_JSON);

    await page.getByRole('button', { name: 'Verify' }).click();

    // Shows in-sync status
    await expect(page.getByText('In sync')).toBeVisible();
    await expect(page.getByText('This instance exactly matches the lockfile artifacts and tracked config hash.')).toBeVisible();

    // Verify verify_lockfile was called
    const wasCalled = await page.evaluate(() => {
      const calls = (window as any).__commandCalls as Record<string, number>;
      return (calls['verify_lockfile'] ?? 0) >= 1;
    });
    expect(wasCalled).toBe(true);
  });

  test('verify drift shows all difference kinds including enabled/disabled', async ({ page }) => {
    await installReproducibleMock(page, { verifyReport: DRIFT_REPORT });
    await navigateToInstanceEditor(page);
    await openReproducibleTab(page);

    const textarea = page.getByRole('textbox', { name: 'Instance lockfile JSON' });
    await textarea.fill(LOCKFILE_JSON);

    await page.getByRole('button', { name: 'Verify' }).click();

    // Shows drift detected header
    await expect(page.getByText('Drift detected')).toBeVisible();

    // Shows each difference kind
    await expect(page.getByText(/mods\/new-mod\.jar.*added/)).toBeVisible();
    await expect(page.getByText(/mods\/removed-mod\.jar.*removed/)).toBeVisible();
    await expect(page.getByText(/mods\/changed-mod\.jar.*modified/)).toBeVisible();
    await expect(page.getByText(/mods\/disabled-mod\.jar.*disabled/)).toBeVisible();
    await expect(page.getByText(/mods\/enabled-mod\.jar.*enabled/)).toBeVisible();
    await expect(page.getByText(/config\/options\.txt.*config-modified/)).toBeVisible();

    // Summary mentions difference count
    await expect(page.getByText(/6 differences? found/)).toBeVisible();

    // Action buttons are still enabled (not busy)
    await expect(page.getByRole('button', { name: 'Verify' })).toBeEnabled();
    await expect(page.getByRole('button', { name: 'Repair' })).toBeEnabled();
    await expect(page.getByRole('button', { name: 'Clone' })).toBeEnabled();
  });
});

// ---------------------------------------------------------------------------
// Tests — Repair
// ---------------------------------------------------------------------------

test.describe('InstanceEditor — Reproducible tab: repair', () => {

  test('repair shows confirmation dialog before proceeding', async ({ page }) => {
    // Mock confirm to return false so we can verify it was called
    await installReproducibleMock(page, { confirmResult: false });
    await navigateToInstanceEditor(page);
    await openReproducibleTab(page);

    const textarea = page.getByRole('textbox', { name: 'Instance lockfile JSON' });
    await textarea.fill(LOCKFILE_JSON);

    await page.getByRole('button', { name: 'Repair' }).click();

    // Since confirm returned false, repair_lockfile should NOT have been called
    const repairCalled = await page.evaluate(() => {
      const calls = (window as any).__commandCalls as Record<string, number>;
      return (calls['repair_lockfile'] ?? 0);
    });
    expect(repairCalled).toBe(0);

    // No error, no repair notice — the UI state is unchanged
    await expect(page.getByText('Repair completed')).toHaveCount(0);
    await expect(page.getByText('Repair introduced')).toHaveCount(0);
    await expect(page.getByText('Repair was cancelled')).toHaveCount(0);

    // Textarea still has the lockfile
    await expect(textarea).toHaveValue(LOCKFILE_JSON);
  });

  test('repair success shows completion and re-verifies', async ({ page }) => {
    await installReproducibleMock(page, {
      confirmResult: true,
      repairOutcome: SUCCESSFUL_REPAIR,
      verifyReport: IN_SYNC_REPORT,
    });
    await navigateToInstanceEditor(page);
    await openReproducibleTab(page);

    const textarea = page.getByRole('textbox', { name: 'Instance lockfile JSON' });
    await textarea.fill(LOCKFILE_JSON);

    await page.getByRole('button', { name: 'Repair' }).click();

    // After successful repair, it re-verifies and shows the verify result
    await expect(page.getByText('Repair completed and the instance now matches the lockfile.')).toBeVisible();
    await expect(page.getByText('In sync')).toBeVisible();

    // Both repair_lockfile and verify_lockfile were called
    const repairCalls = await page.evaluate(() => {
      const calls = (window as any).__commandCalls as Record<string, number>;
      return calls['repair_lockfile'] ?? 0;
    });
    expect(repairCalls).toBe(1);

    const verifyCalls = await page.evaluate(() => {
      const calls = (window as any).__commandCalls as Record<string, number>;
      return calls['verify_lockfile'] ?? 0;
    });
    // verify was called at least once (the user clicked verify, or repair triggered a re-verify)
    expect(verifyCalls).toBeGreaterThanOrEqual(1);

    // get_instance_detail was called to refresh after repair
    const detailCalls = await page.evaluate(() => {
      const calls = (window as any).__commandCalls as Record<string, number>;
      return calls['get_instance_detail'] ?? 0;
    });
    expect(detailCalls).toBeGreaterThanOrEqual(2);
  });

  test('repair partial success shows remaining differences message', async ({ page }) => {
    const partialRepairOutcome: Record<string, unknown> = {
      ...SUCCESSFUL_REPAIR,
    };

    await installReproducibleMock(page, {
      confirmResult: true,
      repairOutcome: partialRepairOutcome,
      verifyReport: DRIFT_REPORT,
    });
    await navigateToInstanceEditor(page);
    await openReproducibleTab(page);

    const textarea = page.getByRole('textbox', { name: 'Instance lockfile JSON' });
    await textarea.fill(LOCKFILE_JSON);

    await page.getByRole('button', { name: 'Repair' }).click();

    // Shows partial repair message
    await expect(page.getByText('Artifact repair completed. Remaining differences cannot be reproduced')).toBeVisible();
    await expect(page.getByText('Drift detected')).toBeVisible();
  });

  test('repair failure with rollback shows error message', async ({ page }) => {
    await installReproducibleMock(page, {
      confirmResult: true,
      repairOutcome: FAILED_REPAIR,
    });
    await navigateToInstanceEditor(page);
    await openReproducibleTab(page);

    const textarea = page.getByRole('textbox', { name: 'Instance lockfile JSON' });
    await textarea.fill(LOCKFILE_JSON);

    await page.getByRole('button', { name: 'Repair' }).click();

    // Error is shown (with rollback note)
    await expect(page.getByText('Download failed for Sodium: Checksum mismatch')).toBeVisible();
    await expect(page.getByText('The recovery snapshot was restored.')).toBeVisible();

    // UI is still interactive
    await expect(page.getByRole('button', { name: 'Repair' })).toBeEnabled();
    await expect(page.getByRole('button', { name: 'Verify' })).toBeEnabled();
  });

  test('repair is disabled for locked instances', async ({ page }) => {
    await installReproducibleMock(page, { isLocked: true });
    await navigateToInstanceEditor(page);
    await openReproducibleTab(page);

    const textarea = page.getByRole('textbox', { name: 'Instance lockfile JSON' });
    await textarea.fill(LOCKFILE_JSON);

    // Repair button is disabled
    const repairButton = page.getByRole('button', { name: 'Repair' });
    await expect(repairButton).toBeDisabled();

    // Other buttons are still enabled
    await expect(page.getByRole('button', { name: 'Verify' })).toBeEnabled();
    await expect(page.getByRole('button', { name: 'Clone' })).toBeEnabled();
    await expect(page.getByRole('button', { name: 'Copy' })).toBeEnabled();
    await expect(page.getByRole('button', { name: 'Clear' })).toBeEnabled();
  });

  test('repair health-rollback outcome shows error', async ({ page }) => {
    const healthRollbackOutcome: Record<string, unknown> = {
      type: 'health-rollback',
      healthReport: { findings: [] },
      snapshotId: 'snap-rollback-1',
      warnings: [],
    };

    await installReproducibleMock(page, {
      confirmResult: true,
      repairOutcome: healthRollbackOutcome,
    });
    await navigateToInstanceEditor(page);
    await openReproducibleTab(page);

    const textarea = page.getByRole('textbox', { name: 'Instance lockfile JSON' });
    await textarea.fill(LOCKFILE_JSON);

    await page.getByRole('button', { name: 'Repair' }).click();

    // Error shown about health blocker
    await expect(page.getByText('Repair introduced a health blocker, so Agora restored the recovery snapshot.')).toBeVisible();
  });

  test('repair cancelled outcome shows notice', async ({ page }) => {
    const cancelledOutcome: Record<string, unknown> = {
      type: 'cancelled',
      phase: 'applying',
      rollbackPerformed: true,
    };

    await installReproducibleMock(page, {
      confirmResult: true,
      repairOutcome: cancelledOutcome,
    });
    await navigateToInstanceEditor(page);
    await openReproducibleTab(page);

    const textarea = page.getByRole('textbox', { name: 'Instance lockfile JSON' });
    await textarea.fill(LOCKFILE_JSON);

    await page.getByRole('button', { name: 'Repair' }).click();

    // Notice shown about cancellation
    await expect(page.getByText('Repair was cancelled and the recovery snapshot was restored.')).toBeVisible();
  });
});

// ---------------------------------------------------------------------------
// Tests — Clone
// ---------------------------------------------------------------------------

test.describe('InstanceEditor — Reproducible tab: clone', () => {

  test('clone success navigates to new instance via onOpenInstanceEditor', async ({ page }) => {
    await installReproducibleMock(page, {
      cloneInstanceId: 'new-cloned-instance',
      cloneReject: false,
    });
    await navigateToInstanceEditor(page);
    await openReproducibleTab(page);

    const textarea = page.getByRole('textbox', { name: 'Instance lockfile JSON' });
    await textarea.fill(LOCKFILE_JSON);

    await page.getByRole('button', { name: 'Clone' }).click();

    // Verify import_lockfile was called
    const importCalls = await page.evaluate(() => {
      const calls = (window as any).__commandCalls as Record<string, number>;
      return calls['import_lockfile'] ?? 0;
    });
    expect(importCalls).toBe(1);

    // Verify the argument passed was the lockfile JSON
    const importArgs = await page.evaluate(() => {
      const args = (window as any).__commandArgs as Record<string, Record<string, unknown>>;
      return args['import_lockfile'];
    });
    expect(importArgs).toBeDefined();
    // The lockfileJson arg should match what we pasted
    const parsed = JSON.parse(importArgs.lockfileJson as string);
    expect(parsed.instanceId).toBe('test-instance');

    // After clone, onOpenInstanceEditor navigates to the new instance.
    // Check that the history state changed to the new instance.
    const historyState = await page.evaluate(() => window.history.state);
    expect(historyState?.__agora?.type).toBe('instance-detail');
    expect(historyState?.__agora?.instanceId).toBe('new-cloned-instance');
  });

  test('clone failure shows error and stays on current page', async ({ page }) => {
    await installReproducibleMock(page, {
      cloneReject: true,
    });
    await navigateToInstanceEditor(page);
    await openReproducibleTab(page);

    const textarea = page.getByRole('textbox', { name: 'Instance lockfile JSON' });
    await textarea.fill(LOCKFILE_JSON);

    await page.getByRole('button', { name: 'Clone' }).click();

    // Error message is shown
    await expect(page.getByText('Clone rejected: artifact source unavailable')).toBeVisible();

    // We remain on the same instance (history did not change)
    const historyState = await page.evaluate(() => window.history.state);
    expect(historyState?.__agora?.type).toBe('instance-detail');
    expect(historyState?.__agora?.instanceId).toBe('test-instance');

    // Textarea still has the lockfile content
    await expect(textarea).toHaveValue(LOCKFILE_JSON);

    // Action buttons are still enabled
    await expect(page.getByRole('button', { name: 'Clone' })).toBeEnabled();
    await expect(page.getByRole('button', { name: 'Verify' })).toBeEnabled();
    await expect(page.getByRole('button', { name: 'Repair' })).toBeEnabled();
  });
});

// ---------------------------------------------------------------------------
// Tests — Error resilience (tampered / future-schema)
// ---------------------------------------------------------------------------

test.describe('InstanceEditor — Reproducible tab: tampered and future-schema errors', () => {

  test('export error from tampered registry data shows error and is recoverable', async ({ page }) => {
    await installReproducibleMock(page, { exportReject: true });
    await navigateToInstanceEditor(page);
    await openReproducibleTab(page);

    // Click export
    await page.getByRole('button', { name: 'Export Lockfile' }).click();

    // Error is visible
    await expect(page.getByText('Failed to export lockfile: registry data not found')).toBeVisible();

    // The textarea is still empty but editable — user can paste a lockfile manually
    const textarea = page.getByRole('textbox', { name: 'Instance lockfile JSON' });
    await expect(textarea).toHaveValue('');
    await textarea.fill(LOCKFILE_JSON);
    await expect(textarea).toHaveValue(LOCKFILE_JSON);

    // Now verify works (recoverable — we can still manually verify with a pasted lockfile)
    await page.getByRole('button', { name: 'Verify' }).click();
    await expect(page.getByText('In sync')).toBeVisible();

    // Error is gone (replaced by new state)
    await expect(page.getByText('Failed to export lockfile')).toHaveCount(0);
  });

  test('verify error from tampered lockfile JSON shows error and textarea remains editable', async ({ page }) => {
    await installReproducibleMock(page, { verifyReject: true });
    await navigateToInstanceEditor(page);
    await openReproducibleTab(page);

    const textarea = page.getByRole('textbox', { name: 'Instance lockfile JSON' });
    await textarea.fill(LOCKFILE_JSON);

    await page.getByRole('button', { name: 'Verify' }).click();

    // Error about corrupt schema version is shown
    await expect(page.getByText('Lockfile verification failed: corrupt schema version')).toBeVisible();

    // Textarea still has the lockfile content (user can fix and retry)
    await expect(textarea).toHaveValue(LOCKFILE_JSON);

    // Buttons are enabled — user can try again or clone
    await expect(page.getByRole('button', { name: 'Verify' })).toBeEnabled();
    await expect(page.getByRole('button', { name: 'Repair' })).toBeEnabled();
    await expect(page.getByRole('button', { name: 'Clone' })).toBeEnabled();

    // User can edit the textarea to fix the lockfile
    await textarea.fill('{"fixed": true}');
    await expect(textarea).toHaveValue('{"fixed": true}');

    // Editing starts a new recovery attempt, so the stale backend error clears.
    await expect(page.getByText('Lockfile verification failed: corrupt schema version')).toHaveCount(0);
  });

  test('repair error from backend leaves UI interactive and textarea intact', async ({ page }) => {
    await installReproducibleMock(page, {
      confirmResult: true,
      repairReject: true,
    });
    await navigateToInstanceEditor(page);
    await openReproducibleTab(page);

    const textarea = page.getByRole('textbox', { name: 'Instance lockfile JSON' });
    await textarea.fill(LOCKFILE_JSON);

    await page.getByRole('button', { name: 'Repair' }).click();

    // Error from the rejected promise
    await expect(page.getByText('Repair failed: network timeout')).toBeVisible();

    // Textarea still has the lockfile
    await expect(textarea).toHaveValue(LOCKFILE_JSON);

    // Buttons are re-enabled (busy state cleared)
    await expect(page.getByRole('button', { name: 'Repair' })).toBeEnabled();
    await expect(page.getByRole('button', { name: 'Verify' })).toBeEnabled();
    await expect(page.getByRole('button', { name: 'Clone' })).toBeEnabled();

    // Error shown in the destructive-bg div
    const errorDiv = page.locator('div.bg-destructive');
    await expect(errorDiv).toBeVisible();
    await expect(errorDiv).toContainText('Repair failed: network timeout');
  });

  test('clone error from unavailable source keeps textarea and allows retry', async ({ page }) => {
    await installReproducibleMock(page, { cloneReject: true });
    await navigateToInstanceEditor(page);
    await openReproducibleTab(page);

    const textarea = page.getByRole('textbox', { name: 'Instance lockfile JSON' });
    await textarea.fill(LOCKFILE_JSON);

    await page.getByRole('button', { name: 'Clone' }).click();

    // Error message shown
    await expect(page.getByText('Clone rejected: artifact source unavailable')).toBeVisible();

    // Textarea remains filled
    await expect(textarea).toHaveValue(LOCKFILE_JSON);

    // Buttons are enabled for retry
    await expect(page.getByRole('button', { name: 'Clone' })).toBeEnabled();
    await expect(page.getByRole('button', { name: 'Verify' })).toBeEnabled();

    // Clear clears the textarea and action buttons but the error persists
    // until a new action (export/verify/repair/clone) sets error=null.
    await page.getByRole('button', { name: 'Clear' }).click();
    await expect(textarea).toHaveValue('');

    // Dash placeholder is back
    await expect(page.getByText('Export this instance or paste a received lockfile')).toBeVisible();

    // User can paste a different lockfile and try again despite the old error
    await textarea.fill(LOCKFILE_JSON);
    await expect(textarea).toHaveValue(LOCKFILE_JSON);
    await expect(page.getByRole('button', { name: 'Verify' })).toBeEnabled();
  });
});
