/**
 * The lines of text the host's detector found on a page's picture, and what the compositor does
 * with them: snapping a drag on a scan to lines, and the shapes of a box and of a stroke drawn by
 * hand. The model is `musaeum-vision`'s `MLIN` bytes, after the page picture's 64-bit hash.
 *
 * Geometry happens on the page as shown, with x and y in one unit, the page's height: a line runs
 * from its start to its end at any angle (level, bent toward the spine, at 90° beside a figure,
 * at 45° on a chart), and every place along it is a fraction of its baseline's length, as the
 * host measures the spaces between its words and the words the OCR reads on it. Across a line,
 * "up" is to the left of the way it reads, where its letters stand. What is kept (the quads, the
 * box, the stroke) goes back to the frame the host stores, the crop box before its rotation, as
 * character quads are.
 */

import { toShown, toStored } from './chars';

/** `"MLIN"`, the model's first word. */
export const LINES_MAGIC = 0x4e494c4d;
/** The version of the model this reads: gaps along the baseline's length, and flags. */
export const LINES_VERSION = 3;
/** A line's flag: it is in a block at another angle than the page's own text. */
export const LINE_TURNED = 1;
const HEADER = 24;
const COLUMN = 16;
const LINE_HEADER = 20;

/** One line as shown: its baseline, from its start to its end, and how far its ink reaches. */
export interface VisualLine {
  /** Its place in the model, which is reading order: column by column, top to bottom. */
  index: number;
  column: number;
  /** In a block at another angle than the page's text. */
  turned: boolean;
  /** The baseline's points, x and y in turn, as fractions of the page as shown. */
  points: Float32Array;
  /** How far along the baseline each point is, in heights of the page; the last is its length. */
  arcs: Float32Array;
  length: number;
  /** Above and below the baseline, across it, as fractions of the page's height as shown. */
  ascent: number;
  descent: number;
  xHeight: number;
  /** The spaces between words: from and to in turn, as fractions of the baseline's length. */
  gaps: Float32Array;
  /** Around the baseline, in one unit: left, top, right, bottom. */
  box: Float32Array;
}

export interface LinePage {
  /** The page picture's hash, as sixteen hex digits. */
  pageHash: string;
  rotation: number;
  skew: number;
  /** Its width over its height, as shown. */
  aspect: number;
  lines: VisualLine[];
}

/** A stretch of one of the model's lines, from and to fractions along it. */
export interface LinePiece {
  index: number;
  from: number;
  to: number;
}

const hex32 = (value: number): string => (value >>> 0).toString(16).padStart(8, '0');

/**
 * Decodes what the host's `line_model` sends, for a page `aspect` times as wide as it is high as
 * shown. Throws on anything malformed, and on a model of another version, which the host
 * replaces with one of this version as the page is traced again.
 */
