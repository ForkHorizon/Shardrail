import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import globalSetup, { waitForState } from './browser-e2e-setup';

test('global setup retries a truncated state file until atomic replacement completes', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'shardrail-e2e-state-'));
  const stateFile = join(dir, 'state.json');
  try {
    await writeFile(stateFile, '{"baseUrl":');
    setTimeout(() => { void writeFile(stateFile, '{"baseUrl":"http://127.0.0.1:1234/"}'); }, 75);
    const state = await waitForState(stateFile, { exitCode: null }, 1000);
    assert.equal(state.baseUrl, 'http://127.0.0.1:1234/');
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('global setup rethrows a server failure and kills its process group and daemon', { timeout: 20_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'shardrail-e2e-failure-'));
  const previous = {
    state: process.env.SHARDRAIL_E2E_STATE,
    pids: process.env.SHARDRAIL_E2E_TEST_PIDS_FILE,
    timeout: process.env.SHARDRAIL_E2E_SETUP_TIMEOUT_MS,
    invalidState: process.env.SHARDRAIL_E2E_TEST_INVALID_STATE,
  };
  const pidsFile = join(dir, 'pids.json');
  process.env.SHARDRAIL_E2E_STATE = join(dir, 'state.json');
  process.env.SHARDRAIL_E2E_TEST_PIDS_FILE = pidsFile;
  process.env.SHARDRAIL_E2E_SETUP_TIMEOUT_MS = '1500';
  process.env.SHARDRAIL_E2E_TEST_INVALID_STATE = '1';
  try {
    await assert.rejects(globalSetup(), /timed out waiting for the test daemon/);
    const { serverPid, daemonPid } = JSON.parse(await readFile(pidsFile, 'utf8')) as { serverPid: number; daemonPid: number };
    const deadline = Date.now() + 5_000;
    while (Date.now() < deadline) {
      try { process.kill(-serverPid, 0); } catch (error) {
        if ((error as NodeJS.ErrnoException).code === 'ESRCH') break;
        throw error;
      }
      await new Promise((resolve) => setTimeout(resolve, 25));
    }
    assert.throws(() => process.kill(-serverPid, 0), { code: 'ESRCH' });
    assert.throws(() => process.kill(daemonPid, 0), { code: 'ESRCH' });
  } finally {
    if (previous.state === undefined) delete process.env.SHARDRAIL_E2E_STATE;
    else process.env.SHARDRAIL_E2E_STATE = previous.state;
    if (previous.pids === undefined) delete process.env.SHARDRAIL_E2E_TEST_PIDS_FILE;
    else process.env.SHARDRAIL_E2E_TEST_PIDS_FILE = previous.pids;
    if (previous.timeout === undefined) delete process.env.SHARDRAIL_E2E_SETUP_TIMEOUT_MS;
    else process.env.SHARDRAIL_E2E_SETUP_TIMEOUT_MS = previous.timeout;
    if (previous.invalidState === undefined) delete process.env.SHARDRAIL_E2E_TEST_INVALID_STATE;
    else process.env.SHARDRAIL_E2E_TEST_INVALID_STATE = previous.invalidState;
    await rm(dir, { recursive: true, force: true });
  }
});
