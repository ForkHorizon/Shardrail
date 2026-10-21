import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

test('Tauri enables isolation and untrusted Markdown cannot execute scripts', async () => {
  const tauri = JSON.parse(await readFile(new URL('../src-tauri/tauri.conf.json', import.meta.url), 'utf8')) as { app: { security: { pattern: { use: string } } } };
  const source = await readFile(new URL('../src/main.tsx', import.meta.url), 'utf8');
  assert.equal(tauri.app.security.pattern.use, 'isolation');
  assert.match(source, /sandbox="allow-same-origin"/);
  assert.doesNotMatch(source, /sandbox="[^"]*allow-scripts/);
});

test('all form controls use React Aria components', async () => {
  const source = await readFile(new URL('../src/main.tsx', import.meta.url), 'utf8');
  assert.match(source, /from 'react-aria-components'/);
  assert.match(source, /<Select\b/);
  assert.match(source, /<Input\b/);
  assert.match(source, /<Button\b/);
  assert.doesNotMatch(source, /<(?:select|input)\b/);
});