export function decodeLines(buffer: ArrayBuffer, aspect = 1): LinePage {
  if (buffer.byteLength < 8 + HEADER) {
    throw new Error('the line model is truncated');
  }
  if (!(aspect > 0) || !Number.isFinite(aspect)) {
    throw new Error('a page of no shape');
  }
  const view = new DataView(buffer);
  const pageHash = hex32(view.getUint32(4, true)) + hex32(view.getUint32(0, true));
  const base = 8;
  if (view.getUint32(base, true) !== LINES_MAGIC || view.getUint32(base + 4, true) !== LINES_VERSION) {
    throw new Error('not a line model of a known version');
  }
  const rotation = view.getUint8(base + 8);
  const skew = view.getFloat32(base + 12, true);
  const columnCount = view.getUint32(base + 16, true);
  const lineCount = view.getUint32(base + 20, true);
  if (rotation > 3 || columnCount > 64 || lineCount > 100_000) {
    throw new Error('not a line model of a known version');
  }
  let at = base + HEADER + columnCount * COLUMN;
  const lines: VisualLine[] = [];
  for (let index = 0; index < lineCount; index++) {
    if (at + LINE_HEADER > buffer.byteLength) {
      throw new Error('the line model is truncated');
    }
    const column = view.getUint16(at, true);
    const pointCount = view.getUint16(at + 2, true);
    const gapCount = view.getUint16(at + 4, true);
    const flags = view.getUint16(at + 6, true);
    const xHeight = view.getFloat32(at + 8, true);
    const ascent = view.getFloat32(at + 12, true);
    const descent = view.getFloat32(at + 16, true);
    at += LINE_HEADER;
    const end = at + (pointCount + gapCount) * 8;
    if (pointCount < 2 || end > buffer.byteLength) {
      throw new Error(`line ${index} is malformed`);
    }
    const points = new Float32Array(pointCount * 2);
    const arcs = new Float32Array(pointCount);
    const box = new Float32Array([Infinity, Infinity, -Infinity, -Infinity]);
    for (let point = 0; point < pointCount; point++) {
      const [x, y] = toShown(view.getFloat32(at, true), view.getFloat32(at + 4, true), rotation);
      points[point * 2] = x;
      points[point * 2 + 1] = y;
      if (point > 0) {
        const dx = (x - points[point * 2 - 2]) * aspect;
        const dy = y - points[point * 2 - 1];
        arcs[point] = arcs[point - 1] + Math.sqrt(dx * dx + dy * dy);
      }
      box[0] = Math.min(box[0], x * aspect);
      box[1] = Math.min(box[1], y);
      box[2] = Math.max(box[2], x * aspect);
      box[3] = Math.max(box[3], y);
      at += 8;
    }
    const gaps = new Float32Array(gapCount * 2);
    for (let value = 0; value < gapCount * 2; value++) {
      gaps[value] = view.getFloat32(at, true);
      at += 4;
    }
    lines.push({
      index,
      column,
      turned: (flags & LINE_TURNED) !== 0,
      points,
      arcs,
      length: arcs[pointCount - 1],
      ascent: Math.max(0, ascent),
      descent: Math.max(0, descent),
      xHeight,
      gaps,
      box,
    });
  }
  return { pageHash, rotation, skew, aspect, lines };
}

/** A place on a line: where it is, as shown, and which way the line goes there, in one unit. */
export interface Along {
  x: number;
  y: number;
  /** The unit direction of the baseline, x and y in heights of the page. */
  dx: number;
  dy: number;
}

/** The segment of the baseline arc length `s` falls in; before the start and past the end, the first and last. */
function segmentAt(line: VisualLine, s: number): number {
  const count = line.arcs.length;
  let segment = 0;
  while (segment < count - 2 && line.arcs[segment + 1] < s) {
    segment++;
  }
  return segment;
}

/** The unit direction of a segment of the baseline, in one unit. */
function direction(line: VisualLine, aspect: number, segment: number): [number, number] {
  const { points } = line;
  const dx = (points[segment * 2 + 2] - points[segment * 2]) * aspect;
  const dy = points[segment * 2 + 3] - points[segment * 2 + 1];
  const length = Math.hypot(dx, dy);
  return length > 0 ? [dx / length, dy / length] : [1, 0];
}

/** The place at fraction `t` of a line; before its start and past its end, it goes on straight. */
export function pointAt(page: LinePage, line: VisualLine, t: number): Along {
  const s = t * line.length;
  const segment = segmentAt(line, s);
  const [dx, dy] = direction(line, page.aspect, segment);
  const into = s - line.arcs[segment];
  return {
    x: line.points[segment * 2] + (dx * into) / page.aspect,
    y: line.points[segment * 2 + 1] + dy * into,
    dx,
    dy,
  };
}

/**
 * Where the last point given to [`placeOf`] is from its line: the arc length of its foot on the
 * line, clamped to it; its signed distance across the line, above (where the letters stand)
 * positive; and how far it is before the line's start or past its end, along it, 0 beside it.
 * Kept here rather than returned, so the snap allocates nothing per line.
 */
