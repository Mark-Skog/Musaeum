/**
 * Text selection and annotation marks on a fixed layout, inside the compositor.
 *
 * Selecting happens here, on the compositor's thread, against each visible page's characters: a
 * drag that starts on a line of text selects (by character; by word after a double click, by line
 * after a triple one and by paragraph after a fourth), and one that starts anywhere else pans, as
 * before. A selection can run across pages, each page a segment of it. Marks and the selection are
 * drawn in the same frame as the tiles under them, so they never lag behind a scroll or a zoom.
 *
 * On the picture of a page, a scan, there are three more tools: along the lines the host's
 * detector found on it (the first line from where the drag starts, those between whole, the last
 * to where it ends, the ends on the edges of words unless snapping is off), a box, and a stroke
 * drawn by hand. Each makes a region of one page, which the main page turns into an annotation.
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
  toStored,
  wordAt,
} from './chars';
import {
  boxQuad,
  clipOf,
  decodeLines,
  dhashRgba,
  type LinePage,
  type LinePiece,
  pieceQuads,
  snapRange,
  storedQuads,
  strokeQuads,
} from './lines';
import type { PlacedPages } from './view';

/** What a press on a page does: select text, or draw on the picture. */
export type Tool = 'text' | 'lines' | 'rect' | 'free';

/** A mark drawn on the picture of one page, before it is an annotation. */
export interface RegionSelection {
  page: number;
  tool: 'lines' | 'rect' | 'free';
  /** What it is drawn with, in the frame the host stores: the crop box before its rotation. */
  quads: number[][];
  /** Its box: left, top, right, bottom. */
  clip: [number, number, number, number];
  /** The detector's lines it covers. */
  lines: LinePiece[];
  /** A stroke's points, u and v in turn. */
  path: number[];
  /** The 64-bit difference hash of the region as the frame shows it, as sixteen hex digits. */
  regionHash: string | null;
}

/** Reads pixels of the frame under the marks: RGBA rows, top first, in device pixels. */
export type FrameReader = (x: number, y: number, width: number, height: number) => Uint8Array | null;

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
  /** What the pointer is over: text to select, a mark to open, a page to draw on, or neither. */
  | { type: 'cursor'; cursor: Cursor }
  | { type: 'need-lines'; pages: number[] }
  /** A region drawn on a page, or none when it goes. */
  | { type: 'region'; region: RegionSelection | null; rect: { x: number; y: number; width: number; height: number } | null };

type Cursor = 'text' | 'mark' | 'default' | 'crosshair';

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
/**
 * The fill and the line of a mark's color: the palette's for its names; for a color a PDF gave
 * (`#rrggbb`, or `#rrggbbaa` with an opacity), that color itself for both, laid over white paper
 * as much as its opacity says.
 */
