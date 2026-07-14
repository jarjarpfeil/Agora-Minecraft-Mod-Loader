import { test, expect, type Page } from '@playwright/test';

async function installOnboardingMock(page: Page) {
  await page.addInitScript(() => {
    let pollResolve: ((value: unknown) => void) | null = null;
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
          if (args.key === 'onboarding_complete') return Promise.resolve(false);
          if (args.key === 'modrinth_enabled') return Promise.resolve(true);
          if (args.key === 'ai_mcp_enabled') return Promise.resolve(false);
          if (args.key === 'ai_chat_enabled') return Promise.resolve(true);
          return Promise.resolve(null);
        }
        if (command === 'set_setting') return Promise.resolve(null);
        if (command === 'get_windows_accent_color') return Promise.resolve(null);
        if (command.startsWith('plugin:event|') || command.startsWith('plugin:shell|')) return Promise.resolve(null);
        if (command === 'ensure_java_runtime') {
          return Promise.resolve({ path: '/mock/java21', version: 21, version_string: 'Java 21.0.1', source: 'Managed', arch: 'x64' });
        }
        if (command === 'github_login') {
          return Promise.resolve({
            device_code: 'device',
            user_code: 'ABCD-EFGH',
            verification_uri: 'https://github.com/login/device',
            expires_in: 900,
            interval: 1,
          });
        }
        if (command === 'github_login_poll') {
          return new Promise((resolve) => { pollResolve = resolve; });
        }
        return Promise.resolve(null);
      },
    };
    Object.assign(window as unknown as Record<string, unknown>, {
      __TAURI_INTERNALS__: internals,
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
      __resolveGithubPoll(value: unknown) { pollResolve?.(value); },
    });
  });
}

test('app shell renders with Agora branding', async ({ page }) => {
  await page.goto('/');
  // The app should load the home page (Vite app shell).
  await expect(page.locator('body')).toBeVisible();
  // Look for the Agora Launcher text (sidebar brand) somewhere on the page.
  await expect(page.getByText('Agora Launcher')).toBeVisible({ timeout: 10000 });
});

test('navigation to browse works', async ({ page }) => {
  await page.goto('/');
  // Sidebar uses <button> elements (not <a> links) for navigation.
  const browseButton = page.getByRole('button', { name: 'Browse', exact: true });
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
    await expect(page.getByRole('button', { name: tab, exact: true })).toBeVisible();
  }
});

test('persisted service choices survive Back and Continue', async ({ page }) => {
  await installOnboardingMock(page);
  await page.goto('/');
  await page.getByRole('button', { name: 'Get Started' }).click();
  await expect(page.getByRole('button', { name: 'Continue' })).toBeEnabled();

  // On Services step: 3 switches (modrinth, aiMcp, aiChat)
  const switches = page.getByRole('switch');
  await expect(switches.nth(0)).toHaveAttribute('aria-checked', 'true');
  await expect(switches.nth(1)).toHaveAttribute('aria-checked', 'false');
  await expect(switches.nth(2)).toHaveAttribute('aria-checked', 'true');

  await switches.nth(1).click();
  // Go to Java step
  await page.getByRole('button', { name: 'Continue' }).click();
  await expect(page.getByRole('heading', { name: 'Prepare Java for Minecraft' })).toBeVisible({ timeout: 3000 });
  // Back to Services
  await page.getByRole('button', { name: 'Back' }).click();
  await expect(page.getByRole('button', { name: 'Continue' })).toBeEnabled();
  // Back to Welcome
  await page.getByRole('button', { name: 'Back' }).click();
  await page.getByRole('button', { name: 'Get Started' }).click();
  // aiMcp toggle should still be checked
  await expect(page.getByRole('switch').nth(1)).toHaveAttribute('aria-checked', 'true');
});

