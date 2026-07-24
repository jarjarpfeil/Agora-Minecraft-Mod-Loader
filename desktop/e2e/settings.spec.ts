import { test, expect } from '@playwright/test';

/**
 * Verify that one failed setting read does not prevent others from loading,
 * and that the page still renders fully.
 */
test('one failed setting does not cascade and settings page renders', async ({ page }) => {
  await page.addInitScript(() => {
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
          const key = args.key as string;
          // ai_mcp_enabled fails to load — simulate backend error.
          if (key === 'ai_mcp_enabled') {
            return Promise.reject(new Error('Backend unavailable'));
          }
          // All other settings succeed.
          if (key === 'modrinth_enabled') return Promise.resolve(true);
          if (key === 'always_pre_touch') return Promise.resolve(true);
            if (key === 'mojang_launcher_path') return Promise.resolve('');
          if (key === 'launch_mode') return Promise.resolve('delegation');
          if (key === 'onboarding_complete') return Promise.resolve(true);
          if (key === 'ai_chat_enabled') return Promise.resolve(true);
          return Promise.resolve(null);
        }
        if (command === 'get_windows_accent_color') return Promise.resolve(null);
        if (command === 'list_instances') return Promise.resolve([]);
        if (command === 'list_snapshots') return Promise.resolve([]);
        if (command.startsWith('plugin:event|')) return Promise.resolve(1);
        if (command === 'set_setting') return Promise.resolve(null);
        return Promise.resolve(null);
      },
    };
    Object.assign(window as unknown as Record<string, unknown>, {
      __TAURI_INTERNALS__: internals,
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
    });
  });

  await page.goto('/');
  await page.getByRole('button', { name: 'Settings', exact: true }).click();

  // The settings page should render all sections even though ai_mcp_enabled
  // failed to load. Sensible defaults are used for the failed setting.
  await expect(page.getByText('Modrinth Access')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'GitHub Account' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Launch Mode' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Launcher Path' })).toBeVisible();

  const contents = page.getByRole('navigation', { name: 'Settings sections' });
  await expect(contents.getByRole('button', { name: 'Appearance' })).toBeVisible();
  await contents.getByRole('button', { name: 'Launching' }).click();
  await expect(page.locator('#settings-launching')).toContainText('Launch Mode');
  await expect(page.locator('#settings-launching')).toBeInViewport();
});

test('boolean settings are sent to Tauri as JSON booleans', async ({ page }) => {
  await page.addInitScript(() => {
    const callbacks = new Map<number, (...args: unknown[]) => void>();
    const writes: Array<{ key: string; value: unknown }> = [];
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
          const key = args.key as string;
          if (key === 'onboarding_complete') return Promise.resolve(true);
          if (key === 'modrinth_enabled') return Promise.resolve(false);
          if (key === 'always_pre_touch') return Promise.resolve(true);
          if (key === 'launch_mode') return Promise.resolve('delegation');
          if (key === 'mojang_launcher_path') return Promise.resolve('');
          return Promise.resolve(null);
        }
        if (command === 'set_setting') {
          writes.push({ key: args.key, value: args.value });
          return Promise.resolve(null);
        }
        if (command === 'get_windows_accent_color') return Promise.resolve(null);
        if (command === 'list_instances') return Promise.resolve([]);
        if (command.startsWith('plugin:event|')) return Promise.resolve(1);
        return Promise.resolve(null);
      },
    };
    Object.assign(window as unknown as Record<string, unknown>, {
      __TAURI_INTERNALS__: internals,
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
      __settingWrites: writes,
    });
  });

  await page.goto('/');
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(page.getByText('Modrinth Access')).toBeVisible();
  await page.locator('#settings-services input[type="checkbox"]').first().check();

  await expect.poll(async () => page.evaluate(() => (window as any).__settingWrites)).toContainEqual({
    key: 'modrinth_enabled',
    value: true,
  });
});
