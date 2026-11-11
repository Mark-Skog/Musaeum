/**
 * The compositor worker. It owns the canvas and runs the frame loop on its own thread: upload
 * arriving tiles, move the reader, plan the frame, ask for what is missing, draw. Nothing in the
 * loop waits for anything, and the main thread is not involved.
 */

import { SegmentRecorder } from '../bench/metrics';
import {
  buildScenario,
  type ScenarioName,
  ScenarioPlayer,
  type SegmentContext,
} from '../bench/scenarios';
import {
  type BenchOptions,
  PROBE_HOST_WORD,
  PROBE_SCRIPT_WORD,
  SCALE_ONE,
  type SessionInfo,
  tileBaseUrl,
} from '../protocol';
import { SabRingConsumer } from '../sab-ring';
import { buildDemand, DemandEncoder, DemandSender } from './demand';
import {
  type Delivery,
  DirectDelivery,
  PullDelivery,
  RingDelivery,
  type TileConsumer,
} from './delivery';
import { FramePlanner, TileList, wantedScale } from './frame-planner';
import { Annotator } from './annotator';
import { parseGeometry } from './layout';
import type {
  FromWorker,
  InitMessage,
  Inspection,
  ProbeRequest,
  ShownPage,
  ToWorker,
} from './messages';
import { Reader, type ReadingSnapshot } from './reader';
import { TileRenderer } from './renderer';
import type { PageLayoutHint } from './spreads';
import { defaultBudgetBytes, TileCache } from './tile-cache';
import { PlacedPages } from './view';

const scope = self as unknown as DedicatedWorkerGlobalScope;

/** Time allowed per frame for tile uploads, out of 8.3 ms at 120 Hz. */
const UPLOAD_BUDGET_MS = 2;
/** Most often the wish list is rebuilt. */
const DEMAND_INTERVAL_MS = 16;
/**
 * An unchanged list that still has tiles on it is sent again at this interval. The host
 * ignores repeats of what it is working on, so this only matters if something got lost.
 */
const DEMAND_REFRESH_MS = 250;
/** How long the loop keeps running after the last sign of activity, in interactive use. */
const IDLE_AFTER_MS = 400;
/** How long the position has to stay put before it is saved. */
const SAVE_AFTER_MS = 400;
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

function snapshotsDiffer(a: ReadingSnapshot | null, b: ReadingSnapshot): boolean {
  return (
    a === null ||
    a.page !== b.page ||
    a.u !== b.u ||
    a.v !== b.v ||
    a.mode !== b.mode ||
    a.pageTurnAnimation !== b.pageTurnAnimation ||
    a.zoom !== b.zoom ||
    a.spreads !== b.spreads
  );
}

class Compositor {
  private readonly cache: TileCache;
  private readonly encoder = new DemandEncoder();
  private readonly planner: FramePlanner;
  private readonly sender: DemandSender;
  private readonly renderer: TileRenderer;
  private readonly delivery: Delivery;
  private readonly current = new PlacedPages(64);
  private readonly target = new PlacedPages(64);
  private readonly around = new PlacedPages(64);
  private readonly wanted = new TileList(2048);
  private readonly neighbours = new TileList(2048);
  private readonly annotator: Annotator;

  private viewWidth: number;
  private viewHeight: number;

  private frameNumber = 0;
  private frameScheduled = false;
  private lastFrameTime = 0;
  private lastActivityTime = 0;
  private contextLost = false;

  private epoch = 0;
  private lastDemandTime = 0;
  private lastDemandSentTime = 0;
  private lastFingerprint = -1;

  private saved: ReadingSnapshot | null;
  private pendingSnapshot: ReadingSnapshot | null = null;
  private snapshotChangedAt = 0;
  private lastStatus = '';

  private lastHudTime = 0;
  private hudFrames = 0;
  private hudTiles = 0;

  private player: ScenarioPlayer | null = null;
  private recorders: SegmentRecorder[] = [];
  private overall: SegmentRecorder | null = null;
  private segmentNames: string[] = [];
  private segmentIndex = -1;
  private lastMoveTime = 0;
  private awaitingSharp = false;
  private wasResting = true;
  private reportedFirstSharp = false;

