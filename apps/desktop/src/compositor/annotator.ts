/**
 * Text selection and annotation marks on a fixed layout, inside the compositor.
 *
 * Selecting happens here, on the compositor's thread, against each visible page's characters: a
 * drag that starts on a line of text selects (by character; by word after a double click, by line
 * after a triple one and by paragraph after a fourth), and one that starts anywhere else pans, as
 * before. A selection can run across pages, each page a segment of it. Marks and the selection are
 * drawn in the same frame as the tiles under them, so they never lag behind a scroll or a zoom.
 */

import {
  type CharPage,
  decodeChars,
  hitTest,
  lineAt,
  lineQuads,
  paragraphAt,
  textOf,
  toShown,
  wordAt,
} from './chars';
import type { PlacedPages } from './view';

/** A mark as the main page describes it: one annotation's stretch of one page. */
export interface FixedMark {
  id: string;
  /** The palette's name for it. */
  color: string;
  kind: 'highlight' | 'underline' | 'note';
  /** Whether it has a note, which a small square at its start shows. */
  note: boolean;
  page: number;
  quads: number[][];
}

/** Characters `start..end` of a page. */
export interface PageSpan {
  page: number;
  start: number;
  end: number;
}

export type AnnotatorMessage =
  | { type: 'need-chars'; pages: number[] }
  | {
      type: 'selection';
      spans: PageSpan[];
      text: string;
      /** Around the selection's first line, in device pixels of the canvas. */
      rect: { x: number; y: number; width: number; height: number } | null;
    }
  | { type: 'mark-click'; id: string; rect: { x: number; y: number; width: number; height: number } }
  /** What the pointer is over: text to select, a mark to open, or neither. */
  | { type: 'cursor'; cursor: 'text' | 'mark' | 'default' };

type Granularity = 'char' | 'word' | 'line' | 'paragraph';

interface Caret {
  page: number;
  index: number;
}

/** Highlighter colors over white paper, as 0–1 channels. */
export const PALETTE: Record<string, [number, number, number]> = {
  yellow: [1.0, 0.89, 0.42],
  green: [0.66, 0.9, 0.63],
  blue: [0.6, 0.79, 0.99],
  pink: [0.98, 0.68, 0.84],
  purple: [0.78, 0.71, 0.99],
};
/** What an underline in each color is drawn with: the same hue, dark enough to read as a line. */
const INK: Record<string, [number, number, number]> = {
  yellow: [0.85, 0.6, 0.0],
  green: [0.13, 0.6, 0.2],
  blue: [0.1, 0.4, 0.85],
  pink: [0.85, 0.2, 0.5],
  purple: [0.45, 0.25, 0.85],
};
/**
 * Over a dark page the renderer takes the maximum with a color instead of the minimum, so a
 * highlight is its color dimmed (`DARK_FILL`), and a line or a note tab, which on light paper is
 * dark ink, is the bright fill color.
 */
export const DARK_FILL = 0.55;
const dimmed = (color: readonly [number, number, number]): [number, number, number] => [
  color[0] * DARK_FILL,
  color[1] * DARK_FILL,
  color[2] * DARK_FILL,
];
const SELECTION: [number, number, number] = [0.7, 0.83, 1.0];
const SELECTION_DARK: [number, number, number] = [0.25, 0.42, 0.75];
/** Two presses this close in time and place are one double click. */
const MULTI_CLICK_MS = 450;
const MULTI_CLICK_PX = 8;
/** A press that moves less than this is a click, not a drag. */
const DRAG_PX = 4;
/**
 * How long a page has to stay on the screen before its characters are asked for. Asking goes ahead
 * of every tile in the render pool, so pages that only fly past while scrolling are not asked about.
 */
const SETTLED_MS = 300;

const order = (a: Caret, b: Caret): number => a.page - b.page || a.index - b.index;

