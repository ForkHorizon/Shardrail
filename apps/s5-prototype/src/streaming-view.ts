export function rangeForWindow(total: number, start: number, limit: number, followTail: boolean) {
  const size = Math.max(1, Math.min(128, Math.max(32, limit)));
  return { start: followTail ? Math.max(0, total - size) : Math.max(0, Math.min(start, total)), limit: size };
}

export function liveTailWindowSize(isB4: boolean) { return isB4 ? 128 : 32; }
export function transcriptOverscan(isB4: boolean) { return isB4 ? 32 : 8; }

export function markdownCacheKey(rowId: number, contentHash: string, contentVersion = 0) {
  return `${rowId}:${contentHash}:${contentVersion}`;
}

export function sameMarkdownVersion(previous: { rowId: number; source: string; contentHash: string; contentVersion: number }, next: { rowId: number; source: string; contentHash: string; contentVersion: number }) {
  return markdownCacheKey(previous.rowId, previous.contentHash, previous.contentVersion) === markdownCacheKey(next.rowId, next.contentHash, next.contentVersion) && previous.source === next.source;
}

export function hashContent(source: string) {
  let hash = 2166136261;
  for (let index = 0; index < source.length; index++) hash = Math.imul(hash ^ source.charCodeAt(index), 16777619);
  return (hash >>> 0).toString(36);
}

export function coalesceChannelFrame<T>(channel: { onmessage: ((message: T) => void) | null }, onFrame: (message: T) => void, request: (callback: FrameRequestCallback) => number = requestAnimationFrame, cancel: (id: number) => void = cancelAnimationFrame) {
  let latest: T | undefined;
  let frame = 0;
  channel.onmessage = (message) => {
    latest = message;
    if (!frame) frame = request(() => {
      frame = 0;
      const pending = latest;
      latest = undefined;
      if (pending !== undefined) onFrame(pending);
    });
  };
  return () => { if (frame) cancel(frame); channel.onmessage = null; };
}
