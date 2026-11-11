/**
 * The reader's state and motion, with no DOM and no clock of its own: input goes in, and each frame
 * asks where the pages are. Everything here can be driven by a test, step by step.
 *
 * The reading mode is `paginated(animate)` or `continuous`. Continuous mode has no animation field
 * at all, so turning pages with a slide there is impossible by construction rather than by a check;
 * the preference lives beside the mode, and going back to paginated reading restores it.
 *
 * The position is a Locator, a page and a point on it, and it is the one thing both modes share.
 * It only changes when the reader moves, never because the mode, the zoom or the window did, which
 * is what brings a reader back to exactly where they were after switching back and forth.
 */

import { WheelClassifier, type WheelInput, type WheelSource } from './input';
import {
  buildRows,
  type PageLayoutHint,
  planSpreads,
  type ReadingDirection,
  type Row,
  type SpreadsSetting,
} from './spreads';
import { Spring } from './spring';
import {
  ContinuousView,
  type Locator,
  PaginatedView,
  type Pan,
  PlacedPages,
  type Viewport,
} from './view';

export type ReadingMode = { kind: 'paginated'; animate: boolean } | { kind: 'continuous' };
export type ModeKind = ReadingMode['kind'];

/** What is saved in `reading_state`, and what a session starts from. */
export interface ReadingSnapshot {
  page: number;
  u: number;
  v: number;
  progress: number;
  mode: ModeKind;
  pageTurnAnimation: boolean;
  zoom: number;
  spreads: SpreadsSetting;
}

export interface DocumentShape {
  widths: Float32Array;
  heights: Float32Array;
  pageLayout: PageLayoutHint;
  direction: 'L2R' | 'R2L' | null;
}

export interface ReaderOptions {
  /** Device pixels per CSS pixel. */
  devicePixelRatio: number;
  /** Device pixels per point that continuous reading starts at, before the window limits it. */
  comfortableScale: number;
  /** Whether the reader has asked for less motion, which makes the page turn start off. */
  prefersReducedMotion: boolean;
}

export interface WheelEventInput extends WheelInput {
  ctrlKey: boolean;
  /** Pointer position over the canvas, in device pixels. */
  x: number;
  y: number;
}

/** Where things were when a finger came down, which everything it does is measured from. */
interface Drag {
  pointerX: number;
  pointerY: number;
  strip: number;
  pan: Pan;
  topPt: number;
  leftPt: number;
  turned: boolean;
}

/** A trackpad gesture in progress, which turns at most one page however long it lasts. */
interface Swipe {
  start: number;
  travelled: number;
  lastTime: number;
  turned: boolean;
}

const PAGE_GAP_PT = 12;
const MARGIN_CSS_PX = 16;
const LINE_CSS_PX = 40;
const WHEEL_SMOOTHING_MS = 70;
/** Weight of the newest frame in the smoothed scroll speed. */
const VELOCITY_BLEND = 0.25;
const ZOOM_SMOOTHING_MS = 60;
/** How close a smoothed zoom has to come to land on its target: a fifth of a percent, unseen. */
const ZOOM_LANDS_WITHIN = 2e-3;
/** One notch of zoom, as browsers do it. */
const ZOOM_NOTCH = 1.1;
const ZOOM_KEY_STEP = 1.25;
export const MIN_ZOOM = 0.25;
export const MAX_ZOOM = 16;
/** A zoom target that has not changed for this long is where the reader is going. */
export const ZOOM_SETTLE_MS = 50;
/** A drag past this fraction of the viewport turns the page when released. */
const SWIPE_COMMIT_FRACTION = 0.3;
/** A release faster than this, in CSS pixels per second, turns the page whatever the distance. */
const FLING_CSS_PX_PER_S = 500;
/** With the page turn off, a swipe this long turns the page at once. */
const INSTANT_SWIPE_CSS_PX = 80;
/** A trackpad swipe ends when its events stop for this long. */
const SWIPE_GAP_MS = 120;
/** The fewest milliseconds between pages turned by successive mouse notches. */
const NOTCH_TURN_INTERVAL_MS = 90;

const clamp = (value: number, low: number, high: number): number =>
  Math.min(high, Math.max(low, value));

/** Recent motion of a finger or a swipe, for the speed it is released with. */
class VelocityTracker {
  private readonly samples: { t: number; x: number }[] = [];

