import { test, expect } from '@playwright/test';

/**
 * Launcher path Browse, Auto-detect, and Test actions (B3).
 *
 * Uses the same Tauri mock infrastructure as settings.spec.ts.
 * The test simulates each backend response to verify the UI behaves
 * correctly: inline errors, success messages, and button states.
 */

function setupTauriMocks(page: import('@playwright/test').Page, overrides: Record<string, unknown> = {}) {
  return page.addInitScript((overrides) => {
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
        // Default settings responses
        if (command === 'get_setting') {
          const key = args.key as string;
          if (key === 'modrinth_enabled') return Promise.resolve(true);
          if (key === 'ai_mcp_enabled') return Promise.resolve(false);
          if (key === 'ai_chat_enabled') return Promise.resolve(true);
          if (key === 'mojang_launcher_path') return Promise.resolve('');
          if (key === 'always_pre_touch') return Promise.resolve(true);
          if (key === 'launch_mode') return Promise.resolve('delegation');
          if (key === 'onboarding_complete') return Promise.resolve(true);
          if (key === 'java_path') return Promise.resolve('');
          return Promise.resolve(null);
        }
        if (command === 'set_setting') return Promise.resolve(null);
        if (command === 'get_windows_accent_color') return Promise.resolve(null);
        if (command === 'list_instances') return Promise.resolve([]);

        // --- Launcher path command mocks ---
        if (command === 'detect_mojang_launcher') {
          if (overrides.detectError) {
            return Promise.reject(new Error(overrides.detectError as string));
          }
          return Promise.resolve(overrides.detectResult ?? 'C:\\Program Files\\Minecraft Launcher\\MinecraftLauncher.exe');
        }
        if (command === 'test_launcher_path') {
          if (overrides.testError) {
            return Promise.reject(new Error(overrides.testError as string));
          }
          return Promise.resolve(true);
        }
        if (command === 'pick_open_file') {
          return Promise.resolve(overrides.pickResult ?? 'C:\\Custom\\Minecraft.exe');
        }
        return Promise.resolve(null);
      },
    };
    Object.assign(window as unknown as Record<string, unknown>, {
      __TAURI_INTERNALS__: internals,
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
    });
  }, overrides);
}

test.describe('Launcher Path (B3)', () => {
  test('renders launcher path section with all buttons', async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await page.getByRole('button', { name: 'Settings', exact: true }).click();

    await expect(page.getByRole('heading', { name: 'Launcher Path' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Save' }).first()).toBeVisible();
    await expect(page.getByRole('button', { name: 'Browse…' }).first()).toBeVisible();
    await expect(page.getByRole('button', { name: 'Auto-detect' }).first()).toBeVisible();
    await expect(page.getByRole('button', { name: 'Test' }).first()).toBeVisible();
  });

  test('Auto-detect fills path and shows success', async ({ page }) => {
    await setupTauriMocks(page, {
      detectResult: 'C:\\Program Files\\Minecraft Launcher\\MinecraftLauncher.exe',
    });
    await page.goto('/');
    await page.getByRole('button', { name: 'Settings', exact: true }).click();

    await page.getByRole('button', { name: 'Auto-detect' }).click();

    // The input should be filled with the detected path
    const input = page.getByPlaceholder('Auto-discovered if empty');
    await expect(input).toHaveValue('C:\\Program Files\\Minecraft Launcher\\MinecraftLauncher.exe');

    // Success message should appear
    await expect(page.getByText('Detected:')).toBeVisible();
  });

  test('Auto-detect failure shows inline error', async ({ page }) => {
    await setupTauriMocks(page, {
      detectError: 'Mojang launcher not found',
    });
    await page.goto('/');
    await page.getByRole('button', { name: 'Settings', exact: true }).click();

    await page.getByRole('button', { name: 'Auto-detect' }).click();

    // Error should appear inline
    await expect(page.getByText(/Auto-detect failed/)).toBeVisible();
  });

  test('Test validates current path and shows success', async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await page.getByRole('button', { name: 'Settings', exact: true }).click();

    // Type a path in the input
    const input = page.getByPlaceholder('Auto-discovered if empty');
    await input.fill('C:\\Minecraft\\MinecraftLauncher.exe');

    await page.getByRole('button', { name: 'Test' }).click();

    // Success message should appear
    await expect(page.getByText('Path is valid.')).toBeVisible();
  });

  test('Test failure shows inline error', async ({ page }) => {
    await setupTauriMocks(page, {
      testError: 'Path does not exist: C:\\Minecraft\\MinecraftLauncher.exe',
    });
    await page.goto('/');
    await page.getByRole('button', { name: 'Settings', exact: true }).click();

    const input = page.getByPlaceholder('Auto-discovered if empty');
    await input.fill('C:\\Minecraft\\MinecraftLauncher.exe');

    await page.getByRole('button', { name: 'Test' }).click();

    // Error should appear inline
    await expect(page.getByText(/Test failed/)).toBeVisible();
  });

  test('Test is disabled when path is empty', async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await page.getByRole('button', { name: 'Settings', exact: true }).click();

    const testButton = page.getByRole('button', { name: 'Test' });
    await expect(testButton).toBeDisabled();
  });
});

