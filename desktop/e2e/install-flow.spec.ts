import { test, expect, type Page } from '@playwright/test';

// ---------------------------------------------------------------------------
// Types for the install call queue
// ---------------------------------------------------------------------------

interface InstallCall {
  command: string;
  args: Record<string, unknown>;
  resolve: (value: unknown) => void;
  reject: (reason?: unknown) => void;
}

type SourceType = 'curated' | 'modrinth' | 'manual';
type ConflictResolution = 'replace' | 'skip' | 'disable-existing' | 'abort';

// ---------------------------------------------------------------------------
// Fixtures — minimal realistic ResolvedInstallPlan / InstallOutcome builders
// ---------------------------------------------------------------------------

function makePlan(overrides: Record<string, unknown> = {}) {
  return {
    fingerprint: 'plan-fp-test-001',
    intent: {
      action: { type: 'install', sourceType: 'curated' as SourceType, itemId: 'test-mod', candidateVersion: '1.0.0' },
      targetInstance: 'test-instance',
      optionalDeps: { type: 'prompt' },
      requestedBy: 'interactive',
      overrides: { allowReplace: false, skipHealthScan: false, forceConflictResolution: {} },
    },
    operation: {
      type: 'install',
      artifact: {
        type: 'download' as const,
        itemId: 'test-mod',
        versionId: '1.0.0',
        source: { type: 'download', url: 'https://example.com/test-mod-1.0.0.jar' },
        hashes: { values: [{ algorithm: 'sha256', value: 'a'.repeat(64) }] },
        size: 250_000,
        filename: 'test-mod-1.0.0.jar',
        metadata: { sourceType: 'curated' as SourceType, registryId: 'test-mod', modrinthId: null, contentType: 'mod' },
      },
    },
    dependencies: [],
    conflicts: [],
    filesToAdd: [{ targetFilename: 'test-mod-1.0.0.jar', stagingFilename: 'staging-test-mod.jar', artifact: {}, hashes: { values: [{ algorithm: 'sha256', value: 'a'.repeat(64) }] }, size: 250_000 }],
    filesToRemove: [],
    filesToDisable: [],
    snapshot: { label: 'Before installing test-mod 1.0.0', estimatedBytes: 500_000 },
    diskEstimate: { downloadBytes: 250_000, snapshotBytes: 500_000, applyOverheadBytes: 100_000, peakAdditionalBytes: 600_000, postCommitDeltaBytes: 250_000 },
    warnings: [],
    blockingErrors: [],
    pendingChoices: [],
    createdAt: '2026-07-12T17:00:00Z',
    instanceStateHash: 'abc123def456',
    registryRevision: 'v20260712',
    ...overrides,
  };
}

function planWithOptionalDeps(fingerprint = 'plan-fp-optional-001') {
  return makePlan({
    fingerprint,
    dependencies: [
      { modJarId: 'required-dep-a', requirement: 'required', source: 'jar', disposition: { type: 'install-candidate', artifact: {} } },
      { modJarId: 'optional-dep-b', requirement: 'optional', source: 'jar', disposition: { type: 'install-candidate', artifact: {} } },
    ],
    pendingChoices: [
      { type: 'optional-dependencies', choiceId: 'opt-deps', options: [{ modJarId: 'optional-dep-b', displayName: 'Optional Dep B' }] },
    ],
  });
}

function planWithBlockingConflict(fingerprint = 'plan-fp-conflict-001') {
  return makePlan({
    fingerprint,
    conflicts: [
      {
        conflictId: 'conflict-1',
        kind: 'duplicate-mod',
        existingModJarId: 'existing-mod',
        incomingModJarId: 'incoming-mod',
        message: 'incoming-mod conflicts with existing-mod — they provide the same features.',
        blocking: true,
        resolutionOptions: ['replace', 'skip'] as ConflictResolution[],
      },
    ],
  });
}

function makeSuccessOutcome(snapshotId = 'snap-001') {
  return {
    type: 'success' as const,
    installedItems: ['test-mod-1.0.0.jar'],
    existingItemsReused: [],
    warnings: [],
    health: { type: 'completed' as const, report: {} },
    snapshotId,
  };
}

