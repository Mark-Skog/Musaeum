/**
 * The main thread. Its job in the tile pipeline is deliberately small: pick a delivery path,
 * start things, pass input along, and on Path B copy tiles from the WebView2 buffer into the
 * worker's ring.
 */

import { invoke } from '@tauri-apps/api/core';

import { Samples } from './bench/metrics';
import { runBook } from './book-host';
import type { FromWorker, ReaderStatus, ToWorker } from './compositor/messages';
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
  TILE_BYTES,
} from './protocol';
import { RingPump } from './ring-pump';
import { createSabRing, SabRingProducer } from './sab-ring';
import { SharedBufferInbox, type WebviewApi } from './shared-buffer-inbox';

/** A power of two, in tiles. Holds about two screens' worth of tiles in transit. */
const SAB_RING_CAPACITY = 32;
const BUFFER_WAIT_MS = 10_000;
/** How often the main-thread stress test blocks the thread. */
const STRESS_PERIOD_MS = 200;
/** How long the memory scenario holds each resting state. */
const MEMORY_SETTLE_MS = 15_000;
const MEMORY_READING_MS = 30_000;
const MEMORY_SCROLL_EVERY_MS = 1000;

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

/** Keys the reader uses, which the page must not also act on. */
const READER_KEYS = new Set([
  'ArrowUp',
  'ArrowDown',
  'ArrowLeft',
  'ArrowRight',
  'PageUp',
  'PageDown',
  'Home',
  'End',
  ' ',
]);

/**
 * The reading bar: the mode, the page turn, facing pages and the zoom. It only sends what was
 * clicked and shows what the compositor reports, so it can never disagree with the reader.
 */
function connectReadingBar(send: (message: ToWorker) => void): (status: ReaderStatus) => void {
  const bar = document.getElementById('reading-bar') as HTMLElement;
  const paginated = document.getElementById('mode-paginated') as HTMLButtonElement;
  const continuous = document.getElementById('mode-continuous') as HTMLButtonElement;
  const animation = document.getElementById('page-turn') as HTMLInputElement;
  const spreads = document.getElementById('spreads') as HTMLSelectElement;
  const zoom = document.getElementById('zoom-level') as HTMLOutputElement;
  const page = document.getElementById('page-number') as HTMLOutputElement;
  bar.hidden = false;

  paginated.addEventListener('click', () => send({ type: 'set-mode', mode: 'paginated' }));
  continuous.addEventListener('click', () => send({ type: 'set-mode', mode: 'continuous' }));
  animation.addEventListener('change', () =>
    send({ type: 'set-animation', on: animation.checked }),
  );
  spreads.addEventListener('change', () =>
    send({ type: 'set-spreads', spreads: spreads.value as 'auto' | 'single' | 'double' }),
  );
  const click = (id: string, message: ToWorker): void =>
    document.getElementById(id)?.addEventListener('click', () => send(message));
  click('zoom-out', { type: 'zoom', factor: 1 / 1.25 });
  click('zoom-in', { type: 'zoom', factor: 1.25 });
  click('page-back', { type: 'turn', step: -1 });
  click('page-forward', { type: 'turn', step: 1 });

  return (status) => {
    paginated.setAttribute('aria-pressed', String(status.mode === 'paginated'));
    continuous.setAttribute('aria-pressed', String(status.mode === 'continuous'));
    // Continuous reading has no page turn to animate, so the switch is off and out of reach there.
    animation.checked = status.animating;
    animation.disabled = status.mode === 'continuous';
    spreads.value = status.spreads;
    zoom.value = `${Math.round(status.zoom * 100)}%`;
    page.value = `${status.page + 1} / ${status.pageCount}`;
  };
}

/** Sends what the pointer and the keyboard do to the compositor, in device pixels. */
function forwardInput(canvas: HTMLCanvasElement, send: (message: ToWorker) => void): void {
  const at = (event: MouseEvent): { x: number; y: number } => {
    const box = canvas.getBoundingClientRect();
    return {
      x: (event.clientX - box.left) * devicePixelRatio,
      y: (event.clientY - box.top) * devicePixelRatio,
    };
  };
  canvas.addEventListener(
    'wheel',
    (event) => {
      event.preventDefault();
      const legacy = (event as WheelEvent & { wheelDeltaY?: number }).wheelDeltaY;
      send({
        type: 'wheel',
        deltaX: event.deltaX,
        deltaY: event.deltaY,
        deltaMode: event.deltaMode,
        wheelDeltaY: legacy,
        ctrlKey: event.ctrlKey,
        ...at(event),
        timeMs: event.timeStamp,
      });
    },
    { passive: false },
  );
  canvas.addEventListener('pointerdown', (event) => {
    canvas.setPointerCapture(event.pointerId);
    send({ type: 'pointer', phase: 'down', ...at(event) });
  });
  canvas.addEventListener('pointermove', (event) => {
    if (canvas.hasPointerCapture(event.pointerId)) {
      send({ type: 'pointer', phase: 'move', ...at(event) });
    }
  });
  const end = (event: PointerEvent): void => {
    if (canvas.hasPointerCapture(event.pointerId)) {
      canvas.releasePointerCapture(event.pointerId);
    }
    send({ type: 'pointer', phase: 'up', ...at(event) });
  };
  canvas.addEventListener('pointerup', end);
  canvas.addEventListener('pointercancel', end);
  window.addEventListener('keydown', (event) => {
    const target = event.target as HTMLElement | null;
    const typing = target !== null && ['INPUT', 'SELECT', 'TEXTAREA'].includes(target.tagName);
    if (typing && target.id !== 'page-turn') {
      return;
    }
    const zoomKey = event.ctrlKey && ['=', '+', '-', '0'].includes(event.key);
    if (zoomKey || (!event.ctrlKey && !event.altKey && READER_KEYS.has(event.key))) {
      event.preventDefault();
      send({ type: 'key', key: event.key, ctrl: event.ctrlKey, shift: event.shiftKey });
    }
  });
}

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

