import { test, expect, type Page } from '@playwright/test';

type BrowseResult = { items: unknown[]; total: number; page: number; hasMore: boolean };

const item = (id: string, name: string) => ({
  id,
  source: 'curated',
  registryItem: {
    id,
    name,
    content_type: 'mod',
    download_strategy: 'github_release',
    upvotes: 0,
    downvotes: 0,
    net_score: 0,
  },
  modrinthResult: null,
  name,
  iconUrl: null,
  description: null,
  contentType: 'mod',
});

async function installBrowseMock(page: Page) {
  await page.addInitScript(() => {
    const calls: Array<{
      command: string;
      args: Record<string, unknown>;
      resolve: (value: unknown) => void;
      reject: (reason?: unknown) => void;
    }> = [];
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
        if (command === 'get_setting') {
          const key = args.key;
          if (key === 'onboarding_complete') return Promise.resolve(true);
          if (key === 'modrinth_enabled') return Promise.resolve(true);
          return Promise.resolve(false);
        }
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
        if (command === 'list_categories') {
          return Promise.resolve([
            { id: 'performance', display_name: 'Performance', is_community: false, content_types: ['mod'] },
            { id: 'questing', display_name: 'Questing', is_community: false, content_types: ['pack'] },
            { id: 'visuals', display_name: 'Visuals', is_community: true, content_types: ['mod', 'shader'] },
          ]);
        }
        if (command === 'list_modrinth_categories') {
          return Promise.resolve([
            { name: 'technology', project_type: 'mod', header: 'categories' },
            { name: 'adventure', project_type: 'mod', header: 'categories' },
            { name: 'adventure', project_type: 'modpack', header: 'categories' },
            { name: 'kitchen-sink', project_type: 'modpack', header: 'categories' },
            { name: 'realistic', project_type: 'shader', header: 'features' },
            { name: 'audio', project_type: 'resourcepack', header: 'features' },
            { name: 'worldgen', project_type: 'datapack', header: 'categories' },
            { name: 'minigame', project_type: 'minecraft_java_server', header: 'minecraft_server_gameplay' },
          ]);
        }
        if (command === 'list_manifest_loaders' || command === 'list_manifest_mc_versions') {
          return Promise.resolve([]);
        }
        if (command === 'get_windows_accent_color') return Promise.resolve(null);
        if (command === 'list_instances') return Promise.resolve([]);
        if (command === 'list_snapshots') return Promise.resolve([]);
        if (command.startsWith('plugin:event|')) return Promise.resolve(1);
        if (command === 'browse_search' || command === 'browse_load_more') {
          return new Promise((resolve, reject) => calls.push({ command, args, resolve, reject }));
        }
        return Promise.resolve(null);
      },
    };
    Object.assign(window as unknown as Record<string, unknown>, {
      __TAURI_INTERNALS__: internals,
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
      __browseCalls: calls,
      __resolveBrowse(index: number, value: unknown) { calls[index].resolve(value); },
      __rejectBrowse(index: number, value: unknown) { calls[index].reject(value); },
    });
  });
}

async function waitForCalls(page: Page, count: number) {
  await expect.poll(() => page.evaluate(() => (window as any).__browseCalls.length)).toBeGreaterThanOrEqual(count);
}

async function findCall(page: Page, command: string, query?: string, excluded: number[] = []) {
  let index = -1;
  await expect.poll(async () => {
    index = await page.evaluate(({ command, query, excluded }) => {
      const calls = (window as any).__browseCalls as Array<{ command: string; args: Record<string, unknown> }>;
      return calls.findIndex((call, i) =>
        !excluded.includes(i)
        && call.command === command
        && (query === undefined || call.args.query === query),
      );
    }, { command, query, excluded });
    return index;
  }).toBeGreaterThanOrEqual(0);
  return index;
}

async function resolveCall(page: Page, index: number, result: BrowseResult) {
  await page.evaluate(({ index, result }) => (window as any).__resolveBrowse(index, result), { index, result });
}

async function openBrowse(page: Page) {
  await page.goto('/');
  await page.getByRole('button', { name: 'Browse', exact: true }).click();
  // React StrictMode intentionally runs mount effects twice in development.
  await waitForCalls(page, 2);
  return page.evaluate(() => (window as any).__browseCalls.length - 1) as Promise<number>;
}

