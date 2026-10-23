import { defineConfig } from '@playwright/test';
import { join } from 'node:path';
import { tmpdir } from 'node:os';

const stateFile = join(tmpdir(), 'shardrail-browser-e2e-state.json');
process.env.SHARDRAIL_E2E_STATE = stateFile;
process.env.SHARDRAILD_BIN = process.env.SHARDRAILD_BIN ?? join(process.cwd(), '../../target/debug/shardraild');

export default defineConfig({
  testDir: './tests',
  testMatch: 'browser-pairing.spec.ts',
  fullyParallel: false,
  workers: 1,
  globalSetup: './tests/browser-e2e-setup.ts',
  use: { headless: true },
  projects: [
    { name: 'chromium', use: { browserName: 'chromium', headless: true, launchOptions: { channel: 'chromium' } } },
    { name: 'webkit', use: { browserName: 'webkit', headless: true } },
  ],
});
