/**
 * Decides what to draw this frame and which tiles are still missing.
 *
 * This is where "zero blank frames" is guaranteed. The scroll position and the choice of
 * what to draw are made here, together, on the compositor's own thread. So unlike a
 * virtualized DOM, there is no moment when the page has moved but its content has not
 * been mounted yet. Each visible page always gets a paper plate with its exact geometry,
 * then the best tiles available on top of it:
 *
 *   1. the tile at the right level of detail, or
 *   2. the matching part of a coarser tile, which looks soft but correct, or
 *   3. nothing, leaving the plate visible for a moment.
 */

import {
  type DocumentLayout,
  lodForScale,
  pageAt,
  tileCount,
  tileKey,
  tileSpanPt,
  wholePageLod,
} from './layout';
import type { TileCache } from './tile-cache';

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
  /** Visible tile positions at the target level of detail. */
  tilesVisible: number;
  /** ...drawn sharp. */
  exact: number;
  /** ...drawn from a coarser tile. */
  fallback: number;
  /** ...with nothing available, so the plate shows through. */
  uncovered: number;
}

/** A tile the compositor would like, flattened so that planning allocates nothing. */
export class TileList {
  readonly pages: Uint32Array;
  readonly lods: Int32Array;
  readonly columns: Uint32Array;
  readonly rows: Uint32Array;
  readonly priorities: Float32Array;
  length = 0;

  constructor(readonly capacity: number) {
    this.pages = new Uint32Array(capacity);
    this.lods = new Int32Array(capacity);
    this.columns = new Uint32Array(capacity);
    this.rows = new Uint32Array(capacity);
    this.priorities = new Float32Array(capacity);
  }

  add(page: number, lod: number, x: number, y: number, priority: number): void {
    if (this.length < this.capacity) {
      const at = this.length++;
      this.pages[at] = page;
      this.lods[at] = lod;
      this.columns[at] = x;
      this.rows[at] = y;
      this.priorities[at] = priority;
    }
  }

  clear(): void {
    this.length = 0;
  }
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
  /** Sharp tiles missing for what is on screen, nearest the screen centre first. */
  readonly missingSharp = new TileList(2048);
  firstVisiblePage = 0;
  lastVisiblePage = -1;

  constructor(
    private readonly layout: DocumentLayout,
    private readonly cache: TileCache,
  ) {}

