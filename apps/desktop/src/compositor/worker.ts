/**
 * The compositor worker. It owns the canvas and runs the frame loop on its own thread:
 * upload arriving tiles, move the camera, plan the frame, ask for what is missing, draw.
 * Nothing in the loop waits for anything, and the main thread is not involved.
 */

import { SegmentRecorder } from '../bench/metrics';
import { buildScenario, ScenarioPlayer, type SegmentContext } from '../bench/scenarios';
import {
  type BenchOptions,
  PROBE_HOST_WORD,
  PROBE_SCRIPT_WORD,
  type SessionInfo,
  tileBaseUrl,
} from '../protocol';
import { SabRingConsumer } from '../sab-ring';
import { nextVelocity } from './camera';
import { buildDemand, DemandEncoder, DemandSender } from './demand';
import {
  type Delivery,
  DirectDelivery,
  PullDelivery,
  RingDelivery,
  type TileConsumer,
} from './delivery';
import { FramePlanner } from './frame-planner';
import { buildLayout, type DocumentLayout, lodForScale, parseGeometry, tileKey } from './layout';
import type { FromWorker, InitMessage, ProbeRequest, ToWorker } from './messages';
import { TileRenderer } from './renderer';
import { TileCache } from './tile-cache';

const scope = self as unknown as DedicatedWorkerGlobalScope;

/** 256 MiB of VRAM. A 4K screen shows about 54 tiles, so this holds several screens. */
const GPU_LAYERS = 256;
/** Time allowed per frame for tile uploads, out of 8.3 ms at 120 Hz. */
const UPLOAD_BUDGET_MS = 2;
const PAGE_GAP_PT = 12;
/** Most often the wish list is rebuilt. */
const DEMAND_INTERVAL_MS = 16;
/**
 * An unchanged list that still has tiles on it is sent again at this interval. The host
 * ignores repeats of what it is working on, so this only matters if something got lost.
 */
const DEMAND_REFRESH_MS = 250;
/** How long the loop keeps running after the last sign of activity, in interactive use. */
const IDLE_AFTER_MS = 400;
/** Time constant of the wheel's smoothing. */
const WHEEL_SMOOTHING_MS = 70;
const HUD_INTERVAL_MS = 500;
const CLOCK_SAMPLES = 16;

function post(message: FromWorker): void {
  scope.postMessage(message);
}

interface HostClock {
  /** Add to `performance.now() × 1000` to get the host's clock, in microseconds. */
  offsetUs: number;
  /** The fastest round trip seen, which bounds the clock's error. */
  rttMs: number;
}

/** Estimates the host clock's offset from the fastest of several round trips. */
async function calibrateClock(baseUrl: string): Promise<HostClock> {
  let best: HostClock = { offsetUs: 0, rttMs: Number.POSITIVE_INFINITY };
  for (let sample = 0; sample < CLOCK_SAMPLES; sample++) {
    const sent = performance.now();
    const response = await fetch(`${baseUrl}/clock`, { cache: 'no-store', credentials: 'omit' });
    const bytes = await response.arrayBuffer();
    const received = performance.now();
    const rttMs = received - sent;
    if (rttMs < best.rttMs) {
      const hostUs = Number(new DataView(bytes).getBigUint64(0, true));
      best = { offsetUs: hostUs - ((sent + received) / 2) * 1000, rttMs };
    }
  }
  return best;
}

class Compositor {
  private readonly cache = new TileCache(GPU_LAYERS);
  private readonly encoder = new DemandEncoder();
  private readonly planner: FramePlanner;
  private readonly sender: DemandSender;
  private readonly renderer: TileRenderer;
  private readonly delivery: Delivery;

  private viewWidth: number;
  private viewHeight: number;
  private scale = 1;
  private scrollPt = 0;
  private targetScrollPt = 0;
  private velocityPt = 0;

  private frameNumber = 0;
  private frameScheduled = false;
  private lastFrameTime = 0;
  private lastActivityTime = 0;
  private contextLost = false;

  private epoch = 0;
  private lastDemandTime = 0;
  private lastDemandSentTime = 0;
  private lastFingerprint = -1;

  private lastHudTime = 0;
  private hudFrames = 0;
  private hudTiles = 0;

  private player: ScenarioPlayer | null = null;
  private recorders: SegmentRecorder[] = [];
  private overall: SegmentRecorder | null = null;
  private segmentNames: string[] = [];
  private lastMoveTime = 0;
  private awaitingSharp = false;
  private wasResting = true;