let placeS = 0;
let placeAbove = 0;
let placeBeyond = 0;

/** Places the point `(px, y)`, x already in heights of the page, against a line. */
function placeOf(line: VisualLine, aspect: number, px: number, y: number): void {
  const { points, arcs } = line;
  const count = arcs.length;
  let bestDistance = Infinity;
  placeS = 0;
  placeAbove = 0;
  placeBeyond = 0;
  for (let segment = 0; segment < count - 1; segment++) {
    const ax = points[segment * 2] * aspect;
    const ay = points[segment * 2 + 1];
    const length = arcs[segment + 1] - arcs[segment];
    let dx = 1;
    let dy = 0;
    if (length > 0) {
      dx = (points[segment * 2 + 2] * aspect - ax) / length;
      dy = (points[segment * 2 + 3] - ay) / length;
    }
    const t = (px - ax) * dx + (y - ay) * dy;
    // Upward across a line going right, with y growing downward: (dy, -dx).
    const above = (px - ax) * dy - (y - ay) * dx;
    let beyond = 0;
    let clamped = t;
    if (t < 0) {
      clamped = 0;
      beyond = segment === 0 ? -t : 0;
    } else if (t > length) {
      clamped = length;
      beyond = segment === count - 2 ? t - length : 0;
    }
    // Inside the line's turns, the distance to the segment's nearest point; at its ends, to the
    // line going on straight.
    let distance: number;
    if (beyond > 0) {
      distance = Math.abs(above) + beyond;
    } else {
      const fx = px - (ax + dx * clamped);
      const fy = y - (ay + dy * clamped);
      distance = Math.sqrt(fx * fx + fy * fy);
    }
    if (distance < bestDistance) {
      bestDistance = distance;
      placeS = arcs[segment] + clamped;
      placeAbove = above;
      placeBeyond = beyond;
    }
  }
}

/** Where a point, as shown, is along a line, from 0 at its start to 1 at its end. */
export function along(page: LinePage, line: VisualLine, x: number, y: number): number {
  if (!(line.length > 0)) {
    return 0;
  }
  placeOf(line, page.aspect, x * page.aspect, y);
  return Math.min(1, Math.max(0, placeS / line.length));
}

/**
 * The line under a point of the page as shown, or with `nearest` the closest one within `within`
 * of its own heights: the distance across a line to its band counts, and a point past a line's
 * ends counts as far as it is past them, twice over. With `accept`, only the lines it accepts.
 */
export function lineAt(
  page: LinePage,
  x: number,
  y: number,
  nearest: boolean,
  within = Infinity,
  accept?: (line: VisualLine) => boolean,
): VisualLine | null {
  let best: VisualLine | null = null;
  let bestDistance = Infinity;
  const px = x * page.aspect;
  for (const line of page.lines) {
    if (accept !== undefined && !accept(line)) {
      continue;
    }
    const reach = (line.ascent + line.descent) * 0.5;
    // Every point on the line's band, and as far past its ends as still counts, is inside its
    // box grown by this much; a point outside it is at least as far from the line as from it.
    const grow = Math.max(line.ascent, line.descent) + reach;
    const { box } = line;
    const outX = Math.max(box[0] - grow - px, 0, px - box[2] - grow);
    const outY = Math.max(box[1] - grow - y, 0, y - box[3] - grow);
    if (outX > 0 || outY > 0) {
      if (!nearest || Math.sqrt(outX * outX + outY * outY) >= bestDistance) {
        continue;
      }
    }
    placeOf(line, page.aspect, px, y);
    const across = placeAbove > line.ascent ? placeAbove - line.ascent : placeAbove < -line.descent ? -line.descent - placeAbove : 0;
    const past = Math.max(0, placeBeyond - reach);
    if (across === 0 && past === 0) {
      return line;
    }
    const distance = across + past * 2;
    if (nearest && distance < bestDistance && distance <= within * (line.ascent + line.descent)) {
      bestDistance = distance;
      best = line;
    }
  }
  return best;
}

