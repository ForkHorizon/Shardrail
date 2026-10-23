import { randomUUID } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const stateFile = join(tmpdir(), `shardrail-browser-e2e-${randomUUID()}.json`);
const assetsDir = await mkdtemp(join(tmpdir(), 'shardrail-browser-e2e-assets-'));
let status = 1;
try {
  const build = spawnSync('npm', ['run', 'build', '--', '--outDir', assetsDir], { stdio: 'inherit' });
  if (build.status === 0) {
    const playwright = spawnSync(process.execPath, ['node_modules/@playwright/test/cli.js', 'test'], {
      stdio: 'inherit',
      env: { ...process.env, SHARDRAIL_E2E_STATE: stateFile, SHARDRAIL_E2E_ASSETS_DIR: assetsDir },
    });
    status = playwright.status ?? 1;
  } else {
    status = build.status ?? 1;
  }
} finally {
  await rm(assetsDir, { recursive: true, force: true });
}
process.exit(status);
