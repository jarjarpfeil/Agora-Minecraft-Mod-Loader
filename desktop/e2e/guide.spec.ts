import { expect, test, type Page } from '@playwright/test';

async function installGuideMock(page: Page) {
  await page.addInitScript(() => {
    const callbacks = new Map<number, (...args: unknown[]) => void>();
    let callbackId = 0;
    const registryStatus = {
      has_cached_db: true,
      cached_tag: 'test',
      cached_schema_version: 5,
      latest_tag: 'test',
      update_available: false,
      checked: true,
      message: 'Registry ready.',
    };

    const internals = {
      transformCallback(callback: (...args: unknown[]) => void) {
        const id = ++callbackId;
        callbacks.set(id, callback);
        return id;
      },
      unregisterCallback(id: number) {
        callbacks.delete(id);
      },
      invoke(command: string, args: Record<string, unknown> = {}) {
        if (command === 'get_setting') {
          const key = args.key as string;
          if (key === 'onboarding_complete') return Promise.resolve(true);
          if (key === 'ai_chat_enabled') return Promise.resolve(false);
          if (key === 'advanced_mode') return Promise.resolve('false');
          return Promise.resolve(null);
        }
        if (command === 'get_registry_status') return Promise.resolve(registryStatus);
        if (command === 'list_instances') return Promise.resolve([]);
        if (command === 'list_categories') return Promise.resolve([]);
        if (command === 'list_manifest_loaders') return Promise.resolve([]);
        if (command === 'list_manifest_mc_versions') return Promise.resolve([]);
        if (command === 'browse_search') return Promise.resolve({ items: [], total: 0, page: 0, hasMore: false });
        if (command === 'browse_load_more') return Promise.resolve({ items: [], total: 0, page: 1, hasMore: false });
        if (command === 'get_windows_accent_color') return Promise.resolve(null);
        if (command.startsWith('plugin:event|')) return Promise.resolve(1);
        return Promise.resolve(null);
      },
    };

    Object.assign(window as unknown as Record<string, unknown>, {
      __TAURI_INTERNALS__: internals,
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
    });
  });
}

test.beforeEach(async ({ page }) => {
  await installGuideMock(page);
  await page.goto('/');
});

test('links the guide from the sidebar and Home page', async ({ page }) => {
  await expect(page.getByTestId('sidebar').getByRole('button', { name: 'Help & Guide', exact: true })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Learn Agora at your level' })).toBeVisible();
  await expect(page.getByText('36 guide pages')).toBeVisible();

  await page.getByRole('button', { name: /Open Help & Guide/ }).click();

  await expect(page.getByTestId('guide-page')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Learn the launcher. Understand your modded game.' })).toBeVisible();
  await expect(page.getByText('18 topics, each with a beginner-friendly walkthrough')).toBeVisible();
});

test('switches experience levels and searches all guide content', async ({ page }) => {
  await page.getByTestId('sidebar').getByRole('button', { name: 'Help & Guide', exact: true }).click();

  await expect(page.getByRole('heading', { name: 'Getting started with Agora' })).toBeVisible();
  await expect(page.getByText(/Agora helps you discover, organize, and recover/)).toBeVisible();

  await page.getByRole('button', { name: /^Advanced guide Deeper/ }).click();
  await expect(page.getByText(/Treat onboarding as a policy decision/)).toBeVisible();

  await page.getByLabel('Search the guide').fill('port 39741');
  await expect(page.getByText('1 topic found')).toBeVisible();
  await page.getByRole('button', { name: 'MCP & automation' }).click();

  await expect(page.getByRole('heading', { name: 'MCP and external AI tools' })).toBeVisible();
  await expect(page.getByText(/Operate MCP as a privileged local automation surface/)).toBeVisible();

  await page.getByRole('button', { name: /^Basic guide Clear/ }).click();
  await expect(page.getByText(/MCP is an advanced optional bridge/)).toBeVisible();
});

test('tracks completed pages and restores the selected guide page', async ({ page }) => {
  await page.getByTestId('sidebar').getByRole('button', { name: 'Help & Guide', exact: true }).click();
  await page.getByRole('button', { name: /^Advanced guide Deeper/ }).click();
  await page.getByRole('button', { name: 'Mark complete' }).click();

  await expect(page.getByRole('button', { name: 'Completed', exact: true })).toBeVisible();
  await expect(page.getByText('1 of 36 pages')).toBeVisible();

  await page.reload();

  await expect(page.getByTestId('guide-page')).toBeVisible();
  await expect(page.getByText(/Treat onboarding as a policy decision/)).toBeVisible();
  await expect(page.getByRole('button', { name: 'Completed', exact: true })).toBeVisible();

  await page.getByRole('button', { name: 'Reset', exact: true }).click();
  await expect(page.getByText('0 of 36 pages')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Mark complete' })).toBeVisible();
});

test('supports sequential, quick-start, and command-palette navigation', async ({ page }) => {
  await page.getByRole('button', { name: 'Open command palette' }).click();
  await page.getByPlaceholder('Type a command or search…').fill('help');
  await page.getByRole('option', { name: 'Help & Guide' }).click();

  await expect(page.getByTestId('guide-page')).toBeVisible();
  await page.getByRole('button', { name: /Next page Getting started: Advanced guide/ }).click();
  await expect(page.getByText(/Treat onboarding as a policy decision/)).toBeVisible();

  await page.getByRole('button', { name: /Experienced modder Go straight to advanced/ }).click();
  await expect(page.getByRole('heading', { name: 'Java, memory, and performance' })).toBeVisible();
  await expect(page.getByText(/Override Java and JVM behavior per instance/)).toBeVisible();
});