function pause(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * The memory scenario: a person opening a document, reading for a while, and stepping away. The
 * compositor runs as it would for that person, going idle when nothing moves, which a scripted
 * scroll scenario would not let it do. Without `scroll`, there is nothing open to read.
 */
async function runMemoryScenario(
  scroll: ((deltaPx: number) => void) | null,
  extra: () => Record<string, unknown>,
): Promise<void> {
  const mark = (name: string): Promise<void> => invoke('bench_mark_phase', { name });
  await mark(scroll === null ? 'empty' : 'opened');
  await pause(MEMORY_SETTLE_MS);
  if (scroll !== null) {
    await mark('reading');
    const ends = performance.now() + MEMORY_READING_MS;
    while (performance.now() < ends) {
      scroll(window.innerHeight * devicePixelRatio * 0.75);
      await pause(MEMORY_SCROLL_EVERY_MS);
    }
    await mark('idle-after-reading');
    await pause(MEMORY_SETTLE_MS);
  }
  await mark('end');
  await invoke('bench_finish', { frontend: JSON.stringify(extra()) });
}

async function run(options: StartupOptions): Promise<void> {
  const hud = document.getElementById('hud') as HTMLPreElement;
  const canvas = document.getElementById('surface') as HTMLCanvasElement;
  const { bench } = options;
  const measuringMemory = bench?.scenario === 'memory';

  if (options.epub !== null) {
    await runBook(options, (message) => void failBench(bench, message));
    return;
  }
  if (options.pdf === null) {
    hud.textContent = 'Abra um documento com: musaeum-desktop --pdf CAMINHO ou --epub CAMINHO';
    if (measuringMemory) {
      await runMemoryScenario(null, () => ({}));
    }
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
  const nowEpochMs = (): number => performance.timeOrigin + performance.now();
  const openRequestedAt = nowEpochMs();
  const uptimeWhenOpening = await invoke<number>('uptime_ms');
  let opening: Record<string, number> = {};
  let documentId: number | null = null;
  let showStatus: ((status: ReaderStatus) => void) | null = null;

  worker.addEventListener('message', (event: MessageEvent<FromWorker>) => {
    const message = event.data;
    switch (message.type) {
      case 'hud':
        hud.textContent = message.text;
        break;
      case 'status':
        showStatus?.(message);
        break;
      case 'reading-state':
        if (documentId !== null) {
          const reading = { document: documentId, reading: message.state };
          invoke('save_reading_state', reading).catch((error) =>
            console.warn('saving the reading position failed', error),
          );
        }
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
            ...opening,
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
      case 'first-sharp':
        opening = {
          // From asking for the document, which is what a reader who is already in the app waits.
          openToFirstSharpMs: message.atEpochMs - openRequestedAt,
          // From the program starting, which is what someone opening a file from outside waits.
          coldStartToFirstSharpMs: uptimeWhenOpening + (message.atEpochMs - openRequestedAt),
        };
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
  documentId = session.documentId;
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
  const tileBudgetBytes =
    options.tileBudgetMb !== null
      ? options.tileBudgetMb * 1024 * 1024
      : options.gpuLayers !== null
        ? options.gpuLayers * TILE_BYTES
        : null;
  send(
    {
      type: 'init',
      canvas: offscreen,
      width: size.width,
      height: size.height,
      session,
      ring,
      direct: direct !== null ? { slab: direct.slab, ring: direct.ring } : null,
      // The compositor plays scripted scenarios, and the memory scenario is not one of those.
      bench: measuringMemory ? null : bench,
      devicePixelRatio,
      comfortableScale: bench?.zoom ?? 1.5 * devicePixelRatio,
      prefersReducedMotion: matchMedia('(prefers-reduced-motion: reduce)').matches,
      screenWidth: Math.round(screen.width * devicePixelRatio),
      screenHeight: Math.round(screen.height * devicePixelRatio),
      tileBudgetBytes,
    },
    direct !== null ? [offscreen, direct.slab, direct.ring] : [offscreen],
  );
  if (bench === null) {
    showStatus = connectReadingBar(send);
  }

  new ResizeObserver(() => {
    const { width, height } = devicePixels(canvas);
    send({ type: 'resize', width, height });
  }).observe(canvas);

  forwardInput(canvas, send);

  if (bench !== null) {
    startMainThreadStress(bench.mainStressMs);
  }
  if (measuringMemory) {
    // A notch of a wheel, as the reader's own input would arrive.
    const scroll = (deltaPx: number): void =>
      send({
        type: 'wheel',
        deltaX: 0,
        deltaY: deltaPx / devicePixelRatio,
        deltaMode: 0,
        ctrlKey: false,
        x: 0,
        y: 0,
        timeMs: performance.now(),
      });
    void runMemoryScenario(scroll, () => report).catch((error) =>
      failBench(bench, String(error)),
    );
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
  if (pump !== null || (bench !== null && !measuringMemory)) {
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