export class Annotator {
  private readonly pages = new Map<number, CharPage>();
  private readonly asked = new Set<number>();
  /** Pages on the screen without characters yet, and since when. */
  private readonly onScreenSince = new Map<number, number>();
  private readonly marksByPage = new Map<number, FixedMark[]>();
  /** The selection as it started (a word, a line…) and as it is now. */
  private anchor: { start: Caret; end: Caret } | null = null;
  private focus: { start: Caret; end: Caret } | null = null;
  private granularity: Granularity = 'char';
  private pressing = false;
  private moved = false;
  private down = { x: 0, y: 0, time: -Infinity, clicks: 0 };
  /** A mark pressed on, which a click without a drag opens. */
  private pressedMark: FixedMark | null = null;

  constructor(
    private readonly widths: Float32Array,
    private readonly heights: Float32Array,
    private readonly post: (message: AnnotatorMessage) => void,
  ) {}

  /** Asks for the characters of pages that have stayed on the screen a moment and have none yet. */
  want(placed: PlacedPages, now: number): void {
    const missing: number[] = [];
    const shown = new Set<number>();
    for (let at = 0; at < placed.count; at++) {
      const page = placed.pages[at];
      shown.add(page);
      if (this.pages.has(page) || this.asked.has(page)) {
        continue;
      }
      const since = this.onScreenSince.get(page) ?? now;
      this.onScreenSince.set(page, since);
      if (now - since >= SETTLED_MS) {
        this.ask(page, missing);
      }
    }
    for (const page of this.onScreenSince.keys()) {
      if (!shown.has(page)) {
        this.onScreenSince.delete(page);
      }
    }
    if (missing.length > 0) {
      this.post({ type: 'need-chars', pages: missing });
    }
  }

  private ask(page: number, into: number[]): void {
    this.asked.add(page);
    this.onScreenSince.delete(page);
    into.push(page);
  }

  /** A page's characters, or why there are none: an empty layer stands for a page without text. */
  setChars(page: number, buffer: ArrayBuffer | null): void {
    if (buffer === null) {
      this.pages.set(page, decodeEmpty());
      return;
    }
    try {
      this.pages.set(page, decodeChars(buffer));
    } catch (error) {
      console.warn(`page ${page}'s characters were refused`, error);
      this.pages.set(page, decodeEmpty());
    }
  }

  /** A point of a page as stored, as a point of the page as shown. */
  shownPoint(page: number, u: number, v: number): [number, number] {
    return toShown(u, v, this.pages.get(page)?.rotation ?? 0);
  }

  setMarks(marks: FixedMark[]): void {
    this.marksByPage.clear();
    for (const mark of marks) {
      const list = this.marksByPage.get(mark.page) ?? [];
      list.push(mark);
      this.marksByPage.set(mark.page, list);
    }
  }

  /** Where a point of the canvas is on a page, in the page's stored frame. */
  private pageAt(x: number, y: number, placed: PlacedPages, nearest: boolean): { page: number; u: number; v: number } | null {
    let best: { page: number; u: number; v: number } | null = null;
    let bestDistance = Infinity;
    for (let at = 0; at < placed.count; at++) {
      const page = placed.pages[at];
      const scale = placed.scales[at];
      const width = this.widths[page] * scale;
      const height = this.heights[page] * scale;
      const left = placed.x[at];
      const top = placed.y[at];
      const dx = x < left ? left - x : x > left + width ? x - left - width : 0;
      const dy = y < top ? top - y : y > top + height ? y - top - height : 0;
      const distance = dx + dy;
      if (distance < bestDistance && (nearest || distance === 0)) {
        bestDistance = distance;
        const layer = this.pages.get(page);
        const shownX = Math.min(1, Math.max(0, (x - left) / width));
        const shownY = Math.min(1, Math.max(0, (y - top) / height));
        const [u, v] = layer === undefined ? [shownX, shownY] : storedOf(shownX, shownY, layer.rotation);
        best = { page, u, v };
      }
    }
    return best;
  }

