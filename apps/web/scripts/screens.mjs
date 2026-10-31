import { spawnSync } from 'node:child_process';
import { mkdtemp, mkdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';
import { randomUUID } from 'node:crypto';
import { fileURLToPath } from 'node:url';

const appDir = resolve(fileURLToPath(new URL('..', import.meta.url)));
const cwd = process.cwd();
const repoRoot = resolve(appDir, '../..');
const args = process.argv.slice(2);
const outIndex = args.indexOf('--out');
if (outIndex < 0 || !args[outIndex + 1]) throw new Error('Usage: node apps/s5-prototype/scripts/screens.mjs --out <dir>');
const outDir = resolve(cwd === appDir ? cwd : repoRoot, args[outIndex + 1]);
await mkdir(outDir, { recursive: true });
const stateFile = join(tmpdir(), `shardrail-screens-${randomUUID()}.json`);
const assetsDir = await mkdtemp(join(tmpdir(), 'shardrail-screens-assets-'));
let status = 1;
try {
  const build = spawnSync('nice', ['-n', '10', 'npm', 'run', 'build', '--', '--outDir', assetsDir], { cwd: appDir, stdio: 'inherit' });
  if (build.status !== 0) status = build.status ?? 1;
  else {
    const run = spawnSync(process.execPath, ['node_modules/@playwright/test/cli.js', 'test', '--config=playwright.screens.config.ts'], {
      cwd: appDir,
      stdio: 'inherit',
      env: { ...process.env, SHARDRAIL_E2E_STATE: stateFile, SHARDRAIL_E2E_ASSETS_DIR: assetsDir, SHARDRAIL_SCREEN_OUT: outDir },
    });
    status = run.status ?? 1;
  }
} finally {
  await rm(assetsDir, { recursive: true, force: true });
  await rm(stateFile, { force: true });
}
process.exit(status);
