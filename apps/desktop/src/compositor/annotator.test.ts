import { describe, expect, it } from 'vitest';
import { Annotator, type AnnotatorMessage } from './annotator';
import { CHARS_MAGIC, GENERATED, LINE_END } from './chars';
import { PlacedPages } from './view';

/** A layer of lines, each character 1/100 of the page wide, each line 1/20 high. */
function layer(lines: string[], rotation = 0): ArrayBuffer {
  const glyphs: { code: number; quad: number[]; flags: number }[] = [];
  lines.forEach((line, row) => {
    const top = 0.1 + row * 0.05;
    [...line].forEach((ch, column) => {
      const left = 0.1 + column * 0.01;
      const blank = ch === ' ';
      glyphs.push({
        code: ch.codePointAt(0) ?? 32,
        quad: blank ? [0, 0, 0, 0, 0, 0, 0, 0] : [left, top, left + 0.01, top, left + 0.01, top + 0.03, left, top + 0.03],
        flags: blank ? GENERATED : 0,
      });
    });
    glyphs.push({ code: 10, quad: [0, 0, 0, 0, 0, 0, 0, 0], flags: GENERATED | LINE_END });
  });
  const buffer = new ArrayBuffer(16 + glyphs.length * 24);
  const view = new DataView(buffer);
  view.setUint32(0, CHARS_MAGIC, true);
  view.setUint32(4, 1, true);
  view.setUint32(8, glyphs.length, true);
  view.setUint32(12, rotation, true);
  glyphs.forEach((glyph, index) => {
    const at = 16 + index * 24;
    view.setUint32(at, glyph.code, true);
    glyph.quad.forEach((value, corner) => view.setUint16(at + 4 + corner * 2, Math.round(value * 65535), true));
    view.setUint16(at + 20, glyph.flags, true);
  });
  return buffer;
}

const PAGE_ONE = ['Era uma vez um leitor', 'que marcava tudo'];
const PAGE_TWO = ['na margem do livro', 'para lembrar depois'];

/** Two 1000 × 1000 pixel pages, one above the other. */
function setup(rotation = 0): { annotator: Annotator; placed: PlacedPages; posted: AnnotatorMessage[] } {
  const posted: AnnotatorMessage[] = [];
  const widths = new Float32Array([500, 500]);
  const heights = new Float32Array([500, 500]);
  const annotator = new Annotator(widths, heights, (message) => posted.push(message));
  const placed = new PlacedPages(4);
  placed.add(0, 0, 0, 2);
  placed.add(1, 0, 1000, 2);
  annotator.setChars(0, layer(PAGE_ONE, rotation));
  annotator.setChars(1, layer(PAGE_TWO));
  return { annotator, placed, posted };
}

/** The canvas point of character `column` of line `row` of a page, at its left or right edge. */
const at = (page: number, row: number, column: number, edge: 'left' | 'middle' | 'right' = 'middle') => ({
  x: (0.1 + column * 0.01 + (edge === 'left' ? 0.002 : edge === 'right' ? 0.008 : 0.005)) * 1000,
  y: page * 1000 + (0.1 + row * 0.05 + 0.015) * 1000,
});