test.describe('Inline setting errors (B3)', () => {
  test('shows per-section error when save fails', async ({ page }) => {
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
            if (key === 'modrinth_enabled') return Promise.resolve(true);
            if (key === 'ai_mcp_enabled') return Promise.resolve(false);
            if (key === 'ai_chat_enabled') return Promise.resolve(true);
            if (key === 'mojang_launcher_path') return Promise.resolve('');
            if (key === 'always_pre_touch') return Promise.resolve(true);
            if (key === 'launch_mode') return Promise.resolve('delegation');
            if (key === 'onboarding_complete') return Promise.resolve(true);
            if (key === 'java_path') return Promise.resolve('');
            return Promise.resolve(null);
          }
          // Simulate save failure for launch_mode
          if (command === 'set_setting' && (args.key as string) === 'launch_mode') {
            return Promise.reject(new Error('Backend write failed'));
          }
          if (command === 'set_setting') return Promise.resolve(null);
          if (command === 'get_windows_accent_color') return Promise.resolve(null);
          if (command === 'list_instances') return Promise.resolve([]);
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

    // Toggle launch mode to trigger a save that will fail
    const launchModeCheckbox = page.getByRole('heading', { name: 'Launch Mode' }).locator('..').getByRole('checkbox');
    // The checkbox reflects the current state; clicking should toggle and trigger save
    await launchModeCheckbox.click();

    // The inline error should appear in the Launch Mode section
    await expect(
      page.getByRole('heading', { name: 'Launch Mode' }).locator('..').getByText('Backend write failed', { exact: true }),
    ).toBeVisible();
  });

  test('one failed setting banner shows on load', async ({ page }) => {
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
            if (key === 'ai_mcp_enabled') {
              return Promise.reject(new Error('Backend unavailable'));
            }
            if (key === 'modrinth_enabled') return Promise.resolve(true);
            if (key === 'always_pre_touch') return Promise.resolve(true);
            if (key === 'mojang_launcher_path') return Promise.resolve('');
            if (key === 'launch_mode') return Promise.resolve('delegation');
            if (key === 'onboarding_complete') return Promise.resolve(true);
            if (key === 'ai_chat_enabled') return Promise.resolve(true);
            if (key === 'java_path') return Promise.resolve('');
            return Promise.resolve(null);
          }
          if (command === 'get_windows_accent_color') return Promise.resolve(null);
          if (command === 'list_instances') return Promise.resolve([]);
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

    // The error banner should appear
    await expect(page.getByText('Some settings failed to load')).toBeVisible();
    // The specific error code should be shown
    await expect(page.getByText('ai_mcp_enabled')).toBeVisible();

    // But the page should still render fully
    await expect(page.getByRole('heading', { name: 'Launch Mode' })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Launcher Path' })).toBeVisible();
  });
});