/** The start of the word at fraction `t` of a line, or of the next word if `t` is in a space. */
export function wordStart(line: VisualLine, t: number): number {
  let start = 0;
  for (let at = 0; at < line.gaps.length; at += 2) {
    const from = line.gaps[at];
    const to = line.gaps[at + 1];
    if (t < from) {
      break;
    }
    start = to;
  }
  return Math.min(1, start);
}

/** The end of the word at fraction `t` of a line, or of the word before if `t` is in a space. */
export function wordEnd(line: VisualLine, t: number): number {
  for (let at = 0; at < line.gaps.length; at += 2) {
    // The first space that ends past `t`: before it, `t`'s word ends there; inside it, the word
    // before does.
    if (t <= line.gaps[at + 1]) {
      return Math.max(0, line.gaps[at]);
    }
  }
  return 1;
}

/** The end of the word that starts at `start`. */
function endOfWordFrom(line: VisualLine, start: number): number {
  for (let at = 0; at < line.gaps.length; at += 2) {
    if (line.gaps[at] > start) {
      return line.gaps[at];
    }
  }
  return 1;
}

/**
 * How far from a line, in its own heights, a drag may start or end and still take it: beside a
 * word, in the space between two lines, but not on a photograph or a figure.
 */
const NEAR_LINES = 3;

/**
 * How near two lines run to one way, as the cosine of the angle between them, for one drag to
 * take both: within 30°. A page set on its side has its lines at 90° and a folio or a scanner's
 * mark at 0°; a chart has labels at 45° beside its level caption. A drag along one kind does not
 * run on into the other, whatever the order of the model puts between them.
 */
const SAME_WAY = Math.cos(Math.PI / 6);

/** The unit direction from a line's start to its end, in one unit. */
export function heading(page: LinePage, line: VisualLine): [number, number] {
  const { points } = line;
  const last = points.length - 2;
  const dx = (points[last] - points[0]) * page.aspect;
  const dy = points[last + 1] - points[1];
  const length = Math.hypot(dx, dy);
  return length > 0 ? [dx / length, dy / length] : [1, 0];
}

/** Whether two lines run the same way, within 30°, and read the same way along it. */
export function sameWay(page: LinePage, a: VisualLine, b: VisualLine): boolean {
  const [ax, ay] = heading(page, a);
  const [bx, by] = heading(page, b);
  return ax * bx + ay * by >= SAME_WAY;
}

/**
 * What a drag from one point to another selects along a page's lines, both points as shown: the
 * first line from where the drag starts, the lines between whole, the last to where it ends. With
 * `snap` on, the ends go to the edges of the words they are in; off, they stay where the pointer
 * is. Empty when either point is nowhere near a line: on a picture, say. Only lines that run the
 * same way as the one the drag starts on count, for its end and between: a drag up the lines of
 * a page set on its side takes none of the level folio under them.
 */
export function snapRange(page: LinePage, from: { x: number; y: number }, to: { x: number; y: number }, snap: boolean): LinePiece[] {
  const a = lineAt(page, from.x, from.y, true, NEAR_LINES);
  if (a === null) {
    return [];
  }
  const withA = (line: VisualLine): boolean => sameWay(page, a, line);
  const b = lineAt(page, to.x, to.y, true, NEAR_LINES, withA);
  if (b === null) {
    return [];
  }
  let start = { line: a, t: along(page, a, from.x, from.y) };
  let end = { line: b, t: along(page, b, to.x, to.y) };
  if (start.line.index > end.line.index || (start.line.index === end.line.index && start.t > end.t)) {
    [start, end] = [end, start];
  }
  const first = snap ? wordStart(start.line, start.t) : start.t;
  const last = snap ? wordEnd(end.line, end.t) : end.t;
  const pieces: LinePiece[] = [];
  for (let index = start.line.index; index <= end.line.index; index++) {
    if (!withA(page.lines[index])) {
      continue;
    }
    const piece = {
      index,
      from: index === start.line.index ? first : 0,
      to: index === end.line.index ? last : 1,
    };
    if (piece.to > piece.from) {
      pieces.push(piece);
    }
  }
  if (pieces.length === 0 && snap && start.line === end.line) {
    // Both ends in one space: the word after it.
    const from = wordStart(start.line, start.t);
    const to = endOfWordFrom(start.line, from);
    if (to > from) {
      pieces.push({ index: start.line.index, from, to });
    }
  }
  return pieces;
}

