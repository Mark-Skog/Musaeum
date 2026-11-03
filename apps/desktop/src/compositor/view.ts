/**
 * Where rows of pages go on screen in each reading mode, and the Locator that ties them together.
 *
 * - **Continuous:** rows one below the other, all at one physical scale. The scale is limited by
 *   the widest row, so a landscape page is never cut off, and the space a narrower row leaves
 *   becomes margin. Zooming past that is the reader's choice, and pans sideways.
 * - **Paginated:** one row at a time, fitted to the viewport whole, without cropping or distortion.
 *   Each row has its own scale, so rows of different proportions each fill what they can. Rows sit
 *   side by side in a strip, one viewport apart, which is what the page turn slides along.
 *
 * A Locator is a page and a point on it as fractions of its width and height: the point at the
 * viewport's top-left corner. It means the same in either mode, at any zoom and any window size.
 */

import type { Row } from './spreads';
import { BLANK } from './spreads';

export interface Locator {
  page: number;
  u: number;
  v: number;
}

/** The viewport's size, in device pixels. */
export interface Viewport {
  width: number;
  height: number;
}

/** A pan, in device pixels from a row's top-left corner. */
export interface Pan {
  x: number;
  y: number;
}

/** Pages placed for one frame: top-left corner in device pixels, and device pixels per point. */
export class PlacedPages {
  count = 0;
  readonly pages: Int32Array;
  readonly x: Float64Array;
  readonly y: Float64Array;
  readonly scales: Float64Array;

  constructor(readonly capacity = 64) {
    this.pages = new Int32Array(capacity);
    this.x = new Float64Array(capacity);
    this.y = new Float64Array(capacity);
    this.scales = new Float64Array(capacity);
  }

  add(page: number, x: number, y: number, scale: number): void {
    if (this.count < this.capacity) {
      const at = this.count++;
      this.pages[at] = page;
      this.x[at] = x;
      this.y[at] = y;
      this.scales[at] = scale;
    }
  }

  clear(): void {
    this.count = 0;
  }
}

const clamp01 = (value: number): number => Math.min(1, Math.max(0, value));

/** Where each slot of a row sits inside it, in points: side by side, centred vertically. */
function slotOffsets(row: Row): { x: number[]; y: number[] } {
  const x: number[] = [];
  const y: number[] = [];
  let left = 0;
  for (let slot = 0; slot < row.slots.length; slot++) {
    x.push(left);
    y.push((row.heightPt - row.slotHeights[slot]) / 2);
    left += row.slotWidths[slot];
  }
  return { x, y };
}

/** Rows in a column, all at one scale. Positions are in points down the column. */
export class ContinuousView {
  readonly tops: Float64Array;
  readonly totalHeightPt: number;
  readonly maxRowWidthPt: number;
  private readonly offsets: { x: number[]; y: number[] }[];
  private readonly rowOfPage: Int32Array;
  private readonly slotOfPage: Int32Array;

  constructor(
    readonly rows: Row[],
    private readonly widths: Float32Array,
    private readonly heights: Float32Array,
    readonly gapPt: number,
  ) {
    this.tops = new Float64Array(rows.length);
    this.rowOfPage = new Int32Array(widths.length);
    this.slotOfPage = new Int32Array(widths.length);
    let y = 0;
    let widest = 0;
    rows.forEach((row, index) => {
      this.tops[index] = y;
      y += row.heightPt + gapPt;
      widest = Math.max(widest, row.widthPt);
      row.slots.forEach((page, slot) => {
        if (page !== BLANK) {
          this.rowOfPage[page] = index;
          this.slotOfPage[page] = slot;
        }
      });
    });
    this.totalHeightPt = rows.length > 0 ? y - gapPt : 0;
    this.maxRowWidthPt = widest;
    this.offsets = rows.map(slotOffsets);
  }

  /**
   * One physical scale for every row. `comfortable` is the scale reading starts at on a large
   * window, and the widest row always fits at a zoom of 1.
   */
  scaleFor(view: Viewport, comfortable: number, zoom: number, marginPx: number): number {
    const fitsWidth = Math.max(1, view.width - 2 * marginPx) / Math.max(this.maxRowWidthPt, 1);
    return zoom * Math.min(comfortable, fitsWidth);
  }