test('Java step checked invokes ensure_java_runtime with onboarding operationId', async ({ page }) => {
  const ensureJavaCalls: string[] = [];
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
          if (args.key === 'onboarding_complete') return Promise.resolve(false);
          if (args.key === 'modrinth_enabled') return Promise.resolve(true);
          if (args.key === 'ai_mcp_enabled') return Promise.resolve(false);
          if (args.key === 'ai_chat_enabled') return Promise.resolve(true);
          return Promise.resolve(null);
        }
        if (command === 'set_setting') return Promise.resolve(null);
        if (command === 'get_windows_accent_color') return Promise.resolve(null);
        if (command.startsWith('plugin:event|') || command.startsWith('plugin:shell|')) return Promise.resolve(null);
        if (command === 'ensure_java_runtime') {
          (window as any).__ensureJavaCalls ??= [];
          (window as any).__ensureJavaCalls.push(args);
          return Promise.resolve({ path: '/mock/java21', version: 21, version_string: 'Java 21.0.1', source: 'Managed', arch: 'x64' });
        }
        return Promise.resolve(null);
      },
    };
    Object.assign(window as unknown as Record<string, unknown>, {
      __TAURI_INTERNALS__: internals,
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
      __ensureJavaCalls: [] as Array<Record<string, unknown>>,
    });
  });

  await page.goto('/');
  await page.getByRole('button', { name: 'Get Started' }).click();
  // Services → Continue
  await page.getByRole('button', { name: 'Continue' }).click();
  // Java step — should be checked by default
  await expect(page.getByRole('heading', { name: 'Prepare Java for Minecraft' })).toBeVisible({ timeout: 3000 });
  const javaSwitch = page.getByRole('switch');
  await expect(javaSwitch).toHaveAttribute('aria-checked', 'true');
  // Continue with checked — should invoke ensure_java_runtime with onboarding operationId
  await page.getByRole('button', { name: 'Continue' }).click();
  // Wait for the call
  await page.waitForTimeout(500);
  const calls = await page.evaluate(() => (window as any).__ensureJavaCalls ?? []);
  expect(calls.length).toBeGreaterThanOrEqual(1);
  expect(calls[0]).toHaveProperty('major', 21);
  expect(calls[0]).toHaveProperty('operationId', 'onboarding-java-21');
});

test('Java step unchecked does not invoke ensure_java_runtime', async ({ page }) => {
  const ensureJavaCalls: string[] = [];
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
          if (args.key === 'onboarding_complete') return Promise.resolve(false);
          if (args.key === 'modrinth_enabled') return Promise.resolve(true);
          if (args.key === 'ai_mcp_enabled') return Promise.resolve(false);
          if (args.key === 'ai_chat_enabled') return Promise.resolve(true);
          return Promise.resolve(null);
        }
        if (command === 'set_setting') return Promise.resolve(null);
        if (command === 'get_windows_accent_color') return Promise.resolve(null);
        if (command.startsWith('plugin:event|') || command.startsWith('plugin:shell|')) return Promise.resolve(null);
        if (command === 'ensure_java_runtime') {
          (window as any).__ensureJavaCalls ??= [];
          (window as any).__ensureJavaCalls.push(args);
          return Promise.resolve({ path: '/mock/java21', version: 21, version_string: 'Java 21.0.1', source: 'Managed', arch: 'x64' });
        }
        return Promise.resolve(null);
      },
    };
    Object.assign(window as unknown as Record<string, unknown>, {
      __TAURI_INTERNALS__: internals,
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
      __ensureJavaCalls: [] as Array<Record<string, unknown>>,
    });
  });

  await page.goto('/');
  await page.getByRole('button', { name: 'Get Started' }).click();
  // Services → Continue
  await page.getByRole('button', { name: 'Continue' }).click();
  // Java step — uncheck
  await expect(page.getByRole('heading', { name: 'Prepare Java for Minecraft' })).toBeVisible({ timeout: 3000 });
  const javaSwitch = page.getByRole('switch');
  await javaSwitch.click();
  await expect(javaSwitch).toHaveAttribute('aria-checked', 'false');
  // Continue without Java
  await page.getByRole('button', { name: 'Continue' }).click();
  await page.waitForTimeout(500);
  const calls = await page.evaluate(() => (window as any).__ensureJavaCalls ?? []);
  expect(calls.length).toBe(0);
});

