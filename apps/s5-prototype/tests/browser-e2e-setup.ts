import { spawn } from 'node:child_process';
import { readFile } from 'node:fs/promises';

export default async function globalSetup() {
  const stateFile = process.env.SHARDRAIL_E2E_STATE;
  if (!stateFile) throw new Error('SHARDRAIL_E2E_STATE is required');
  const server = spawn(process.execPath, ['tests/browser-e2e-server.mjs'], { stdio: 'inherit', env: process.env });
  const stop = async () => {
    server.kill('SIGTERM');
    await new Promise<void>((resolve) => {
      const timer = setTimeout(() => { server.kill('SIGKILL'); resolve(); }, 3000);
      server.once('exit', () => { clearTimeout(timer); resolve(); });
    });
  };
  const deadline = Date.now() + 15_000;
  while (Date.now() < deadline) {
    if (server.exitCode !== null) throw new Error(`e2e daemon setup exited with ${server.exitCode}`);
    try {
      JSON.parse(await readFile(stateFile, 'utf8'));
      return stop;
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  await stop();
  throw new Error('timed out waiting for the test daemon');
}
