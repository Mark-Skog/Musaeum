/**
 * Decides what to draw this frame and which tiles are still missing.
 *
 * This is where "zero blank frames" is guaranteed. The camera and the choice of what to draw are
 * made here, together, on the compositor's own thread, so there is no moment when a page has moved
 * but its content has not been mounted yet. Each visible page always gets a paper plate with its
 * exact geometry, then the best tiles available on top of it:
 *
 *   1. tiles at exactly the scale it is shown at, drawn pixel for pixel;
 *   2. otherwise, tiles of another scale, cut to fit and resampled by the GPU: a finer scale when
 *      zooming out, a coarser one when zooming in, the whole-page placeholder as a last resort.
 *      That looks soft for a moment, but correct;
 *   3. nothing, leaving the plate visible for a moment.
 *
 * Scales are not powers of two, so a tile of another scale rarely lines up with the grid of the
 * one wanted. Each visible tile is covered by whichever tiles of the fallback scale overlap it,
 * each cut to the overlap.
 */

import { TILE_SIZE } from '../protocol';
import type { TileCache } from './tile-cache';
import { clampScaleToPage, placeholderScale, quantizeScale, scaleFactor, tileCount } from './tiles';
import type { PlacedPages } from './view';

export interface QuadSink {
  plate(x: number, y: number, width: number, height: number): void;
  tile(
    x: number,
    y: number,
    width: number,
    height: number,
    u0: number,
    v0: number,
    u1: number,
    v1: number,
    layer: number,
  ): void;
}

export interface FrameCounters {
  /** Pages at least partly on screen. */
  visiblePages: number;
  /** Plates drawn. Anything other than `visiblePages` would be a blank-frame bug. */
  plates: number;
  /** Visible tile positions at the scale each page is shown at. */
  tilesVisible: number;
  /** ...drawn sharp, at exactly that scale. */
  exact: number;
  /** ...drawn from a tile of another scale. */
  fallback: number;
  /** ...with nothing available, so the plate shows through. */
  uncovered: number;
}

/** A tile the compositor would like, flattened so that planning allocates nothing. */
export class TileList {
  readonly pages: Uint32Array;
  readonly scales: Uint32Array;
  readonly columns: Uint32Array;
  readonly rows: Uint32Array;
  readonly priorities: Float32Array;
  length = 0;

  constructor(readonly capacity: number) {
    this.pages = new Uint32Array(capacity);
    this.scales = new Uint32Array(capacity);
    this.columns = new Uint32Array(capacity);
    this.rows = new Uint32Array(capacity);
    this.priorities = new Float32Array(capacity);
  }

  add(page: number, scale: number, x: number, y: number, priority: number): void {
    if (this.length < this.capacity) {
      const at = this.length++;
      this.pages[at] = page;
      this.scales[at] = scale;
      this.columns[at] = x;
      this.rows[at] = y;
      this.priorities[at] = priority;
    }
  }

  clear(): void {
    this.length = 0;
  }
}

/** Fallback scales further than this from the one wanted cost more quads than they are worth. */
const MAX_FINER_RATIO = 4;

/**
 * A page is drawn at exactly its tiles' scale, so that every texel lands on one pixel. At a reading
 * scale, quantizing moves it by a few hundredths of a percent, which nobody sees. A page whose
 * tiles are further off than a tenth of a percent is drawn at its own scale and resampled: one too
 * large for a grid at that scale, whose tiles had to be coarser, or a thumbnail so small that one
 * step of the scale is a visible part of it.
 */
function settledScale(pixelsPerPoint: number, target: number): number {
  const exact = scaleFactor(target);
  return Math.abs(exact - pixelsPerPoint) <= pixelsPerPoint * 1e-3 ? exact : pixelsPerPoint;
}

/** The scale tiles are wanted at for a page shown at `pixelsPerPoint`. */
export function wantedScale(pixelsPerPoint: number, widthPt: number, heightPt: number): number {
  return clampScaleToPage(quantizeScale(pixelsPerPoint), widthPt, heightPt);
}

export class FramePlanner {
  readonly counters: FrameCounters = {
    visiblePages: 0,
    plates: 0,
    tilesVisible: 0,
    exact: 0,
    fallback: 0,
    uncovered: 0,
  };
  /** Whole-page placeholders missing for visible pages. */
  readonly missingPlaceholders = new TileList(256);
  /** Visible pages, lowest first, for looking ahead from. */
  firstVisiblePage = 0;
  lastVisiblePage = -1;