  constructor(
    private readonly layout: DocumentLayout,
    canvas: OffscreenCanvas,
    width: number,
    height: number,
    private readonly session: SessionInfo,
    private readonly clock: HostClock,
    delivery: Delivery | null,
    bench: BenchOptions | null,
    private readonly zoom: number,
  ) {
    const base = tileBaseUrl(session.id);
    this.viewWidth = width;
    this.viewHeight = height;
    this.renderer = new TileRenderer(
      canvas,
      GPU_LAYERS,
      () => {
        this.contextLost = true;
      },
      () => {
        this.contextLost = false;
        // The GPU's copies are gone. Asking again is all it takes to get them back.
        this.cache.clear();
        this.lastFingerprint = -1;
        this.wake();
      },
    );
    this.planner = new FramePlanner(layout, this.cache);
    this.sender = new DemandSender(`${base}/demand`);
    this.delivery = delivery ?? new PullDelivery(`${base}/next`);
    this.applySize(width, height);

    if (bench !== null) {
      const segments = buildScenario(bench.scenario);
      this.player = new ScenarioPlayer(segments);
      this.segmentNames = segments.map((segment) => segment.name);
      this.recorders = segments.map((segment) => new SegmentRecorder(segment.name));
      this.overall = new SegmentRecorder('overall');
    }
  }

  get gpuName(): string {
    return this.renderer.gpuName;
  }

  /** Uniform physical scale, limited so that the widest page is never cut off. */
  private applySize(width: number, height: number): void {
    this.viewWidth = width;
    this.viewHeight = height;
    this.scale = Math.min(this.zoom, width / Math.max(this.layout.maxWidth, 1));
    this.renderer.resize(width, height);
  }

  resize(width: number, height: number): void {
    this.applySize(width, height);
    this.wake();
  }

  wheel(deltaPx: number): void {
    this.targetScrollPt = this.clampScroll(this.targetScrollPt + deltaPx / this.scale);
    this.wake();
  }

  private clampScroll(scrollPt: number): number {
    const max = Math.max(0, this.layout.totalHeight - this.viewHeight / this.scale);
    return Math.min(max, Math.max(0, scrollPt));
  }

  wake(): void {
    this.lastActivityTime = performance.now();
    if (!this.frameScheduled) {
      this.frameScheduled = true;
      scope.requestAnimationFrame(this.onFrame);
    }
  }

  private readonly acceptTile: TileConsumer = (page, lod, x, y, pixels, doneUs) => {
    const key = tileKey(page, lod, x, y);
    if (this.cache.find(key) >= 0) {
      this.record((recorder) => recorder.duplicateTiles++);
      return;
    }
    const layer = this.cache.allocate(key, this.frameNumber);
    if (layer < 0) {
      this.record((recorder) => recorder.droppedTiles++);
      return;
    }
    const started = performance.now();
    this.renderer.upload(layer, pixels);
    const finished = performance.now();
    this.hudTiles++;

    // Both clocks wrap together at 2^32 microseconds, which is about 71 minutes.
    const hostNowUs = (finished * 1000 + this.clock.offsetUs) % 4_294_967_296;
    const elapsedUs = (hostNowUs - doneUs + 4_294_967_296) % 4_294_967_296;
    // A small negative value caused by the clock's error shows up as a huge one here.
    const deliveryMs = elapsedUs > 2_147_483_648 ? 0 : (elapsedUs - (finished - started) * 1000) / 1000;
    const uploadMs = finished - started;
    this.record((recorder) => {
      recorder.tileUpload.add(uploadMs);
      recorder.tileDelivery.add(Math.max(0, deliveryMs));
      recorder.tilesUploaded++;
    });
  };

  /** Applies `update` to the current segment's recorder and to the overall one. */
  private record(update: (recorder: SegmentRecorder) => void): void {
    if (this.overall === null || this.player === null) {
      return;
    }
    const recorder = this.recorders[this.player.currentIndex];
    if (recorder !== undefined) {
      update(recorder);
      update(this.overall);
    }
  }

  private readonly onFrame = (now: number): void => {
    this.frameScheduled = false;
    if (this.contextLost) {
      return;
    }
    const workStarted = performance.now();
    this.frameNumber++;
    const frameInterval = this.lastFrameTime > 0 ? now - this.lastFrameTime : 0;
    this.lastFrameTime = now;

    const uploadStarted = performance.now();
    this.delivery.drain(this.acceptTile, UPLOAD_BUDGET_MS);
    const uploadMs = performance.now() - uploadStarted;

    const previousScroll = this.scrollPt;
    if (!this.advanceCamera(now, frameInterval)) {
      this.finishBench();
      return;
    }

    this.planner.plan(
      this.frameNumber,
      this.scrollPt,
      this.scale,
      this.viewWidth,
      this.viewHeight,
      this.renderer,
    );
    this.updateDemand(now);
    this.renderer.flush();

    const workMs = performance.now() - workStarted;
    this.measureFrame(now, frameInterval, workMs, uploadMs, this.scrollPt !== previousScroll);
    this.updateHud(now);

    const moving = Math.abs(this.targetScrollPt - this.scrollPt) > 0.01;
    if (moving || uploadMs > 0.05) {
      this.lastActivityTime = now;
    }
    if (this.player !== null || now - this.lastActivityTime < IDLE_AFTER_MS) {
      this.frameScheduled = true;
      scope.requestAnimationFrame(this.onFrame);
    } else {
      this.delivery.notifyOnArrival(() => this.wake());
    }
  };

