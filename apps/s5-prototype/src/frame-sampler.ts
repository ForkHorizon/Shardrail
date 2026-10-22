export function intersectingVirtualItems<T extends { start: number; size: number }>(items: T[], scrollTop: number, clientHeight: number) {
  return items.filter((item) => item.start < scrollTop + clientHeight && item.start + item.size > scrollTop);
}

export function createFrameSampler(
  request: (callback: FrameRequestCallback) => number,
  cancel: (id: number) => void,
  onFrame: (intervalMs: number) => void,
  now: () => number,
) {
  let id: number | null = null;
  let previous = 0;
  function frame() {
    if (id === null) return;
    const current = now();
    onFrame(current - previous);
    previous = current;
    id = request(frame);
  }
  return {
    start() {
      if (id !== null) return;
      previous = now();
      id = request(frame);
    },
    stop() {
      if (id !== null) cancel(id);
      id = null;
    },
  };
}