test('out-of-order searches only display the newest query', async ({ page }) => {
  await installBrowseMock(page);
  const initial = await openBrowse(page);
  await resolveCall(page, initial, { items: [item('initial', 'Initial')], total: 1, page: 0, hasMore: false });

  const search = page.getByPlaceholder('Search mods, packs, and more…');
  await search.fill('alpha');
  const alpha = await findCall(page, 'browse_search', 'alpha');
  await search.fill('beta');
  const beta = await findCall(page, 'browse_search', 'beta');

  await resolveCall(page, beta, { items: [item('beta', 'Beta Result')], total: 1, page: 0, hasMore: false });
  await expect(page.getByText('Beta Result')).toBeVisible();
  await resolveCall(page, alpha, { items: [item('alpha', 'Alpha Result')], total: 1, page: 0, hasMore: false });

  await expect(page.getByText('Beta Result')).toBeVisible();
  await expect(page.getByText('Alpha Result')).toHaveCount(0);
});

test('stale pagination is ignored and new query can paginate', async ({ page }) => {
  await installBrowseMock(page);
  const initial = await openBrowse(page);
  await resolveCall(page, initial, { items: [item('a', 'Query A')], total: 40, page: 0, hasMore: true });
  await page.getByTestId('browse-load-sentinel').scrollIntoViewIfNeeded();
  const staleLoad = await findCall(page, 'browse_load_more');

  const search = page.getByPlaceholder('Search mods, packs, and more…');
  await search.fill('beta');
  const beta = await findCall(page, 'browse_search', 'beta');
  await resolveCall(page, beta, { items: [item('b', 'Query B')], total: 40, page: 0, hasMore: true });
  await resolveCall(page, staleLoad, { items: [item('a-more', 'Stale A Page')], total: 40, page: 1, hasMore: false });

  await page.getByTestId('browse-load-sentinel').scrollIntoViewIfNeeded();
  const betaLoad = await findCall(page, 'browse_load_more', undefined, [staleLoad]);
  const args = await page.evaluate((index) => (window as any).__browseCalls[index].args, betaLoad);
  expect(args.queryKey).toContain('beta');
  await resolveCall(page, betaLoad, { items: [item('b', 'Query B'), item('b-more', 'Query B Page')], total: 40, page: 1, hasMore: false });

  await expect(page.getByText('Stale A Page')).toHaveCount(0);
  await expect(page.getByText('Query B Page')).toBeVisible();
  await expect(page.getByText('Query B', { exact: true })).toHaveCount(1);
});

test('pagination failure is visible and retryable', async ({ page }) => {
  await installBrowseMock(page);
  const initial = await openBrowse(page);
  await resolveCall(page, initial, { items: [item('a', 'Initial Page')], total: 40, page: 0, hasMore: true });
  await page.getByTestId('browse-load-sentinel').scrollIntoViewIfNeeded();
  const failedLoad = await findCall(page, 'browse_load_more');
  await page.evaluate((index) => (window as any).__rejectBrowse(index, new Error('Pagination failed')), failedLoad);

  await expect(page.getByText('Pagination failed')).toBeVisible();
  await page.getByRole('button', { name: 'Retry loading more' }).click();
  const retry = await findCall(page, 'browse_load_more', undefined, [failedLoad]);
  await resolveCall(page, retry, { items: [item('more', 'Next Page')], total: 40, page: 1, hasMore: false });
  await expect(page.getByText('Next Page')).toBeVisible();
  await expect(page.getByText('Pagination failed')).toHaveCount(0);
});

