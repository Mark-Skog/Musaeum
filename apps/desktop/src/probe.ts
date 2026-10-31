/**
 * Checks, on the real WebView2 runtime, what the main thread and a worker can do with a
 * shared buffer.
 *
 * Microsoft documents `sharedbufferreceived` for the page, and does not mention workers.
 * So this asks the runtime directly: does the main thread see the host's writes, can the
 * `ArrayBuffer` be transferred, and if so, does the worker get the same live memory or a
 * copy? The delivery path depends on the answers, and since the WebView2 runtime updates
 * itself, they have to be checked on every start.
 *
 * Runs on small dedicated buffers, because a failed experiment must not harm the real ones.
 */

import { invoke } from '@tauri-apps/api/core';

import type { FromWorker, ProbeReply, ProbeRequest } from './compositor/messages';
import { PROBE_HOST_WORD, type DeliveryFault } from './protocol';

export interface ProbeOutcome {
  label: string;
  /** How `postMessage` with a transfer list responded. */
  transfer: string;
  /** Whether the buffer ended up detached on the main thread. */
  detachedAfterTransfer: boolean;
  /** Whether the worker received a buffer with the right size and contents. */
  received: boolean;
  /** Whether the worker sees a value the host wrote after the transfer. */
  workerSeesHostWrites: boolean;
  /** Whether the host sees a value the worker wrote after the transfer. */
  hostSeesWorkerWrites: boolean | null;
  workerHasWebviewApi: boolean;
}

const REPLY_TIMEOUT_MS = 1000;
const MAIN_THREAD_VALUE = 0x5eed_cafe;
const HOST_VALUE = 0x600d_f00d;
const WORKER_VALUE = 0x0c0f_fee0;

function exchange(
  worker: Worker,
  request: ProbeRequest,
  transfer: Transferable[] = [],
): Promise<ProbeReply | null> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      worker.removeEventListener('message', onMessage);
      resolve(null);
    }, REPLY_TIMEOUT_MS);
    function onMessage(event: MessageEvent<FromWorker>): void {
      const message = event.data;
      if (message.type === 'probe-reply' && message.label === request.label) {
        clearTimeout(timer);
        worker.removeEventListener('message', onMessage);
        resolve(message);
      }
    }
    worker.addEventListener('message', onMessage);
    try {
      worker.postMessage(request, transfer);
    } catch (error) {
      clearTimeout(timer);
      worker.removeEventListener('message', onMessage);
      reject(error);
    }
  });
}

/**
 * Whether the main thread sees a value the host writes into `buffer`. Path B stands on this.
 * Must run before the buffer is transferred away.
 */
export async function mainThreadSeesHostWrites(buffer: ArrayBuffer): Promise<boolean> {
  await invoke<number[]>('probe_exchange', { value: MAIN_THREAD_VALUE });
  return Atomics.load(new Uint32Array(buffer), PROBE_HOST_WORD) === MAIN_THREAD_VALUE;
}

/**
 * Probes one buffer. `probeIndex` is its position in the order the host posted them, and
 * `writable` says whether it was posted read-write. Writing to a read-only buffer would
 * crash the renderer, so that half of the experiment only runs on the writable one.
 */
export async function probeSharedBuffer(
  worker: Worker,
  label: string,
  buffer: ArrayBuffer,
  probeIndex: number,
  writable: boolean,
  faults: readonly DeliveryFault[] = [],
): Promise<ProbeOutcome> {
  const outcome: ProbeOutcome = {
    label,
    transfer: 'accepted',
    detachedAfterTransfer: false,
    received: false,
    workerSeesHostWrites: false,
    hostSeesWorkerWrites: null,
    workerHasWebviewApi: false,
  };

  let taken: ProbeReply | null;
  try {
    if (faults.includes('rejected-transfer')) {
      throw new DOMException('simulated by --delivery-fault', 'DataCloneError');
    }
    taken = await exchange(
      worker,
      {
        type: 'probe',
        step: 'take',
        label,
        buffer,
        simulateCopy: faults.includes('copied-transfer'),
      },
      [buffer],
    );
  } catch (error) {
    outcome.transfer = error instanceof Error ? `${error.name}: ${error.message}` : String(error);
    return outcome;
  }
  outcome.detachedAfterTransfer = buffer.byteLength === 0;
  if (taken === null) {
    return outcome;
  }
  outcome.received = taken.byteLength > 0;
  outcome.workerHasWebviewApi = taken.workerHasWebviewApi;

  await invoke<number[]>('probe_exchange', { value: HOST_VALUE });
  const read = await exchange(worker, { type: 'probe', step: 'read', label });
  outcome.workerSeesHostWrites = read?.hostWord === HOST_VALUE;

  if (writable) {
    await exchange(worker, { type: 'probe', step: 'write', label, value: WORKER_VALUE });
    const seenByHost = await invoke<number[]>('probe_exchange', { value: HOST_VALUE });
    outcome.hostSeesWorkerWrites = seenByHost[probeIndex] === WORKER_VALUE;
  }
  // The host closes this memory when a session opens, so the worker lets go of it now.
  await exchange(worker, { type: 'probe', step: 'forget', label });
  return outcome;
}

/** Whether the probes prove that transferred buffers remain live memory in a worker. */
export function directPathAvailable(outcomes: ProbeOutcome[]): boolean {
  return (
    outcomes.length === 2 &&
    outcomes.every((outcome) => outcome.received && outcome.workerSeesHostWrites) &&
    outcomes.some((outcome) => outcome.hostSeesWorkerWrites === true)
  );
}
