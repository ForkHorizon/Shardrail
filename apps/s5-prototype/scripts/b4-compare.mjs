#!/usr/bin/env node
import { readdir, readFile, mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

export function sampleFloor(events) { return Math.min(events, Math.max(20, Math.ceil(events / 100))); }
const percentile = (values, fraction) => values.length ? [...values].sort((a, b) => a - b)[Math.min(values.length - 1, Math.floor(values.length * fraction))] : null;
const median = (values) => percentile(values, 0.5);

export function compareRuns(runs) {
  const groups = new Map();
  for (const run of runs) {
    const { stack, events, streams, temperature } = run;
    const key = [events, streams, temperature].join(':');
    const group = groups.get(key) ?? { events, streams, temperature, stacks: {} };
    const samples = Array.isArray(run.b4?.samplesMs) ? run.b4.samplesMs.filter(Number.isFinite) : [];
    const count = run.b4?.count;
    const valid = !run.error && run.pageVisibility === 'visible' && run.daemonAgentChunkCount === events && Number.isInteger(count) && count === samples.length && count >= sampleFloor(events);
    const stackRuns = group.stacks[stack] ??= [];
    stackRuns.push({ valid, count, samples, medianMs: median(samples) });
    groups.set(key, group);
  }
  return [...groups.values()].sort((a, b) => a.events - b.events || a.streams - b.streams || a.temperature.localeCompare(b.temperature)).map((group) => {
    const stacks = Object.fromEntries(Object.entries(group.stacks).sort(([a], [b]) => a.localeCompare(b)).map(([stack, values]) => {
      const valid = values.filter((run) => run.valid);
      const pooled = valid.flatMap((run) => run.samples);
      const runMedians = valid.map((run) => run.medianMs).filter(Number.isFinite);
      return [stack, {
        validRuns: valid.length, totalRuns: values.length,
        measuredRows: valid.reduce((sum, run) => sum + run.count, 0), sampleFloor: sampleFloor(group.events) * values.length,
        pooledSamples: pooled.length, pooledMedianMs: median(pooled), pooledP95Ms: percentile(pooled, 0.95),
        perRunMedianMedianMs: median(runMedians), perRunMedianP95Ms: percentile(runMedians, 0.95),
      }];
    }));
    return { ...group, stacks };
  });
}

export function renderMarkdown(groups) {
  const metric = (value) => value ? `${value.validRuns}/${value.totalRuns} · ${value.measuredRows}/${value.sampleFloor} · ${value.pooledMedianMs ?? '—'}/${value.pooledP95Ms ?? '—'} ms · ${value.perRunMedianMedianMs ?? '—'}/${value.perRunMedianP95Ms ?? '—'} ms` : '—';
  const lines = ['| Events | Streams | Temperature | Tauri: valid/total · rows/floor · pooled med/p95 · run medians med/p95 | Swift: valid/total · rows/floor · pooled med/p95 · run medians med/p95 |', '| ---: | ---: | --- | --- | --- |'];
  for (const group of groups) lines.push(`| ${group.events} | ${group.streams} | ${group.temperature} | ${metric(group.stacks.tauri)} | ${metric(group.stacks.swift)} |`);
  return `${lines.join('\n')}\n`;
}

async function jsonFiles(directory) {
  const found = [];
  let entries;
  try { entries = await readdir(directory, { withFileTypes: true }); }
  catch (error) {
    if (error.code === 'ENOENT') throw new Error(`input directory does not exist: ${directory}`);
    throw error;
  }
  for (const entry of entries) {
    const file = path.join(directory, entry.name);
    if (entry.isDirectory()) found.push(...await jsonFiles(file));
    else if (entry.isFile() && entry.name.endsWith('.json')) found.push(file);
  }
  return found;
}

export async function readRuns(inputs) {
  const runs = [];
  for (const { stack, directory } of inputs) {
    const files = await jsonFiles(directory);
    for (const file of files) {
      let value;
      try { value = JSON.parse(await readFile(file, 'utf8')); }
      catch (error) {
        process.stderr.write(`Skipping invalid result ${file}: ${error instanceof Error ? error.message : error}\n`);
        continue;
      }
      if (!value || typeof value !== 'object' || Array.isArray(value)) continue;
      const events = Number.isInteger(value.events) ? value.events : (!value.error ? value.daemonAgentChunkCount : undefined);
      if (!Number.isInteger(events) || !Number.isInteger(value.streams) || (!value.b4 && !value.error)) continue;
      runs.push({ ...value, events, stack, temperature: value.temperature ?? 'unspecified' });
    }
  }
  return runs;
}

function parseArgs(args) {
  const inputs = [];
  let output;
  for (let i = 0; i < args.length; i++) {
    if (args[i] === '--tauri' || args[i] === '--swift') {
      const stack = args[i++].slice(2);
      if (!args[i]) throw new Error(`${stack} directory missing`);
      inputs.push({ stack, directory: args[i] });
    } else if (args[i] === '--out') {
      output = args[++i];
      if (!output) throw new Error('--out prefix missing');
    } else throw new Error(`unknown argument: ${args[i]}`);
  }
  if (!inputs.some(({ stack }) => stack === 'tauri') || !inputs.some(({ stack }) => stack === 'swift')) throw new Error('provide at least one --tauri and one --swift directory');
  return { inputs, output };
}

async function main() {
  try {
    const { inputs, output } = parseArgs(process.argv.slice(2));
    const groups = compareRuns(await readRuns(inputs));
    const markdown = renderMarkdown(groups);
    if (output) {
      await mkdir(path.dirname(output), { recursive: true });
      await Promise.all([writeFile(`${output}.md`, markdown), writeFile(`${output}.json`, `${JSON.stringify(groups, null, 2)}\n`)]);
      process.stdout.write(`Wrote ${output}.md and ${output}.json\n`);
    } else process.stdout.write(markdown);
  } catch (error) {
    process.stderr.write(`${error instanceof Error ? error.message : error}\n`);
    process.exitCode = 1;
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) await main();