  /** Moves the camera. Returns false when a benchmark scenario has finished. */
  private advanceCamera(now: number, frameInterval: number): boolean {
    const previous = this.scrollPt;
    const viewHeightPt = this.viewHeight / this.scale;

    if (this.player !== null) {
      const context: SegmentContext = {
        minScrollPt: 0,
        maxScrollPt: this.clampScroll(Number.POSITIVE_INFINITY),
        viewHeightPt,
      };
      const position = this.player.sample(now, this.scrollPt, context);
      if (position === null) {
        return false;
      }
      this.scrollPt = position;
      this.targetScrollPt = position;
    } else {
      const blend = 1 - Math.exp(-frameInterval / WHEEL_SMOOTHING_MS);
      const remaining = this.targetScrollPt - this.scrollPt;
      this.scrollPt = Math.abs(remaining) < 0.01 ? this.targetScrollPt : this.scrollPt + remaining * blend;
    }

    this.velocityPt = nextVelocity(
      this.velocityPt,
      this.scrollPt - previous,
      frameInterval,
      viewHeightPt,
    );
    return true;
  }

  private updateDemand(now: number): void {
    if (now - this.lastDemandTime < DEMAND_INTERVAL_MS) {
      return;
    }
    this.lastDemandTime = now;
    buildDemand(this.encoder, {
      layout: this.layout,
      cache: this.cache,
      missingPlaceholders: this.planner.missingPlaceholders,
      missingSharp: this.planner.missingSharp,
      firstVisiblePage: this.planner.firstVisiblePage,
      lastVisiblePage: this.planner.lastVisiblePage,
      targetLod: lodForScale(this.scale),
      velocityPt: this.velocityPt,
      viewHeightPt: this.viewHeight / this.scale,
    });
    const fingerprint = this.encoder.fingerprint();
    const refreshDue =
      this.encoder.tileCount > 0 && now - this.lastDemandSentTime >= DEMAND_REFRESH_MS;
    if (fingerprint !== this.lastFingerprint || refreshDue) {
      this.lastFingerprint = fingerprint;
      this.lastDemandSentTime = now;
      this.epoch = (this.epoch + 1) >>> 0;
      this.sender.submit(this.encoder.finish(this.epoch));
    }
  }

  private measureFrame(
    now: number,
    frameInterval: number,
    workMs: number,
    uploadMs: number,
    moved: boolean,
  ): void {
    this.hudFrames++;
    if (this.player === null) {
      return;
    }
    const { counters } = this.planner;
    const resting = this.player.isResting(now);
    if (moved) {
      if (this.awaitingSharp && this.wasResting) {
        this.record((recorder) => recorder.sharpAbandoned++);
      }
      this.lastMoveTime = now;
      this.awaitingSharp = true;
    } else if (
      this.awaitingSharp &&
      resting &&
      counters.uncovered === 0 &&
      counters.fallback === 0
    ) {
      const waited = now - this.lastMoveTime;
      this.record((recorder) => recorder.timeToSharp.add(waited));
      this.awaitingSharp = false;
    }
    this.wasResting = resting && !moved;

    this.record((recorder) => {
      recorder.frames++;
      if (frameInterval > 0) {
        recorder.frameInterval.add(frameInterval);
      }
      recorder.frameWork.add(workMs);
      recorder.uploadWork.add(uploadMs);
      recorder.visibleTiles += counters.tilesVisible;
      recorder.exactTiles += counters.exact;
      if (counters.plates !== counters.visiblePages) {
        recorder.blankFrames++;
      }
      if (counters.uncovered > 0) {
        recorder.plateFrames++;
      } else if (counters.fallback > 0) {
        recorder.softFrames++;
      } else {
        recorder.sharpFrames++;
      }
    });
  }

  private updateHud(now: number): void {
    if (now - this.lastHudTime < HUD_INTERVAL_MS) {
      return;
    }
    const seconds = (now - this.lastHudTime) / 1000;
    const { counters } = this.planner;
    const segment = this.player?.current?.name;
    post({
      type: 'hud',
      text:
        `caminho ${this.session.delivery.toUpperCase()}  ` +
        `${(this.hudFrames / seconds).toFixed(0)} fps  ` +
        `${(this.hudTiles / seconds).toFixed(0)} tiles/s  ` +
        `página ${this.planner.firstVisiblePage + 1}/${this.layout.pageCount}\n` +
        `nítidos ${counters.exact}/${counters.tilesVisible}  ` +
        `suaves ${counters.fallback}  placa ${counters.uncovered}` +
        (segment === undefined ? '' : `  [${segment}]`),
    });
    this.lastHudTime = now;
    this.hudFrames = 0;
    this.hudTiles = 0;
  }