  reset(t: number, x: number): void {
    this.samples.length = 0;
    this.samples.push({ t, x });
  }

  add(t: number, x: number): void {
    this.samples.push({ t, x });
    while (this.samples.length > 2 && t - this.samples[0].t > 100) {
      this.samples.shift();
    }
  }

  /** Units per second over the 100 ms before `at`. Nothing recent means standing still. */
  speed(at: number): number {
    const recent = this.samples.filter((sample) => at - sample.t <= 100);
    if (recent.length < 2) {
      return 0;
    }
    const first = recent[0];
    const last = recent[recent.length - 1];
    const dt = last.t - first.t;
    return dt > 0 ? ((last.x - first.x) / dt) * 1000 : 0;
  }
}

export class Reader {
  readonly pageCount: number;
  private readonly widths: Float32Array;
  private readonly heights: Float32Array;
  private readonly hints: { pageLayout: PageLayoutHint; direction: 'L2R' | 'R2L' | null };

  mode: ReadingMode;
  /** The paginated mode's page turn, kept while reading continuously. */
  animationPreference: boolean;
  spreads: SpreadsSetting;
  locator: Locator;

  private rows: Row[] = [];
  private direction: ReadingDirection = 'ltr';
  private continuous!: ContinuousView;
  private paginated!: PaginatedView;
  private readonly view: Viewport = { width: 1, height: 1 };

  // Zoom: a factor over what fits, moving toward its target.
  private zoom: number;
  private zoomTarget: number;
  private zoomChangedAt = Number.NEGATIVE_INFINITY;
  private zoomSmooth = true;
  /** The document point the zoom keeps under the pointer. */
  private zoomAnchor: { x: number; y: number; locator: Locator } | null = null;

  // Continuous camera, in points.
  private topPt = 0;
  private leftPt = 0;
  private targetTopPt = 0;
  private velocityPt = 0;
  /** Where the column was at the last frame, whatever moved it since: wheel, trackpad, finger. */
  private lastFrameTopPt = 0;

  // Paginated camera: the screen the reader is on, the strip's position in device pixels along the
  // reading direction, and the pan inside a zoomed screen.
  private screen = 0;
  private readonly strip = new Spring(0);
  private pan: Pan = { x: 0, y: 0 };

  private readonly wheelClassifier = new WheelClassifier();
  private dragging: Drag | null = null;
  private readonly dragVelocity = new VelocityTracker();
  private swipe: Swipe | null = null;
  private readonly swipeVelocity = new VelocityTracker();
  private lastNotchTurn = Number.NEGATIVE_INFINITY;
  private lastTime = Number.NaN;

  constructor(
    document: DocumentShape,
    private readonly options: ReaderOptions,
    saved: ReadingSnapshot | null,
  ) {
    this.widths = document.widths;
    this.heights = document.heights;
    this.pageCount = document.widths.length;
    this.hints = { pageLayout: document.pageLayout, direction: document.direction };
    this.animationPreference = saved?.pageTurnAnimation ?? !options.prefersReducedMotion;
    this.mode =
      saved?.mode === 'continuous'
        ? { kind: 'continuous' }
        : { kind: 'paginated', animate: this.animationPreference };
    this.spreads = saved?.spreads ?? 'auto';
    this.zoom = clamp(saved?.zoom ?? 1, MIN_ZOOM, MAX_ZOOM);
    this.zoomTarget = this.zoom;
    this.locator = saved
      ? { page: clamp(saved.page, 0, Math.max(0, this.pageCount - 1)), u: saved.u, v: saved.v }
      : { page: 0, u: 0, v: 0 };
    this.relayout();
  }

  private get dpr(): number {
    return this.options.devicePixelRatio;
  }

  private get margin(): number {
    return MARGIN_CSS_PX * this.dpr;
  }

  private get stride(): number {
    return this.view.width + this.margin;
  }

  /** +1 when the next screen is to the right, -1 when it is to the left. */
  private get sign(): number {
    return this.direction === 'rtl' ? -1 : 1;
  }

  // --- Layout ---

