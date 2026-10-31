import assert from 'node:assert/strict';
import { test } from 'node:test';
import * as React from 'react';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { EntryArea } from '../src/screens/entry/EntryArea';

Object.assign(globalThis, { React });

test('shows a neutral session skeleton while the initial daemon connection is pending', () => {
  const html = renderToStaticMarkup(createElement(EntryArea, {
    sessions: [], events: [], selectedId: null, workdirs: null, pickedWorkdir: '', createError: null,
    createPending: false, phase: 'connecting', onSelect: () => {}, onPickWorkdir: () => {}, onCreateSession: () => {},
  }));
  assert.match(html, /aria-label="Loading sessions"/);
  assert.doesNotMatch(html, /No sessions yet/);
});