  private finishBench(): void {
    this.delivery.stop();
    const open = this.recorders[0]?.frameInterval.summary();
    // While nothing moves, the median interval is simply the display's refresh interval.
    const refreshIntervalMs = open !== undefined && open.p50 > 0 ? open.p50 : 1000 / 60;
    post({
      type: 'bench-done',
      report: {
        gpu: this.renderer.gpuName,
        viewport: { width: this.viewWidth, height: this.viewHeight },
        scale: this.scale,
        lod: lodForScale(this.scale),
        pageCount: this.layout.pageCount,
        gpuLayers: GPU_LAYERS,
        refreshIntervalMs,
        clockRttMs: this.clock.rttMs,
        demandsSent: this.sender.sent,
        demandFailures: this.sender.failures,
        gpuEvictions: this.cache.evictions,
        droppedQuads: this.renderer.droppedQuads,
        overall: this.overall?.report(refreshIntervalMs),
        segments: this.recorders.map((recorder) => recorder.report(refreshIntervalMs)),
        segmentOrder: this.segmentNames,
      },
    });
  }
}

let compositor: Compositor | null = null;

async function start(message: InitMessage): Promise<void> {
  const base = tileBaseUrl(message.session.id);
  const response = await fetch(`${base}/geometry`, { cache: 'no-store', credentials: 'omit' });
  if (!response.ok) {
    throw new Error(`the host answered ${response.status} for the page geometry`);
  }
  const layout = buildLayout(parseGeometry(await response.arrayBuffer()), PAGE_GAP_PT);
  const clock = await calibrateClock(base);
  let delivery: Delivery | null = null;
  if (message.ring !== null) {
    delivery = new RingDelivery(new SabRingConsumer(message.ring));
  } else if (message.direct !== null) {
    delivery = new DirectDelivery(message.direct.slab, message.direct.ring);
  }
  compositor = new Compositor(
    layout,
    message.canvas,
    message.width,
    message.height,
    message.session,
    clock,
    delivery,
    message.bench,
    message.zoom,
  );
  post({
    type: 'ready',
    gpu: compositor.gpuName,
    pageCount: layout.pageCount,
    clockRttMs: clock.rttMs,
  });
  compositor.wake();
}

function fail(error: unknown): void {
  post({ type: 'fatal', message: error instanceof Error ? (error.stack ?? error.message) : String(error) });
}

scope.addEventListener('error', (event) => fail(event.error ?? event.message));
scope.addEventListener('unhandledrejection', (event) => fail(event.reason));

scope.addEventListener('message', (event: MessageEvent<ToWorker>) => {
  const message = event.data;
  switch (message.type) {
    case 'init':
      start(message).catch(fail);
      break;
    case 'resize':
      compositor?.resize(message.width, message.height);
      break;
    case 'wheel':
      compositor?.wheel(message.deltaPx);
      break;
    case 'probe':
      answerProbe(message);
      break;
  }
});

const probeBuffers = new Map<string, ArrayBuffer>();

/** The worker's half of the experiment described in `probe.ts`. */
function answerProbe(request: ProbeRequest): void {
  if (request.step === 'take') {
    // A runtime that copies on transfer leaves the worker with exactly this.
    probeBuffers.set(request.label, request.simulateCopy ? request.buffer.slice(0) : request.buffer);
  }
  const buffer = request.step === 'forget' ? undefined : probeBuffers.get(request.label);
  if (request.step === 'forget') {
    probeBuffers.delete(request.label);
  }
  if (buffer === undefined || buffer.byteLength < 16) {
    post({
      type: 'probe-reply',
      label: request.label,
      byteLength: buffer?.byteLength ?? 0,
      firstWord: 0,
      hostWord: 0,
      workerHasWebviewApi: false,
    });
    return;
  }
  const words = new Uint32Array(buffer);
  if (request.step === 'write') {
    Atomics.store(words, PROBE_SCRIPT_WORD, request.value);
  }
  const chrome = (self as unknown as { chrome?: { webview?: unknown } }).chrome;
  post({
    type: 'probe-reply',
    label: request.label,
    byteLength: buffer.byteLength,
    firstWord: words[0],
    hostWord: Atomics.load(words, PROBE_HOST_WORD),
    workerHasWebviewApi: chrome?.webview !== undefined,
  });
}