  /** The last row whose top is at or above `yPt`. */
  rowAt(yPt: number): number {
    let low = 0;
    let high = this.rows.length - 1;
    while (low < high) {
      const middle = (low + high + 1) >>> 1;
      if (this.tops[middle] <= yPt) {
        low = middle;
      } else {
        high = middle - 1;
      }
    }
    return low;
  }

  /** Whether the column is wider than the viewport, which is when it pans sideways. */
  overflows(scale: number, view: Viewport): boolean {
    return this.maxRowWidthPt * scale > view.width;
  }

  /** How far the column's left edge is from the viewport's, in device pixels. */
  private columnLeft(leftPt: number, scale: number, view: Viewport): number {
    const width = this.maxRowWidthPt * scale;
    return width <= view.width ? (view.width - width) / 2 : -leftPt * scale;
  }

  /** The most `leftPt` can be: none unless the column is wider than the viewport. */
  maxLeftPt(scale: number, view: Viewport): number {
    return Math.max(0, this.maxRowWidthPt - view.width / scale);
  }

  maxTopPt(scale: number, view: Viewport): number {
    return Math.max(0, this.totalHeightPt - view.height / scale);
  }

  /** Places what the viewport shows with its top at `topPt` and its left at `leftPt`. */
  place(topPt: number, leftPt: number, scale: number, view: Viewport, out: PlacedPages): void {
    if (this.rows.length === 0) {
      return;
    }
    const columnLeft = this.columnLeft(leftPt, scale, view);
    const bottomPt = topPt + view.height / scale;
    for (let index = this.rowAt(topPt); index < this.rows.length; index++) {
      const row = this.rows[index];
      if (this.tops[index] >= bottomPt) {
        break;
      }
      if (this.tops[index] + row.heightPt <= topPt) {
        continue; // Only the gap below this row is on screen.
      }
      const rowLeft = columnLeft + ((this.maxRowWidthPt - row.widthPt) / 2) * scale;
      const offsets = this.offsets[index];
      row.slots.forEach((page, slot) => {
        if (page !== BLANK) {
          const y = (this.tops[index] + offsets.y[slot] - topPt) * scale;
          out.add(page, rowLeft + offsets.x[slot] * scale, y, scale);
        }
      });
    }
  }

  /** Top and left of a page, in points, in the column's coordinates. */
  private pageOrigin(page: number): { x: number; y: number } {
    const index = this.rowOfPage[page];
    const row = this.rows[index];
    const slot = this.slotOfPage[page];
    const offsets = this.offsets[index];
    const rowLeftPt = (this.maxRowWidthPt - row.widthPt) / 2;
    return { x: rowLeftPt + offsets.x[slot], y: this.tops[index] + offsets.y[slot] };
  }

  private clampPage(page: number): number {
    return Math.min(Math.max(0, page), this.rowOfPage.length - 1);
  }

  /** The Locator of the point at the viewport's top-left corner. */
  locatorAt(topPt: number, leftPt: number, scale: number, view: Viewport): Locator {
    if (this.rows.length === 0) {
      return { page: 0, u: 0, v: 0 };
    }
    let index = this.rowAt(topPt);
    // In the gap below a row, the reader is looking at the next one.
    if (this.tops[index] + this.rows[index].heightPt <= topPt && index + 1 < this.rows.length) {
      index++;
    }
    const page = this.rows[index].pages[0];
    const origin = this.pageOrigin(page);
    // A column narrower than the viewport is centred, and then the left edge is the page's own.
    const left = this.overflows(scale, view) ? leftPt : origin.x;
    return {
      page,
      u: clamp01((left - origin.x) / this.widths[page]),
      v: clamp01((topPt - origin.y) / this.heights[page]),
    };
  }

  /** A Locator's point, in points across and down the column, wherever the viewport is. */
  pointOf(locator: Locator): { xPt: number; yPt: number } {
    const page = this.clampPage(locator.page);
    const origin = this.pageOrigin(page);
    return {
      xPt: origin.x + clamp01(locator.u) * this.widths[page],
      yPt: origin.y + clamp01(locator.v) * this.heights[page],
    };
  }