// ---------------------------------------------------------------------------
// Shared mock data
// ---------------------------------------------------------------------------

const CURATED_MOD: Record<string, unknown> = {
  id: 'test-mod',
  name: 'Test Mod',
  content_type: 'mod',
  download_strategy: 'github_release',
  source_identifier: 'test-mod/releases',
  sha256: '',
  upvotes: 10,
  downvotes: 2,
  net_score: 8,
  velocity: 1.5,
  status: 'active',
  is_immune: false,
  immunity_reason: null,
  allow_comments: true,
  icon_url: null,
  gallery_urls_json: null,
  date_added: '2026-01-01',
  compatible_versions_json: JSON.stringify([{ mc_version: '1.20.1', loader: 'fabric', mod_version: '1.0.0' }]),
  description: 'A test mod for verifying the install flow.',
  body_markdown: null,
  page_url: 'https://example.com/test-mod',
  license_id: 'MIT',
  source_updated_at: '2026-06-01T00:00:00Z',
  modrinth_id: null,
};

const MODRINTH_BRIDGE_MOD: Record<string, unknown> = {
  ...CURATED_MOD,
  id: 'bridged-mod',
  name: 'Bridged Mod',
  download_strategy: 'modrinth_id',
  modrinth_id: 'modrinth-abc',
  source_identifier: 'modrinth-abc',
};

const MODRINTH_PROJECT: Record<string, unknown> = {
  id: 'modrinth-abc',
  title: 'Bridged Mod',
  description: 'A Modrinth-linked mod description.',
  body: '# Bridged Mod\n\nFetched from Modrinth.',
  project_type: 'mod',
  icon_url: null,
  gallery_urls: [],
  page_url: 'https://modrinth.com/mod/modrinth-abc',
  license_id: 'MIT',
  source_updated_at: '2026-07-01T00:00:00Z',
};

// ---------------------------------------------------------------------------
// Shared mock installer
// ---------------------------------------------------------------------------

interface MockOptions {
  modrinthEnabled?: boolean;
  registryItems?: Record<string, unknown>;
  /** Override for fetch_modrinth_project responses. Key = itemId, value = project or null. */
  modrinthProject?: Record<string, unknown>;
}