  constructor(
    private readonly widths: Float32Array,
    private readonly heights: Float32Array,
    private readonly cache: TileCache,
  ) {}

  /** Plans one frame from where the pages are now. */
  plan(
    frame: number,
    placed: PlacedPages,
    viewWidth: number,
    viewHeight: number,
    sink: QuadSink,
  ): void {
    const { counters } = this;
    counters.visiblePages = 0;
    counters.plates = 0;
    counters.tilesVisible = 0;
    counters.exact = 0;
    counters.fallback = 0;
    counters.uncovered = 0;
    this.missingPlaceholders.clear();
    this.firstVisiblePage = Number.MAX_SAFE_INTEGER;
    this.lastVisiblePage = -1;

    for (let at = 0; at < placed.count; at++) {
      const page = placed.pages[at];
      const widthPt = this.widths[page];
      const heightPt = this.heights[page];
      const target = wantedScale(placed.scales[at], widthPt, heightPt);
      const scale = settledScale(placed.scales[at], target);
      // Whole device pixels keep text crisp, and every tile of a page derives its position from
      // the same rounded corner, so tiles can never crack apart.
      const pageX = Math.round(placed.x[at]);
      const pageY = Math.round(placed.y[at]);
      const width = widthPt * scale;
      const height = heightPt * scale;
      if (pageX >= viewWidth || pageY >= viewHeight || pageX + width <= 0 || pageY + height <= 0) {
        continue;
      }
      counters.visiblePages++;
      this.firstVisiblePage = Math.min(this.firstVisiblePage, page);
      this.lastVisiblePage = Math.max(this.lastVisiblePage, page);
      sink.plate(pageX, pageY, width, height);
      counters.plates++;

      const placeholder = Math.min(placeholderScale(widthPt, heightPt), target);
      if (this.cache.find(page, placeholder, 0, 0) < 0) {
        this.missingPlaceholders.add(page, placeholder, 0, 0, 0);
      }
      this.drawPage(frame, page, pageX, pageY, scale, target, viewWidth, viewHeight, sink);
    }
    if (this.lastVisiblePage < 0) {
      this.firstVisiblePage = 0;
    }
  }

  private drawPage(
    frame: number,
    page: number,
    pageX: number,
    pageY: number,
    scale: number,
    target: number,
    viewWidth: number,
    viewHeight: number,
    sink: QuadSink,
  ): void {
    const { counters } = this;
    const widthPt = this.widths[page];
    const heightPt = this.heights[page];
    // Points along one tile's edge at the wanted scale.
    const span = TILE_SIZE / scaleFactor(target);
    const tileDevice = span * scale;
    const columns = tileCount(widthPt, target);
    const rows = tileCount(heightPt, target);
    const firstColumn = Math.max(0, Math.floor(-pageX / tileDevice));
    const lastColumn = Math.min(columns - 1, Math.floor((viewWidth - 1 - pageX) / tileDevice));
    const firstRow = Math.max(0, Math.floor(-pageY / tileDevice));
    const lastRow = Math.min(rows - 1, Math.floor((viewHeight - 1 - pageY) / tileDevice));
    const scales = this.cache.scalesOf(page);

    for (let y = firstRow; y <= lastRow; y++) {
      const top = y * span;
      const bottom = Math.min(heightPt, top + span);
      for (let x = firstColumn; x <= lastColumn; x++) {
        const left = x * span;
        const right = Math.min(widthPt, left + span);
        counters.tilesVisible++;

        const layer = this.cache.find(page, target, x, y);
        if (layer >= 0) {
          this.cache.touch(layer, frame);
          sink.tile(
            pageX + left * scale,
            pageY + top * scale,
            (right - left) * scale,
            (bottom - top) * scale,
            0,
            0,
            (right - left) / span,
            (bottom - top) / span,
            layer,
          );
          counters.exact++;
          continue;
        }
        const covered = this.drawFallback(
          frame,
          page,
          scales,
          target,
          left,
          top,
          right,
          bottom,
          pageX,
          pageY,
          scale,
          sink,
        );
        if (covered) {
          counters.fallback++;
        } else {
          counters.uncovered++;
        }
      }
    }
  }

