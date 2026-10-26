import { spawn } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { once } from 'node:events';

export default async function globalSetup() {
  const stateFile = process.env.SHARDRAIL_E2E_STATE;
  if (!stateFile) throw new Error('SHARDRAIL_E2E_STATE is required');
  const server = spawn(process.execPath, ['tests/browser-e2e-server.mjs'], { stdio: 'inherit', env: process.env, detached: true });
  const stop = async () => {
    if (server.pid === undefined) return;
    try { process.kill(-server.pid!, 'SIGTERM'); } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ESRCH') return;
      throw error;
    }
    if (server.exitCode === null && server.signalCode === null) {
      let timer: NodeJS.Timeout | undefined;
      await Promise.race([
        once(server, 'exit').then(() => clearTimeout(timer)),
        new Promise((resolve) => { timer = setTimeout(resolve, 3000); }),
      ]);
      clearTimeout(timer);
      if (server.exitCode === null && server.signalCode === null) {
        try { process.kill(-server.pid!, 'SIGKILL'); } catch (error) {
          if ((error as NodeJS.ErrnoException).code !== 'ESRCH') throw error;
        }
        await once(server, 'exit');
      }
    }
    try { process.kill(-server.pid!, 'SIGKILL'); } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ESRCH') throw error;
    }
  };
  try {
    await waitForState(stateFile, server, Number(process.env.SHARDRAIL_E2E_SETUP_TIMEOUT_MS ?? 15_000));
    return stop;
  } catch (error) {
    await stop();
    throw error;
  }
}

export async function waitForState(stateFile: string, server: { exitCode: number | null; signalCode?: NodeJS.Signals | null }, timeoutMs = 15_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (server.exitCode !== null || server.signalCode) throw new Error(`e2e daemon setup exited with ${server.exitCode ?? server.signalCode}`);
    try {
      return JSON.parse(await readFile(stateFile, 'utf8'));
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT' && !(error instanceof SyntaxError)) throw error;
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error('timed out waiting for the test daemon');
}