async function installFlowMock(page: Page, opts: MockOptions = {}) {
  const { modrinthEnabled = true, registryItems } = opts;
  const effectiveRegistryItems = registryItems ?? { 'test-mod': CURATED_MOD, 'bridged-mod': MODRINTH_BRIDGE_MOD };
  const modrinthProject = opts.modrinthProject ?? { 'modrinth-abc': MODRINTH_PROJECT };

  await page.addInitScript(
    (params: { mrEnabled: boolean; items: Record<string, unknown>; mrProject: Record<string, unknown> }) => {
      const { mrEnabled, items, mrProject } = params;

      const installCalls: InstallCall[] = [];

      const callbacks = new Map<number, (...args: unknown[]) => void>();
      let callbackId = 0;

      const internals = {
        transformCallback(callback: (...args: unknown[]) => void) {
          const id = ++callbackId;
          callbacks.set(id, callback);
          return id;
        },
        unregisterCallback(id: number) { callbacks.delete(id); },
        invoke(command: string, args: Record<string, unknown> = {}) {
          // Install pipeline commands — tracked in call queue
          if (command === 'resolve_install_plan') {
            return new Promise((resolve, reject) => installCalls.push({ command, args, resolve, reject } as any));
          }
          if (command === 'apply_install_plan') {
            return new Promise((resolve, reject) => installCalls.push({ command, args, resolve, reject } as any));
          }
          if (command === 'cancel_install') return Promise.resolve(null);

          // Event plugin (used by subscribeProgress for progress events)
          if (command.startsWith('plugin:event|')) return Promise.resolve(1);

          // Settings
          if (command === 'get_setting') {
            const key = args.key as string;
            if (key === 'onboarding_complete') return Promise.resolve(true);
            if (key === 'modrinth_enabled') return Promise.resolve(mrEnabled);
            if (key === 'ai_chat_enabled') return Promise.resolve(false);
            if (key === 'mojang_launcher_path') return Promise.resolve('');
            if (key === 'launch_mode') return Promise.resolve('delegation');
            return Promise.resolve(null);
          }

          // Registry
          if (command === 'get_registry_status') {
            return Promise.resolve({ has_cached_db: true, cached_tag: 'test', cached_schema_version: 5, latest_tag: 'test', update_available: false, checked: true, message: 'Registry ready.' });
          }
          if (command === 'list_categories') return Promise.resolve([]);
          if (command === 'list_manifest_loaders') return Promise.resolve(['fabric', 'forge', 'quilt']);
          if (command === 'list_manifest_mc_versions') return Promise.resolve(['1.20.1', '1.21']);

          // Misc
          if (command === 'get_windows_accent_color') return Promise.resolve(null);
          if (command === 'get_auth_status') return Promise.resolve(true);
          if (command === 'get_github_profile') return Promise.resolve(null);
          if (command === 'get_flag_rate_limit') return Promise.resolve(null);
          if (command === 'list_mod_reviews') return Promise.resolve([]);
          if (command === 'get_curated_annotation') return Promise.resolve(null);

          // Instances
          if (command === 'list_instances') {
            return Promise.resolve([
              { instance_id: 'test-instance', name: 'Test Instance', minecraft_version: '1.20.1', loader: 'fabric', loader_version: '0.15.11', is_modpack: false, is_locked: false, last_launched_at: null, jvm_memory_mb: 4096, jvm_gc: 'G1GC', jvm_custom_args: '', created_at: '2026-01-01T00:00:00Z' },
            ]);
          }
          if (command === 'check_instance_updates') return Promise.resolve([]);
          if (command === 'batch_check_compat') return Promise.resolve({});
          if (command === 'get_instance_detail') {
            const instanceId = args.instanceId as string;
            return Promise.resolve({
              row: { instance_id: instanceId, name: 'Test Instance', minecraft_version: '1.20.1', loader: 'fabric', loader_version: '0.15.11', is_modpack: false, is_locked: false, last_launched_at: null, jvm_memory_mb: 4096, jvm_gc: 'G1GC', jvm_custom_args: '', created_at: '2026-01-01T00:00:00Z' },
              manifest: { instance_id: instanceId, name: 'Test Instance', created_from_pack: null, minecraft_version: '1.20.1', loader: 'fabric', loader_version: '0.15.11', mods: [{ filename: 'installed-test-mod.jar', registry_id: null, modrinth_id: null, mod_jar_id: 'test-mod', source: 'manual_drag_drop', version: '1.0.0', sha256: 'a'.repeat(64), installed_at: '2026-07-01T00:00:00Z', enabled: true, content_type: 'mod' }], resourcepacks: [], shaders: [], datapacks: [], worlds: [], user_preferences: {} },
            });
          }
          if (command === 'list_snapshots') return Promise.resolve([]);
          if (command === 'list_loadout_profiles') return Promise.resolve([]);
          if (command === 'restore_snapshot') return Promise.resolve(null);

          // Mod detail
          if (command === 'get_registry_item') {
            return Promise.resolve((items as any)[args.itemId as string] ?? null);
          }
          if (command === 'fetch_modrinth_project') {
            const projectId = args.projectId as string;
            return Promise.resolve((mrProject as any)[projectId] ?? null);
          }
          if (command === 'is_modrinth_enabled') return Promise.resolve(mrEnabled);
          if (command === 'list_mod_versions') {
            return Promise.resolve({
              items: [
                { version: '1.0.0', filename: 'test-mod-1.0.0.jar', mc_version: '1.20.1', loader: 'fabric', version_compat: 'compatible', release_date: '2026-06-01', sha256: 'abc123def456' },
                { version: '0.9.0', filename: 'test-mod-0.9.0.jar', mc_version: '1.20.1', loader: 'fabric', version_compat: 'major_match', release_date: '2026-05-01', sha256: 'def789abc012' },
              ],
              hasMore: false,
            });
          }
          if (command === 'list_mod_versions_load_more') {
            return Promise.resolve({ items: [], hasMore: false });
          }
          if (command === 'list_raw_modrinth_versions') {
            return Promise.resolve([
              { version_id: 'mrv-001', version: '2.0.0', filename: 'bridged-mod-2.0.0.jar', mc_versions: ['1.20.1'], loaders: ['fabric'], release_date: '2026-07-01T00:00:00Z', sha1: 'aaaabbbbccccddddeeee', primary: true },
              { version_id: 'mrv-002', version: '1.9.0', filename: 'bridged-mod-1.9.0.jar', mc_versions: ['1.20.1'], loaders: ['fabric'], release_date: '2026-06-15T00:00:00Z', sha1: 'ffffgggghhhhiiiijjjj', primary: false },
            ]);
          }

          // Browse
          if (command === 'browse_search') {
            return Promise.resolve({
              items: [
                { id: 'test-mod', source: 'curated', registryItem: { id: 'test-mod', name: 'Test Mod', content_type: 'mod', download_strategy: 'github_release', upvotes: 10, downvotes: 2, net_score: 8, velocity: 1.5 }, modrinthResult: null, name: 'Test Mod', iconUrl: null, description: 'A test mod for verifying the install flow.', contentType: 'mod' },
              ],
              total: 1,
              page: 0,
              hasMore: false,
            });
          }
          if (command === 'browse_load_more') return Promise.resolve({ items: [], total: 0, page: 1, hasMore: false });
          if (command === 'for_you_items') return Promise.resolve({ items: [] });

          // Instance editor add-mod browse
          if (command === 'browse_items') {
            return Promise.resolve([
              { id: 'test-mod', name: 'Test Mod', content_type: 'mod', download_strategy: 'github_release', source_identifier: 'test-mod', upvotes: 10, downvotes: 2, net_score: 8, velocity: 1.5, description: 'A test mod', icon_url: null },
            ]);
          }

          // Fallback
          return Promise.resolve(null);
        },
      };
      Object.assign(window as unknown as Record<string, unknown>, {
        __TAURI_INTERNALS__: internals,
        __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
        __installCalls: installCalls,
      });
    },
    { mrEnabled: modrinthEnabled, items: effectiveRegistryItems, mrProject: modrinthProject } as any,
  );
}

