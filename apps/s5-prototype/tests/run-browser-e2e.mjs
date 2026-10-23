import { randomUUID } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const build = spawnSync('npm', ['run', 'build'], { stdio: 'inherit' });
if (build.status !== 0) process.exit(build.status ?? 1);

const stateFile = join(tmpdir(), `shardrail-browser-e2e-${randomUUID()}.json`);
const playwright = spawnSync(process.execPath, ['node_modules/@playwright/test/cli.js', 'test'], {
  stdio: 'inherit',
  env: { ...process.env, SHARDRAIL_E2E_STATE: stateFile },
});
process.exit(playwright.status ?? 1);
