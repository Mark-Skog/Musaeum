import { describe, expect, it } from 'vitest';
import {
  CHARS_MAGIC,
  CHARS_VERSION,
  decodeChars,
  GENERATED,
  hitTest,
  LINE_END,
  lineAt,
  lineQuads,
  paragraphAt,
  textOf,
  toShown,
  toStored,
  wordAt,
} from './chars';

/** Encodes lines as a layer, each character 1/100 wide and each line 1/20 high. */
function encode(lines: string[], rotation = 0, gapAfter: number[] = []): ArrayBuffer {
  const glyphs: { code: number; quad: number[]; flags: number }[] = [];
  let top = 0.05;
  lines.forEach((line, row) => {
    [...line].forEach((ch, column) => {
      const left = 0.05 + column * 0.01;
      const blank = ch === ' ';
      glyphs.push({
        code: ch.codePointAt(0) ?? 32,
        quad: blank ? [0, 0, 0, 0, 0, 0, 0, 0] : [left, top, left + 0.01, top, left + 0.01, top + 0.03, left, top + 0.03],
        flags: blank ? GENERATED : 0,
      });
    });
    glyphs.push({ code: 10, quad: [0, 0, 0, 0, 0, 0, 0, 0], flags: GENERATED | LINE_END });
    top += 0.05 + (gapAfter.includes(row) ? 0.05 : 0);
  });
  const buffer = new ArrayBuffer(16 + glyphs.length * 24);
  const view = new DataView(buffer);
  view.setUint32(0, CHARS_MAGIC, true);
  view.setUint32(4, CHARS_VERSION, true);
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

const LINES = ['Era uma vez um leitor', 'que marcava tudo o que', 'lia.', 'Outro parágrafo aqui.'];

describe('a page of characters', () => {
  it('decodes into lines with their boxes, refusing what is not a layer', () => {
    const page = decodeChars(encode(LINES));
    expect(page.lines).toHaveLength(4);
    expect(textOf(page, page.lines[0].start, page.lines[0].end)).toBe('Era uma vez um leitor\n');
    expect(page.lines[0].left).toBeCloseTo(0.05, 4);
    expect(page.lines[0].right).toBeCloseTo(0.05 + 21 * 0.01, 4);
    expect(() => decodeChars(new ArrayBuffer(8))).toThrow();
    const wrong = encode(LINES);
    new DataView(wrong).setUint32(4, 99, true);
    expect(() => decodeChars(wrong)).toThrow();
    const short = encode(LINES).slice(0, 100);
    expect(() => decodeChars(short)).toThrow();
  });

  it('finds the caret before or after the character a point is on', () => {
    const page = decodeChars(encode(LINES));
    // The right half of "m" in "uma", on the first line.
    const hit = hitTest(page, 0.05 + 5 * 0.01 + 0.007, 0.06);
    expect(hit?.index).toBe(5);
    expect(hit?.caret).toBe(6);
    expect(hit?.onText).toBe(true);
    // Far below everything: the last line, but not on text.
    const below = hitTest(page, 0.1, 0.9);
    expect(page.lineOf[below?.index ?? 0]).toBe(3);
    expect(below?.onText).toBe(false);
    // Past the end of a line: after its last character.
    const after = hitTest(page, 0.6, 0.06);
    expect(after?.caret).toBe(21);
  });

  it('knows words, lines and paragraphs', () => {
    const page = decodeChars(encode(LINES, 0, [2]));
    const leitor = textOf(page, ...wordAt(page, 17));
    expect(leitor).toBe('leitor');
    expect(textOf(page, ...lineAt(page, 25))).toBe('que marcava tudo o que');
    const [start, end] = paragraphAt(page, 3);
    expect(textOf(page, start, end)).toBe('Era uma vez um leitor\nque marcava tudo o que\nlia.');
    expect(textOf(page, ...paragraphAt(page, page.lines[3].start))).toBe('Outro parágrafo aqui.');
  });

  it('covers a stretch with one quad per line', () => {
    const page = decodeChars(encode(LINES));
    const quads = lineQuads(page, 15, 30);
    expect(quads).toHaveLength(2);
    expect(quads[0][0]).toBeCloseTo(0.05 + 15 * 0.01, 4);
    expect(quads[0][2]).toBeCloseTo(0.05 + 21 * 0.01, 4);
    expect(quads[1][1]).toBeCloseTo(0.1, 4);
  });

  it('turns points between the page as shown and as stored', () => {
    for (const rotation of [0, 1, 2, 3]) {
      const [x, y] = toShown(0.2, 0.7, rotation);
      const [u, v] = toStored(x, y, rotation);
      expect(u).toBeCloseTo(0.2, 9);
      expect(v).toBeCloseTo(0.7, 9);
    }
    // A quarter turn clockwise puts the stored top left at the shown top right.
    expect(toShown(0, 0, 1)).toEqual([1, 0]);
  });
});
