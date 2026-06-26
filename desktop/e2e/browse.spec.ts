import { test, expect } from '@playwright/test';

test('home renders with content', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('body')).not.toBeEmpty();
  // Home page should show "Home" heading and "Agora" branding.
  await expect(page.getByRole('heading', { name: 'Home', level: 2 })).toBeVisible({ timeout: 10000 });
});

test('no console errors on load', async ({ page }) => {
  const errors: string[] = [];
  page.on('console', (msg) => {
    if (msg.type() === 'error') errors.push(msg.text());
  });
  await page.goto('/');
  await page.waitForTimeout(2000);
  // Filter expected Tauri "invoke not available in dev" errors.
  const realErrors = errors.filter((e) => !e.includes('invoke') && !e.includes('Tauri'));
  expect(realErrors).toEqual([]);
});