  /** Where the viewport goes to show a Locator at its top-left corner. */
  positionOf(locator: Locator, scale: number, view: Viewport): { topPt: number; leftPt: number } {
    const point = this.pointOf(locator);
    return {
      topPt: Math.min(this.maxTopPt(scale, view), Math.max(0, point.yPt)),
      leftPt: Math.min(this.maxLeftPt(scale, view), Math.max(0, point.xPt)),
    };
  }
}

/** One row at a time, each fitted to the viewport, side by side in a strip. */
export class PaginatedView {
  private readonly offsets: { x: number[]; y: number[] }[];
  private readonly rowOfPage: Int32Array;

  constructor(
    readonly rows: Row[],
    private readonly widths: Float32Array,
    private readonly heights: Float32Array,
  ) {
    this.offsets = rows.map(slotOffsets);
    this.rowOfPage = new Int32Array(widths.length);
    rows.forEach((row, index) => {
      for (const page of row.pages) {
        this.rowOfPage[page] = index;
      }
    });
  }

  screenOf(page: number): number {
    return this.rowOfPage[Math.min(Math.max(0, page), this.rowOfPage.length - 1)];
  }

  /** A row's scale: the whole row fits inside the margins, times the zoom. */
  scaleOf(screen: number, view: Viewport, marginPx: number, zoom: number): number {
    const row = this.rows[screen];
    const width = Math.max(1, view.width - 2 * marginPx) / Math.max(row.widthPt, 1);
    const height = Math.max(1, view.height - 2 * marginPx) / Math.max(row.heightPt, 1);
    return zoom * Math.min(width, height);
  }

  /** How far a zoomed row can pan, in device pixels, along each axis. */
  panRange(screen: number, scale: number, view: Viewport): Pan {
    const row = this.rows[screen];
    return {
      x: Math.max(0, row.widthPt * scale - view.width),
      y: Math.max(0, row.heightPt * scale - view.height),
    };
  }

  /**
   * Places a row whose slot in the strip starts `slotLeft` device pixels from the viewport's left
   * edge. A row smaller than the viewport is centred in it; a larger one is offset by `pan`.
   */
  placeScreen(
    screen: number,
    slotLeft: number,
    scale: number,
    pan: Pan,
    view: Viewport,
    out: PlacedPages,
  ): void {
    const row = this.rows[screen];
    const width = row.widthPt * scale;
    const height = row.heightPt * scale;
    const left = slotLeft + (width <= view.width ? (view.width - width) / 2 : -pan.x);
    const top = height <= view.height ? (view.height - height) / 2 : -pan.y;
    const offsets = this.offsets[screen];
    row.slots.forEach((page, slot) => {
      if (page !== BLANK) {
        out.add(page, left + offsets.x[slot] * scale, top + offsets.y[slot] * scale, scale);
      }
    });
  }

  /** The Locator at the viewport's top-left corner, for a row panned by `pan`. */
  locatorAt(screen: number, scale: number, pan: Pan, view: Viewport): Locator {
    const row = this.rows[screen];
    const page = row.pages[0];
    const slot = row.slots.indexOf(page);
    const offsets = this.offsets[screen];
    const left = row.widthPt * scale <= view.width ? 0 : pan.x / scale;
    const top = row.heightPt * scale <= view.height ? 0 : pan.y / scale;
    return {
      page,
      u: clamp01((left - offsets.x[slot]) / this.widths[page]),
      v: clamp01((top - offsets.y[slot]) / this.heights[page]),
    };
  }

  /** A Locator's point on its row, in device pixels from the row's top-left corner. */
  pointOf(locator: Locator, scale: number): Pan {
    const screen = this.screenOf(locator.page);
    const row = this.rows[screen];
    const slot = Math.max(0, row.slots.indexOf(locator.page));
    const offsets = this.offsets[screen];
    return {
      x: (offsets.x[slot] + clamp01(locator.u) * this.widths[locator.page]) * scale,
      y: (offsets.y[slot] + clamp01(locator.v) * this.heights[locator.page]) * scale,
    };
  }

  /** The pan that puts a Locator at the viewport's top-left corner, as far as the row allows. */
  panFor(locator: Locator, scale: number, view: Viewport): Pan {
    const range = this.panRange(this.screenOf(locator.page), scale, view);
    const point = this.pointOf(locator, scale);
    return {
      x: Math.min(range.x, Math.max(0, point.x)),
      y: Math.min(range.y, Math.max(0, point.y)),
    };
  }
}