  private relayout(): void {
    const plan = planSpreads(this.spreads, this.hints.pageLayout, this.hints.direction);
    this.direction = plan.direction;
    this.rows = buildRows(this.widths, this.heights, plan);
    this.continuous = new ContinuousView(this.rows, this.widths, this.heights, PAGE_GAP_PT);
    this.paginated = new PaginatedView(this.rows, this.widths, this.heights);
    this.moveTo(this.locator);
  }

  /** Puts the camera of the current mode at a Locator, without moving through anything. */
  private moveTo(locator: Locator): void {
    if (this.pageCount === 0) {
      return;
    }
    this.zoomAnchor = null;
    if (this.mode.kind === 'continuous') {
      const scale = this.continuousScale(this.zoom);
      const position = this.continuous.positionOf(locator, scale, this.view);
      this.topPt = position.topPt;
      this.targetTopPt = position.topPt;
      this.leftPt = position.leftPt;
      // Being put somewhere is not moving there.
      this.lastFrameTopPt = position.topPt;
      this.velocityPt = 0;
    } else {
      this.screen = this.paginated.screenOf(locator.page);
      this.strip.snap(this.screen * this.stride);
      const scale = this.screenScale(this.screen, this.zoom);
      this.pan = this.paginated.panFor(locator, scale, this.view);
    }
  }

  private continuousScale(zoom: number): number {
    return this.continuous.scaleFor(this.view, this.options.comfortableScale, zoom, this.margin);
  }

  private screenScale(screen: number, zoom: number): number {
    return this.paginated.scaleOf(screen, this.view, this.margin, zoom);
  }

  resize(width: number, height: number): void {
    this.view.width = Math.max(1, width);
    this.view.height = Math.max(1, height);
    this.moveTo(this.locator);
  }

  // --- The state machine ---

  /** Switches mode, keeping the position. Paginated reading gets back its page turn preference. */
  setMode(kind: ModeKind): void {
    if (kind === this.mode.kind) {
      return;
    }
    this.endGestures();
    this.mode =
      kind === 'continuous'
        ? { kind: 'continuous' }
        : { kind: 'paginated', animate: this.animationPreference };
    this.moveTo(this.locator);
  }

  /** Sets the page turn preference, which only paginated reading has a use for. */
  setAnimation(on: boolean): void {
    this.animationPreference = on;
    if (this.mode.kind === 'paginated') {
      this.mode = { kind: 'paginated', animate: on };
      if (!on) {
        this.strip.snap(this.screen * this.stride);
      }
    }
  }

  setSpreads(setting: SpreadsSetting): void {
    if (setting !== this.spreads) {
      this.spreads = setting;
      this.relayout();
    }
  }

  /** Zooms to a factor over what fits, keeping the point under `anchor` where it is. */
  setZoom(zoom: number, now: number, anchor?: { x: number; y: number }, smooth = true): void {
    const target = clamp(zoom, MIN_ZOOM, MAX_ZOOM);
    if (target === this.zoomTarget) {
      return;
    }
    const at = anchor ?? { x: this.view.width / 2, y: this.view.height / 2 };
    if (this.zoomAnchor === null || this.zoomAnchor.x !== at.x || this.zoomAnchor.y !== at.y) {
      this.zoomAnchor = { x: at.x, y: at.y, locator: this.locatorUnder(at.x, at.y) };
    }
    this.zoomTarget = target;
    this.zoomChangedAt = now;
    this.zoomSmooth = smooth;
    if (!smooth) {
      this.applyZoom(target);
    }
  }

  get zoomFactor(): number {
    return this.zoomTarget;
  }

  // --- Navigation ---

  /** Turns one screen in reading order: +1 is the next one. */
  turn(step: 1 | -1): void {
    if (this.mode.kind === 'continuous') {
      const scale = this.continuousScale(this.zoom);
      const screenPt = (this.view.height - LINE_CSS_PX * this.dpr) / scale;
      this.scrollContinuous(step * screenPt, true);
      return;
    }
    this.goToScreen(this.screen + step);
  }

  /** Shows a point of a page, as fractions of the page as shown, a little below the top. */
  goToPoint(page: number, u: number, v: number): void {
    const target = clamp(page, 0, this.pageCount - 1);
    this.locator = { page: target, u: clamp(u, 0, 1), v: clamp(v - 0.04, 0, 1) };
    this.moveTo(this.locator);
  }

