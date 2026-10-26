import assert from 'node:assert/strict';
import { test } from 'node:test';
import * as React from 'react';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { AgentDescription } from '../src/screens/session/SessionArea';

Object.assign(globalThis, { React });

test('approval descriptions show empty state and clamp long untrusted text with a toggle', () => {
  const empty = renderToStaticMarkup(createElement(AgentDescription, { title: null, description: null }));
  assert.match(empty, /Agent’s description · untrusted/);
  assert.match(empty, /No description provided/);

  const description = 'untrusted description '.repeat(20);
  const long = renderToStaticMarkup(createElement(AgentDescription, { title: null, description }));
  assert.match(long, /untrusted-content clamped/);
  assert.match(long, /Show more/);
});
