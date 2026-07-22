import { test, expect } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    Object.assign(window as unknown as Record<string, unknown>, {
      __TAURI_INTERNALS__: {
        transformCallback() { return 1; },
        unregisterCallback() {},
        invoke(command: string, args: Record<string, unknown> = {}) {
          if (command === 'get_setting') return Promise.resolve(args.key === 'onboarding_complete');
          if (command === 'get_windows_accent_color') return Promise.resolve(null);
          if (command.startsWith('plugin:event|')) return Promise.resolve(1);
          if (command === 'get_registry_status') return Promise.resolve({ has_cached_db: false, checked: true, message: 'Missing' });
          if (command === 'list_instances' || command === 'list_snapshots' || command === 'for_you_items') return Promise.resolve([]);
          return Promise.resolve(null);
        },
      },
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
    });
  });
});

test('sidebar keyboard resize and collapse state persist', async ({ page }) => {
  await page.goto('/');
  const sidebar = page.getByTestId('sidebar');
  const separator = page.getByRole('separator', { name: 'Resize sidebar' });

  await separator.focus();
  await separator.press('ArrowRight');
  await expect(sidebar).toHaveCSS('width', '264px');

  await page.getByRole('button', { name: 'Collapse sidebar' }).click();
  await expect(sidebar).toHaveCSS('width', '64px');
  await page.reload();
  await expect(sidebar).toHaveCSS('width', '64px');
  await page.getByRole('button', { name: 'Expand sidebar' }).click();
  await expect(sidebar).toHaveCSS('width', '264px');
});

test('layout clamps persisted widths and recovers from corrupt storage', async ({ page }) => {
  await page.goto('/');
  await page.evaluate(() => {
    localStorage.setItem('agora-shell-layout', JSON.stringify({
      version: 1,
      sidebar: { collapsed: false, width: 9999, lastExpandedWidth: 9999 },
    }));
  });
  await page.reload();
  await expect(page.getByTestId('sidebar')).toHaveCSS('width', '420px');

  await page.evaluate(() => localStorage.setItem('agora-shell-layout', '{broken'));
  await page.reload();
  await expect(page.getByTestId('sidebar')).toHaveCSS('width', '256px');
});

test('layout can be reset independently from appearance', async ({ page }) => {
  await page.goto('/');
  const separator = page.getByRole('separator', { name: 'Resize sidebar' });
  await separator.focus();
  await separator.press('ArrowRight');
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('button', { name: 'Reset layout' }).click();
  await expect(page.getByTestId('sidebar')).toHaveCSS('width', '256px');
  await expect(page.getByLabel('Accent source')).toHaveValue('agora');
});
