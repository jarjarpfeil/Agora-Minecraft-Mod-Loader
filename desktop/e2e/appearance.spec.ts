import { test, expect } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    Object.assign(window as unknown as Record<string, unknown>, {
      __TAURI_INTERNALS__: {
        transformCallback() { return 1; },
        unregisterCallback() {},
        invoke(command: string, args: Record<string, unknown> = {}) {
          if (command === 'get_setting') return Promise.resolve(args.key === 'onboarding_complete');
          if (command === 'get_windows_accent_color') return Promise.resolve('hsl(12 80% 45%)');
          if (command.startsWith('plugin:event|')) return Promise.resolve(1);
          if (command === 'get_registry_status') return Promise.resolve({ has_cached_db: false, checked: true, message: 'Missing' });
          if (command === 'list_instances' || command === 'list_snapshots' || command === 'for_you_items') return Promise.resolve([]);
          return Promise.resolve(null);
        },
      },
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
    });
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
});

test('custom accent updates semantic tokens, persists, and resets', async ({ page }) => {
  await page.getByLabel('Accent source').selectOption('custom');
  await page.getByLabel('Custom accent color').fill('#663399');

  await expect.poll(() => page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue('--primary').trim())).toBe('270 50% 40%');
  await expect(page.getByRole('button', { name: 'Settings', exact: true })).toHaveCSS('background-color', 'rgb(102, 51, 153)');
  await expect(page.getByText('Open to all')).toHaveCSS('color', 'rgb(102, 51, 153)');

  await page.reload();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(page.getByLabel('Accent source')).toHaveValue('custom');
  await expect(page.getByLabel('Custom accent color')).toHaveValue('#663399');

  await page.getByRole('button', { name: 'Reset appearance' }).click();
  await expect(page.getByLabel('Accent source')).toHaveValue('agora');
  await expect.poll(() => page.evaluate(() => document.documentElement.style.getPropertyValue('--primary'))).toBe('');
});

test('system accent remains a mode and is not persisted as a custom color', async ({ page }) => {
  await page.getByLabel('Accent source').selectOption('system');
  await expect.poll(() => page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue('--primary').trim())).toBe('12 80% 42%');

  const stored = await page.evaluate(() => JSON.parse(localStorage.getItem('agora-ui-preferences') ?? '{}'));
  expect(stored.accentMode).toBe('system');
  expect(stored.customAccent).toBe('#247786');
});

test('font, scale, contrast, and reduced motion persist', async ({ page }) => {
  await page.getByLabel('Interface font').selectOption('readable');
  await page.getByLabel('Text scale').fill('1.15');
  await page.getByLabel('Motion preference').selectOption('reduced');
  await page.getByLabel('High contrast').check();

  await expect(page.locator('html')).toHaveAttribute('data-font', 'readable');
  await expect(page.locator('html')).toHaveAttribute('data-motion', 'reduced');
  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('data-contrast', 'high');
  await expect.poll(() => page.evaluate(() => document.documentElement.style.getPropertyValue('--font-scale'))).toBe('1.15');
});

test('background, text, density, scale, corners, effects, and fonts apply broadly', async ({ page }) => {
  const appearance = page.getByTestId('appearance-settings');
  const sampleText = appearance.getByText('Color, readability, spacing, and motion preferences apply immediately.');
  const initialPadding = await appearance.evaluate((element) => getComputedStyle(element).paddingTop);
  const initialFontSize = await sampleText.evaluate((element) => getComputedStyle(element).fontSize);
  const initialRadius = await appearance.evaluate((element) => getComputedStyle(element).borderRadius);

  await page.getByLabel('Information density').selectOption('compact');
  await page.getByLabel('Text scale').fill('1.25');
  await page.getByLabel('Corner style').selectOption('square');
  await page.getByLabel('Interface font').selectOption('playful');
  await expect.poll(() => appearance.evaluate((element) => getComputedStyle(element).paddingTop)).not.toBe(initialPadding);
  await expect.poll(() => sampleText.evaluate((element) => getComputedStyle(element).fontSize)).not.toBe(initialFontSize);
  await expect.poll(() => appearance.evaluate((element) => getComputedStyle(element).borderRadius)).not.toBe(initialRadius);
  await expect(page.locator('body')).toHaveCSS('font-family', /Comic Sans MS/);

  await page.getByLabel('Toggle custom colors').click();
  await page.getByLabel('Use custom background', { exact: true }).check();
  await page.getByLabel('Background color').fill('#102030');
  await page.getByLabel('Use custom text color').check();
  await page.getByLabel('Block text color').fill('#f0e0d0');
  await expect.poll(() => page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue('--background').trim())).toBe('210 50% 13%');
  await expect.poll(() => page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue('--foreground').trim())).toBe('30 52% 88%');

  const sidebar = page.getByTestId('sidebar');
  await page.getByLabel('Decorative background effects').uncheck();
  await expect(sidebar).toHaveCSS('box-shadow', 'none');
  await expect(page.locator('body')).toHaveCSS('background-image', 'none');
});

test('block, navigation, and background text colors have independent opacity-aware tokens', async ({ page }) => {
  await page.getByLabel('Toggle custom colors').click();
  await page.getByLabel('Use custom block color').check();
  await page.getByLabel('Block color', { exact: true }).fill('#345678');
  await page.getByLabel('Block opacity').fill('0.65');
  await page.getByLabel('Use custom navigation color').check();
  await page.getByLabel('Navigation color', { exact: true }).fill('#221144');
  await page.getByLabel('Navigation opacity').fill('0.55');
  await page.getByLabel('Use custom background text color').check();
  await page.getByLabel('Background text color', { exact: true }).fill('#abcdef');

  await expect.poll(() => page.evaluate(() => document.documentElement.style.getPropertyValue('--card'))).toBe('210 40% 34%');
  await expect.poll(() => page.evaluate(() => document.documentElement.style.getPropertyValue('--surface-opacity'))).toBe('0.65');
  await expect.poll(() => page.evaluate(() => document.documentElement.style.getPropertyValue('--nav-surface'))).toBe('260 60% 17%');
  await expect.poll(() => page.evaluate(() => document.documentElement.style.getPropertyValue('--nav-opacity'))).toBe('0.55');
  await expect.poll(() => page.evaluate(() => document.documentElement.style.getPropertyValue('--background-foreground'))).toBe('210 68% 80%');
  await expect(page.getByTestId('sidebar')).toHaveCSS('background-color', 'rgba(35, 17, 69, 0.55)');
});

test('extra color controls stay collapsed until requested', async ({ page }) => {
  await expect(page.getByLabel('Use custom block color')).not.toBeVisible();
  await page.getByLabel('Toggle custom colors').click();
  await expect(page.getByLabel('Use custom block color')).toBeVisible();
});

test('appearance presets apply grouped preferences and Agora default restores defaults', async ({ page }) => {
  await page.getByLabel('Appearance preset').selectOption('terminal');
  await expect(page.locator('html')).toHaveClass(/dark/);
  await expect(page.getByLabel('Information density')).toHaveValue('compact');
  await expect(page.getByLabel('Interface font')).toHaveValue('mono');
  await expect(page.getByLabel('Use custom block color')).toBeChecked();

  await page.getByLabel('Appearance preset').selectOption('agora');
  await expect(page.getByLabel('Accent source')).toHaveValue('agora');
  await expect(page.getByLabel('Information density')).toHaveValue('comfortable');
  await expect(page.getByLabel('Interface font')).toHaveValue('system');
  await expect(page.getByLabel('Use custom block color')).not.toBeChecked();
});