  /** The stretch a point selects at the current granularity. */
  private stretch(page: number, layer: CharPage, u: number, v: number): { start: Caret; end: Caret } | null {
    const hit = hitTest(layer, u, v);
    if (hit === null) {
      return null;
    }
    let range: [number, number];
    switch (this.granularity) {
      case 'word':
        range = wordAt(layer, hit.index);
        break;
      case 'line':
        range = lineAt(layer, hit.index);
        break;
      case 'paragraph':
        range = paragraphAt(layer, hit.index);
        break;
      default:
        range = [hit.caret, hit.caret];
    }
    return { start: { page, index: range[0] }, end: { page, index: range[1] } };
  }

  private cursor: 'text' | 'mark' | 'default' = 'default';

  /** The pointer moved with no button down: tells the main page when what is under it changes. */
  hover(x: number, y: number, placed: PlacedPages): void {
    const at = this.pageAt(x, y, placed, false);
    const layer = at === null ? undefined : this.pages.get(at.page);
    const hit = at === null || layer === undefined ? null : hitTest(layer, at.u, at.v);
    const cursor =
      at !== null && hit?.onText === true ? (this.markAt(at.page, at.u, at.v) !== null ? 'mark' : 'text') : 'default';
    if (cursor !== this.cursor) {
      this.cursor = cursor;
      this.post({ type: 'cursor', cursor });
    }
  }

  /** A press. Returns whether it starts a selection; if not, the reader pans as before. */
  pointerDown(x: number, y: number, now: number, placed: PlacedPages): boolean {
    const near = Math.hypot(x - this.down.x, y - this.down.y) <= MULTI_CLICK_PX;
    const clicks = near && now - this.down.time <= MULTI_CLICK_MS ? this.down.clicks + 1 : 1;
    this.down = { x, y, time: now, clicks };
    this.moved = false;
    this.pressedMark = null;
    const at = this.pageAt(x, y, placed, false);
    const layer = at === null ? undefined : this.pages.get(at.page);
    if (at !== null && layer === undefined && !this.asked.has(at.page)) {
      // Pressed on a page whose characters were not asked for yet: now they are, for the next press.
      const missing: number[] = [];
      this.ask(at.page, missing);
      this.post({ type: 'need-chars', pages: missing });
    }
    const hit = at === null || layer === undefined ? null : hitTest(layer, at.u, at.v);
    if (at === null || layer === undefined || hit === null || !hit.onText) {
      // Not on text: whatever was selected goes, and the press is the reader's.
      this.clear(true);
      return false;
    }
    this.pressedMark = this.markAt(at.page, at.u, at.v);
    this.granularity = (['char', 'word', 'line', 'paragraph'] as const)[Math.min(clicks, 4) - 1];
    this.anchor = this.stretch(at.page, layer, at.u, at.v);
    this.focus = this.anchor;
    this.pressing = true;
    return true;
  }

  pointerMove(x: number, y: number, placed: PlacedPages): boolean {
    if (!this.pressing) {
      return false;
    }
    if (!this.moved && Math.hypot(x - this.down.x, y - this.down.y) < DRAG_PX) {
      return true;
    }
    this.moved = true;
    const at = this.pageAt(x, y, placed, true);
    const layer = at === null ? undefined : this.pages.get(at.page);
    if (at !== null && layer !== undefined) {
      this.focus = this.stretch(at.page, layer, at.u, at.v) ?? this.focus;
    }
    return true;
  }

  /** The end of a press. Returns whether it was this annotator's. */
  pointerUp(placed: PlacedPages): boolean {
    if (!this.pressing) {
      return false;
    }
    this.pressing = false;
    if (!this.moved && this.granularity === 'char') {
      // A click on text: a mark under it opens; otherwise any selection goes.
      const mark = this.pressedMark;
      this.clear(true);
      if (mark !== null) {
        this.post({ type: 'mark-click', id: mark.id, rect: this.boundsOf(mark.page, mark.quads, placed) ?? { x: 0, y: 0, width: 0, height: 0 } });
      }
      return true;
    }
    this.report(placed);
    return true;
  }

  get selecting(): boolean {
    return this.pressing;
  }