  goToPage(page: number): void {
    const target = clamp(page, 0, this.pageCount - 1);
    this.locator = { page: target, u: 0, v: 0 };
    this.moveTo(this.locator);
  }

  private goToScreen(screen: number): void {
    const target = clamp(screen, 0, this.rows.length - 1);
    if (this.mode.kind !== 'paginated') {
      return;
    }
    this.screen = target;
    this.pan = { x: 0, y: 0 };
    this.locator = { page: this.rows[target].pages[0], u: 0, v: 0 };
    if (this.mode.animate) {
      // Retargeting keeps the velocity, so turning again mid-slide continues smoothly.
      this.strip.target = target * this.stride;
    } else {
      this.strip.snap(target * this.stride);
    }
  }

  // --- Input ---

  wheel(input: WheelEventInput): void {
    const source = this.wheelClassifier.classify(input);
    const unit =
      input.deltaMode === 1 ? LINE_CSS_PX : input.deltaMode === 2 ? this.view.height / this.dpr : 1;
    const dx = input.deltaX * unit * this.dpr;
    const dy = input.deltaY * unit * this.dpr;

    if (input.ctrlKey) {
      // A pinch on a trackpad arrives as a wheel with Ctrl held, and follows the fingers exactly.
      const factor = ZOOM_NOTCH ** (-dy / (100 * this.dpr));
      const anchor = { x: input.x, y: input.y };
      this.setZoom(this.zoomTarget * factor, input.timeMs, anchor, source === 'mouse');
      return;
    }
    if (this.mode.kind === 'continuous') {
      const scale = this.continuousScale(this.zoom);
      this.scrollContinuous(dy / scale, source === 'mouse');
      if (dx !== 0) {
        const max = this.continuous.maxLeftPt(scale, this.view);
        this.leftPt = clamp(this.leftPt + dx / scale, 0, max);
        this.updateContinuousLocator();
      }
      return;
    }
    this.paginatedWheel(dx, dy, source, input.timeMs);
  }

  private scrollContinuous(deltaPt: number, smooth: boolean): void {
    const scale = this.continuousScale(this.zoom);
    const max = this.continuous.maxTopPt(scale, this.view);
    this.targetTopPt = clamp(this.targetTopPt + deltaPt, 0, max);
    if (!smooth) {
      this.topPt = this.targetTopPt;
      this.updateContinuousLocator();
    }
  }

  private panRange(): Pan {
    const scale = this.screenScale(this.screen, this.zoom);
    return this.paginated.panRange(this.screen, scale, this.view);
  }

  private swipeLength(): number {
    return INSTANT_SWIPE_CSS_PX * this.dpr;
  }

  private paginatedWheel(dx: number, dy: number, source: WheelSource, now: number): void {
    const range = this.panRange();
    if (Math.abs(dx) > Math.abs(dy)) {
      if (range.x > 0) {
        this.pan = { x: clamp(this.pan.x + dx, 0, range.x), y: this.pan.y };
        this.updatePaginatedLocator();
      } else {
        this.swipeBy(this.sign * dx, now);
      }
      return;
    }
    if (dy === 0) {
      return;
    }
    // A zoomed page pans first, and turns once its edge has been reached.
    const canPan = dy > 0 ? this.pan.y < range.y : this.pan.y > 0;
    if (range.y > 0 && canPan) {
      this.pan = { x: this.pan.x, y: clamp(this.pan.y + dy, 0, range.y) };
      this.updatePaginatedLocator();
      return;
    }
    if (source === 'mouse') {
      if (now - this.lastNotchTurn >= NOTCH_TURN_INTERVAL_MS) {
        this.lastNotchTurn = now;
        this.goToScreen(this.screen + (dy > 0 ? 1 : -1));
      }
      return;
    }
    // A trackpad gesture turns one page, however long its momentum lasts.
    const swipe = this.continueSwipe(now);
    swipe.travelled += dy;
    if (!swipe.turned && Math.abs(swipe.travelled) > this.swipeLength()) {
      swipe.turned = true;
      this.goToScreen(swipe.start + (dy > 0 ? 1 : -1));
    }
  }

  /** The swipe in progress, or a new one if the last ended long enough ago. */
  private continueSwipe(now: number): Swipe {
    if (this.swipe === null || now - this.swipe.lastTime > SWIPE_GAP_MS) {
      this.swipe = { start: this.screen, travelled: 0, lastTime: now, turned: false };
      this.swipeVelocity.reset(now, 0);
    }
    this.swipe.lastTime = now;
    return this.swipe;
  }

