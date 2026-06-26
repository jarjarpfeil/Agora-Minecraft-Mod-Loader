import { test, expect } from '@playwright/test';

test('app shell renders with Agora branding', async ({ page }) => {
  await page.goto('/');
  // The app should load the home page (Vite app shell).
  await expect(page.locator('body')).toBeVisible();
  // Look for the Agora title text (sidebar <h1>) somewhere on the page.
  await expect(page.getByRole('heading', { name: 'Agora', level: 1 })).toBeVisible({ timeout: 10000 });
});

test('navigation to browse works', async ({ page }) => {
  await page.goto('/');
  // Sidebar uses <button> elements (not <a> links) for navigation.
  const browseButton = page.getByRole('button', { name: /browse/i });
  await expect(browseButton).toBeVisible();
  await browseButton.click();
  // After clicking Browse, the page should show a "Browse" heading.
  await expect(page.getByRole('heading', { name: 'Browse', level: 2 })).toBeVisible({ timeout: 5000 });
});

test('sidebar navigation buttons are visible', async ({ page }) => {
  await page.goto('/');
  // All base sidebar tabs should be present as buttons.
  const tabs = ['Home', 'Browse', 'My Instances', 'Community Governance', 'Settings'];
  for (const tab of tabs) {
    await expect(page.getByRole('button', { name: tab })).toBeVisible();
  }
});