  /** The pages whose characters are here. */
  get charPages(): number[] {
    return [...this.pages.keys()].sort((a, b) => a - b);
  }

  /** How many marks there are, on every page. */
  get markCount(): number {
    let count = 0;
    for (const marks of this.marksByPage.values()) {
      count += marks.length;
    }
    return count;
  }

  /** The selection, in order, as one span per page. */
  spans(): PageSpan[] {
    if (this.anchor === null || this.focus === null) {
      return [];
    }
    const starts = [this.anchor.start, this.focus.start].sort(order);
    const ends = [this.anchor.end, this.focus.end].sort(order);
    const start = starts[0];
    const end = ends[1];
    if (order(start, end) >= 0) {
      return [];
    }
    const spans: PageSpan[] = [];
    for (let page = start.page; page <= end.page; page++) {
      const layer = this.pages.get(page);
      const count = layer?.count ?? 0;
      const from = page === start.page ? start.index : 0;
      const to = page === end.page ? end.index : count;
      if (to > from) {
        spans.push({ page, start: from, end: Math.min(to, count) });
      }
    }
    return spans.filter((span) => span.end > span.start);
  }

  private report(placed: PlacedPages): void {
    const spans = this.spans();
    if (spans.length === 0) {
      this.clear(true);
      return;
    }
    const text = spans
      .map((span) => {
        const layer = this.pages.get(span.page);
        return layer === undefined ? '' : textOf(layer, span.start, span.end);
      })
      .join('\n');
    const first = spans[0];
    const layer = this.pages.get(first.page);
    const quads = layer === undefined ? [] : lineQuads(layer, first.start, first.end);
    this.post({ type: 'selection', spans, text, rect: this.boundsOf(first.page, quads.slice(0, 1), placed) });
  }

  /** Forgets the selection, telling the main page if there was one. */
  clear(tell: boolean): void {
    const had = this.anchor !== null;
    this.anchor = null;
    this.focus = null;
    this.pressing = false;
    if (had && tell) {
      this.post({ type: 'selection', spans: [], text: '', rect: null });
    }
  }

  private markAt(page: number, u: number, v: number): FixedMark | null {
    for (const mark of this.marksByPage.get(page) ?? []) {
      for (const quad of mark.quads) {
        const left = Math.min(quad[0], quad[6]);
        const right = Math.max(quad[2], quad[4]);
        const top = Math.min(quad[1], quad[3]);
        const bottom = Math.max(quad[5], quad[7]);
        if (u >= left && u <= right && v >= top && v <= bottom) {
          return mark;
        }
      }
    }
    return null;
  }

  /** The box around quads of a page, in device pixels of the canvas, if the page is on screen. */
  private boundsOf(page: number, quads: number[][], placed: PlacedPages): { x: number; y: number; width: number; height: number } | null {
    for (let at = 0; at < placed.count; at++) {
      if (placed.pages[at] !== page) {
        continue;
      }
      const rotation = this.pages.get(page)?.rotation ?? 0;
      const width = this.widths[page] * placed.scales[at];
      const height = this.heights[page] * placed.scales[at];
      let left = Infinity;
      let top = Infinity;
      let right = -Infinity;
      let bottom = -Infinity;
      for (const quad of quads) {
        for (let corner = 0; corner < 4; corner++) {
          const [sx, sy] = toShown(quad[corner * 2], quad[corner * 2 + 1], rotation);
          const x = placed.x[at] + sx * width;
          const y = placed.y[at] + sy * height;
          left = Math.min(left, x);
          right = Math.max(right, x);
          top = Math.min(top, y);
          bottom = Math.max(bottom, y);
        }
      }
      return Number.isFinite(left) ? { x: left, y: top, width: right - left, height: bottom - top } : null;
    }
    return null;
  }