  /** A sideways trackpad swipe: the strip follows it exactly, and settles when it stops. */
  private swipeBy(along: number, now: number): void {
    const swipe = this.continueSwipe(now);
    swipe.travelled += along;
    this.swipeVelocity.add(now, swipe.travelled);
    if (this.mode.kind !== 'paginated') {
      return;
    }
    if (!this.mode.animate) {
      if (!swipe.turned && Math.abs(swipe.travelled) > this.swipeLength()) {
        swipe.turned = true;
        this.goToScreen(swipe.start + Math.sign(swipe.travelled));
      }
      return;
    }
    // One page per gesture, so momentum cannot fling through several.
    const travelled = clamp(swipe.travelled, -this.stride, this.stride);
    this.strip.snap(swipe.start * this.stride + travelled);
  }

  private endSwipe(now: number): void {
    const swipe = this.swipe;
    if (swipe === null || now - swipe.lastTime <= SWIPE_GAP_MS) {
      return;
    }
    this.swipe = null;
    const animated = this.mode.kind === 'paginated' && this.mode.animate;
    if (animated && !swipe.turned && this.strip.atRest) {
      this.release(this.swipeVelocity.speed(swipe.lastTime), swipe.start);
    }
  }

  pointerDown(x: number, y: number, now: number): void {
    this.dragging = {
      pointerX: x,
      pointerY: y,
      strip: this.strip.position,
      pan: { ...this.pan },
      topPt: this.topPt,
      leftPt: this.leftPt,
      turned: false,
    };
    this.dragVelocity.reset(now, 0);
    // A finger catches a sliding page exactly where it is.
    this.strip.snap(this.strip.position);
  }

  pointerMove(x: number, y: number, now: number): void {
    const drag = this.dragging;
    if (drag === null) {
      return;
    }
    const dx = x - drag.pointerX;
    const dy = y - drag.pointerY;
    if (this.mode.kind === 'continuous') {
      const scale = this.continuousScale(this.zoom);
      this.topPt = clamp(drag.topPt - dy / scale, 0, this.continuous.maxTopPt(scale, this.view));
      this.targetTopPt = this.topPt;
      const maxLeft = this.continuous.maxLeftPt(scale, this.view);
      this.leftPt = clamp(drag.leftPt - dx / scale, 0, maxLeft);
      this.updateContinuousLocator();
      return;
    }
    const range = this.panRange();
    if (range.x > 0 || range.y > 0) {
      this.pan = { x: clamp(drag.pan.x - dx, 0, range.x), y: clamp(drag.pan.y - dy, 0, range.y) };
      this.updatePaginatedLocator();
      return;
    }
    const along = -this.sign * dx;
    this.dragVelocity.add(now, along);
    if (this.mode.animate) {
      // Exactly under the finger, one screen either way at most.
      this.strip.snap(drag.strip + clamp(along, -this.stride, this.stride));
    } else if (!drag.turned && Math.abs(along) > this.swipeLength()) {
      drag.turned = true;
      this.goToScreen(this.screen + Math.sign(along));
    }
  }

  pointerUp(now: number): void {
    const drag = this.dragging;
    this.dragging = null;
    if (drag === null || this.mode.kind !== 'paginated' || !this.mode.animate || drag.turned) {
      return;
    }
    this.release(this.dragVelocity.speed(now), this.screen);
  }

  /** Lets go of the strip: on to the next screen if it went far or fast enough, back if not. */
  private release(speed: number, from: number): void {
    const displacement = this.strip.position - from * this.stride;
    const fling = FLING_CSS_PX_PER_S * this.dpr;
    let step = 0;
    if (Math.abs(speed) > fling) {
      step = Math.sign(speed);
    } else if (Math.abs(displacement) > SWIPE_COMMIT_FRACTION * this.stride) {
      step = Math.sign(displacement);
    }
    this.goToScreen(from + step);
    // The page carries on at the speed it was let go with.
    this.strip.velocity = speed;
  }

  private endGestures(): void {
    this.dragging = null;
    this.swipe = null;
  }

