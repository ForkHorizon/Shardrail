import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests',
  testMatch: ['screens.spec.ts'],
  fullyParallel: false,
  workers: 1,
  globalSetup: './tests/browser-e2e-setup.ts',
  timeout: 30_000,
  use: { headless: true },
  projects: [{ name: 'chromium', use: { browserName: 'chromium', headless: true, launchOptions: { channel: 'chromium' } } }],
});
