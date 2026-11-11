/**
 * A page's characters, as the host sends them from the worker that read them (the layout of
 * `musaeum_shm::chars`), and what selecting text needs from them: which character a point is at,
 * words, lines and paragraphs, and the quads that cover a stretch of them.
 *
 * Quads are fractions of the page's crop box before its rotation, which is the frame the host
 * anchors annotations in. A point on the screen is turned back into that frame first.
 */

/** `"MCHR"`, read as a little-endian word. */
export const CHARS_MAGIC = 0x5248434d;
export const CHARS_VERSION = 1;
export const LINE_END = 1;
export const GENERATED = 2;

const HEADER = 16;
const GLYPH = 24;
const UNIT = 65535;

/** One line of a page: its characters and the box around their glyphs. */
export interface Line {
  start: number;
  /** One past its last character, the line break included. */
  end: number;
  left: number;
  top: number;
  right: number;
  bottom: number;
}

export interface CharPage {
  /** Quarter turns clockwise the page is shown with. */
  rotation: number;
  count: number;
  codes: Uint32Array;
  /** Eight numbers per character: top left, top right, bottom right, bottom left, x then y. */
  quads: Float32Array;
  flags: Uint16Array;
  lines: Line[];
  /** The line of each character. */
  lineOf: Uint32Array;
  /** Where each paragraph starts, as indices into `lines`. */
  paragraphs: number[];
}

function hasGlyph(page: CharPage, index: number): boolean {
  return (page.flags[index] & GENERATED) === 0 && page.quads[index * 8 + 4] > page.quads[index * 8];
}

/** Reads what the host sent, refusing anything that is not a whole layer of this version. */
export function decodeChars(buffer: ArrayBuffer): CharPage {
  if (buffer.byteLength < HEADER) {
    throw new Error('character layer is too short');
  }
  const view = new DataView(buffer);
  if (view.getUint32(0, true) !== CHARS_MAGIC || view.getUint32(4, true) !== CHARS_VERSION) {
    throw new Error('not a character layer of this version');
  }
  const count = view.getUint32(8, true);
  const rotation = view.getUint32(12, true);
  if (rotation > 3 || buffer.byteLength < HEADER + count * GLYPH) {
    throw new Error('character layer is malformed');
  }
  const codes = new Uint32Array(count);
  const quads = new Float32Array(count * 8);
  const flags = new Uint16Array(count);
  for (let index = 0; index < count; index++) {
    const at = HEADER + index * GLYPH;
    codes[index] = view.getUint32(at, true);
    for (let corner = 0; corner < 8; corner++) {
      quads[index * 8 + corner] = view.getUint16(at + 4 + corner * 2, true) / UNIT;
    }
    flags[index] = view.getUint16(at + 20, true);
  }
  const page: CharPage = { rotation, count, codes, quads, flags, lines: [], lineOf: new Uint32Array(count), paragraphs: [] };
  measureLines(page);
  return page;
}