  constructor(
    private readonly reader: Reader,
    private readonly widths: Float32Array,
    private readonly heights: Float32Array,
    canvas: OffscreenCanvas,
    width: number,
    height: number,
    private readonly session: SessionInfo,
    private readonly clock: HostClock,
    delivery: Delivery | null,
    bench: BenchOptions | null,
    budgetBytes: number,
  ) {
    const base = tileBaseUrl(session.id);
    this.annotator = new Annotator(widths, heights, (message) => post(message));
    this.viewWidth = width;
    this.viewHeight = height;
    this.renderer = new TileRenderer(
      canvas,
      budgetBytes,
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
    this.cache = new TileCache(this.renderer.layers);
    this.planner = new FramePlanner(widths, heights, this.cache);
    this.sender = new DemandSender(`${base}/demand`);
    this.delivery = delivery ?? new PullDelivery(`${base}/next`);
    this.saved = session.readingState;
    this.applySize(width, height);

    if (bench !== null) {
      const segments = buildScenario(bench.scenario as ScenarioName);
      this.player = new ScenarioPlayer(segments);
      this.segmentNames = segments.map((segment) => segment.name);
      this.recorders = segments.map((segment) => new SegmentRecorder(segment.name));
      this.overall = new SegmentRecorder('overall');
      // The scenarios script a scroll, which only continuous reading has.
      this.reader.setMode('continuous');
    }
  }

  get gpuName(): string {
    return this.renderer.gpuName;
  }

  private applySize(width: number, height: number): void {
    this.viewWidth = width;
    this.viewHeight = height;
    this.reader.resize(width, height);
    this.renderer.resize(width, height);
  }

  resize(width: number, height: number): void {
    this.applySize(width, height);
    this.wake();
  }

  /** Applies something the reader did, then makes sure a frame follows. */
  input(message: ToWorker): void {
    const now = performance.now();
    const { reader } = this;
    switch (message.type) {
      case 'wheel':
        reader.wheel({ ...message, timeMs: now });
        break;
      case 'key':
        reader.key(message.key, message.ctrl, message.shift, now);
        break;
      case 'pointer': {
        // A press on text selects; anywhere else it pans, as it always did.
        const placed = this.current;
        if (message.phase === 'down') {
          if (!this.annotator.pointerDown(message.x, message.y, now, placed)) {
            reader.pointerDown(message.x, message.y, now);
          }
        } else if (message.phase === 'move') {
          if (!this.annotator.pointerMove(message.x, message.y, placed)) {
            reader.pointerMove(message.x, message.y, now);
          }
        } else if (!this.annotator.pointerUp(placed)) {
          reader.pointerUp(now);
        }
        break;
      }
      case 'chars':
        this.annotator.setChars(message.page, message.buffer);
        break;
      case 'marks':
        this.annotator.setMarks(message.marks);
        break;
      case 'clear-selection':
        this.annotator.clear(false);
        break;
      case 'hover':
        this.annotator.hover(message.x, message.y, this.current);
        return;
      case 'set-mode':
        reader.setMode(message.mode);
        break;
      case 'set-animation':
        reader.setAnimation(message.on);
        break;
      case 'set-spreads':
        reader.setSpreads(message.spreads);
        break;
      case 'turn':
        reader.turn(message.step);
        break;
      case 'go-to-page':
        if (Number.isSafeInteger(message.page) && message.page >= 0 && message.page < reader.pageCount) {
          reader.goToPage(message.page);
        }
        break;
      case 'go-to-point':
        if (Number.isSafeInteger(message.page) && message.page >= 0 && message.page < reader.pageCount) {
          const [u, v] = this.annotator.shownPoint(message.page, message.u, message.v);
          reader.goToPoint(message.page, u, v);
        }
        break;
      case 'zoom':
        reader.setZoom(reader.zoomFactor * message.factor, now);
        break;
      case 'inspect':
        post(this.inspect());
        return;
      case 'flush':
        // The document is about to close: the position waiting out its pause is sent now.
        if (this.player === null && this.pendingSnapshot !== null) {
          this.saved = this.pendingSnapshot;
          this.pendingSnapshot = null;
          post({ type: 'reading-state', state: this.saved });
        }
        post({ type: 'flushed' });
        return;
      default:
        return;
    }
    this.wake();
  }

  /** The reader as the last frame drew it, for a test driving the real window. */
  private inspect(): Inspection {
    const { reader, current, planner } = this;
    const pages: ShownPage[] = [];
    for (let at = 0; at < current.count; at++) {
      const page = current.pages[at];
      const widthPt = this.widths[page];
      const heightPt = this.heights[page];
      const x = Math.round(current.x[at]);
      const y = Math.round(current.y[at]);
      const scale = wantedScale(current.scales[at], widthPt, heightPt);
      const right = x + widthPt * (scale / SCALE_ONE);
      const bottom = y + heightPt * (scale / SCALE_ONE);
      if (x < this.viewWidth && y < this.viewHeight && right > 0 && bottom > 0) {
        pages.push({ page, x, y, scale, widthPt, heightPt });
      }
    }
    const { counters } = planner;
    return {
      type: 'inspection',
      snapshot: reader.snapshot(),
      animating: reader.mode.kind === 'paginated' && reader.mode.animate,
      moving: reader.isMoving(),
      sharp: counters.tilesVisible > 0 && counters.uncovered === 0 && counters.fallback === 0,
      framePending: this.frameScheduled,
      viewWidth: this.viewWidth,
      viewHeight: this.viewHeight,
      pages,
      selection: this.annotator.spans(),
      marks: this.annotator.markCount,
      charPages: this.annotator.charPages,
    };
  }

  wake(): void {
    this.lastActivityTime = performance.now();
    if (!this.frameScheduled) {
      this.frameScheduled = true;
      scope.requestAnimationFrame(this.onFrame);
    }
  }

  private readonly acceptTile: TileConsumer = (page, scale, x, y, pixels, doneUs) => {
    if (this.cache.find(page, scale, x, y) >= 0) {
      this.record((recorder) => recorder.duplicateTiles++);
      return;
    }
    const layer = this.cache.allocate(page, scale, x, y, this.frameNumber);
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

    const before = this.reader.snapshot();
    const scaleBefore = this.reader.currentScale;
    if (!this.playScenario(now)) {
      this.finishBench();
      return;
    }
    const moving = this.reader.advance(now);
    const after = this.reader.snapshot();
    const moved =
      moving ||
      this.reader.currentScale !== scaleBefore ||
      after.page !== before.page ||
      after.v !== before.v ||
      after.u !== before.u;

    this.current.clear();
    this.reader.placeCurrent(this.current);
    const { viewWidth, viewHeight } = this;
    this.planner.plan(this.frameNumber, this.current, viewWidth, viewHeight, this.renderer);
    // Marks and the selection go over the very pages this frame drew.
    this.annotator.want(this.current, now);
    this.annotator.draw(this.renderer, this.current);
    this.updateDemand(now);
    this.renderer.flush();

    const workMs = performance.now() - workStarted;
    this.measureFrame(now, frameInterval, workMs, uploadMs, moved);
    this.updateHud(now);
    this.reportState(now, after);

    if (moved || uploadMs > 0.05) {
      this.lastActivityTime = now;
    }
    const savePending = this.pendingSnapshot !== null;
    if (this.player !== null || savePending || now - this.lastActivityTime < IDLE_AFTER_MS) {
      this.frameScheduled = true;
      scope.requestAnimationFrame(this.onFrame);
    } else {
      this.delivery.notifyOnArrival(() => this.wake());
    }
  };

  /** Drives the reader from a benchmark scenario. Returns false when the scenario has finished. */
  private playScenario(now: number): boolean {
    if (this.player === null) {
      return true;
    }
    const range = this.reader.scrollRange();
    const context: SegmentContext = {
      minScrollPt: range.min,
      maxScrollPt: range.max,
      viewHeightPt: range.viewHeightPt,
    };
    const position = this.player.sample(now, this.reader.scrollPt, context);
    if (position === null) {
      return false;
    }
    const segment = this.player.current;
    if (this.player.currentIndex !== this.segmentIndex) {
      this.segmentIndex = this.player.currentIndex;
      if (segment?.zoom !== undefined) {
        this.reader.setZoom(segment.zoom, now);
      }
    }
    // A zoom keeps the point under it in place, which the scenario must not undo.
    if (segment?.zoom === undefined) {
      this.reader.scrollTo(position);
    }
    return true;
  }

  private updateDemand(now: number): void {
    if (now - this.lastDemandTime < DEMAND_INTERVAL_MS) {
      return;
    }
    this.lastDemandTime = now;
    // Sharp tiles for where the reader is heading, as soon as that is known.
    this.wanted.clear();
    this.neighbours.clear();
    if (this.reader.zoomSettled(now)) {
      this.target.clear();
      this.reader.placeTarget(this.target);
      this.planner.collectWanted(this.target, this.viewWidth, this.viewHeight, this.wanted);
      this.around.clear();
      this.reader.neighbours(this.around);
      this.planner.collectWanted(this.around, this.viewWidth, this.viewHeight, this.neighbours);
    }
    buildDemand(this.encoder, {
      widths: this.widths,
      heights: this.heights,
      cache: this.cache,
      missingPlaceholders: this.planner.missingPlaceholders,
      wanted: this.wanted,
      neighbours: this.neighbours,
      firstVisiblePage: this.planner.firstVisiblePage,
      lastVisiblePage: this.planner.lastVisiblePage,
      pagesPerSecond: this.reader.pagesPerSecond,
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

  /** Tells the main thread where the reader is, once it has stopped there for a moment. */
  private reportState(now: number, snapshot: ReadingSnapshot): void {
    const { reader } = this;
    const status = {
      type: 'status' as const,
      mode: reader.mode.kind,
      animating: reader.mode.kind === 'paginated' && reader.mode.animate,
      animationPreference: reader.animationPreference,
      spreads: reader.spreads,
      zoom: reader.zoomFactor,
      page: snapshot.page,
      pageCount: reader.pageCount,
    };
    const statusText = JSON.stringify(status);
    if (statusText !== this.lastStatus) {
      this.lastStatus = statusText;
      post(status);
    }
    if (this.player !== null) {
      return; // A benchmark's position is not the reader's.
    }
    if (snapshotsDiffer(this.pendingSnapshot ?? this.saved, snapshot)) {
      this.pendingSnapshot = snapshot;
      this.snapshotChangedAt = now;
    } else if (this.pendingSnapshot !== null && now - this.snapshotChangedAt >= SAVE_AFTER_MS) {
      this.saved = this.pendingSnapshot;
      this.pendingSnapshot = null;
      post({ type: 'reading-state', state: this.saved });
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
    const { counters } = this.planner;
    if (
      !this.reportedFirstSharp &&
      counters.tilesVisible > 0 &&
      counters.uncovered === 0 &&
      counters.fallback === 0
    ) {
      this.reportedFirstSharp = true;
      post({ type: 'first-sharp', atEpochMs: performance.timeOrigin + performance.now() });
    }
    if (this.player === null) {
      return;
    }
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
    const mode = this.reader.mode;
    const modeText =
      mode.kind === 'continuous'
        ? 'contínuo'
        : `paginado (${mode.animate ? 'animação' : 'sem animação'})`;
    post({
      type: 'hud',
      text:
        `caminho ${this.session.delivery.toUpperCase()}  ` +
        `${(this.hudFrames / seconds).toFixed(0)} fps  ` +
        `${(this.hudTiles / seconds).toFixed(0)} tiles/s  ` +
        `página ${this.reader.locator.page + 1}/${this.reader.pageCount}  ${modeText}  ` +
        `zoom ${(this.reader.zoomFactor * 100).toFixed(0)}%\n` +
        `nítidos ${counters.exact}/${counters.tilesVisible}  ` +
        `suaves ${counters.fallback}  placa ${counters.uncovered}  ` +
        `cache ${this.cache.resident}/${this.cache.capacity} MiB` +
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
        scale: this.reader.currentScale,
        pageCount: this.reader.pageCount,
        gpuLayers: this.cache.capacity,
        tileBudgetBytes: this.cache.budgetBytes,
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
  const sizes = parseGeometry(await response.arrayBuffer());
  const pageCount = sizes.length / 2;
  const widths = new Float32Array(pageCount);
  const heights = new Float32Array(pageCount);
  for (let page = 0; page < pageCount; page++) {
    widths[page] = sizes[page * 2];
    heights[page] = sizes[page * 2 + 1];
  }
  const { session } = message;
  const reader = new Reader(
    {
      widths,
      heights,
      pageLayout: session.pageLayout as PageLayoutHint,
      direction:
        session.direction === 'R2L' || session.direction === 'L2R' ? session.direction : null,
    },
    {
      devicePixelRatio: message.devicePixelRatio,
      comfortableScale: message.comfortableScale,
      prefersReducedMotion: message.prefersReducedMotion,
    },
    session.readingState,
  );
  const clock = await calibrateClock(base);
  let delivery: Delivery | null = null;
  if (message.ring !== null) {
    delivery = new RingDelivery(new SabRingConsumer(message.ring));
  } else if (message.direct !== null) {
    delivery = new DirectDelivery(message.direct.slab, message.direct.ring);
  }
  compositor = new Compositor(
    reader,
    widths,
    heights,
    message.canvas,
    message.width,
    message.height,
    session,
    clock,
    delivery,
    message.bench,
    message.tileBudgetBytes ?? defaultBudgetBytes(message.screenWidth, message.screenHeight),
  );
  post({
    type: 'ready',
    gpu: compositor.gpuName,
    pageCount,
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
    case 'probe':
      answerProbe(message);
      break;
    default:
      compositor?.input(message);
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