/**
 * Upward across a line, in one unit, into `out`: across the mean of its segments `from` to `to`.
 * At a point of the baseline that is the two segments meeting there, so neighbouring quads share
 * their edge; inside a segment, that segment alone.
 */
function normalAt(line: VisualLine, aspect: number, from: number, to: number, out: Float64Array): void {
  const { points } = line;
  const last = line.arcs.length - 1;
  let dx = 0;
  let dy = 0;
  for (let segment = from; segment <= to; segment++) {
    if (segment < 0 || segment >= last) {
      continue;
    }
    const ex = (points[segment * 2 + 2] - points[segment * 2]) * aspect;
    const ey = points[segment * 2 + 3] - points[segment * 2 + 1];
    const length = Math.sqrt(ex * ex + ey * ey);
    if (length > 0) {
      dx += ex / length;
      dy += ey / length;
    }
  }
  const length = Math.sqrt(dx * dx + dy * dy);
  if (length > 1e-9) {
    out[0] = dy / length;
    out[1] = -dx / length;
  } else {
    out[0] = 0;
    out[1] = -1;
  }
}

const normalA = new Float64Array(2);
const normalB = new Float64Array(2);

/**
 * The quads a stretch of a line is drawn with, as shown: with `follow`, one per segment of the
 * baseline, so a curved line near the spine is covered as it curves; otherwise one box around
 * it, level for the page's own text and along the stretch for a turned line.
 */
