import { test, expect } from '@playwright/test';

test('hanging startup invokes still render a branded shell immediately', async ({ page }) => {
  await page.addInitScript(() => {
    const never = new Promise(() => {});
    Object.assign(window as unknown as Record<string, unknown>, {
      __TAURI_INTERNALS__: {
        transformCallback() { return 1; },
        unregisterCallback() {},
        invoke(command: string) {
          if (command === 'get_setting' || command === 'get_windows_accent_color') return never;
          return Promise.resolve(null);
        },
      },
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
    });
  });
  await page.goto('/');
  await expect(page.getByText('Agora Launcher')).toBeVisible();
  await expect(page.getByText('Preparing your library…')).toBeVisible();
});

test('stored preferences apply before optional Windows accent resolves', async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('agora-ui-preferences', JSON.stringify({
      version: 1,
      colorMode: 'dark',
      accentMode: 'custom',
      customAccent: '#336699',
      fontFamily: 'system',
      fontScale: 1,
      density: 'comfortable',
      cornerStyle: 'soft',
      motion: 'system',
      highContrast: false,
      backgroundEffects: true,
    }));
    Object.assign(window as unknown as Record<string, unknown>, {
      __TAURI_INTERNALS__: {
        transformCallback() { return 1; },
        unregisterCallback() {},
        invoke(command: string, args: Record<string, unknown> = {}) {
          if (command === 'get_setting') return Promise.resolve(args.key === 'onboarding_complete');
          if (command === 'get_windows_accent_color') return new Promise(() => {});
          if (command.startsWith('plugin:event|')) return Promise.resolve(1);
          if (command === 'get_registry_status') return Promise.resolve({ has_cached_db: false, checked: true, message: 'Missing' });
          return Promise.resolve(null);
        },
      },
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
    });
  });
  await page.goto('/');
  await expect(page.locator('html')).toHaveClass(/dark/);
  await expect.poll(() => page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue('--primary').trim())).toBe('210 50% 52%');
});
