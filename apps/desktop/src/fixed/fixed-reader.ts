/**
 * The fixed-layout reader on the main thread: a compositor worker drawing on a canvas, the render
 * pool behind it, and the delivery path between them. Its job on this thread is deliberately
 * small: pick a delivery path, start things, pass input along, and on Path B copy tiles from the
 * WebView2 buffer into the worker's ring.
 *
 * One of these is one open document. The app opens one when a book is clicked and closes it when
 * the reader goes back to the library; the benchmarks and the tests of Phase 2 open one at start.
 */

import { invoke } from '@tauri-apps/api/core';

import { Samples } from '../bench/metrics';
import type { FromWorker, Inspection, ReaderStatus, ToWorker } from '../compositor/messages';
import { type DeliveryEvidence, type DeliveryPlan, planDelivery } from '../delivery-select';
import { mainThreadSeesHostWrites, probeSharedBuffer } from '../probe';
import {
  type DeliveryPath,
  PROBE_MAGIC,
  RING_MAGIC,
  type SavedReadingState,
  type SessionInfo,
  SLAB_MAGIC,
  type StartupOptions,
  TILE_BYTES,
} from '../protocol';
import { RingPump } from '../ring-pump';
import { createSabRing, SabRingProducer } from '../sab-ring';
import { SharedBufferInbox, type WebviewApi } from '../shared-buffer-inbox';

