/**
 * Shared FIFO serialization queue for fixed-layout and flowing reader sessions.
 *
 * Because the host maintains a hard ceiling of 1 active document session (and 0 while on the Home
 * tab), serializing open and close operations across both `FixedReader` and `FlowReader` ensures:
 * - Outgoing tab reading positions (`save_reading_state` / `save_flow_position`) commit before the
 *   next session replaces or reopens the slot.
 * - Rapid tab switches (`Tab A -> Tab B -> Tab C -> Home -> Tab A`) abort queued intermediate
 *   opens in 0ms without spawning workers, posting WebView2 shared buffers, or mixing origins.
 */

let sessionQueue: Promise<void> = Promise.resolve();

export function withReaderSessionLock<T>(work: () => Promise<T>): Promise<T> {
  const run = sessionQueue.then(work, work);
  sessionQueue = run.then(
    () => undefined,
    () => undefined,
  );
  return run;
}

export function enqueueReaderCloseTask(closeTask: () => Promise<void> | undefined): Promise<void> {
  return withReaderSessionLock(async () => {
    await closeTask();
  });
}

export function createAbortError(message = 'The document open was cancelled'): Error {
  const error = new Error(message);
  error.name = 'AbortError';
  return error;
}

export function isAbortError(error: unknown): boolean {
  return error instanceof Error && error.name === 'AbortError';
}
