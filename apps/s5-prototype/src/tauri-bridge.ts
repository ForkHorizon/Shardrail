export type TauriEventBridge = {
  listen<T>(event: string, handler: (event: { payload: T }) => void): Promise<() => void>;
  emit(event: string, payload: Record<string, never>): Promise<void>;
};

export async function listenAndAnnounceReady<TFrame>(
  bridge: TauriEventBridge,
  onFrames: (event: { payload: TFrame[] }) => void,
  onStatus: (event: { payload: { connected: boolean; error?: string } }) => void,
): Promise<() => void> {
  const unlisten = await Promise.all([
    bridge.listen('daemon-frames', onFrames),
    bridge.listen('daemon-status', onStatus),
  ]);
  try {
    await bridge.emit('s5-ready', {});
  } catch (error) {
    unlisten.forEach((remove) => remove());
    throw error;
  }
  return () => unlisten.forEach((remove) => remove());
}