/** Splits the page into lines where the text layer says one ends, with each line's box. */
function measureLines(page: CharPage): void {
  let start = 0;
  for (let index = 0; index < page.count; index++) {
    if ((page.flags[index] & LINE_END) !== 0 || index === page.count - 1) {
      const line: Line = { start, end: index + 1, left: 1, top: 1, right: 0, bottom: 0 };
      for (let at = start; at <= index; at++) {
        page.lineOf[at] = page.lines.length;
        if (!hasGlyph(page, at)) {
          continue;
        }
        const q = at * 8;
        line.left = Math.min(line.left, page.quads[q], page.quads[q + 6]);
        line.right = Math.max(line.right, page.quads[q + 2], page.quads[q + 4]);
        line.top = Math.min(line.top, page.quads[q + 1], page.quads[q + 3]);
        line.bottom = Math.max(line.bottom, page.quads[q + 5], page.quads[q + 7]);
      }
      page.lines.push(line);
      start = index + 1;
    }
  }
  // A paragraph starts after a gap taller than the usual one between lines, or after a short line.
  const drawn = page.lines.filter((line) => line.right > line.left);
  const heights = drawn.map((line) => line.bottom - line.top).sort((a, b) => a - b);
  const height = heights[heights.length >> 1] ?? 0;
  const gaps: number[] = [];
  for (let at = 1; at < page.lines.length; at++) {
    const gap = page.lines[at].top - page.lines[at - 1].bottom;
    if (page.lines[at].right > page.lines[at].left && gap > -height) {
      gaps.push(gap);
    }
  }
  gaps.sort((a, b) => a - b);
  const usual = gaps[gaps.length >> 1] ?? 0;
  const widest = drawn.reduce((most, line) => Math.max(most, line.right), 0);
  page.paragraphs = [0];
  for (let at = 1; at < page.lines.length; at++) {
    const before = page.lines[at - 1];
    const line = page.lines[at];
    const gap = line.top - before.bottom;
    const tall = gap > usual + height * 0.5;
    const short = before.right > before.left && widest - before.right > (widest - before.left) * 0.2;
    if (tall || short || gap < -height) {
      page.paragraphs.push(at);
    }
  }
}

