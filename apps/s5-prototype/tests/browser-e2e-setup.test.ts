import assert from 'node:assert/strict';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { waitForState } from './browser-e2e-setup';

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