  /** Plans one frame. `scrollPt` is the column's position at the top of the viewport. */
  plan(
    frame: number,
    scrollPt: number,
    scale: number,
    viewWidth: number,
    viewHeight: number,
    sink: QuadSink,
  ): void {
    const { layout, cache, counters } = this;
    counters.visiblePages = 0;
    counters.plates = 0;
    counters.tilesVisible = 0;
    counters.exact = 0;
    counters.fallback = 0;
    counters.uncovered = 0;
    this.missingPlaceholders.clear();
    this.missingSharp.clear();
    this.lastVisiblePage = -1;
    if (layout.pageCount === 0) {
      return;
    }

    const targetLod = lodForScale(scale);
    const span = tileSpanPt(targetLod);
    const tileDevice = span * scale;
    const viewBottom = scrollPt + viewHeight / scale;
    const centreX = viewWidth / 2;
    const centreY = viewHeight / 2;

    this.firstVisiblePage = pageAt(layout, scrollPt);
    for (let page = this.firstVisiblePage; page < layout.pageCount; page++) {
      const top = layout.tops[page];
      if (top >= viewBottom) {
        break;
      }
      const widthPt = layout.widths[page];
      const heightPt = layout.heights[page];
      if (top + heightPt <= scrollPt) {
        continue; // Only the gap below this page is on screen.
      }
      this.lastVisiblePage = page;
      counters.visiblePages++;

      // Whole device pixels keep text crisp, and the position is always derived from the
      // same expression, so tiles of the same page can never crack apart.
      const pageX = Math.round((viewWidth - widthPt * scale) / 2);
      const pageY = Math.round((top - scrollPt) * scale);
      sink.plate(pageX, pageY, widthPt * scale, heightPt * scale);
      counters.plates++;

      const placeholderLod = Math.min(wholePageLod(widthPt, heightPt), targetLod);
      const placeholderLayer = cache.find(tileKey(page, placeholderLod, 0, 0));
      if (placeholderLayer >= 0) {
        cache.touch(placeholderLayer, frame);
      } else {
        this.missingPlaceholders.add(page, placeholderLod, 0, 0, 0);
      }

      const columns = tileCount(widthPt, targetLod);
      const rows = tileCount(heightPt, targetLod);
      const firstColumn = Math.max(0, Math.floor(-pageX / tileDevice));
      const lastColumn = Math.min(columns - 1, Math.floor((viewWidth - 1 - pageX) / tileDevice));
      const firstRow = Math.max(0, Math.floor(-pageY / tileDevice));
      const lastRow = Math.min(rows - 1, Math.floor((viewHeight - 1 - pageY) / tileDevice));

      for (let y = firstRow; y <= lastRow; y++) {
        // The last row and column usually cover only part of a tile.
        const tileHeightPt = Math.min(span, heightPt - y * span);
        for (let x = firstColumn; x <= lastColumn; x++) {
          const tileWidthPt = Math.min(span, widthPt - x * span);
          const deviceX = pageX + x * tileDevice;
          const deviceY = pageY + y * tileDevice;
          const deviceWidth = tileWidthPt * scale;
          const deviceHeight = tileHeightPt * scale;
          counters.tilesVisible++;

          const layer = cache.find(tileKey(page, targetLod, x, y));
          if (layer >= 0) {
            cache.touch(layer, frame);
            sink.tile(
              deviceX,
              deviceY,
              deviceWidth,
              deviceHeight,
              0,
              0,
              tileWidthPt / span,
              tileHeightPt / span,
              layer,
            );
            counters.exact++;
            continue;
          }

          const distance =
            Math.abs(deviceX + deviceWidth / 2 - centreX) +
            Math.abs(deviceY + deviceHeight / 2 - centreY);
          this.missingSharp.add(page, targetLod, x, y, distance);

          if (
            this.drawFromCoarserTile(
              frame,
              page,
              targetLod,
              placeholderLod,
              x,
              y,
              span,
              tileWidthPt,
              tileHeightPt,
              deviceX,
              deviceY,
              deviceWidth,
              deviceHeight,
              sink,
            )
          ) {
            counters.fallback++;
          } else {
            counters.uncovered++;
          }
        }
      }
    }
  }

  private drawFromCoarserTile(
    frame: number,
    page: number,
    targetLod: number,
    coarsestLod: number,
    x: number,
    y: number,
    span: number,
    tileWidthPt: number,
    tileHeightPt: number,
    deviceX: number,
    deviceY: number,
    deviceWidth: number,
    deviceHeight: number,
    sink: QuadSink,
  ): boolean {
    for (let lod = targetLod - 1; lod >= coarsestLod; lod--) {
      const levels = targetLod - lod;
      const parentX = x >> levels;
      const parentY = y >> levels;
      const layer = this.cache.find(tileKey(page, lod, parentX, parentY));
      if (layer < 0) {
        continue;
      }
      this.cache.touch(layer, frame);
      const parentSpan = span * 2 ** levels;
      const u0 = (x * span - parentX * parentSpan) / parentSpan;
      const v0 = (y * span - parentY * parentSpan) / parentSpan;
      sink.tile(
        deviceX,
        deviceY,
        deviceWidth,
        deviceHeight,
        u0,
        v0,
        u0 + tileWidthPt / parentSpan,
        v0 + tileHeightPt / parentSpan,
        layer,
      );
      return true;
    }
    return false;
  }
}
