import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import process from 'node:process';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { compareRuns, readRuns, renderMarkdown } from './b4-compare.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));

test('groups fixture runs, excludes incomplete samples, and reports pooled and per-run latency beside each stack', async () => {
  const root = path.join(here, 'fixtures/b4-compare');
  const runs = await readRuns([{ stack: 'tauri', directory: path.join(root, 'tauri') }, { stack: 'swift', directory: path.join(root, 'swift') }]);
  const [group] = compareRuns(runs);
  assert.deepEqual([group.events, group.streams, group.temperature], [1000, 2, 'cold']);
  assert.deepEqual(group.stacks.tauri, { validRuns: 1, totalRuns: 2, measuredRows: 20, sampleFloor: 20, pooledSamples: 3, pooledMedianMs: 20, pooledP95Ms: 30, perRunMedianMedianMs: 20, perRunMedianP95Ms: 20 });
  assert.deepEqual(group.stacks.swift, { validRuns: 1, totalRuns: 1, measuredRows: 25, sampleFloor: 20, pooledSamples: 4, pooledMedianMs: 15, pooledP95Ms: 35, perRunMedianMedianMs: 15, perRunMedianP95Ms: 15 });
  assert.match(renderMarkdown([group]), /1\/2 · 20\/20 · 20\/30 ms/);
  assert.match(renderMarkdown([group]), /1\/1 · 25\/20 · 15\/35 ms/);
});

test('CLI writes paired Markdown and JSON reports', () => {
  const root = path.join(here, 'fixtures/b4-compare');
  const directory = mkdtempSync(path.join(os.tmpdir(), 'b4-compare-'));
  const output = path.join(directory, 'report');
  try {
    const run = spawnSync(process.execPath, [path.join(here, 'b4-compare.mjs'), '--tauri', path.join(root, 'tauri'), '--swift', path.join(root, 'swift'), '--out', output], { encoding: 'utf8' });
    assert.equal(run.status, 0, run.stderr);
    assert.match(readFileSync(`${output}.md`, 'utf8'), /Events \| Streams/);
    assert.equal(JSON.parse(readFileSync(`${output}.json`, 'utf8')).length, 1);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