// ---------------------------------------------------------------------------
// Helpers: wait for and resolve install pipeline calls
// ---------------------------------------------------------------------------

async function totalInstallCalls(page: Page): Promise<number> {
  return page.evaluate(() => (window as any).__installCalls?.length ?? 0);
}

async function lastInstallCall(page: Page, command: string): Promise<number> {
  let index = -1;
  await expect.poll(async () => {
    const calls: InstallCall[] = await page.evaluate(() => (window as any).__installCalls ?? []);
    const indices = calls
      .map((c: InstallCall, i: number) => ({ c, i }))
      .filter(({ c }) => c.command === command)
      .map(({ i }) => i);
    index = indices.length > 0 ? indices[indices.length - 1] : -1;
    return index;
  }).toBeGreaterThanOrEqual(0);
  return index;
}

async function resolveInstallCall(page: Page, index: number, result: unknown) {
  await page.evaluate(
    ({ idx, res }: { idx: number; res: unknown }) => {
      const calls = (window as any).__installCalls as InstallCall[];
      if (calls[idx]) calls[idx].resolve(res);
    },
    { idx: index, res: result },
  );
}

async function rejectInstallCall(page: Page, index: number, error: unknown) {
  await page.evaluate(
    ({ idx, err }: { idx: number; err: unknown }) => {
      const calls = (window as any).__installCalls as InstallCall[];
      if (calls[idx]) calls[idx].reject(err);
    },
    { idx: index, err: error },
  );
}

// ---------------------------------------------------------------------------
// Helpers: common assertions on the InstallFlow dialog
// ---------------------------------------------------------------------------

async function expectReviewView(page: Page) {
  await expect(page.getByRole('dialog')).toBeVisible();
  await expect(page.getByText('Review Instance Changes')).toBeVisible();
  await expect(page.getByText(/\+1 to add/)).toBeVisible();
  await expect(page.getByText(/Before installing/)).toBeVisible();
  await expect(page.getByRole('button', { name: /Install|Review Selected Changes/ })).toBeVisible();
}