test('onboarding Java step cancel allows continue without Java', async ({ page }) => {
  let ensureJavaReject: ((reason: unknown) => void) | null = null;
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
          if (args.key === 'onboarding_complete') return Promise.resolve(false);
          if (args.key === 'modrinth_enabled') return Promise.resolve(true);
          if (args.key === 'ai_mcp_enabled') return Promise.resolve(false);
          if (args.key === 'ai_chat_enabled') return Promise.resolve(true);
          return Promise.resolve(null);
        }
        if (command === 'set_setting') return Promise.resolve(null);
        if (command === 'get_windows_accent_color') return Promise.resolve(null);
        if (command.startsWith('plugin:event|') || command.startsWith('plugin:shell|')) return Promise.resolve(null);
        if (command === 'ensure_java_runtime') {
          return new Promise((_, reject) => {
            (window as any).__ensureJavaRejectRef = reject;
          });
        }
        if (command === 'cancel_java_runtime') {
          // Simulate cancellation by rejecting the pending ensure_java_runtime
          const reject = (window as any).__ensureJavaRejectRef;
          if (reject) {
            reject({
              code: 'ERR_JAVA_RUNTIME_CANCELLED',
              message: 'Java 21 runtime provisioning was cancelled.',
              details: { major: 21, suggested_actions: ['cancel'] },
              suggested_action: null,
            });
          }
          return Promise.resolve(null);
        }
        return Promise.resolve(null);
      },
    };
    Object.assign(window as unknown as Record<string, unknown>, {
      __TAURI_INTERNALS__: internals,
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
      __ensureJavaRejectRef: null,
    });
  });

  await page.goto('/');
  await page.getByRole('button', { name: 'Get Started' }).click();
  // Services → Continue
  await page.getByRole('button', { name: 'Continue' }).click();
  // Java step
  await expect(page.getByRole('heading', { name: 'Prepare Java for Minecraft' })).toBeVisible({ timeout: 3000 });
  // Click Continue to start download
  await page.getByRole('button', { name: 'Continue' }).click();
  // Wait for Cancel button to appear (means download started)
  await expect(page.getByRole('button', { name: 'Cancel' })).toBeVisible({ timeout: 3000 });
  // Click Cancel
  await page.getByRole('button', { name: 'Cancel' }).click();
  // After cancel, we should be able to continue to GitHub step
  await expect(page.getByRole('heading', { name: 'Connect GitHub' })).toBeVisible({ timeout: 5000 });
});

test('cancelling GitHub device flow invalidates the active poll', async ({ page }) => {
  await installOnboardingMock(page);
  await page.goto('/');
  // Welcome → Get Started
  await page.getByRole('button', { name: 'Get Started' }).click();
  // Services → Continue (navigates to Java step)
  await page.getByRole('button', { name: 'Continue' }).click();
  // Java step — uncheck Java download to skip to GitHub quickly
  await expect(page.getByRole('heading', { name: 'Prepare Java for Minecraft' })).toBeVisible({ timeout: 3000 });
  await page.getByRole('switch').click(); // uncheck
  await page.getByRole('button', { name: 'Continue' }).click();
  // GitHub step
  await page.getByRole('button', { name: 'Sign in with GitHub' }).click();
  await expect(page.getByRole('button', { name: 'Copy Code' })).toBeVisible();

  await page.getByRole('button', { name: 'Cancel' }).click();
  await page.evaluate(() => (window as any).__resolveGithubPoll(true));
  await expect(page.getByRole('heading', { name: 'Connect GitHub' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Download Registry' })).toHaveCount(0, { timeout: 1500 });
});