/** A power of two, in tiles. Holds about two screens' worth of tiles in transit. */
const SAB_RING_CAPACITY = 32;
const BUFFER_WAIT_MS = 10_000;

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
export const READER_KEYS = new Set([
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

/** What the reader is told about where it runs, and what it tells back. */
export interface FixedReaderConfig {
  options: StartupOptions;
  /** A canvas never used before: its drawing goes to the compositor worker for good. */
  canvas: HTMLCanvasElement;
  document: string;
  /** Whether the keys C and A switch the mode and the animation here, as in the page of Phase 2. */
  shortcuts: boolean;
  /** The benchmark's own extras, or null in the app. */
  bench: StartupOptions['bench'];
  onStatus?(status: ReaderStatus): void;
  onHud?(text: string): void;
  onFatal?(message: string): void;
  onBenchDone?(report: Record<string, unknown>): void;
  onFirstSharp?(atEpochMs: number): void;
  onReady?(): void;
  /** What is selected now, in CSS pixels of the canvas; no spans when nothing is. */
  onSelection?(selection: FixedSelection): void;
  /** A click on a mark, which opens it. */
  onMarkClick?(id: string, rect: CssRect): void;
}

/** A box in CSS pixels, relative to the canvas. */
export interface CssRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface FixedSelection {
  spans: { page: number; start: number; end: number }[];
  text: string;
  rect: CssRect | null;
}

const toCss = (rect: { x: number; y: number; width: number; height: number }): CssRect => ({
  x: rect.x / devicePixelRatio,
  y: rect.y / devicePixelRatio,
  width: rect.width / devicePixelRatio,
  height: rect.height / devicePixelRatio,
});

/** One listener for the WebView2 buffers the host posts, for as long as the page lives. */
let sharedInbox: SharedBufferInbox | null | undefined;

function inbox(): SharedBufferInbox | null {
  if (sharedInbox === undefined) {
    const webview = window.chrome?.webview;
    sharedInbox = webview !== undefined ? new SharedBufferInbox(webview) : null;
  }
  return sharedInbox;
}

/** The delivery plan, found once per run of the app: the runtime does not change in between. */
let knownPlan: { evidence: DeliveryEvidence; plan: DeliveryPlan } | null = null;

/** Tries everything the choice of path depends on, on this runtime, right now. */
async function gatherDeliveryEvidence(
  worker: Worker,
  received: SharedBufferInbox | null,
  options: StartupOptions,
): Promise<DeliveryEvidence> {
  const faults = options.deliveryFaults;
  const evidence: DeliveryEvidence = {
    hasWebviewApi: received !== null,
    hostBufferError: null,
    mainThreadSeesHostWrites: false,
    crossOriginIsolated: crossOriginIsolated && !faults.includes('not-isolated'),
    hasAtomicsWaitAsync: typeof Atomics.waitAsync === 'function',
    workerProbes: [],
  };
  // Path A needs none of this, so asking for it leaves the shared buffers alone.
  if (received === null || options.delivery === 'a') {
    return evidence;
  }
  try {
    await invoke('open_delivery_probe');
    const readOnly = await received.next(PROBE_MAGIC, BUFFER_WAIT_MS);
    const readWrite = await received.next(PROBE_MAGIC, BUFFER_WAIT_MS);
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
  document: string,
  path: DeliveryPath,
  workers: number,
  received: SharedBufferInbox | null,
): Promise<OpenedSession> {
  let session: SessionInfo;
  try {
    session = await invoke<SessionInfo>('open_session', {
      args: { document, delivery: path, workers },
    });
  } catch (error) {
    const failure = error as OpenSessionError;
    throw failure.deliveryFailed
      ? new DeliveryPathFailure(failure.message)
      : new Error(failure.message);
  }
  if (path === 'a') {
    return { session, buffers: null };
  }
  if (received === null) {
    throw new DeliveryPathFailure('chrome.webview is missing');
  }
  try {
    const [ring, slab] = await Promise.all([
      received.next(RING_MAGIC, BUFFER_WAIT_MS),
      received.next(SLAB_MAGIC, BUFFER_WAIT_MS),
    ]);
    return { session, buffers: { ring, slab } };
  } catch (error) {
    throw new DeliveryPathFailure(error instanceof Error ? error.message : String(error));
  }
}

/** Walks down the plan until a path opens, recording every path that failed on the way. */
async function openFirstWorkingPath(
  document: string,
  plan: DeliveryPlan,
  workers: number,
  received: SharedBufferInbox | null,
): Promise<OpenedSession> {
  for (const path of plan.candidates) {
    try {
      return await openOnPath(document, path, workers, received);
    } catch (error) {
      if (!(error instanceof DeliveryPathFailure)) {
        throw error;
      }
      plan.rejected.push({ path, reason: error.message });
    }
  }
  throw new Error(`no delivery path can be used: ${JSON.stringify(plan.rejected)}`);
}

/**
 * Sends what the pointer and the keyboard do to the compositor, in device pixels. `status` gives
 * what the reading bar last showed, which the shortcuts that act like its controls start from.
 */
function forwardInput(
  canvas: HTMLCanvasElement,
  send: (message: ToWorker) => void,
  status: () => ReaderStatus | null,
  shortcuts: boolean,
  signal: AbortSignal,
): void {
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
    { passive: false, signal },
  );
  canvas.addEventListener(
    'pointerdown',
    (event) => {
      canvas.setPointerCapture(event.pointerId);
      send({ type: 'pointer', phase: 'down', ...at(event) });
    },
    { signal },
  );
  // A pointer over the pages without a press only says where it is, now and then, so the cursor can
  // show text where there is some.
  let lastHover = 0;
  canvas.addEventListener(
    'pointermove',
    (event) => {
      if (canvas.hasPointerCapture(event.pointerId)) {
        send({ type: 'pointer', phase: 'move', ...at(event) });
      } else if (event.timeStamp - lastHover > 60) {
        lastHover = event.timeStamp;
        send({ type: 'hover', ...at(event) });
      }
    },
    { signal },
  );
  const end = (event: PointerEvent): void => {
    if (canvas.hasPointerCapture(event.pointerId)) {
      canvas.releasePointerCapture(event.pointerId);
    }
    send({ type: 'pointer', phase: 'up', ...at(event) });
  };
  canvas.addEventListener('pointerup', end, { signal });
  canvas.addEventListener('pointercancel', end, { signal });
  window.addEventListener(
    'keydown',
    (event) => {
      const target = event.target as HTMLElement | null;
      const typing =
        target !== null &&
        (['INPUT', 'SELECT', 'TEXTAREA'].includes(target.tagName) || target.isContentEditable);
      if (typing && target.id !== 'page-turn') {
        return;
      }
      if (event.defaultPrevented) {
        return;
      }
      const plain = !event.ctrlKey && !event.altKey && !event.metaKey;
      const shown = status();
      // The reading bar's controls, from the keyboard: C switches between paginated and
      // continuous, A turns the page-turn animation on or off, which only paginated reading has.
      if (shortcuts && plain && shown !== null && (event.key === 'c' || event.key === 'C')) {
        event.preventDefault();
        send({ type: 'set-mode', mode: shown.mode === 'paginated' ? 'continuous' : 'paginated' });
        return;
      }
      if (shortcuts && plain && shown !== null && (event.key === 'a' || event.key === 'A')) {
        event.preventDefault();
        if (shown.mode === 'paginated') {
          send({ type: 'set-animation', on: !shown.animating });
        }
        return;
      }
      const zoomKey = event.ctrlKey && ['=', '+', '-', '0'].includes(event.key);
      if (zoomKey || (plain && READER_KEYS.has(event.key))) {
        event.preventDefault();
        send({ type: 'key', key: event.key, ctrl: event.ctrlKey, shift: event.shiftKey });
      }
    },
    { signal },
  );
}

/** An open fixed-layout document. */
export class FixedReader {
  private finished = false;
  private lastStatus: ReaderStatus | null = null;
  private readonly inspections: ((inspection: Inspection) => void)[] = [];
  private readonly savedStates: SavedReadingState[] = [];
  private readonly listeners = new AbortController();
  private pump: RingPump | null = null;
  private resizeObserver: ResizeObserver | null = null;
  private compositorReady = false;
  private readyWaiters: (() => void)[] = [];
  private flushWaiters: (() => void)[] = [];
  /** Saves of the position still on their way to the library. */
  private readonly saving = new Set<Promise<unknown>>();
  readonly copySamples = new Samples(1 << 16);
  readonly mainFrameIntervals = new Samples(1 << 16);
  /** What the benchmark report carries about how the document opened. */
  report: Record<string, unknown> = {};
  session: SessionInfo | null = null;

  private constructor(
    private readonly config: FixedReaderConfig,
    private readonly worker: Worker,
  ) {}

  /** Opens a document. Rejects if no delivery path can carry its tiles or the file is not one. */
  static async open(config: FixedReaderConfig): Promise<FixedReader> {
    const worker = new Worker(new URL('../compositor/worker.ts', import.meta.url), {
      type: 'module',
    });
    const reader = new FixedReader(config, worker);
    try {
      await reader.start();
    } catch (error) {
      reader.dispose();
      throw error;
    }
    return reader;
  }

  send(message: ToWorker, transfer: Transferable[] = []): void {
    this.worker.postMessage(message, transfer);
  }

  status(): ReaderStatus | null {
    return this.lastStatus;
  }

  saved(): SavedReadingState[] {
    return this.savedStates.slice();
  }

  inspect(): Promise<Inspection> {
    return new Promise((resolve) => {
      this.inspections.push(resolve);
      this.send({ type: 'inspect' });
    });
  }

  /** Resolves once the compositor runs: anything asked before that would be dropped. */
  ready(): Promise<void> {
    if (this.compositorReady) {
      return Promise.resolve();
    }
    return new Promise((resolve) => this.readyWaiters.push(resolve));
  }

  private async start(): Promise<void> {
    const { config, worker } = this;
    const { options, canvas, bench } = config;
    const measuringMemory = bench?.scenario === 'memory';
    const documentIdHolder: { id: number | null } = { id: null };

    worker.addEventListener('message', (event: MessageEvent<FromWorker>) => {
      const message = event.data;
      switch (message.type) {
        case 'hud':
          config.onHud?.(message.text);
          break;
        case 'status':
          this.lastStatus = message;
          config.onStatus?.(message);
          break;
        case 'inspection':
          this.inspections.shift()?.(message);
          break;
        case 'reading-state':
          this.savedStates.push(message.state as SavedReadingState);
          if (documentIdHolder.id !== null) {
            const reading = { document: documentIdHolder.id, reading: message.state };
            const save = invoke('save_reading_state', reading)
              .catch((error) => console.warn('saving the reading position failed', error))
              .finally(() => this.saving.delete(save));
            this.saving.add(save);
          }
          break;
        case 'ready':
          console.info(`GPU: ${message.gpu}; clock round trip ${message.clockRttMs.toFixed(2)} ms`);
          this.compositorReady = true;
          for (const waiter of this.readyWaiters.splice(0)) {
            waiter();
          }
          config.onReady?.();
          break;
        case 'bench-done':
          this.finished = true;
          config.onBenchDone?.(message.report);
          break;
        case 'fatal':
          this.finished = true;
          config.onFatal?.(`compositor: ${message.message}`);
          break;
        case 'first-sharp':
          config.onFirstSharp?.(message.atEpochMs);
          break;
        case 'flushed':
          for (const waiter of this.flushWaiters.splice(0)) {
            waiter();
          }
          break;
        case 'probe-reply':
          break;
        case 'need-chars':
          for (const page of message.pages) {
            void this.sendChars(page);
          }
          break;
        case 'selection':
          config.onSelection?.({
            spans: message.spans,
            text: message.text,
            rect: message.rect === null ? null : toCss(message.rect),
          });
          break;
        case 'mark-click':
          config.onMarkClick?.(message.id, toCss(message.rect));
          break;
        case 'cursor':
          canvas.style.cursor = message.cursor === 'text' ? 'text' : message.cursor === 'mark' ? 'pointer' : '';
          break;
      }
    });
    worker.addEventListener('error', (event) => {
      this.finished = true;
      config.onFatal?.(`compositor failed to load: ${event.message}`);
    });

    // Listening starts before the host is asked to post anything.
    const received = inbox();
    const selectionStarted = performance.now();
    let evidence: DeliveryEvidence;
    let plan: DeliveryPlan;
    if (knownPlan !== null && bench === null) {
      evidence = knownPlan.evidence;
      plan = { ...knownPlan.plan, rejected: [] };
    } else {
      evidence = await gatherDeliveryEvidence(worker, received, options);
      plan = planDelivery(options.delivery, evidence);
      knownPlan = { evidence, plan: { ...plan, rejected: [...plan.rejected] } };
    }
    const { session, buffers } = await openFirstWorkingPath(
      config.document,
      plan,
      options.workers,
      received,
    );
    for (const { path, reason } of plan.rejected) {
      console.warn(`delivery path ${path} is not available: ${reason}`);
    }
    this.session = session;
    documentIdHolder.id = session.documentId;
    this.report = {
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
      this.pump = new RingPump(buffers.slab, buffers.ring, new SabRingProducer(ring), (ms) =>
        this.copySamples.add(ms),
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
    this.send(
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

    this.resizeObserver = new ResizeObserver(() => {
      const { width, height } = devicePixels(canvas);
      this.send({ type: 'resize', width, height });
    });
    this.resizeObserver.observe(canvas);

    forwardInput(
      canvas,
      (message) => this.send(message),
      () => this.lastStatus,
      config.shortcuts,
      this.listeners.signal,
    );

    let lastFrame = 0;
    const onAnimationFrame = (now: number): void => {
      if (lastFrame > 0) {
        this.mainFrameIntervals.add(now - lastFrame);
      }
      lastFrame = now;
      this.pump?.poll();
      if (!this.finished) {
        requestAnimationFrame(onAnimationFrame);
      }
    };
    // Only Path B needs the main thread for tiles, but measuring its responsiveness on both
    // paths is what shows whether the stress test really happened.
    if (this.pump !== null || (bench !== null && !measuringMemory)) {
      requestAnimationFrame(onAnimationFrame);
    }
  }

  /** Fetches a page's characters from the host and hands them to the compositor. */
  private async sendChars(page: number): Promise<void> {
    const session = this.session;
    if (session === null || this.finished) {
      return;
    }
    try {
      const bytes = await invoke<ArrayBuffer | number[]>('page_chars', { session: session.id, page });
      const buffer = bytes instanceof ArrayBuffer ? bytes : new Uint8Array(bytes).buffer;
      this.send({ type: 'chars', page, buffer }, [buffer]);
    } catch (error) {
      console.warn(`page ${page}'s characters`, error);
      this.send({ type: 'chars', page, buffer: null });
    }
  }

  /** Every mark to draw, replacing the ones before. */
  setMarks(marks: import('../compositor/annotator').FixedMark[]): void {
    this.send({ type: 'marks', marks });
  }

  clearSelection(): void {
    this.send({ type: 'clear-selection' });
  }

  /** How many shared slots arrived damaged, for the benchmark report. */
  badSlots(): number {
    return this.pump?.badSlots ?? 0;
  }

  private dispose(): void {
    this.finished = true;
    this.listeners.abort();
    this.resizeObserver?.disconnect();
    this.worker.terminate();
  }

  /**
   * Closes the document: the compositor stops, then the host stops the render pool and lets go of
   * the WebView2 buffers, which nothing on this side uses any more.
   */
  async close(): Promise<void> {
    if (this.compositorReady && !this.finished) {
      // The last move may still be waiting out its pause before being sent: it is sent now, and
      // kept, before anything stops.
      await Promise.race([
        new Promise<void>((resolve) => {
          this.flushWaiters.push(resolve);
          this.send({ type: 'flush' });
        }),
        new Promise<void>((resolve) => setTimeout(resolve, 1000)),
      ]);
      await Promise.all([...this.saving]);
    }
    this.dispose();
    await invoke('close_session');
  }
}