  /** Keyboard navigation. Returns whether the key meant something here. */
  key(key: string, ctrl: boolean, shift: boolean, now: number): boolean {
    if (ctrl) {
      if (key === '=' || key === '+') {
        this.setZoom(this.zoomTarget * ZOOM_KEY_STEP, now);
      } else if (key === '-') {
        this.setZoom(this.zoomTarget / ZOOM_KEY_STEP, now);
      } else if (key === '0') {
        this.setZoom(1, now);
      } else {
        return false;
      }
      return true;
    }
    return this.mode.kind === 'continuous'
      ? this.continuousKey(key, shift)
      : this.pageKey(key, shift);
  }

  private continuousKey(key: string, shift: boolean): boolean {
    const line = (LINE_CSS_PX * this.dpr) / this.continuousScale(this.zoom);
    switch (key) {
      case 'ArrowDown':
        this.scrollContinuous(line, true);
        return true;
      case 'ArrowUp':
        this.scrollContinuous(-line, true);
        return true;
      case 'PageDown':
      case ' ':
        this.turn(shift && key === ' ' ? -1 : 1);
        return true;
      case 'PageUp':
        this.turn(-1);
        return true;
      case 'Home':
        this.goToPage(0);
        return true;
      case 'End':
        this.goToPage(this.pageCount - 1);
        return true;
      default:
        return false;
    }
  }

  private pageKey(key: string, shift: boolean): boolean {
    const forward = this.direction === 'rtl' ? 'ArrowLeft' : 'ArrowRight';
    const backward = this.direction === 'rtl' ? 'ArrowRight' : 'ArrowLeft';
    switch (key) {
      case forward:
      case 'PageDown':
      case 'ArrowDown':
        this.goToScreen(this.screen + 1);
        return true;
      case ' ':
        this.goToScreen(this.screen + (shift ? -1 : 1));
        return true;
      case backward:
      case 'PageUp':
      case 'ArrowUp':
        this.goToScreen(this.screen - 1);
        return true;
      case 'Home':
        this.goToScreen(0);
        return true;
      case 'End':
        this.goToScreen(this.rows.length - 1);
        return true;
      default:
        return false;
    }
  }

  // --- Motion ---

  /** Moves everything forward to `now`. Returns whether anything is still moving. */
  advance(now: number): boolean {
    const dt = Number.isNaN(this.lastTime) ? 0 : Math.max(0, now - this.lastTime);
    this.lastTime = now;
    this.endSwipe(now);

    if (this.zoom !== this.zoomTarget) {
      const blend = this.zoomSmooth ? 1 - Math.exp(-dt / ZOOM_SMOOTHING_MS) : 1;
      const next = this.zoom + (this.zoomTarget - this.zoom) * blend;
      const landed = Math.abs(next - this.zoomTarget) < this.zoomTarget * ZOOM_LANDS_WITHIN;
      this.applyZoom(landed ? this.zoomTarget : next);
    } else {
      this.zoomAnchor = null;
    }

    if (this.mode.kind === 'continuous') {
      const remaining = this.targetTopPt - this.topPt;
      if (remaining !== 0) {
        const blend = 1 - Math.exp(-dt / WHEEL_SMOOTHING_MS);
        this.topPt =
          Math.abs(remaining) < 0.01 ? this.targetTopPt : this.topPt + remaining * blend;
        this.updateContinuousLocator();
      }
      this.updateVelocity(dt);
    } else if (this.dragging === null) {
      this.strip.step(dt);
    }
    return this.isMoving();
  }

  /**
   * The smoothed speed through the column, which decides what to ask the host for. Smoothing keeps
   * one uneven frame from flipping that decision, but standing still and jumping take effect at
   * once: a reader who stopped wants sharp tiles now, and landing somewhere from the scrollbar is
   * not a speed.
   */
  private updateVelocity(dt: number): void {
    const moved = this.topPt - this.lastFrameTopPt;
    this.lastFrameTopPt = this.topPt;
    if (dt <= 0) {
      return;
    }
    const viewHeightPt = this.view.height / this.continuousScale(this.zoom);
    if (moved === 0 || Math.abs(moved) > viewHeightPt) {
      this.velocityPt = 0;
    } else {
      const instant = (moved / dt) * 1000;
      this.velocityPt = this.velocityPt * (1 - VELOCITY_BLEND) + instant * VELOCITY_BLEND;
    }
  }