/** A point of the page as stored, from a point of the page as shown. */
export function toStored(x: number, y: number, rotation: number): [number, number] {
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

/** A point of the page as shown, from a point of the page as stored. */
export function toShown(u: number, v: number, rotation: number): [number, number] {
  switch (rotation & 3) {
    case 1:
      return [1 - v, u];
    case 2:
      return [1 - u, 1 - v];
    case 3:
      return [v, 1 - u];
    default:
      return [u, v];
  }
}

/** Where a point falls among the characters: before which one, and whether it is over text. */
export interface Hit {
  /** A caret: the point is before character `caret`, from 0 to `count`. */
  caret: number;
  /** The character under or nearest the point. */
  index: number;
  /** Whether the point is on a line of text, not in a margin or a picture. */
  onText: boolean;
}

/** The caret nearest a point of the page as stored. */
export function hitTest(page: CharPage, u: number, v: number): Hit | null {
  let best = -1;
  let bestDistance = Infinity;
  for (let at = 0; at < page.lines.length; at++) {
    const line = page.lines[at];
    if (line.right <= line.left) {
      continue;
    }
    const distance = v < line.top ? line.top - v : v > line.bottom ? v - line.bottom : 0;
    const across = u < line.left ? line.left - u : u > line.right ? u - line.right : 0;
    const score = distance * 4 + across;
    if (score < bestDistance) {
      bestDistance = score;
      best = at;
    }
  }
  if (best < 0) {
    return null;
  }
  const line = page.lines[best];
  const height = line.bottom - line.top;
  const onText =
    v >= line.top - height * 0.35 && v <= line.bottom + height * 0.35 && u >= line.left - height && u <= line.right + height;
  let index = line.start;
  let caret = line.start;
  let nearest = Infinity;
  for (let at = line.start; at < line.end; at++) {
    if (!hasGlyph(page, at)) {
      continue;
    }
    const q = at * 8;
    const left = Math.min(page.quads[q], page.quads[q + 6]);
    const right = Math.max(page.quads[q + 2], page.quads[q + 4]);
    const middle = (left + right) / 2;
    const distance = u < left ? left - u : u > right ? u - right : 0;
    if (distance < nearest) {
      nearest = distance;
      index = at;
      caret = u > middle ? at + 1 : at;
    }
  }
  return { caret, index, onText };
}

function isWordChar(code: number): boolean {
  const ch = String.fromCodePoint(code);
  return /[\p{L}\p{N}\p{M}_'’-]/u.test(ch);
}

/** The word around a character: letters, digits and the marks between them. */
export function wordAt(page: CharPage, index: number): [number, number] {
  if (index < 0 || index >= page.count) {
    return [index, index];
  }
  if (!isWordChar(page.codes[index])) {
    return [index, index + 1];
  }
  let start = index;
  while (start > 0 && isWordChar(page.codes[start - 1]) && page.lineOf[start - 1] === page.lineOf[index]) {
    start--;
  }
  let end = index + 1;
  while (end < page.count && isWordChar(page.codes[end]) && page.lineOf[end] === page.lineOf[index]) {
    end++;
  }
  return [start, end];
}

/** The line a character is on, without its line break. */
export function lineAt(page: CharPage, index: number): [number, number] {
  const line = page.lines[page.lineOf[Math.max(0, Math.min(index, page.count - 1))]];
  if (line === undefined) {
    return [index, index];
  }
  let end = line.end;
  while (end > line.start && !hasGlyph(page, end - 1)) {
    end--;
  }
  return [line.start, end];
}

/** The paragraph a character is in. */
export function paragraphAt(page: CharPage, index: number): [number, number] {
  const line = page.lineOf[Math.max(0, Math.min(index, page.count - 1))];
  let first = 0;
  let next = page.lines.length;
  for (const start of page.paragraphs) {
    if (start <= line) {
      first = start;
    } else {
      next = start;
      break;
    }
  }
  const start = page.lines[first]?.start ?? 0;
  let end = page.lines[next - 1]?.end ?? page.count;
  while (end > start && !hasGlyph(page, end - 1)) {
    end--;
  }
  return [start, end];
}

/** Characters `start..end` as text, one line break where a line ends. */
export function textOf(page: CharPage, start: number, end: number): string {
  let text = '';
  for (let at = Math.max(0, start); at < Math.min(end, page.count); at++) {
    text += String.fromCodePoint(page.codes[at]);
  }
  return text;
}

/**
 * Quads that cover characters `start..end`, one per stretch of a line, as the host's `line_quads`
 * makes them: upright characters of a line merge into one box, a turned one keeps its own quad.
 */
export function lineQuads(page: CharPage, start: number, end: number): number[][] {
  const quads: number[][] = [];
  let open: [number, number, number, number] | null = null;
  const close = (): void => {
    if (open !== null) {
      const [l, t, r, b] = open;
      quads.push([l, t, r, t, r, b, l, b]);
      open = null;
    }
  };
  for (let at = Math.max(0, start); at < Math.min(end, page.count); at++) {
    const q = at * 8;
    const ch = page.codes[at];
    if ((page.flags[at] & GENERATED) === 0 && !/\s/.test(String.fromCodePoint(ch))) {
      const quad = page.quads.subarray(q, q + 8);
      const upright =
        Math.abs(quad[1] - quad[3]) <= 1.5 / UNIT &&
        Math.abs(quad[5] - quad[7]) <= 1.5 / UNIT &&
        Math.abs(quad[0] - quad[6]) <= 1.5 / UNIT &&
        Math.abs(quad[2] - quad[4]) <= 1.5 / UNIT;
      if (upright) {
        const rect: [number, number, number, number] = [
          Math.min(quad[0], quad[6]),
          Math.min(quad[1], quad[3]),
          Math.max(quad[2], quad[4]),
          Math.max(quad[5], quad[7]),
        ];
        if (open !== null) {
          const current: [number, number, number, number] = open;
          const overlap = Math.min(current[3], rect[3]) - Math.max(current[1], rect[1]);
          const smaller = Math.max(Math.min(current[3] - current[1], rect[3] - rect[1]), 1e-9);
          if (overlap >= smaller * 0.5 && rect[0] >= current[0] - smaller) {
            open = [
              Math.min(current[0], rect[0]),
              Math.min(current[1], rect[1]),
              Math.max(current[2], rect[2]),
              Math.max(current[3], rect[3]),
            ];
          } else {
            close();
            open = rect;
          }
        } else {
          open = rect;
        }
      } else {
        close();
        quads.push(Array.from(quad));
      }
    }
    if ((page.flags[at] & LINE_END) !== 0) {
      close();
    }
  }
  close();
  return quads;
}