export function pieceQuads(page: LinePage, line: VisualLine, piece: LinePiece, follow: boolean): number[][] {
  const { points, arcs, ascent, descent } = line;
  const { aspect } = page;
  const sa = line.length * piece.from;
  const sb = line.length * piece.to;
  const count = arcs.length;
  if (!(sb > sa) || count < 2) {
    return [];
  }
  // The point at arc length `s` on segment `segment`, in one unit.
  const xOn = (segment: number, s: number): number => {
    const length = arcs[segment + 1] - arcs[segment];
    const t = length > 0 ? (s - arcs[segment]) / length : 0;
    return (points[segment * 2] + (points[segment * 2 + 2] - points[segment * 2]) * t) * aspect;
  };
  const yOn = (segment: number, s: number): number => {
    const length = arcs[segment + 1] - arcs[segment];
    const t = length > 0 ? (s - arcs[segment]) / length : 0;
    return points[segment * 2 + 1] + (points[segment * 2 + 3] - points[segment * 2 + 1]) * t;
  };
  const first = Math.max(0, Math.min(count - 2, segmentAt(line, sa)));
  if (!follow) {
    const lastSegment = Math.max(first, Math.min(count - 2, segmentAt(line, sb)));
    const ax = xOn(first, sa);
    const ay = yOn(first, sa);
    const bx = xOn(lastSegment, sb);
    const by = yOn(lastSegment, sb);
    const chord = Math.sqrt((bx - ax) * (bx - ax) + (by - ay) * (by - ay));
    const dx = line.turned && chord > 0 ? (bx - ax) / chord : 1;
    const dy = line.turned && chord > 0 ? (by - ay) / chord : 0;
    const ux = dy;
    const uy = -dx;
    let low = Infinity;
    let high = -Infinity;
    let top = -Infinity;
    let bottom = Infinity;
    const take = (x: number, y: number): void => {
      const run = (x - ax) * dx + (y - ay) * dy;
      const rise = (x - ax) * ux + (y - ay) * uy;
      low = Math.min(low, run);
      high = Math.max(high, run);
      top = Math.max(top, rise + ascent);
      bottom = Math.min(bottom, rise - descent);
    };
    take(ax, ay);
    for (let vertex = first + 1; vertex <= lastSegment; vertex++) {
      take(points[vertex * 2] * aspect, points[vertex * 2 + 1]);
    }
    take(bx, by);
    const cx = (run: number, rise: number): number => (ax + dx * run + ux * rise) / aspect;
    const cy = (run: number, rise: number): number => ay + dy * run + uy * rise;
    return [[cx(low, top), cy(low, top), cx(high, top), cy(high, top), cx(high, bottom), cy(high, bottom), cx(low, bottom), cy(low, bottom)]];
  }
  const quads: number[][] = [];
  for (let segment = first; segment < count - 1; segment++) {
    const s0 = Math.max(sa, arcs[segment]);
    const s1 = Math.min(sb, arcs[segment + 1]);
    if (arcs[segment] >= sb) {
      break;
    }
    if (!(s1 > s0)) {
      continue;
    }
    // At a point of the baseline, the mean of the segments meeting there; inside a segment, its own.
    if (s0 === arcs[segment]) {
      normalAt(line, aspect, segment - 1, segment, normalA);
    } else {
      normalAt(line, aspect, segment, segment, normalA);
    }
    if (s1 === arcs[segment + 1]) {
      normalAt(line, aspect, segment, segment + 1, normalB);
    } else {
      normalAt(line, aspect, segment, segment, normalB);
    }
    const x0 = xOn(segment, s0);
    const y0 = yOn(segment, s0);
    const x1 = xOn(segment, s1);
    const y1 = yOn(segment, s1);
    quads.push([
      (x0 + normalA[0] * ascent) / aspect,
      y0 + normalA[1] * ascent,
      (x1 + normalB[0] * ascent) / aspect,
      y1 + normalB[1] * ascent,
      (x1 - normalB[0] * descent) / aspect,
      y1 - normalB[1] * descent,
      (x0 - normalA[0] * descent) / aspect,
      y0 - normalA[1] * descent,
    ]);
  }
  return quads;
}

/** A box from one corner to the other, as shown, clamped to the page. */
export function boxQuad(from: { x: number; y: number }, to: { x: number; y: number }): number[] {
  const clamp = (value: number): number => Math.min(1, Math.max(0, value));
  const left = clamp(Math.min(from.x, to.x));
  const right = clamp(Math.max(from.x, to.x));
  const top = clamp(Math.min(from.y, to.y));
  const bottom = clamp(Math.max(from.y, to.y));
  return [left, top, right, top, right, bottom, left, bottom];
}

/**
 * The quads a stroke is drawn with: one band per step of its path, `width` wide in points and
 * squared off past each point so the steps join. The path is x and y in turn, as shown; `pageWidth`
 * and `pageHeight` the page in points, so the band is as wide across as along.
 */
export function strokeQuads(path: ArrayLike<number>, width: number, pageWidth: number, pageHeight: number): number[][] {
  const half = width / 2;
  const count = Math.floor(path.length / 2);
  const point = (at: number): [number, number] => [path[at * 2] * pageWidth, path[at * 2 + 1] * pageHeight];
  const band = (ax: number, ay: number, bx: number, by: number): number[] => {
    const length = Math.hypot(bx - ax, by - ay);
    const [dx, dy] = length > 0 ? [(bx - ax) / length, (by - ay) / length] : [1, 0];
    // Across the step, to its left on a page whose y grows downwards (the top edge for a step
    // to the right), and past both its ends.
    const nx = dy * half;
    const ny = -dx * half;
    const sx = ax - dx * half;
    const sy = ay - dy * half;
    const ex = bx + dx * half;
    const ey = by + dy * half;
    return [sx + nx, sy + ny, ex + nx, ey + ny, ex - nx, ey - ny, sx - nx, sy - ny].map((value, index) =>
      index % 2 === 0 ? value / pageWidth : value / pageHeight,
    );
  };
  if (count === 0) {
    return [];
  }
  if (count === 1) {
    const [x, y] = point(0);
    return [band(x, y, x, y)];
  }
  const quads: number[][] = [];
  for (let at = 1; at < count; at++) {
    const [ax, ay] = point(at - 1);
    const [bx, by] = point(at);
    if (ax !== bx || ay !== by) {
      quads.push(band(ax, ay, bx, by));
    }
  }
  return quads.length > 0 ? quads : [band(...point(0), ...point(0))];
}