  /** Pushes this frame's marks and selection, over the pages as they are placed. */
  draw(sink: { mark(corners: ArrayLike<number>, light: ArrayLike<number>, dark: ArrayLike<number>): void }, placed: PlacedPages): void {
    const corners = new Float32Array(8);
    const spans = this.anchor === null ? [] : this.spans();
    for (let at = 0; at < placed.count; at++) {
      const page = placed.pages[at];
      const marks = this.marksByPage.get(page);
      const span = spans.find((candidate) => candidate.page === page);
      if (marks === undefined && span === undefined) {
        continue;
      }
      const rotation = this.pages.get(page)?.rotation ?? 0;
      const width = this.widths[page] * placed.scales[at];
      const height = this.heights[page] * placed.scales[at];
      const x0 = placed.x[at];
      const y0 = placed.y[at];
      const place = (quad: ArrayLike<number>): void => {
        for (let corner = 0; corner < 4; corner++) {
          const [sx, sy] = toShown(quad[corner * 2], quad[corner * 2 + 1], rotation);
          corners[corner * 2] = x0 + sx * width;
          corners[corner * 2 + 1] = y0 + sy * height;
        }
      };
      for (const mark of marks ?? []) {
        const fill = PALETTE[mark.color] ?? PALETTE.yellow;
        const ink = INK[mark.color] ?? INK.yellow;
        const fillDark = dimmed(fill);
        mark.quads.forEach((quad, index) => {
          place(quad);
          if (mark.kind === 'underline') {
            underline(corners);
            sink.mark(corners, ink, fill);
          } else {
            sink.mark(corners, fill, fillDark);
          }
          if (mark.note && index === 0) {
            place(quad);
            noteTab(corners);
            sink.mark(corners, ink, fill);
          }
        });
      }
      const layer = this.pages.get(page);
      if (span !== undefined && layer !== undefined) {
        for (const quad of lineQuads(layer, span.start, span.end)) {
          place(quad);
          sink.mark(corners, SELECTION, SELECTION_DARK);
        }
      }
    }
  }
}

function storedOf(x: number, y: number, rotation: number): [number, number] {
  switch (rotation & 3) {
    case 1:
      return [y, 1 - x];
    case 2:
      return [1 - x, 1 - y];
    case 3:
      return [1 - y, x];
    default:
      return [x, y];
  }
}

function decodeEmpty(): CharPage {
  return {
    rotation: 0,
    count: 0,
    codes: new Uint32Array(0),
    quads: new Float32Array(0),
    flags: new Uint16Array(0),
    lines: [],
    lineOf: new Uint32Array(0),
    paragraphs: [],
  };
}

/** Turns a mark's quad, in place, into the band just under its text, a tenth of its height. */
function underline(c: Float32Array): void {
  const lx = c[6] - c[0];
  const ly = c[7] - c[1];
  const rx = c[4] - c[2];
  const ry = c[5] - c[3];
  const [blx, bly, brx, bry] = [c[6], c[7], c[4], c[5]];
  c[0] = blx + lx * 0.02;
  c[1] = bly + ly * 0.02;
  c[2] = brx + rx * 0.02;
  c[3] = bry + ry * 0.02;
  c[4] = brx + rx * 0.12;
  c[5] = bry + ry * 0.12;
  c[6] = blx + lx * 0.12;
  c[7] = bly + ly * 0.12;
}

/** Turns a mark's first quad, in place, into a small square just before and above its start. */
function noteTab(c: Float32Array): void {
  const ax = c[0];
  const ay = c[1];
  let dx = c[6] - ax;
  let dy = c[7] - ay;
  const height = Math.hypot(dx, dy) || 1;
  dx /= height;
  dy /= height;
  let rx = c[2] - ax;
  let ry = c[3] - ay;
  const width = Math.hypot(rx, ry) || 1;
  rx /= width;
  ry /= width;
  const size = height * 0.35;
  const x = ax - rx * size * 0.5 - dx * size * 1.1;
  const y = ay - ry * size * 0.5 - dy * size * 1.1;
  c.set([x, y, x + rx * size, y + ry * size, x + rx * size + dx * size, y + ry * size + dy * size, x + dx * size, y + dy * size]);
}
