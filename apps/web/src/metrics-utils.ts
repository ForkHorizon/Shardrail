export function supportsLongTaskEntries(entryTypes: readonly string[] | undefined) {
  return entryTypes?.includes('longtask') === true;
}

export const nominalFrameIntervalMs = 1000 / 60;

export type PendingInput = { startedAt: number; draft: string };

// A batched render only commits its latest draft. Discard earlier uncommitted
// values instead of pairing their timestamps with a later DOM commit.
export function takeCommittedInput(pending: PendingInput[], committedDraft: string) {
  let match = -1;
  for (let index = pending.length - 1; index >= 0; index--) {
    if (pending[index]?.draft === committedDraft) { match = index; break; }
  }
  if (match === -1) return undefined;
  const input = pending[match];
  pending.splice(0, match + 1);
  return input;
}

export function matchedInFirstFrame(startedAt: number, frameTime: number, callbackTime: number, domMatches: boolean, frameIntervalMs = nominalFrameIntervalMs) {
  const delay = callbackTime - startedAt;
  const frameTimestampDelta = frameTime - startedAt;
  const withinNextFrame = frameTimestampDelta < 0 ? delay >= 0 && delay <= frameIntervalMs : frameTimestampDelta <= frameIntervalMs;
  return { delay, frameTimestampDelta, matched: domMatches && withinNextFrame };
}

export function percentile(values: number[], p: number) {
  if (!values.length) return null;
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * p))] ?? null;
}

export function measureAfterPaint(
  startedAt: number,
  now: () => number,
  requestFrame: (callback: () => void) => number,
  afterFrame: (callback: () => void) => unknown,
  record: (elapsed: number) => void,
) {
  requestFrame(() => afterFrame(() => record(now() - startedAt)));
}

export function measureReadyCommitAfterFrame(
  ready: boolean,
  startedAt: number,
  now: () => number,
  requestFrame: (callback: () => void) => number,
  afterFrame: (callback: () => void) => unknown,
  record: (elapsed: number) => void,
) {
  if (!ready) return false;
  measureAfterPaint(startedAt, now, requestFrame, afterFrame, record);
  return true;
}
