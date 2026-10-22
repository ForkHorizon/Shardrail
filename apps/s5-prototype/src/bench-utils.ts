export function minimumGeneratedTokens(streams: number, durationMs: number) {
  return Math.ceil(streams * 75 * durationMs / 1000 * 0.5);
}

export function playwrightOutputPath(smoke: boolean, override?: string, now = new Date()) {
  if (override) return override;
  if (!smoke) return 'results/playwright.json';
  return `results/playwright-smoke-${now.toISOString().replace(/[:.]/g, '-')}.json`;
}

export function draftPreview(value: string) {
  const command = value.match(/^\/([^\s]+)(?:\s+.*)?$/);
  return command ? `Slash command: ${command[1]}` : value ? `Draft preview: ${value}` : 'Draft preview: empty';
}

export function withScenarioConfiguration<T extends object>(metrics: T, events: number, streams: number) {
  return { ...metrics, events, streams };
}

export function generatedTokenLoadReached(totalTokens: number, baselineTokens: number, minimum: number) {
  return totalTokens - baselineTokens >= minimum;
}

export function shouldWaitForDaemonStream(query: URLSearchParams, events: number) {
  return query.has('b4') && events > 0;
}

export function daemonStreamFinished(connectedOnce: boolean, chunks: number, expected: number) {
  return connectedOnce && chunks >= expected;
}

export async function waitForDaemonStream(expected: number, chunks: () => number, connectedOnce: () => boolean, pause = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms)), now = () => performance.now()) {
  const deadline = now() + 55_000;
  while (chunks() < expected && now() < deadline) await pause(50);
  const connected = connectedOnce();
  const received = chunks();
  if (!daemonStreamFinished(connected, received, expected)) throw new Error(`Relay/stream incomplete: connectedOnce=${connected}, chunks=${received}/${expected}`);
}

export function missingRequiredSamples(sample: { frameP95: number | null; mockCreationToMountedRowFollowupP95: number | null }) {
  return [sample.frameP95 === null && 'frameP95', sample.mockCreationToMountedRowFollowupP95 === null && 'mockCreationToMountedRowFollowupP95'].filter((value): value is string => Boolean(value));
}

export function benchShouldFail(results: Record<string, unknown>[], failed: boolean) {
  return failed || !results.some((result) => !('launchError' in result));
}