export function paintOf(color: string): { fill: [number, number, number]; ink: [number, number, number] } {
  const named = PALETTE[color];
  if (named !== undefined) {
    return { fill: named, ink: INK[color] ?? INK.yellow };
  }
  const hex = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})?$/i.exec(color);
  if (hex === null) {
    return { fill: PALETTE.yellow, ink: INK.yellow };
  }
  const alpha = hex[4] !== undefined ? parseInt(hex[4], 16) / 255 : 1;
  const channel = (digits: string): number => 1 - alpha * (1 - parseInt(digits, 16) / 255);
  const painted: [number, number, number] = [channel(hex[1]), channel(hex[2]), channel(hex[3])];
  return { fill: painted, ink: painted };
}
const dimmed = (color: readonly [number, number, number]): [number, number, number] => [
  color[0] * DARK_FILL,
  color[1] * DARK_FILL,
  color[2] * DARK_FILL,
];
const SELECTION: [number, number, number] = [0.7, 0.83, 1.0];
const SELECTION_DARK: [number, number, number] = [0.25, 0.42, 0.75];
/** The edge of a box being drawn, so it reads as a box over a white page too. */
const BOX_EDGE: [number, number, number] = [0.25, 0.45, 0.85];
const BOX_EDGE_DARK: [number, number, number] = [0.5, 0.7, 1.0];
/** A stroke's width, in points, as a highlighter's. */
export const STROKE_PT = 9;
/** Points of a stroke closer than this to the last one, in points, are not kept. */
const STROKE_STEP_PT = 1.5;
const MAX_STROKE_POINTS = 4096;
/** A box smaller than this, as a fraction of the page, is a click, not a box. */
const MIN_BOX = 0.004;
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
  private tool: Tool = 'text';
  private snap = true;
  private readonly linePages = new Map<number, LinePage>();
  private readonly askedLines = new Set<number>();
  /** A region being drawn, as shown on its page. */
  private gesture: { page: number; start: { x: number; y: number }; end: { x: number; y: number }; path: number[] } | null = null;
  /** The region drawn, kept on the screen until it becomes an annotation or goes. */
  private region: { selection: RegionSelection; shown: number[][] } | null = null;

  constructor(
    private readonly widths: Float32Array,
    private readonly heights: Float32Array,
    private readonly post: (message: AnnotatorMessage) => void,
    private readonly readFrame: FrameReader | null = null,
  ) {}

  /** Changes what a press on a page does. Whatever is selected or drawn goes. */
  setTool(tool: Tool): void {
    if (tool === this.tool) {
      return;
    }
    this.clear(true);
    this.tool = tool;
  }

  /** Whether a drag along lines snaps its ends to words and follows the lines as they curve. */
  setSnap(on: boolean): void {
    this.snap = on;
  }

  get currentTool(): Tool {
    return this.tool;
  }

  get snapping(): boolean {
    return this.snap;
  }

  /** A page's lines as the host sent them, or null when it has none to send. */
  setLines(page: number, buffer: ArrayBuffer | null): void {
    if (buffer === null) {
      return;
    }
    try {
      // In one unit along and across, the page's height: its width is this many of them.
      this.linePages.set(page, decodeLines(buffer, this.widths[page] / this.heights[page]));
    } catch (error) {
      console.warn(`page ${page}'s lines were refused`, error);
    }
  }

  /** The pages whose lines are here. */
  get linePageList(): number[] {
    return [...this.linePages.keys()].sort((a, b) => a - b);
  }

  /** How many lines a page has, or -1 if they are not here. */
  lineCount(page: number): number {
    return this.linePages.get(page)?.lines.length ?? -1;
  }

  /** The region drawn and not yet made an annotation. */
  get regionSelection(): RegionSelection | null {
    return this.region?.selection ?? null;
  }

  /** The rotation a page is shown with, from its lines or its characters. */
  private rotationOf(page: number): number {
    return this.linePages.get(page)?.rotation ?? this.pages.get(page)?.rotation ?? 0;
  }

  private askLines(page: number, into: number[]): void {
    if (!this.linePages.has(page) && !this.askedLines.has(page)) {
      this.askedLines.add(page);
      into.push(page);
    }
  }

  /** Lets a page's lines be asked for again, as when the host had none yet. */
  forgetAskedLines(page: number): void {
    this.askedLines.delete(page);
  }

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
    if (this.tool === 'lines') {
      const lines: number[] = [];
      for (let at = 0; at < placed.count; at++) {
        this.askLines(placed.pages[at], lines);
      }
      if (lines.length > 0) {
        this.post({ type: 'need-lines', pages: lines });
      }
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
    return toShown(u, v, this.rotationOf(page));
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

  private cursor: Cursor = 'default';

  /** The pointer moved with no button down: tells the main page when what is under it changes. */
  hover(x: number, y: number, placed: PlacedPages): void {
    if (this.tool !== 'text') {
      const cursor = this.shownAt(x, y, placed, null) !== null ? 'crosshair' : 'default';
      if (cursor !== this.cursor) {
        this.cursor = cursor;
        this.post({ type: 'cursor', cursor });
      }
      return;
    }
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

  /**
   * Where a point of the canvas is on a page as shown, in fractions: on the page under it, or with
   * `page` on that page wherever it is, clamped to it. Null when there is no such page placed.
   */
  private shownAt(x: number, y: number, placed: PlacedPages, page: number | null): { page: number; x: number; y: number } | null {
    for (let at = 0; at < placed.count; at++) {
      const candidate = placed.pages[at];
      if (page !== null && candidate !== page) {
        continue;
      }
      const width = this.widths[candidate] * placed.scales[at];
      const height = this.heights[candidate] * placed.scales[at];
      const sx = (x - placed.x[at]) / width;
      const sy = (y - placed.y[at]) / height;
      const inside = sx >= 0 && sx <= 1 && sy >= 0 && sy <= 1;
      if (page !== null || inside) {
        return { page: candidate, x: Math.min(1, Math.max(0, sx)), y: Math.min(1, Math.max(0, sy)) };
      }
    }
    return null;
  }

  /** The region the gesture makes now, as shown, or null if it makes none. */
  private gestureQuads(): { shown: number[][]; lines: LinePiece[] } | null {
    const gesture = this.gesture;
    if (gesture === null) {
      return null;
    }
    switch (this.tool) {
      case 'lines': {
        const model = this.linePages.get(gesture.page);
        if (model === undefined) {
          return null;
        }
        const lines = snapRange(model, gesture.start, gesture.end, this.snap);
        const shown = lines.flatMap((piece) => pieceQuads(model, model.lines[piece.index], piece, this.snap));
        return shown.length > 0 ? { shown, lines } : null;
      }
      case 'rect': {
        const quad = boxQuad(gesture.start, gesture.end);
        return quad[2] - quad[0] >= MIN_BOX && quad[5] - quad[1] >= MIN_BOX ? { shown: [quad], lines: [] } : null;
      }
      case 'free': {
        const shown = strokeQuads(gesture.path, STROKE_PT, this.widths[gesture.page], this.heights[gesture.page]);
        return gesture.path.length >= 4 ? { shown, lines: [] } : null;
      }
      default:
        return null;
    }
  }

  private drawDown(x: number, y: number, placed: PlacedPages): boolean {
    const at = this.shownAt(x, y, placed, null);
    this.clear(true);
    if (at === null) {
      return false;
    }
    if (this.tool === 'lines' && !this.linePages.has(at.page)) {
      const missing: number[] = [];
      this.askLines(at.page, missing);
      if (missing.length > 0) {
        this.post({ type: 'need-lines', pages: missing });
      }
    }
    const point = { x: at.x, y: at.y };
    this.gesture = { page: at.page, start: point, end: point, path: [at.x, at.y] };
    this.pressing = true;
    return true;
  }

  private drawMove(x: number, y: number, placed: PlacedPages): void {
    const gesture = this.gesture;
    if (gesture === null) {
      return;
    }
    const at = this.shownAt(x, y, placed, gesture.page);
    if (at === null) {
      return;
    }
    gesture.end = { x: at.x, y: at.y };
    if (this.tool === 'free' && gesture.path.length < MAX_STROKE_POINTS * 2) {
      const width = this.widths[gesture.page];
      const height = this.heights[gesture.page];
      const lastX = gesture.path[gesture.path.length - 2];
      const lastY = gesture.path[gesture.path.length - 1];
      if (Math.hypot((at.x - lastX) * width, (at.y - lastY) * height) >= STROKE_STEP_PT) {
        gesture.path.push(at.x, at.y);
      }
    }
  }

  private drawUp(placed: PlacedPages): void {
    const gesture = this.gesture;
    const made = this.gestureQuads();
    this.gesture = null;
    if (gesture === null || made === null || this.tool === 'text') {
      return;
    }
    const rotation = this.rotationOf(gesture.page);
    const quads = storedQuads(made.shown, rotation);
    const path: number[] = [];
    if (this.tool === 'free') {
      for (let at = 0; at < gesture.path.length; at += 2) {
        path.push(...toStored(gesture.path[at], gesture.path[at + 1], rotation));
      }
    }
    const rect = this.boundsOf(gesture.page, quads, placed);
    const selection: RegionSelection = {
      page: gesture.page,
      tool: this.tool,
      quads,
      clip: clipOf(quads),
      lines: made.lines,
      path,
      regionHash: rect === null ? null : this.hashOf(rect),
    };
    this.region = { selection, shown: made.shown };
    this.post({ type: 'region', region: selection, rect });
  }

  /** The hash of the frame's pixels under a box of the canvas, if it is big enough to mean anything. */
  private hashOf(rect: { x: number; y: number; width: number; height: number }): string | null {
    if (this.readFrame === null || rect.width < 8 || rect.height < 8) {
      return null;
    }
    const x = Math.floor(rect.x);
    const y = Math.floor(rect.y);
    const width = Math.ceil(rect.x + rect.width) - x;
    const height = Math.ceil(rect.y + rect.height) - y;
    const pixels = this.readFrame(x, y, width, height);
    return pixels === null || pixels.length < width * height * 4 ? null : dhashRgba(pixels, width, height);
  }

  /** A press. Returns whether it starts a selection; if not, the reader pans as before. */
  pointerDown(x: number, y: number, now: number, placed: PlacedPages): boolean {
    if (this.tool !== 'text') {
      return this.drawDown(x, y, placed);
    }
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
    if (this.gesture !== null) {
      this.drawMove(x, y, placed);
      return true;
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
    if (this.gesture !== null) {
      this.drawUp(placed);
      return true;
    }
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

  /** Forgets the selection and any region drawn, telling the main page if there was one. */
  clear(tell: boolean): void {
    const had = this.anchor !== null;
    const hadRegion = this.region !== null;
    this.anchor = null;
    this.focus = null;
    this.gesture = null;
    this.region = null;
    this.pressing = false;
    if (had && tell) {
      this.post({ type: 'selection', spans: [], text: '', rect: null });
    }
    if (hadRegion && tell) {
      this.post({ type: 'region', region: null, rect: null });
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
      const rotation = this.rotationOf(page);
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
    const drawing = this.gesture !== null ? this.gestureQuads() : null;
    const drawnPage = this.gesture?.page ?? this.region?.selection.page ?? -1;
    const drawn = drawing?.shown ?? this.region?.shown ?? null;
    for (let at = 0; at < placed.count; at++) {
      const page = placed.pages[at];
      const marks = this.marksByPage.get(page);
      const span = spans.find((candidate) => candidate.page === page);
      const region = page === drawnPage ? drawn : null;
      if (marks === undefined && span === undefined && region === null) {
        continue;
      }
      const rotation = this.rotationOf(page);
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
        const { fill, ink } = paintOf(mark.color);
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
      if (region !== null) {
        // As shown already: placed without turning.
        for (const quad of region) {
          for (let corner = 0; corner < 4; corner++) {
            corners[corner * 2] = x0 + quad[corner * 2] * width;
            corners[corner * 2 + 1] = y0 + quad[corner * 2 + 1] * height;
          }
          sink.mark(corners, SELECTION, SELECTION_DARK);
          if (this.tool === 'rect') {
            for (const edge of boxEdges(corners)) {
              sink.mark(edge, BOX_EDGE, BOX_EDGE_DARK);
            }
          }
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

/** The four edges of an upright box in device pixels, each a band a pixel and a half wide. */
function boxEdges(c: Float32Array): Float32Array[] {
  const left = Math.min(c[0], c[6]);
  const right = Math.max(c[2], c[4]);
  const top = Math.min(c[1], c[3]);
  const bottom = Math.max(c[5], c[7]);
  const w = 1.5;
  const band = (l: number, t: number, r: number, b: number): Float32Array => new Float32Array([l, t, r, t, r, b, l, b]);
  return [band(left, top, right, top + w), band(left, bottom - w, right, bottom), band(left, top, left + w, bottom), band(right - w, top, right, bottom)];
}