/** Quads as shown, in the frame the host stores. */
export function storedQuads(quads: number[][], rotation: number): number[][] {
  return quads.map((quad) => {
    const out: number[] = [];
    for (let corner = 0; corner < 4; corner++) {
      const [u, v] = toStored(quad[corner * 2], quad[corner * 2 + 1], rotation);
      out.push(u, v);
    }
    return out;
  });
}

/** The box around quads: left, top, right, bottom. */
export function clipOf(quads: number[][]): [number, number, number, number] {
  let left = Infinity;
  let top = Infinity;
  let right = -Infinity;
  let bottom = -Infinity;
  for (const quad of quads) {
    for (let corner = 0; corner < 4; corner++) {
      left = Math.min(left, quad[corner * 2]);
      right = Math.max(right, quad[corner * 2]);
      top = Math.min(top, quad[corner * 2 + 1]);
      bottom = Math.max(bottom, quad[corner * 2 + 1]);
    }
  }
  const clamp = (value: number): number => Math.min(1, Math.max(0, value));
  return [clamp(left), clamp(top), clamp(right), clamp(bottom)];
}

/**
 * The 64-bit difference hash of RGBA pixels, rows packed and top first, as sixteen hex digits:
 * the picture shrunk to 9 × 8 gray pixels by averaging, one bit per pair of neighbours saying
 * whether the left one is brighter, as `musaeum_vision::dhash` makes it.
 */
export function dhashRgba(pixels: Uint8Array, width: number, height: number): string {
  const gray = new Uint8Array(width * height);
  for (let at = 0; at < gray.length; at++) {
    const r = pixels[at * 4];
    const g = pixels[at * 4 + 1];
    const b = pixels[at * 4 + 2];
    // Rec. 601 in integers, as musaeum_vision::Gray::from_rgba has it.
    gray[at] = (77 * r + 150 * g + 29 * b) >> 8;
  }
  const small = new Uint8Array(9 * 8);
  const sx = width / 9;
  const sy = height / 8;
  for (let row = 0; row < 8; row++) {
    const y0 = Math.floor(row * sy);
    const y1 = Math.min(height, Math.max(y0 + 1, Math.ceil((row + 1) * sy)));
    for (let column = 0; column < 9; column++) {
      const x0 = Math.floor(column * sx);
      const x1 = Math.min(width, Math.max(x0 + 1, Math.ceil((column + 1) * sx)));
      let sum = 0;
      for (let y = y0; y < y1; y++) {
        for (let x = x0; x < x1; x++) {
          sum += gray[y * width + x];
        }
      }
      small[row * 9 + column] = Math.floor(sum / ((y1 - y0) * (x1 - x0)));
    }
  }
  let high = 0;
  let low = 0;
  for (let bit = 0; bit < 64; bit++) {
    const row = bit >> 3;
    const column = bit & 7;
    const set = small[row * 9 + column] > small[row * 9 + column + 1] ? 1 : 0;
    if (bit < 32) {
      high = ((high << 1) | set) >>> 0;
    } else {
      low = ((low << 1) | set) >>> 0;
    }
  }
  return hex32(high) + hex32(low);
}