async function expectResultView(page: Page) {
  await expect(page.getByRole('dialog')).toBeVisible();
  await expect(page.getByText('All verified changes were applied successfully.')).toBeVisible();
  // The dialog has both a Radix close-X button and a content Close button.
  // Use .first() to disambiguate.
  await expect(page.getByRole('button', { name: 'Close' }).first()).toBeVisible();
  await expect(page.getByRole('button', { name: 'Open Instance' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Roll Back' })).toBeVisible();
}

async function expectErrorView(page: Page, message: string) {
  await expect(page.getByRole('dialog')).toBeVisible();
  await expect(page.getByText(message)).toBeVisible();
  // Two "Close" buttons (Radix X + content button); first() disambiguates.
  await expect(page.getByRole('button', { name: 'Close' }).first()).toBeVisible();
}

async function expectBlockedConfirm(page: Page) {
  await expect(page.getByRole('dialog')).toBeVisible();
  const btn = page.getByRole('button', { name: /Resolve Conflicts First|Cannot Apply/ });
  await expect(btn).toBeVisible();
  await expect(btn).toBeDisabled();
}

// ---------------------------------------------------------------------------
// Helper: select the first <select> that contains an option with given value
// (labels lack htmlFor, so getByLabel doesn't work for these selects)
// ---------------------------------------------------------------------------

async function pickFirstInstanceSelect(page: Page, value: string) {
  await page.locator('select').first().selectOption(value);
}

// ---------------------------------------------------------------------------
// Helper: navigate Browse → ModDetail
// ---------------------------------------------------------------------------

async function browseToModDetail(page: Page) {
  await page.getByRole('button', { name: 'Browse', exact: true }).click();
  await page.getByRole('button', { name: 'View Details', exact: true }).click();
}

// ---------------------------------------------------------------------------
// Helper: primary install flow steps (ModDetail inline) up to InstallFlow
// ---------------------------------------------------------------------------

async function triggerInlineInstallFlow(page: Page) {
  await page.getByRole('button', { name: 'Install to Instance' }).click();
  await pickFirstInstanceSelect(page, 'test-instance');
  await page.getByRole('button', { name: 'Next: Choose Version' }).click();
  // Click the first version entry
  await page.getByText('test-mod-1.0.0.jar').click();
  await page.getByRole('button', { name: /Install test-mod-1.0.0.jar/ }).click();
}

// ---------------------------------------------------------------------------
// Tests — Entry points
// ---------------------------------------------------------------------------

test.describe('Release C3 — Install flow entry points', () => {

  test('ModDetail primary curated install reaches resolve+apply and shows review/progress/result UI', async ({ page }) => {
    await installFlowMock(page);
    await page.goto('/');

    await browseToModDetail(page);
    await triggerInlineInstallFlow(page);

    // Resolve the plan
    await expect.poll(() => totalInstallCalls(page)).toBeGreaterThanOrEqual(1);
    const resolveIdx = await lastInstallCall(page, 'resolve_install_plan');
    await resolveInstallCall(page, resolveIdx, makePlan());

    await expectReviewView(page);

    // Confirm
    await page.getByRole('button', { name: 'Install' }).click();

    // Apply
    await expect.poll(() => totalInstallCalls(page)).toBeGreaterThanOrEqual(2);
    const applyIdx = await lastInstallCall(page, 'apply_install_plan');
    await resolveInstallCall(page, applyIdx, makeSuccessOutcome());

    await expectResultView(page);
  });

  test('ModDetail Versions-tab install across GitHub releases reaches resolve+apply', async ({ page }) => {
    // Disable Modrinth so the Versions tab shows GitHub releases
    await installFlowMock(page, { modrinthEnabled: false });
    await page.goto('/');

    await browseToModDetail(page);

    // Click "Versions" tab
    await page.getByRole('button', { name: 'Versions' }).click();

    // Wait for GitHub version list, click the "1.0.0" version row
    await page.getByText('1.0.0').first().click();

    // Now the selected-version detail panel appears with "Install to instance" label
    // and a <select> plus "Review install plan" button.
    // Pick the select that has option containing "Test Instance" (there are 0-1
    // such selects at this point since the inline install flow is not open).
    const selects = page.locator('select');
    const count = await selects.count();
    let instanceSelected = false;
    for (let i = 0; i < count; i++) {
      const options = await selects.nth(i).locator('option').allTextContents();
      if (options.some((t) => t.includes('Test Instance'))) {
        await selects.nth(i).selectOption('test-instance');
        instanceSelected = true;
        break;
      }
    }
    if (!instanceSelected) {
      await selects.last().selectOption('test-instance');
    }

    // Click "Review install plan"
    await page.getByRole('button', { name: 'Review install plan' }).click();

    // InstallFlow opens and resolves
    await expect.poll(() => totalInstallCalls(page)).toBeGreaterThanOrEqual(1);
    const resolveIdx = await lastInstallCall(page, 'resolve_install_plan');
    await resolveInstallCall(page, resolveIdx, makePlan());

    await expectReviewView(page);

    // Confirm → apply
    await page.getByRole('button', { name: 'Install' }).click();
    await expect.poll(() => totalInstallCalls(page)).toBeGreaterThanOrEqual(2);
    const applyIdx = await lastInstallCall(page, 'apply_install_plan');
    await resolveInstallCall(page, applyIdx, makeSuccessOutcome());

    await expectResultView(page);
  });

  test('ModDetail Modrinth-linked install reaches resolve+apply and shows same UI', async ({ page }) => {
    await installFlowMock(page, {
      registryItems: { 'bridged-mod': MODRINTH_BRIDGE_MOD },
      modrinthProject: { 'modrinth-abc': MODRINTH_PROJECT },
    });
    // Preload mod-detail destination for a Modrinth-bridged item
    await page.addInitScript(() => {
      window.history.replaceState({ __agora: { type: 'mod-detail', itemId: 'bridged-mod' } }, '');
    });
    await page.goto('/');

    // The mod has a modrinth_id so the inline install flow uses Modrinth version
    // picker (listRawModrinthVersions). Click "Install to Instance".
    await page.getByRole('button', { name: 'Install to Instance' }).click();

    // Select instance
    await pickFirstInstanceSelect(page, 'test-instance');
    await page.getByRole('button', { name: 'Next: Choose Version' }).click();

    // Should see Modrinth versions — but wait for them to load. The install flow
    // for Modrinth items renders the version list as <li> items, not a <table>.
    await expect(page.getByText('bridged-mod-2.0.0.jar')).toBeVisible({ timeout: 10000 });
    await page.getByText('bridged-mod-2.0.0.jar').click();
    await page.getByRole('button', { name: /Install bridged-mod-2.0.0.jar/ }).click();

    // Resolve
    await expect.poll(() => totalInstallCalls(page)).toBeGreaterThanOrEqual(1);
    const resolveIdx = await lastInstallCall(page, 'resolve_install_plan');
    await resolveInstallCall(page, resolveIdx, makePlan({
      intent: { action: { type: 'install', sourceType: 'modrinth', itemId: 'modrinth-abc', candidateVersion: 'mrv-001' } },
    }));

    await expectReviewView(page);

    // Confirm → apply
    await page.getByRole('button', { name: 'Install' }).click();
    await expect.poll(() => totalInstallCalls(page)).toBeGreaterThanOrEqual(2);
    const applyIdx = await lastInstallCall(page, 'apply_install_plan');
    await resolveInstallCall(page, applyIdx, makeSuccessOutcome());

    await expectResultView(page);
  });

  test('InstanceEditor Add Mod opens Browse with its instance selected', async ({ page }) => {
    await installFlowMock(page);

    // Navigate directly to the instance editor via history state
    await page.addInitScript(() => {
      window.history.replaceState({ __agora: { type: 'instance-detail', instanceId: 'test-instance' } }, '');
    });
    await page.goto('/');

    // Wait for InstanceEditor to render
    await page.getByText('Test Instance').first().waitFor();

    // Click "+ Add Mod"
    await page.getByRole('button', { name: '+ Add Mod' }).click();

    await expect(page.getByRole('heading', { name: 'Browse' })).toBeVisible();
    await expect(page.locator('#browse-instance-context')).toHaveValue('test-instance');
  });

  test('clicking an installed mod opens its details page', async ({ page }) => {
    await installFlowMock(page);

    await page.addInitScript(() => {
      window.history.replaceState({ __agora: { type: 'instance-detail', instanceId: 'test-instance' } }, '');
    });
    await page.goto('/');

    await expect(page.getByText('Test Instance').first()).toBeVisible();
    await expect(page.getByText('Test Mod', { exact: true })).toBeVisible();
    await expect(page.getByText('Manual', { exact: true })).toBeVisible();
    await expect(page.getByText(/Installed /).first()).toBeVisible();
    const main = page.locator('main');
    const editorScrollTop = await main.evaluate((element) => {
      element.style.paddingBottom = '1000px';
      element.scrollTop = 240;
      return element.scrollTop;
    });
    await page.getByRole('button', { name: /installed-test-mod\.jar/ }).click();

    await expect(page.getByRole('heading', { name: 'Test Mod', exact: true })).toBeVisible();
    await expect.poll(() => main.evaluate((element) => element.scrollTop)).toBe(0);
    await page.getByRole('button', { name: /Back/ }).first().click();
    await expect(page.getByText('Installed Mods', { exact: false })).toBeVisible();
    await expect.poll(() => main.evaluate((element) => element.scrollTop)).toBe(editorScrollTop);
  });

  test('ModDetail Back restores Browse scroll position', async ({ page }) => {
    await installFlowMock(page);
    await page.goto('/');
    await page.getByRole('button', { name: 'Browse', exact: true }).click();
    await expect(page.getByRole('button', { name: 'View Details', exact: true })).toBeVisible();

    const main = page.locator('main');
    const browseScrollTop = await main.evaluate((element) => {
      element.style.paddingBottom = '1000px';
      element.scrollTop = 240;
      return element.scrollTop;
    });
    expect(browseScrollTop).toBeGreaterThan(0);

    await page.getByRole('button', { name: 'View Details', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Test Mod', exact: true })).toBeVisible();
    await expect.poll(() => main.evaluate((element) => element.scrollTop)).toBe(0);

    await page.getByRole('button', { name: /Back/ }).first().click();
    await expect(page.getByRole('heading', { name: 'Browse', level: 2 })).toBeVisible();
    await expect.poll(() => main.evaluate((element) => element.scrollTop)).toBe(browseScrollTop);
  });
});

// ---------------------------------------------------------------------------
// Tests — Behaviors
// ---------------------------------------------------------------------------

test.describe('Release C3 — Install flow behaviors', () => {

  test('blocking conflict prevents confirmation button', async ({ page }) => {
    await installFlowMock(page);
    await page.goto('/');

    await browseToModDetail(page);
    await triggerInlineInstallFlow(page);

    // Resolve with a plan that has a blocking conflict
    await expect.poll(() => totalInstallCalls(page)).toBeGreaterThanOrEqual(1);
    const resolveIdx = await lastInstallCall(page, 'resolve_install_plan');
    await resolveInstallCall(page, resolveIdx, planWithBlockingConflict());

    // Review view should show the conflict
    await expect(page.getByText('incoming-mod conflicts with existing-mod')).toBeVisible();
    // Confirm button should be disabled / show "Resolve Conflicts First"
    await expectBlockedConfirm(page);
  });

  test('optional dependency toggling triggers re-resolution on confirm', async ({ page }) => {
    await installFlowMock(page);
    await page.goto('/');

    await browseToModDetail(page);
    await triggerInlineInstallFlow(page);

    // Resolve with a plan that has pending optional dependency choices
    await expect.poll(() => totalInstallCalls(page)).toBeGreaterThanOrEqual(1);
    const resolveIdx = await lastInstallCall(page, 'resolve_install_plan');
    await resolveInstallCall(page, resolveIdx, planWithOptionalDeps());

    // Review view should show optional dep and "Review Selected Changes" button
    await expect(page.getByText('optional-dep-b')).toBeVisible();
    const reviewBtn = page.getByRole('button', { name: 'Review Selected Changes' });
    await expect(reviewBtn).toBeVisible();
    await expect(reviewBtn).toBeEnabled();

    // Toggle the optional dependency OFF (uncheck the checkbox)
    const checkbox = page.locator('input[type="checkbox"]').first();
    await expect(checkbox).toBeChecked();
    await checkbox.click();
    await expect(checkbox).not.toBeChecked();

    // Click "Review Selected Changes" — triggers re-resolution
    await reviewBtn.click();

    // A new resolve_install_plan call should be made
    await expect.poll(() => totalInstallCalls(page)).toBeGreaterThanOrEqual(2);
    const reResolveIdx = await lastInstallCall(page, 'resolve_install_plan');
    // Verify the intent includes modified optional deps (empty since we unchecked the only one)
    const callArgs = await page.evaluate((idx: number) => {
      const calls = (window as any).__installCalls as InstallCall[];
      return calls[idx]?.args;
    }, reResolveIdx);
    expect(callArgs).toBeTruthy();
    const intent = (callArgs as any).intent as Record<string, unknown>;
    expect(intent).toBeTruthy();
    const optionalDeps = intent.optionalDeps as Record<string, unknown> | undefined;
    if (optionalDeps && optionalDeps.type === 'include') {
      expect((optionalDeps.deps as string[]).length).toBe(0);
    }

    // Resolve with a clean plan (no pending choices)
    await resolveInstallCall(page, reResolveIdx, makePlan({ fingerprint: 'plan-fp-optional-002', pendingChoices: [] }));

    // Should now show "Install" (no longer "Review Selected Changes")
    await expect(page.getByRole('button', { name: 'Install' })).toBeVisible();
  });

  test('resolve failure offers retry and close', async ({ page }) => {
    await installFlowMock(page);
    await page.goto('/');

    await browseToModDetail(page);
    await triggerInlineInstallFlow(page);

    // Reject the resolve call
    await expect.poll(() => totalInstallCalls(page)).toBeGreaterThanOrEqual(1);
    const resolveIdx = await lastInstallCall(page, 'resolve_install_plan');
    await rejectInstallCall(page, resolveIdx, new Error('Network error: unable to resolve dependencies'));

    // Error view appears with retry and close
    await expectErrorView(page, 'Network error: unable to resolve dependencies');
    await expect(page.getByRole('button', { name: 'Retry' })).toBeVisible();

    // Click retry → triggers re-resolution
    await page.getByRole('button', { name: 'Retry' }).click();
    await expect.poll(() => totalInstallCalls(page)).toBeGreaterThanOrEqual(2);
    const retryIdx = await lastInstallCall(page, 'resolve_install_plan');
    await resolveInstallCall(page, retryIdx, makePlan());

    // Should now reach the review view
    await expectReviewView(page);
  });

  test('apply failure offers close', async ({ page }) => {
    await installFlowMock(page);
    await page.goto('/');

    await browseToModDetail(page);
    await triggerInlineInstallFlow(page);

    // Resolve successfully
    await expect.poll(() => totalInstallCalls(page)).toBeGreaterThanOrEqual(1);
    const resolveIdx = await lastInstallCall(page, 'resolve_install_plan');
    await resolveInstallCall(page, resolveIdx, makePlan());

    await expectReviewView(page);

    // Confirm → apply
    await page.getByRole('button', { name: 'Install' }).click();

    // Wait for apply_install_plan to be called
    await expect.poll(() => totalInstallCalls(page)).toBeGreaterThanOrEqual(2);
    const applyIdx = await lastInstallCall(page, 'apply_install_plan');
    await rejectInstallCall(page, applyIdx, new Error('Corrupt download: SHA-256 mismatch'));

    // Error view shows the failure with Close (no Retry for non-retryable errors)
    await expectErrorView(page, 'Corrupt download: SHA-256 mismatch');
    await expect(page.getByRole('button', { name: 'Retry' })).toHaveCount(0);
  });
});
