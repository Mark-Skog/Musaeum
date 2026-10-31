/**
 * The main thread. Its job in the tile pipeline is deliberately small: pick a delivery path,
 * start things, pass input along, and on Path B copy tiles from the WebView2 buffer into the
 * worker's ring.
 */

import { invoke } from '@tauri-apps/api/core';

import { Samples } from './bench/metrics';
import type { FromWorker, ToWorker } from './compositor/messages';
import { type DeliveryEvidence, type DeliveryPlan, planDelivery } from './delivery-select';
import { mainThreadSeesHostWrites, probeSharedBuffer } from './probe';
import {
  type BenchOptions,
  type DeliveryPath,
  PROBE_MAGIC,
  RING_MAGIC,
  type SessionInfo,
  SLAB_MAGIC,
  type StartupOptions,
} from './protocol';
import { RingPump } from './ring-pump';
import { createSabRing, SabRingProducer } from './sab-ring';
import { SharedBufferInbox, type WebviewApi } from './shared-buffer-inbox';

/** A power of two, in tiles. Holds about two screens' worth of tiles in transit. */
const SAB_RING_CAPACITY = 32;
const BUFFER_WAIT_MS = 10_000;
/** How often the main-thread stress test blocks the thread. */
const STRESS_PERIOD_MS = 200;

declare global {
  interface Window {
    chrome?: { webview?: WebviewApi };
  }
}

interface SharedBuffers {
  slab: ArrayBuffer;
  ring: ArrayBuffer;
}

interface OpenedSession {
  session: SessionInfo;
  /** The WebView2 buffers of Paths B and C. Null on Path A. */
  buffers: SharedBuffers | null;
}

/** What `open_session` rejects with. */
interface OpenSessionError {
  deliveryFailed: boolean;
  message: string;
}

/** A delivery path failed to open, and the next one may still work. */
class DeliveryPathFailure extends Error {}

/** Tries everything the choice of path depends on, on this runtime, right now. */
async function gatherDeliveryEvidence(
  worker: Worker,
  inbox: SharedBufferInbox | null,
  options: StartupOptions,
): Promise<DeliveryEvidence> {
  const faults = options.deliveryFaults;
  const evidence: DeliveryEvidence = {
    hasWebviewApi: inbox !== null,
    hostBufferError: null,
    mainThreadSeesHostWrites: false,
    crossOriginIsolated: crossOriginIsolated && !faults.includes('not-isolated'),
    hasAtomicsWaitAsync: typeof Atomics.waitAsync === 'function',
    workerProbes: [],
  };
  // Path A needs none of this, so asking for it leaves the shared buffers alone.
  if (inbox === null || options.delivery === 'a') {
    return evidence;
  }
  try {
    await invoke('open_delivery_probe');
    const readOnly = await inbox.next(PROBE_MAGIC, BUFFER_WAIT_MS);
    const readWrite = await inbox.next(PROBE_MAGIC, BUFFER_WAIT_MS);
    evidence.mainThreadSeesHostWrites = await mainThreadSeesHostWrites(readOnly);
    evidence.workerProbes = [
      await probeSharedBuffer(worker, 'read-only', readOnly, 0, false, faults),
      await probeSharedBuffer(worker, 'read-write', readWrite, 1, true, faults),
    ];
  } catch (error) {
    evidence.hostBufferError = error instanceof Error ? error.message : String(error);
  }
  return evidence;
}

async function openOnPath(
  pdf: string,
  path: DeliveryPath,
  workers: number,
  inbox: SharedBufferInbox | null,
): Promise<OpenedSession> {
  let session: SessionInfo;
  try {
    session = await invoke<SessionInfo>('open_session', { args: { pdf, delivery: path, workers } });
  } catch (error) {
    const failure = error as OpenSessionError;
    throw failure.deliveryFailed
      ? new DeliveryPathFailure(failure.message)
      : new Error(failure.message);
  }
  if (path === 'a') {
    return { session, buffers: null };
  }
  if (inbox === null) {
    throw new DeliveryPathFailure('chrome.webview is missing');
  }
  try {
    const [ring, slab] = await Promise.all([
      inbox.next(RING_MAGIC, BUFFER_WAIT_MS),
      inbox.next(SLAB_MAGIC, BUFFER_WAIT_MS),
    ]);
    return { session, buffers: { ring, slab } };
  } catch (error) {
    throw new DeliveryPathFailure(error instanceof Error ? error.message : String(error));
  }
}

/** Walks down the plan until a path opens, recording every path that failed on the way. */
async function openFirstWorkingPath(
  pdf: string,
  plan: DeliveryPlan,
  workers: number,
  inbox: SharedBufferInbox | null,
): Promise<OpenedSession> {
  for (const path of plan.candidates) {
    try {
      return await openOnPath(pdf, path, workers, inbox);
    } catch (error) {
      if (!(error instanceof DeliveryPathFailure)) {
        throw error;
      }
      plan.rejected.push({ path, reason: error.message });
    }
  }
  throw new Error(`no delivery path can be used: ${JSON.stringify(plan.rejected)}`);
}

/** Blocks the main thread periodically, the way a heavy UI update would. */
function startMainThreadStress(busyMs: number): void {
  if (busyMs > 0) {
    setInterval(() => {
      const end = performance.now() + busyMs;
      while (performance.now() < end) {
        // Deliberately spinning.
      }
    }, STRESS_PERIOD_MS);
  }
}

async function failBench(bench: BenchOptions | null, message: string): Promise<void> {
  console.error(message);
  const hud = document.getElementById('hud');
  if (hud !== null) {
    hud.textContent = message;
  }
  if (bench !== null) {
    await invoke('bench_fail', { message });
  }
}