describe('selecting on a fixed layout', () => {
  it('asks for the characters of pages on the screen once', () => {
    const posted: AnnotatorMessage[] = [];
    const annotator = new Annotator(new Float32Array([500]), new Float32Array([500]), (message) => posted.push(message));
    const placed = new PlacedPages(2);
    placed.add(0, 0, 0, 2);
    // Only once it has stayed on the screen: pages that fly past are not asked about.
    annotator.want(placed, 0);
    annotator.want(placed, 200);
    expect(posted).toEqual([]);
    annotator.want(placed, 320);
    annotator.want(placed, 400);
    expect(posted).toEqual([{ type: 'need-chars', pages: [0] }]);
  });

  it('a drag on text selects from character to character; off text it pans', () => {
    const { annotator, placed, posted } = setup();
    expect(annotator.pointerDown(900, 900, 0, placed)).toBe(false);
    const start = at(0, 0, 4, 'left');
    expect(annotator.pointerDown(start.x, start.y, 1000, placed)).toBe(true);
    const end = at(0, 0, 10, 'right');
    annotator.pointerMove(end.x, end.y, placed);
    annotator.pointerUp(placed);
    expect(annotator.spans()).toEqual([{ page: 0, start: 4, end: 11 }]);
    const last = posted.at(-1);
    expect(last?.type).toBe('selection');
    expect(last?.type === 'selection' && last.text).toBe('uma vez');
  });

  it('a double click selects a word, a triple one the line', () => {
    const { annotator, placed, posted } = setup();
    const point = at(0, 0, 16);
    for (let n = 0; n < 2; n++) {
      annotator.pointerDown(point.x, point.y, 5000 + n * 100, placed);
      annotator.pointerUp(placed);
    }
    let last = posted.at(-1);
    expect(last?.type === 'selection' && last.text).toBe('leitor');
    annotator.pointerDown(point.x, point.y, 5200, placed);
    annotator.pointerUp(placed);
    last = posted.at(-1);
    expect(last?.type === 'selection' && last.text).toBe('Era uma vez um leitor');
  });

  it('a drag across the page break selects on both pages', () => {
    const { annotator, placed, posted } = setup();
    const start = at(0, 1, 4, 'left');
    annotator.pointerDown(start.x, start.y, 9000, placed);
    const end = at(1, 0, 1, 'right');
    annotator.pointerMove(end.x, end.y, placed);
    annotator.pointerUp(placed);
    expect(annotator.spans()).toEqual([
      { page: 0, start: 26, end: 39 },
      { page: 1, start: 0, end: 2 },
    ]);
    const last = posted.at(-1);
    expect(last?.type === 'selection' && last.text).toBe('marcava tudo\n\nna');
  });

  it('a click on a mark opens it, and marks and the selection are drawn where the text is', () => {
    const { annotator, placed, posted } = setup();
    annotator.setMarks([
      { id: 'a', color: 'green', kind: 'highlight', note: true, page: 0, quads: [[0.14, 0.1, 0.21, 0.1, 0.21, 0.13, 0.14, 0.13]] },
    ]);
    const point = at(0, 0, 5);
    annotator.pointerDown(point.x, point.y, 20_000, placed);
    annotator.pointerUp(placed);
    expect(posted.at(-1)).toMatchObject({ type: 'mark-click', id: 'a', rect: { x: 140, y: 100, width: 70, height: 30 } });
    const drawn: { corners: number[]; light: number[]; dark: number[] }[] = [];
    annotator.draw({ mark: (corners, light, dark) => drawn.push({ corners: Array.from(corners), light: Array.from(light), dark: Array.from(dark) }) }, placed);
    // The highlight, and the note's small square.
    expect(drawn).toHaveLength(2);
    expect(drawn[0].corners.map((value) => Math.round(value))).toEqual([140, 100, 210, 100, 210, 130, 140, 130]);
    // Over light paper the fill; over a dark page the fill dimmed, so it shows without hiding the type.
    expect(drawn[0].light).toEqual([0.66, 0.9, 0.63]);
    expect(drawn[0].dark.map((value) => Math.round(value * 1000) / 1000)).toEqual([0.363, 0.495, 0.347]);
    // The note tab is ink on light paper and the bright fill on a dark page.
    expect(drawn[1].dark).toEqual([0.66, 0.9, 0.63]);
  });

  it('on a page shown turned, a point of the screen is turned back before it meets the text', () => {
    const { annotator, placed } = setup(1);
    // Turned a quarter clockwise, the stored point (u, v) is shown at (1 - v, u).
    const u = 0.1 + 4 * 0.01 + 0.002;
    const v = 0.1 + 0.015;
    expect(annotator.pointerDown((1 - v) * 1000, u * 1000, 30_000, placed)).toBe(true);
    const u2 = 0.1 + 10 * 0.01 + 0.008;
    annotator.pointerMove((1 - v) * 1000, u2 * 1000, placed);
    annotator.pointerUp(placed);
    expect(annotator.spans()).toEqual([{ page: 0, start: 4, end: 11 }]);
  });
});
