import { readFileSync } from 'node:fs';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { daemonReceiptToRafMs, getRows, getRowStart, markDaemonRowsCommitted, reset, rowCount, setDaemonClockMapping, setDaemonTotal, setDaemonWindow, subscribe } from '../src/store';
import { coalesceChannelFrame, hashContent, isNearLiveTail, liveTailWindowSize, markdownCacheKey, rangeForWindow, sameMarkdownVersion, transcriptOverscan } from '../src/streaming-view';
import { confirmAgentLink, isConfirmableAgentLink, UntrustedMarkdown } from '../src/UntrustedMarkdown';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

const frame = (cursor: number) => ({ cursor, event: { session_id: 'session-1', kind: 'agent_message_chunk', payload: { content: { text: `chunk ${cursor}` } } }, receipt_wall_clock_ms: 10_000 + cursor });

afterEach(() => { vi.unstubAllGlobals(); reset(0, 1); vi.restoreAllMocks(); });

describe('S5 streaming view', () => {
  it('coalesces fake channel messages into one frame callback', () => {
    const callbacks: FrameRequestCallback[] = [];
    const channel: { onmessage: ((message: number) => void) | null } = { onmessage: null };
    const received: number[] = [];
    const remove = coalesceChannelFrame(channel, (notice) => received.push(notice), (callback) => { callbacks.push(callback); return callbacks.length; }, () => {});
    channel.onmessage!(1);
    channel.onmessage!(2);
    channel.onmessage!(3);
    expect(callbacks).toHaveLength(1);
    callbacks[0]!(0);
    expect(received).toEqual([3]);
    remove();
    expect(channel.onmessage).toBeNull();
  });

  it('coalesces N channel notices in one animation frame before one window commit', () => {
    const callbacks: FrameRequestCallback[] = [];
    vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { callbacks.push(callback); return callbacks.length; });
    let commits = 0;
    const remove = subscribe(() => commits++);
    const channel: { onmessage: ((message: number) => void) | null } = { onmessage: null };
    const removeChannel = coalesceChannelFrame(channel, (version) => setDaemonWindow(version, 100, [frame(version)], version), (callback) => { callbacks.push(callback); return callbacks.length; }, () => {});
    for (let version = 1; version <= 20; version++) channel.onmessage!(version);
    expect(callbacks).toHaveLength(1);
    callbacks[0]!(0);
    expect(commits).toBe(1);
    expect(getRowStart()).toBe(20);
    expect(getRows()[0]?.id).toBe(1_000_000_020);
    removeChannel();
    remove();
  });

  it('selects bounded ranges and follows the live tail', () => {
    expect(isNearLiveTail(1_000, 500, 470)).toBe(true);
    expect(isNearLiveTail(1_000, 500, 450)).toBe(false);
    expect(rangeForWindow(50_000, 120, 40, false)).toEqual({ start: 120, limit: 40 });
    expect(rangeForWindow(50_000, 120, 14, false)).toEqual({ start: 120, limit: 32 });
    expect(rangeForWindow(50_000, 0, 40, true)).toEqual({ start: 49_960, limit: 40 });
    expect(rangeForWindow(50_000, 0, 1000, true)).toEqual({ start: 49_872, limit: 128 });
    expect(liveTailWindowSize(true)).toBe(128);
    expect(transcriptOverscan(true)).toBe(32);
    expect(liveTailWindowSize(false)).toBe(32);
    expect(transcriptOverscan(false)).toBe(8);
  });

  it('uses row ID plus content hash as the markdown cache key', () => {
    const hash = hashContent('same markdown');
    expect(markdownCacheKey(12, hash)).toBe(markdownCacheKey(12, hash));
    expect(markdownCacheKey(12, hash)).not.toBe(markdownCacheKey(13, hash));
    expect(markdownCacheKey(12, hash)).not.toBe(markdownCacheKey(12, hashContent('changed')));
    expect(sameMarkdownVersion({ rowId: 12, contentHash: hash, contentVersion: 0, source: 'same markdown' }, { rowId: 12, contentHash: hash, contentVersion: 0, source: 'same markdown' })).toBe(true);
    expect(sameMarkdownVersion({ rowId: 12, contentHash: hash, contentVersion: 0, source: 'same markdown' }, { rowId: 12, contentHash: hash, contentVersion: 0, source: 'changed' })).toBe(false);
    expect(sameMarkdownVersion({ rowId: 12, contentHash: hash, contentVersion: 0, source: 'same markdown' }, { rowId: 12, contentHash: hash, contentVersion: 1, source: 'same markdown' })).toBe(false);
  });

  it('confirms http links and blocks aux clicks that skip confirmation', () => {
    expect(isConfirmableAgentLink('https://example.com/path')).toBe(true);
    expect(isConfirmableAgentLink('javascript:alert(1)')).toBe(false);
    expect(isConfirmableAgentLink('/relative')).toBe(false);
    let opened = '';
    const event = { preventDefault() { opened = 'prevented'; } };
    confirmAgentLink(event, 'https://example.com/a', () => false, (url) => { opened = url; });
    expect(opened).toBe('prevented');
    confirmAgentLink({ preventDefault() {} }, 'https://example.com/a', () => true, (url) => { opened = url; });
    expect(opened).toBe('https://example.com/a');
    confirmAgentLink({ preventDefault() {} }, 'javascript:alert(1)', () => true, (url) => { opened = url; });
    expect(opened).toBe('https://example.com/a');
  });

  it('keeps hostile HTML, script URLs, and remote images sanitized', () => {
    const html = renderToStaticMarkup(createElement(UntrustedMarkdown, { source: '<img src=x onerror=alert(1)><script>alert(1)</script> [bad](javascript:alert(1))' }));
    expect(html).not.toMatch(/<img|<script|javascript:|onerror=/);
  });

  it('keeps the row measurement path free of selectors and ResizeObserver remeasurement', () => {
    const source = readFileSync(new URL('../src/main.tsx', import.meta.url), 'utf8');
    const rowPath = source.slice(source.indexOf('function Row'), source.indexOf('function App'));
    expect(rowPath).not.toMatch(/querySelector|ResizeObserver|measureElement/);
    expect(source).not.toContain("querySelectorAll('[data-daemon-event-id]')");
    const channelNoticePath = source.slice(source.indexOf('coalesceChannelFrame(channel'), source.indexOf("'daemon-status'"));
    expect(channelNoticePath).toMatch(/if \(isLiveTail\(\)\)/);
  });

  it('preserves D64 receipt mapping and samples an overlapping row only once', () => {
    reset(0, 1, false);
    setDaemonClockMapping(1_000, 2);
    setDaemonWindow(0, 1, [frame(1)], 1);
    const rafs: FrameRequestCallback[] = [];
    markDaemonRowsCommitted([1_000_000_001], (callback) => { rafs.push(callback); return rafs.length; }, () => 9_025, () => true);
    expect(rafs).toHaveLength(1);
    rafs[0]!(0);
    expect(rafs).toHaveLength(2);
    rafs[1]!(0);
    expect(daemonReceiptToRafMs.at(-1)).toBe(24);
    setDaemonWindow(0, 1, [frame(1)], 2);
    markDaemonRowsCommitted([1_000_000_001], (callback) => { rafs.push(callback); return rafs.length; }, () => 9_030, () => true);
    expect(rafs).toHaveLength(2);
    expect(daemonReceiptToRafMs).toHaveLength(1);
  });

  it('updates a 50k-row projection in bounded work per frame', () => {
    const view = Array.from({ length: 64 }, (_, index) => frame(49_936 + index));
    const samples: number[] = [];
    for (const total of [5_000, 50_000]) {
      const started = performance.now();
      for (let count = 0; count < 200; count++) {
        setDaemonTotal(total);
        setDaemonWindow(total - 64, total, view, count + 1);
      }
      samples.push((performance.now() - started) / 200);
    }
    console.info(`S5 projection microbenchmark with 5k/50k total rows: ${samples[0]?.toFixed(4)}/${samples[1]?.toFixed(4)} ms per 64-row view update`);
    expect(rowCount()).toBe(50_000);
    expect(getRows()).toHaveLength(64);
    expect(getRowStart()).toBe(49_936);
  });
});
