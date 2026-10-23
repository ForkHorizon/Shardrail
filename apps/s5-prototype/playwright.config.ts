import { defineConfig } from '@playwright/test';
import { join } from 'node:path';
import { tmpdir } from 'node:os';

const stateFile = join(tmpdir(), 'shardrail-browser-e2e-state.json');
const shardraildBin = join(process.cwd(), '../../target/debug/shardraild');
process.env.SHARDRAILD_BIN = shardraildBin;

export default defineConfig({
  testDir: './tests',
  testMatch: 'browser-pairing.spec.ts',
  fullyParallel: false,
  workers: 1,
  use: { headless: true },
  projects: [
    { name: 'chromium', use: { browserName: 'chromium', headless: true } },
    { name: 'webkit', use: { browserName: 'webkit', headless: true } },
  ],
  webServer: {
    command: 'node tests/browser-e2e-server.mjs',
    url: 'http://127.0.0.1:42937/',
    reuseExistingServer: false,
    timeout: 30_000,
    env: {
      ...process.env,
      SHARDRAIL_E2E_STATE: stateFile,
      SHARDRAILD_BIN: shardraildBin,
    },
  },
});