  /**
   * Covers the region `[left, right) × [top, bottom)` of a page, in points, from one other scale
   * that has every tile the region needs. Finer scales first, nearest first, then coarser ones.
   */
  private drawFallback(
    frame: number,
    page: number,
    scales: readonly number[],
    target: number,
    left: number,
    top: number,
    right: number,
    bottom: number,
    pageX: number,
    pageY: number,
    scale: number,
    sink: QuadSink,
  ): boolean {
    // Ascending, so finer ones are at the end: walk those first from the nearest, then the rest.
    let split = scales.length;
    while (split > 0 && scales[split - 1] > target) {
      split--;
    }
    for (let at = split; at < scales.length; at++) {
      if (scales[at] > target * MAX_FINER_RATIO) {
        break;
      }
      if (this.cover(frame, page, scales[at], left, top, right, bottom, pageX, pageY, scale, sink)) {
        return true;
      }
    }
    for (let at = split - 1; at >= 0; at--) {
      const source = scales[at];
      if (
        source !== target &&
        this.cover(frame, page, source, left, top, right, bottom, pageX, pageY, scale, sink)
      ) {
        return true;
      }
    }
    return false;
  }

  private cover(
    frame: number,
    page: number,
    source: number,
    left: number,
    top: number,
    right: number,
    bottom: number,
    pageX: number,
    pageY: number,
    scale: number,
    sink: QuadSink,
  ): boolean {
    const span = TILE_SIZE / scaleFactor(source);
    const lastColumn = tileCount(this.widths[page], source) - 1;
    const lastRow = tileCount(this.heights[page], source) - 1;
    const firstX = Math.min(lastColumn, Math.floor(left / span));
    const lastX = Math.min(lastColumn, Math.ceil(right / span) - 1);
    const firstY = Math.min(lastRow, Math.floor(top / span));
    const lastY = Math.min(lastRow, Math.ceil(bottom / span) - 1);
    // All or nothing: half a fallback would leave a hole that looks like a bug.
    for (let y = firstY; y <= lastY; y++) {
      for (let x = firstX; x <= lastX; x++) {
        if (this.cache.find(page, source, x, y) < 0) {
          return false;
        }
      }
    }
    for (let y = firstY; y <= lastY; y++) {
      for (let x = firstX; x <= lastX; x++) {
        const layer = this.cache.find(page, source, x, y);
        this.cache.touch(layer, frame);
        const cutLeft = Math.max(left, x * span);
        const cutTop = Math.max(top, y * span);
        const cutRight = Math.min(right, (x + 1) * span);
        const cutBottom = Math.min(bottom, (y + 1) * span);
        sink.tile(
          pageX + cutLeft * scale,
          pageY + cutTop * scale,
          (cutRight - cutLeft) * scale,
          (cutBottom - cutTop) * scale,
          (cutLeft - x * span) / span,
          (cutTop - y * span) / span,
          (cutRight - x * span) / span,
          (cutBottom - y * span) / span,
          layer,
        );
      }
    }
    return true;
  }

  /**
   * Lists the tiles at the wanted scale that `placed` would show and the cache does not have,
   * nearest the viewport's centre first. This is what gets rendered sharp.
   */
  collectWanted(placed: PlacedPages, viewWidth: number, viewHeight: number, out: TileList): void {
    const centreX = viewWidth / 2;
    const centreY = viewHeight / 2;
    for (let at = 0; at < placed.count; at++) {
      const page = placed.pages[at];
      const scale = placed.scales[at];
      const widthPt = this.widths[page];
      const heightPt = this.heights[page];
      const pageX = Math.round(placed.x[at]);
      const pageY = Math.round(placed.y[at]);
      const target = wantedScale(scale, widthPt, heightPt);
      const span = TILE_SIZE / scaleFactor(target);
      const tileDevice = span * scale;
      const columns = tileCount(widthPt, target);
      const rows = tileCount(heightPt, target);
      const firstColumn = Math.max(0, Math.floor(-pageX / tileDevice));
      const lastColumn = Math.min(columns - 1, Math.floor((viewWidth - 1 - pageX) / tileDevice));
      const firstRow = Math.max(0, Math.floor(-pageY / tileDevice));
      const lastRow = Math.min(rows - 1, Math.floor((viewHeight - 1 - pageY) / tileDevice));
      for (let y = firstRow; y <= lastRow; y++) {
        for (let x = firstColumn; x <= lastColumn; x++) {
          if (this.cache.find(page, target, x, y) >= 0) {
            continue;
          }
          const middleX = pageX + (x + 0.5) * tileDevice;
          const middleY = pageY + (y + 0.5) * tileDevice;
          out.add(page, target, x, y, Math.abs(middleX - centreX) + Math.abs(middleY - centreY));
        }
      }
    }
  }
}
