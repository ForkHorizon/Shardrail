import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

test('untrusted Markdown cannot execute scripts', async () => {
  const source = await readFile(new URL('../src/main.tsx', import.meta.url), 'utf8');
  assert.match(source, /sandbox="allow-same-origin"/);
  assert.doesNotMatch(source, /sandbox="[^"]*allow-scripts/);
});

test('all form controls use React Aria components', async () => {
  const source = await readFile(new URL('../src/main.tsx', import.meta.url), 'utf8');
  assert.match(source, /from 'react-aria-components'/);
  assert.match(source, /<Select\b/);
  assert.match(source, /<Input\b/);
  assert.match(source, /<Button\b/);
  assert.doesNotMatch(source, /<(?:select|input|textarea)\b/);
  for (const file of ['MainScreen.tsx', 'screens/entry/EntryArea.tsx', 'screens/entry/PairingScreen.tsx', 'screens/session/SessionArea.tsx']) {
    const component = await readFile(new URL(`../src/${file}`, import.meta.url), 'utf8');
    assert.match(component, /from 'react-aria-components'/);
    assert.doesNotMatch(component, /<(?:select|input|textarea)\b/);
  }
});
