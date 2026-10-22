import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync, mkdirSync } from 'node:fs';
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
  assert.deepEqual(group.stacks.tauri, { validRuns: 1, totalRuns: 2, measuredRows: 20, sampleFloor: 40, pooledSamples: 20, pooledMedianMs: 11, pooledP95Ms: 20, perRunMedianMedianMs: 11, perRunMedianP95Ms: 11 });
  assert.deepEqual(group.stacks.swift, { validRuns: 1, totalRuns: 1, measuredRows: 25, sampleFloor: 20, pooledSamples: 25, pooledMedianMs: 13, pooledP95Ms: 24, perRunMedianMedianMs: 13, perRunMedianP95Ms: 13 });
  assert.match(renderMarkdown([group]), /1\/2 · 20\/40 · 11\/20 ms/);
  assert.match(renderMarkdown([group]), /1\/1 · 25\/20 · 13\/24 ms/);
});

test('keeps timeout attempts, rejects count/sample mismatches, and skips corrupt artifacts', async () => {
  const directory = mkdtempSync(path.join(os.tmpdir(), 'b4-compare-input-'));
  const tauri = path.join(directory, 'tauri'), swift = path.join(directory, 'swift');
  mkdirSync(tauri); mkdirSync(swift);
  try {
    writeFileSync(path.join(tauri, 'timeout.json'), JSON.stringify({ error: 'timed out', events: 1000, streams: 2 }));
    writeFileSync(path.join(tauri, 'short.json'), JSON.stringify({ events: 1000, streams: 2, pageVisibility: 'visible', daemonAgentChunkCount: 1000, b4: { count: 20, samplesMs: [5] } }));
    writeFileSync(path.join(tauri, 'corrupt.json'), '{');
    writeFileSync(path.join(swift, 'legacy.json'), JSON.stringify({ streams: 2, pageVisibility: 'visible', daemonAgentChunkCount: 1000, b4: { count: 20, samplesMs: Array.from({ length: 20 }, (_, i) => i + 1) } }));
    const runs = await readRuns([{ stack: 'tauri', directory: tauri }, { stack: 'swift', directory: swift }]);
    const group = compareRuns(runs)[0];
    assert.deepEqual(group.stacks.tauri, { validRuns: 0, totalRuns: 2, measuredRows: 0, sampleFloor: 40, pooledSamples: 0, pooledMedianMs: null, pooledP95Ms: null, perRunMedianMedianMs: null, perRunMedianP95Ms: null });
    assert.equal(group.stacks.swift.validRuns, 1);
    assert.equal(group.stacks.swift.pooledP95Ms, 20, 'p95 uses the same floor index as the runtime metrics');
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test('missing input directories fail with a clear path', async () => {
  await assert.rejects(readRuns([{ stack: 'tauri', directory: '/definitely/missing/b4-results' }]), /input directory does not exist/);
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