test('category lists follow the selected content type', async ({ page }) => {
  await installBrowseMock(page);
  const initial = await openBrowse(page);
  await resolveCall(page, initial, { items: [], total: 0, page: 0, hasMore: false });

  await expect(page.getByRole('button', { name: 'Technology', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Kitchen Sink', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Adventure', exact: true })).toHaveCount(1);
  await expect(page.getByRole('button', { name: 'Minigame', exact: true })).toBeVisible();

  await page.getByLabel('Content type').selectOption('pack');

  await expect(page.getByText('Categories for pack content.')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Kitchen Sink', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Adventure', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Technology', exact: true })).toHaveCount(0);

  await page.getByLabel('Content type').selectOption('server');
  await expect(page.getByRole('button', { name: 'Minigame', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Kitchen Sink', exact: true })).toHaveCount(0);
});

test('curated category dropdown is searchable and type-aware', async ({ page }) => {
  await installBrowseMock(page);
  const initial = await openBrowse(page);
  await resolveCall(page, initial, { items: [], total: 0, page: 0, hasMore: false });
  await page.getByLabel('Content type').selectOption('pack');

  await page.getByRole('button', { name: 'Curated categories' }).click();
  const categorySearch = page.getByLabel('Search curated categories');
  await categorySearch.fill('perf');
  await expect(page.getByText('No curated categories found.')).toBeVisible();
  await categorySearch.fill('quest');
  await expect(page.getByRole('menuitem', { name: 'Questing' })).toBeVisible();
  await page.getByRole('menuitem', { name: 'Questing' }).click();

  await expect(page.getByRole('button', { name: 'Curated category: Questing' })).toBeVisible();
  await expect.poll(() => page.evaluate(() => {
    const calls = (window as any).__browseCalls as Array<{ command: string; args: Record<string, unknown> }>;
    return calls.some((call) => call.command === 'browse_search'
      && call.args.contentType === 'pack'
      && call.args.category === 'questing');
  })).toBe(true);
});

// ---------------------------------------------------------------------------
// D1: Browse instance-context selector and compatibility labels
// ---------------------------------------------------------------------------

const CONTEXT_INSTANCE: Record<string, unknown> = {
  instance_id: 'fabric-121',
  name: 'My Fabric World',
  minecraft_version: '1.21',
  loader: 'fabric',
  loader_version: '0.16.9',
  is_modpack: false,
  is_locked: false,
  last_launched_at: '2026-07-12T08:00:00Z',
  jvm_memory_mb: 4096,
  jvm_gc: 'G1GC',
  jvm_custom_args: '',
  created_at: '2026-06-01T00:00:00Z',
};

const CONTEXT_DETAIL: Record<string, unknown> = {
  row: CONTEXT_INSTANCE,
  manifest: {
    instance_id: 'fabric-121',
    name: 'My Fabric World',
    created_from_pack: null,
    minecraft_version: '1.21',
    loader: 'fabric',
    loader_version: '0.16.9',
    is_locked: false,
    mods: [
      { filename: 'already-installed.jar', registry_id: 'installed-mod', modrinth_id: null, source: 'curated', version: '1.0.0', sha256: 'a', installed_at: '2026-07-01T00:00:00Z', mod_jar_id: 'installed-mod', enabled: true, content_type: 'mod' },
    ],
    resourcepacks: [],
    shaders: [],
    datapacks: [],
    worlds: [],
    user_preferences: {},
  },
};

async function installBrowseContextMock(page: Page) {
  // Serialize fixture data as params so addInitScript's serialized closure
  // can access them (module-level variables are not captured).
  const instanceData = CONTEXT_INSTANCE;
  const detailData = CONTEXT_DETAIL;

  await page.addInitScript(
    (params: { instance: Record<string, unknown>; detail: Record<string, unknown> }) => {
      const { instance, detail } = params;
      const callbacks = new Map<number, (...args: unknown[]) => void>();
      let callbackId = 0;
      let updateChecks = 0;

      // Helper inlined because addInitScript only serializes the function body.
      function compatItem(id: string, name: string): Record<string, unknown> {
        return {
          id, source: 'curated',
          registryItem: {
            id, name, content_type: 'mod', download_strategy: 'github_release',
            source_identifier: `${id}/releases`, sha256: '', upvotes: 5, downvotes: 0,
            net_score: 5, velocity: 0, status: 'active', is_immune: false,
            immunity_reason: null, allow_comments: true, icon_url: null,
            gallery_urls_json: null, date_added: '2026-06-01',
            compatible_versions_json: JSON.stringify([{ mc_version: '1.21', loader: 'fabric', mod_version: '1.0.0' }]),
            description: null, body_markdown: null, page_url: null, license_id: 'MIT',
            source_updated_at: '2026-07-01T00:00:00Z', modrinth_id: null,
            recommendation_reason: null, recommendation_overlap: null,
          },
          modrinthResult: null, name, iconUrl: null, description: null, contentType: 'mod',
        };
      }

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
            if (key === 'modrinth_enabled') return Promise.resolve(true);
            if (key === 'ai_chat_enabled') return Promise.resolve(false);
            if (key === 'ai_mcp_enabled') return Promise.resolve(false);
            return Promise.resolve(null);
          }

          // Registry
          if (command === 'get_registry_status') {
            return Promise.resolve({
              has_cached_db: true, cached_tag: 'test', cached_schema_version: 5,
              latest_tag: 'test', update_available: false, checked: true,
              message: 'Registry ready.',
            });
          }
          if (command === 'check_registry_update') {
            return Promise.resolve({
              has_cached_db: true, cached_tag: 'test', cached_schema_version: 5,
              latest_tag: 'test', update_available: false, checked: true,
              message: 'Registry ready.',
            });
          }
          if (command === 'list_categories') return Promise.resolve([]);
          if (command === 'list_manifest_loaders') return Promise.resolve(['fabric', 'forge', 'quilt']);
          if (command === 'list_manifest_mc_versions') return Promise.resolve(['1.20.1', '1.21', '1.21.1']);

          // Browse search — immediate (not deferred) for context tests
          if (command === 'browse_search') {
            return Promise.resolve({
              items: [
                compatItem('exact-mod', 'Exact Mod'),
                compatItem('major-mod', 'Major Match Mod'),
                compatItem('installed-mod', 'Already Installed'),
                compatItem('updatable-mod', 'Updatable Mod'),
              ],
              total: 4,
              page: 0,
              hasMore: false,
            });
          }
          if (command === 'browse_load_more') {
            return Promise.resolve({ items: [], total: 0, page: 1, hasMore: false });
          }

          // For You items
          if (command === 'for_you_items') {
            return Promise.resolve([
              {
                id: 'rec-for-you',
                name: 'For You Mod',
                content_type: 'mod',
                download_strategy: 'github_release',
                source_identifier: 'rec-for-you/releases',
                sha256: '',
                upvotes: 10,
                downvotes: 0,
                net_score: 10,
                velocity: 1.5,
                status: 'active',
                is_immune: false,
                immunity_reason: null,
                allow_comments: true,
                icon_url: null,
                gallery_urls_json: null,
                date_added: '2026-07-01',
                compatible_versions_json: null,
                description: 'A personalized recommendation.',
                body_markdown: null,
                page_url: null,
                license_id: 'MIT',
                source_updated_at: '2026-07-10T00:00:00Z',
                modrinth_id: null,
                recommendation_reason: 'Recommended by Agora\'s curated score for fabric 1.21.',
                recommendation_overlap: 5,
              },
            ]);
          }

          // Instances — use the params passed from the outer scope
          if (command === 'list_instances') {
            return Promise.resolve([instance]);
          }
          if (command === 'get_instance_detail') {
            return Promise.resolve(detail);
          }
          if (command === 'check_instance_updates') {
            updateChecks += 1;
            return Promise.resolve([
              {
                filename: 'updatable-mod.jar',
                mod_jar_id: 'updatable-mod',
                current_version: '1.0.0',
                latest_version: '2.0.0',
                target_version: '2.0.0',
                source: 'curated',
              },
            ]);
          }

          // Compatibility
          if (command === 'batch_check_compat') {
            const itemIds = args.itemIds as string[] | undefined;
            const result: Record<string, string> = {};
            if (itemIds) {
              for (const id of itemIds) {
                if (id === 'exact-mod') result[id] = 'compatible';
                else if (id === 'major-mod') result[id] = 'major_match';
                else if (id === 'installed-mod') result[id] = 'compatible';
                else if (id === 'updatable-mod') result[id] = 'compatible';
                else result[id] = '';
              }
            }
            return Promise.resolve(result);
          }

          // Misc
          if (command === 'get_windows_accent_color') return Promise.resolve(null);
          if (command.startsWith('plugin:event|')) return Promise.resolve(1);
          if (command === 'get_auth_status') return Promise.resolve(true);
          if (command === 'get_github_profile') return Promise.resolve(null);
          if (command === 'get_flag_rate_limit') return Promise.resolve(null);

          return Promise.resolve(null);
        },
      };
      Object.assign(window as unknown as Record<string, unknown>, {
        __TAURI_INTERNALS__: internals,
        __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
        __browseUpdateChecks: () => updateChecks,
      });
    },
    { instance: instanceData, detail: detailData },
  );
}

test.describe('D1 — Browse instance-context selector', () => {

  test('instance selector shows compatibility labels via batch_check_compat', async ({ page }) => {
    await installBrowseContextMock(page);
    await page.goto('/');
    await page.getByRole('button', { name: 'Browse', exact: true }).click();

    // Wait for the items to render
    await expect(page.getByText('Exact Mod')).toBeVisible({ timeout: 5000 });

    // Select the instance from the "Discover for an instance" dropdown
    const contextSelect = page.locator('#browse-instance-context');
    await contextSelect.selectOption('fabric-121');

    // Wait for compatibility labels — the effect calls batch_check_compat
    // after activeInstance is set. Since all promises resolve immediately,
    // the labels appear in the next React render.
    await expect(page.getByText(/Compatible with My Fabric World/).first()).toBeVisible({ timeout: 5000 });
    await expect(page.getByText(/May work with My Fabric World/)).toBeVisible({ timeout: 5000 });
  });

  test('exact-compatible label format for an item', async ({ page }) => {
    await installBrowseContextMock(page);
    await page.goto('/');
    await page.getByRole('button', { name: 'Browse', exact: true }).click();

    await expect(page.getByText('Exact Mod')).toBeVisible();

    const contextSelect = page.locator('#browse-instance-context');
    await contextSelect.selectOption('fabric-121');

    // The paginated browse loads items on the page — wait for the "Compatible
    // with" label to appear on items. Use .first() because 3 items return
    // 'compatible' from batch_check_compat.
    await expect(
      page.getByText(/Compatible with My Fabric World · fabric · MC 1\.21/).first(),
    ).toBeVisible({ timeout: 5000 });
  });

  test('major-match label format for an item', async ({ page }) => {
    await installBrowseContextMock(page);
    await page.goto('/');
    await page.getByRole('button', { name: 'Browse', exact: true }).click();

    await expect(page.getByText('Major Match Mod')).toBeVisible();

    const contextSelect = page.locator('#browse-instance-context');
    await contextSelect.selectOption('fabric-121');

    await expect(
      page.getByText(/May work with My Fabric World · same major Minecraft version/),
    ).toBeVisible({ timeout: 5000 });
  });

  test('installed label shown for items in the active instance manifest', async ({ page }) => {
    await installBrowseContextMock(page);
    await page.goto('/');
    await page.getByRole('button', { name: 'Browse', exact: true }).click();

    await expect(page.getByText('Already Installed')).toBeVisible();

    const contextSelect = page.locator('#browse-instance-context');
    await contextSelect.selectOption('fabric-121');

    // "Already Installed" has registry_id = 'installed-mod' which matches item.id
    await expect(page.getByText('Installed').first()).toBeVisible({ timeout: 5000 });
  });

  test('selecting an instance does not check mod updates', async ({ page }) => {
    await installBrowseContextMock(page);
    await page.goto('/');
    await page.getByRole('button', { name: 'Browse', exact: true }).click();

    await expect(page.getByText('Updatable Mod')).toBeVisible();

    const contextSelect = page.locator('#browse-instance-context');
    await contextSelect.selectOption('fabric-121');

    await expect(page.getByText(/Compatible with My Fabric World/).first()).toBeVisible({ timeout: 5000 });
    expect(await page.evaluate(() => (window as any).__browseUpdateChecks())).toBe(0);
    await expect(page.getByText('Update available')).toHaveCount(0);
  });

  test('For You sort shows per-item recommendation reason', async ({ page }) => {
    await installBrowseContextMock(page);
    await page.goto('/');
    await page.getByRole('button', { name: 'Browse', exact: true }).click();

    // Select the instance first — activeInstance is needed for contextFor to
    // return the recommendation reason.
    const contextSelect = page.locator('#browse-instance-context');
    await contextSelect.selectOption('fabric-121');
    await expect(page.getByText(/Compatible with/).first()).toBeVisible({ timeout: 5000 });

    // Now switch sort to "For You"
    const selects = page.locator('select');
    const count = await selects.count();
    let sortFound = false;
    for (let i = 0; i < count; i++) {
      const options = await selects.nth(i).locator('option').allTextContents();
      if (options.includes('For You')) {
        await selects.nth(i).selectOption('for_you');
        sortFound = true;
        break;
      }
    }
    if (!sortFound) {
      // Fallback: last select is the sort selector
      await selects.last().selectOption('for_you');
    }

    // Wait for the For You item to appear (from forYouItems mock)
    await expect(page.getByText('For You Mod')).toBeVisible({ timeout: 5000 });

    // The item's registryItem has recommendation_reason set, and activeInstance
    // is populated, so the "Why:" text appears.
    await expect(
      page.getByText(/Why: Recommended by Agora's curated score for fabric 1\.21/),
    ).toBeVisible();
  });

});