async function run(options: StartupOptions): Promise<void> {
  const hud = document.getElementById('hud') as HTMLPreElement;
  const canvas = document.getElementById('surface') as HTMLCanvasElement;
  const { bench } = options;

  if (options.pdf === null) {
    hud.textContent = 'Abra um PDF com: musaeum-desktop --pdf CAMINHO [--delivery auto|a|b|c]';
    return;
  }

  const worker = new Worker(new URL('./compositor/worker.ts', import.meta.url), { type: 'module' });
  const send = (message: ToWorker, transfer: Transferable[] = []): void => {
    worker.postMessage(message, transfer);
  };

  const copySamples = new Samples(1 << 16);
  const mainFrameIntervals = new Samples(1 << 16);
  let pump: RingPump | null = null;
  let finished = false;
  let report: Record<string, unknown> = {};

  worker.addEventListener('message', (event: MessageEvent<FromWorker>) => {
    const message = event.data;
    switch (message.type) {
      case 'hud':
        hud.textContent = message.text;
        break;
      case 'ready':
        console.info(`GPU: ${message.gpu}; clock round trip ${message.clockRttMs.toFixed(2)} ms`);
        break;
      case 'bench-done':
        finished = true;
        void invoke('bench_finish', {
          frontend: JSON.stringify({
            ...message.report,
            ...report,
            userAgent: navigator.userAgent,
            devicePixelRatio,
            hardwareConcurrency: navigator.hardwareConcurrency,
            crossOriginIsolated,
            mainThreadCopyMs: copySamples.summary(),
            mainThreadFrameIntervalMs: mainFrameIntervals.summary(),
            badSlots: pump?.badSlots ?? 0,
          }),
        });
        break;
      case 'fatal':
        finished = true;
        void failBench(bench, `compositor: ${message.message}`);
        break;
      case 'probe-reply':
        break;
    }
  });
  worker.addEventListener('error', (event) => {
    finished = true;
    void failBench(bench, `compositor failed to load: ${event.message}`);
  });

  // Listening starts before the host is asked to post anything.
  const webview = window.chrome?.webview;
  const inbox = webview !== undefined ? new SharedBufferInbox(webview) : null;

  const selectionStarted = performance.now();
  const evidence = await gatherDeliveryEvidence(worker, inbox, options);
  const plan = planDelivery(options.delivery, evidence);
  const { session, buffers } = await openFirstWorkingPath(
    options.pdf,
    plan,
    options.workers,
    inbox,
  );
  for (const { path, reason } of plan.rejected) {
    console.warn(`delivery path ${path} is not available: ${reason}`);
  }
  report = {
    session,
    sharedBufferProbes: evidence.workerProbes,
    deliverySelection: {
      requested: options.delivery,
      path: session.delivery,
      rejected: plan.rejected,
      evidence,
      elapsedMs: performance.now() - selectionStarted,
    },
  };

  const ring = session.delivery === 'b' ? createSabRing(SAB_RING_CAPACITY) : null;
  if (buffers !== null && ring !== null) {
    pump = new RingPump(buffers.slab, buffers.ring, new SabRingProducer(ring), (ms) =>
      copySamples.add(ms),
    );
  }

  const devicePixels = (element: HTMLElement): { width: number; height: number } => ({
    width: Math.max(1, Math.round(element.clientWidth * devicePixelRatio)),
    height: Math.max(1, Math.round(element.clientHeight * devicePixelRatio)),
  });

  const size = devicePixels(canvas);
  const offscreen = canvas.transferControlToOffscreen();
  const direct = session.delivery === 'c' ? buffers : null;
  send(
    {
      type: 'init',
      canvas: offscreen,
      width: size.width,
      height: size.height,
      session,
      ring,
      direct: direct !== null ? { slab: direct.slab, ring: direct.ring } : null,
      bench,
      zoom: bench?.zoom ?? 1.5 * devicePixelRatio,
    },
    direct !== null ? [offscreen, direct.slab, direct.ring] : [offscreen],
  );

  new ResizeObserver(() => {
    const { width, height } = devicePixels(canvas);
    send({ type: 'resize', width, height });
  }).observe(canvas);

  canvas.addEventListener(
    'wheel',
    (event) => {
      event.preventDefault();
      // Line and page modes come from some mice and from accessibility settings.
      const unit =
        event.deltaMode === WheelEvent.DOM_DELTA_LINE
          ? 40
          : event.deltaMode === WheelEvent.DOM_DELTA_PAGE
            ? canvas.clientHeight
            : 1;
      send({ type: 'wheel', deltaPx: event.deltaY * unit * devicePixelRatio });
    },
    { passive: false },
  );

  if (bench !== null) {
    startMainThreadStress(bench.mainStressMs);
  }

  let lastFrame = 0;
  const onAnimationFrame = (now: number): void => {
    if (lastFrame > 0) {
      mainFrameIntervals.add(now - lastFrame);
    }
    lastFrame = now;
    pump?.poll();
    if (!finished) {
      requestAnimationFrame(onAnimationFrame);
    }
  };
  // Only Path B needs the main thread for tiles, but measuring its responsiveness on both
  // paths is what shows whether the stress test really happened.
  if (pump !== null || bench !== null) {
    requestAnimationFrame(onAnimationFrame);
  }
}

async function main(): Promise<void> {
  let options: StartupOptions | null = null;
  try {
    options = await invoke<StartupOptions>('startup_options');
    await run(options);
  } catch (error) {
    const message = error instanceof Error ? (error.stack ?? error.message) : String(error);
    await failBench(options?.bench ?? null, message);
  }
}

void main();