  isMoving(): boolean {
    const camera =
      this.mode.kind === 'continuous' ? this.topPt !== this.targetTopPt : !this.strip.atRest;
    return (
      this.zoom !== this.zoomTarget || camera || this.dragging !== null || this.swipe !== null
    );
  }

  /** Whether the zoom has stopped changing long enough to render for where it is going. */
  zoomSettled(now: number): boolean {
    return this.zoom === this.zoomTarget || now - this.zoomChangedAt >= ZOOM_SETTLE_MS;
  }

  /** Signed speed through the document, in pages per second: positive is forward. */
  get pagesPerSecond(): number {
    if (this.mode.kind !== 'continuous' || this.pageCount === 0) {
      return 0;
    }
    return (this.velocityPt * this.pageCount) / Math.max(this.continuous.totalHeightPt, 1);
  }

  /** The continuous camera that keeps the zoom's anchor under the pointer at `zoom`. */
  private anchoredContinuous(zoom: number): { topPt: number; leftPt: number; scale: number } {
    const scale = this.continuousScale(zoom);
    const maxTop = this.continuous.maxTopPt(scale, this.view);
    const maxLeft = this.continuous.maxLeftPt(scale, this.view);
    const anchor = this.zoomAnchor;
    if (anchor === null) {
      return {
        topPt: clamp(this.targetTopPt, 0, maxTop),
        leftPt: clamp(this.leftPt, 0, maxLeft),
        scale,
      };
    }
    const point = this.continuous.pointOf(anchor.locator);
    const leftPt = this.continuous.overflows(scale, this.view) ? point.xPt - anchor.x / scale : 0;
    return {
      topPt: clamp(point.yPt - anchor.y / scale, 0, maxTop),
      leftPt: clamp(leftPt, 0, maxLeft),
      scale,
    };
  }

  /** The pan that keeps the zoom's anchor under the pointer at `zoom`. */
  private anchoredPan(zoom: number): { pan: Pan; scale: number } {
    const scale = this.screenScale(this.screen, zoom);
    const range = this.paginated.panRange(this.screen, scale, this.view);
    const anchor = this.zoomAnchor;
    if (anchor === null) {
      return {
        pan: { x: clamp(this.pan.x, 0, range.x), y: clamp(this.pan.y, 0, range.y) },
        scale,
      };
    }
    const point = this.paginated.pointOf(anchor.locator, scale);
    return {
      pan: { x: clamp(point.x - anchor.x, 0, range.x), y: clamp(point.y - anchor.y, 0, range.y) },
      scale,
    };
  }

  private applyZoom(zoom: number): void {
    this.zoom = zoom;
    if (this.mode.kind === 'continuous') {
      const camera = this.anchoredContinuous(zoom);
      this.topPt = camera.topPt;
      this.targetTopPt = camera.topPt;
      this.leftPt = camera.leftPt;
      this.updateContinuousLocator();
      return;
    }
    this.pan = this.anchoredPan(zoom).pan;
    this.updatePaginatedLocator();
  }

  /** The document point under a pixel of the viewport, as a Locator. */
  private locatorUnder(x: number, y: number): Locator {
    const placed = new PlacedPages(16);
    this.placeCurrent(placed);
    for (let at = 0; at < placed.count; at++) {
      const page = placed.pages[at];
      const scale = placed.scales[at];
      const u = (x - placed.x[at]) / (this.widths[page] * scale);
      const v = (y - placed.y[at]) / (this.heights[page] * scale);
      if (u >= 0 && u <= 1 && v >= 0 && v <= 1) {
        return { page, u, v };
      }
    }
    // Off the pages, the nearest point of the page the reader is on.
    return this.locator;
  }

  private updateContinuousLocator(): void {
    const scale = this.continuousScale(this.zoom);
    this.locator = this.continuous.locatorAt(this.topPt, this.leftPt, scale, this.view);
  }

  private updatePaginatedLocator(): void {
    const scale = this.screenScale(this.screen, this.zoom);
    this.locator = this.paginated.locatorAt(this.screen, scale, this.pan, this.view);
  }

  // --- What the frame shows ---

