import assert from 'node:assert/strict';
import test from 'node:test';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { UntrustedMarkdown } from '../src/UntrustedMarkdown';

test('sanitizes hostile Markdown before returning renderable elements', () => {
  const html = renderToStaticMarkup(createElement(UntrustedMarkdown, { source: '[unsafe](javascript:alert(1)) ![remote](https://example.com/image.png) **safe**' }));
  const scriptHtml = renderToStaticMarkup(createElement(UntrustedMarkdown, { source: '<script>alert(1)</script>' }));
  assert.doesNotMatch(html + scriptHtml, /<script|javascript:|<img|https:\/\/example\.com\/image/);
  assert.match(html, /<strong>safe<\/strong>/);
  assert.match(html, /<a[^>]*>unsafe<\/a>/);
  assert.doesNotMatch(html, /href=/);
});