  /** The pages on screen right now, wherever an animation has them. */
  placeCurrent(out: PlacedPages): void {
    if (this.pageCount === 0) {
      return;
    }
    if (this.mode.kind === 'continuous') {
      const scale = this.continuousScale(this.zoom);
      this.continuous.place(this.topPt, this.leftPt, scale, this.view, out);
      return;
    }
    // Whichever screens the strip shows, at most two while sliding.
    const position = this.strip.position / this.stride;
    const first = clamp(Math.floor(position), 0, this.rows.length - 1);
    const last = clamp(Math.ceil(position), 0, this.rows.length - 1);
    for (let screen = first; screen <= last; screen++) {
      const slotLeft = this.sign * (screen * this.stride - this.strip.position);
      if (slotLeft >= this.view.width || slotLeft + this.view.width <= 0) {
        continue;
      }
      const pan = screen === this.screen ? this.pan : { x: 0, y: 0 };
      const scale = this.screenScale(screen, this.zoom);
      this.paginated.placeScreen(screen, slotLeft, scale, pan, this.view, out);
    }
  }

  /** The pages where every motion in progress will come to rest: what to render sharp. */
  placeTarget(out: PlacedPages): void {
    if (this.pageCount === 0) {
      return;
    }
    if (this.mode.kind === 'continuous') {
      const camera = this.anchoredContinuous(this.zoomTarget);
      this.continuous.place(camera.topPt, camera.leftPt, camera.scale, this.view, out);
      return;
    }
    const { pan, scale } = this.anchoredPan(this.zoomTarget);
    this.paginated.placeScreen(this.screen, 0, scale, pan, this.view, out);
  }

  /**
   * Where the reader is likely to go next, fully rendered ahead of time at the scale it will be
   * shown: the screens on either side, or the rows just past the viewport.
   */
  neighbours(out: PlacedPages): void {
    if (this.pageCount === 0) {
      return;
    }
    if (this.mode.kind === 'paginated') {
      for (const screen of [this.screen + 1, this.screen - 1]) {
        if (screen >= 0 && screen < this.rows.length) {
          const scale = this.screenScale(screen, this.zoomTarget);
          this.paginated.placeScreen(screen, 0, scale, { x: 0, y: 0 }, this.view, out);
        }
      }
      return;
    }
    const scale = this.continuousScale(this.zoomTarget);
    const viewPt = this.view.height / scale;
    // The viewport just below, and just above, as if scrolled there.
    this.continuous.place(this.targetTopPt + viewPt, this.leftPt, scale, this.view, out);
    const above = Math.max(0, this.targetTopPt - viewPt);
    this.continuous.place(above, this.leftPt, scale, this.view, out);
  }

  /** What to save. */
  snapshot(): ReadingSnapshot {
    const { page, u, v } = this.locator;
    return {
      page,
      u,
      v,
      progress: this.pageCount > 0 ? clamp((page + v) / this.pageCount, 0, 1) : 0,
      mode: this.mode.kind,
      pageTurnAnimation: this.animationPreference,
      zoom: this.zoomTarget,
      spreads: this.spreads,
    };
  }

  /** For benchmarks: the continuous column's scroll position, in points. */
  get scrollPt(): number {
    return this.topPt;
  }

  scrollRange(): { min: number; max: number; viewHeightPt: number } {
    const scale = this.continuousScale(this.zoom);
    const max = this.continuous.maxTopPt(scale, this.view);
    return { min: 0, max, viewHeightPt: this.view.height / scale };
  }

  /** For benchmarks: puts the continuous column somewhere, with no smoothing. */
  scrollTo(topPt: number): void {
    const range = this.scrollRange();
    this.topPt = clamp(topPt, range.min, range.max);
    this.targetTopPt = this.topPt;
    this.updateContinuousLocator();
  }

  /** The device pixels per point the current mode shows the first visible page at. */
  get currentScale(): number {
    return this.mode.kind === 'continuous'
      ? this.continuousScale(this.zoom)
      : this.screenScale(this.screen, this.zoom);
  }

  get rowCount(): number {
    return this.rows.length;
  }

  get currentScreen(): number {
    return this.screen;
  }

  get stripPosition(): number {
    return this.strip.position;
  }

  get readingDirection(): ReadingDirection {
    return this.direction;
  }

  get spreadRows(): readonly Row[] {
    return this.rows;
  }
}
