import { describe, expect, it } from 'vitest';
import { encodeLines, type Line } from './lines-fixture';
import {
  boxQuad,
  clipOf,
  decodeLines,
  dhashRgba,
  lineAt,
  pieceQuads,
  pointAt,
  snapRange,
  storedQuads,
  strokeQuads,
  wordEnd,
  wordStart,
  type LinePage,
} from './lines';

/** Five straight lines, 0.1 to 0.9 across, every 0.04 down from 0.2; words of a fifth with a space after each. */
const WORDS: [number, number][] = [
  [0.18, 0.2],
  [0.38, 0.4],
  [0.58, 0.6],
  [0.78, 0.8],
];
function straight(count = 5): LinePage {
  const lines: Line[] = [];
  for (let row = 0; row < count; row++) {
    const y = 0.2 + row * 0.04;
    lines.push({ points: [[0.1, y], [0.9, y]], gaps: WORDS });
  }
  return decodeLines(encodeLines(lines));
}

const close = (a: number, b: number): boolean => Math.abs(a - b) < 1e-5;

describe('the line model', () => {
  it('decodes the hash, and points turned into the frame they are shown in', () => {
    for (const rotation of [0, 1, 2, 3]) {
      const page = decodeLines(encodeLines([{ points: [[0.1, 0.3], [0.5, 0.31], [0.9, 0.3]] }], { rotation }));
      expect(page.pageHash).toBe('0123456789abcdef');
      expect(page.rotation).toBe(rotation);
      const line = page.lines[0];
      expect(close(line.points[0], 0.1) && close(line.points[line.points.length - 2], 0.9)).toBe(true);
      expect(close(line.points[3], 0.31)).toBe(true);
      expect(line.length).toBeCloseTo(Math.hypot(0.4, 0.01) * 2, 5);
    }
  });

  it('refuses what is not a model', () => {
    const buffer = encodeLines([{ points: [[0.1, 0.3], [0.9, 0.3]] }]);
    new DataView(buffer).setUint32(8, 0, true);
    expect(() => decodeLines(buffer)).toThrow();
    expect(() => decodeLines(encodeLines([{ points: [[0.1, 0.3], [0.9, 0.3]] }]).slice(0, 40))).toThrow();
    expect(() => decodeLines(new ArrayBuffer(4))).toThrow();
    // A model of the first detector, whose gaps were fractions of the line's width: traced again.
    expect(() => decodeLines(encodeLines([{ points: [[0.1, 0.3], [0.9, 0.3]] }], { version: 1 }))).toThrow();
  });

  it('finds the line under a point, or the nearest', () => {
    const page = straight();
    expect(lineAt(page, 0.5, 0.196, false)?.index).toBe(0);
    expect(lineAt(page, 0.5, 0.238, false)?.index).toBe(1);
    expect(lineAt(page, 0.5, 0.6, false)).toBeNull();
    expect(lineAt(page, 0.5, 0.6, true)?.index).toBe(4);
  });

  it('snaps to the edges of words, inside a space to the word on each side', () => {
    const line = straight().lines[0];
    expect(wordStart(line, 0.3)).toBeCloseTo(0.2);
    expect(wordEnd(line, 0.3)).toBeCloseTo(0.38);
    expect(wordStart(line, 0.05)).toBe(0);
    expect(wordEnd(line, 0.95)).toBe(1);
    // In the space between the first and second words.
    expect(wordStart(line, 0.19)).toBeCloseTo(0.2);
    expect(wordEnd(line, 0.19)).toBeCloseTo(0.18);
  });
});

describe('selecting along lines', () => {
  const page = straight();
  const x = (t: number): number => 0.1 + 0.8 * t;

  it('takes the first line from the start, the middle ones whole and the last to the end', () => {
    const pieces = snapRange(page, { x: x(0.3), y: 0.198 }, { x: x(0.5), y: 0.318 }, true);
    expect(pieces.map((piece) => piece.index)).toEqual([0, 1, 2, 3]);
    expect(pieces[0].from).toBeCloseTo(0.2);
    expect(pieces[0].to).toBe(1);
    expect([pieces[1].from, pieces[1].to, pieces[2].from, pieces[2].to]).toEqual([0, 1, 0, 1]);
    expect(pieces[3].from).toBe(0);
    expect(pieces[3].to).toBeCloseTo(0.58);
  });

  it('takes nothing for a drag far from every line, as on a photograph', () => {
    expect(snapRange(page, { x: 0.5, y: 0.6 }, { x: 0.7, y: 0.8 }, true)).toEqual([]);
    // Starting beside the first word still counts.
    expect(snapRange(page, { x: 0.09, y: 0.198 }, { x: x(0.3), y: 0.198 }, true)).toHaveLength(1);
  });

  it('the same backwards', () => {
    const forwards = snapRange(page, { x: x(0.3), y: 0.198 }, { x: x(0.5), y: 0.318 }, true);
    const backwards = snapRange(page, { x: x(0.5), y: 0.318 }, { x: x(0.3), y: 0.198 }, true);
    expect(backwards).toEqual(forwards);
  });

  it('with snapping off, the ends stay where the pointer is', () => {
    const pieces = snapRange(page, { x: x(0.3), y: 0.198 }, { x: x(0.5), y: 0.238 }, false);
    expect(pieces[0].from).toBeCloseTo(0.3);
    expect(pieces[1].to).toBeCloseTo(0.5);
  });

  it('within one line, from word to word, and a click in a space takes the word after it', () => {
    const pieces = snapRange(page, { x: x(0.25), y: 0.2 }, { x: x(0.45), y: 0.2 }, true);
    expect(pieces).toHaveLength(1);
    expect(pieces[0].from).toBeCloseTo(0.2);
    expect(pieces[0].to).toBeCloseTo(0.58);
    const word = snapRange(page, { x: x(0.19), y: 0.2 }, { x: x(0.19), y: 0.2 }, true);
    expect(word[0].from).toBeCloseTo(0.2);
    expect(word[0].to).toBeCloseTo(0.38);
  });

  it('follows a curved line near the spine segment by segment', () => {
    const curved = decodeLines(encodeLines([{ points: [[0.1, 0.3], [0.5, 0.3], [0.7, 0.31], [0.9, 0.34]] }]));
    const line = curved.lines[0];
    const quads = pieceQuads(curved, line, { index: 0, from: 0, to: 1 }, true);
    expect(quads).toHaveLength(3);
    // Each quad's bottom edge sits on the baseline at its ends.
    expect(quads[2][5] - line.descent).toBeCloseTo(0.34);
    expect(quads[1][7] - line.descent).toBeCloseTo(0.3);
    const box = pieceQuads(curved, line, { index: 0, from: 0, to: 1 }, false);
    expect(box).toHaveLength(1);
    expect(box[0][1]).toBeCloseTo(0.3 - line.ascent);
    expect(box[0][5]).toBeCloseTo(0.34 + line.descent);
  });

  it('keeps quads in the stored frame, which turns back into the same place', () => {
    const quad = boxQuad({ x: 0.2, y: 0.3 }, { x: 0.6, y: 0.5 });
    for (const rotation of [0, 1, 2, 3]) {
      const [stored] = storedQuads([quad], rotation);
      const clip = clipOf([stored]);
      const width = clip[2] - clip[0];
      expect(width).toBeCloseTo(rotation % 2 === 0 ? 0.4 : 0.2);
    }
  });

  it('snaps within a millisecond on a page of two dense columns', () => {
    const lines: Line[] = [];
    for (const [left, right, column] of [[0.08, 0.48, 0], [0.52, 0.92, 1]] as const) {
      for (let row = 0; row < 70; row++) {
        const y = 0.06 + row * 0.0125;
        const points: [number, number][] = [];
        for (let step = 0; step <= 8; step++) {
          points.push([left + ((right - left) * step) / 8, y + Math.sin(step) * 0.001]);
        }
        const gaps: [number, number][] = [];
        for (let word = 1; word < 12; word++) {
          gaps.push([word / 12 - 0.01, word / 12 + 0.005]);
        }
        lines.push({ column, points, gaps, ascent: 0.007, descent: 0.002 });
      }
    }
    const dense = decodeLines(encodeLines(lines, { columns: 2 }));
    let quads = 0;
    const snap = (step: number): void => {
      const pieces = snapRange(dense, { x: 0.1 + (step % 30) * 0.01, y: 0.07 }, { x: 0.55 + (step % 37) * 0.01, y: 0.06 + (step % 70) * 0.0125 }, true);
      for (const piece of pieces) {
        quads += pieceQuads(dense, dense.lines[piece.index], piece, true).length;
      }
    };
    // As in the compositor, which has snapped before the drag that counts: compiled, not cold.
    for (let step = 0; step < 100; step++) {
      snap(step);
    }
    // Each drag timed three times and the quickest kept: the other test files run beside this one,
    // and a time slice they take, or a collection of their garbage, is no cost of the snap.
    const times: number[] = [];
    for (let step = 0; step < 1000; step++) {
      let quickest = Infinity;
      for (let attempt = 0; attempt < 3; attempt++) {
        const started = performance.now();
        snap(step);
        quickest = Math.min(quickest, performance.now() - started);
      }
      times.push(quickest);
    }
    times.sort((a, b) => a - b);
    const median = times[500];
    const p99 = times[990];
    console.info(`snap over 140 lines: median ${(median * 1000).toFixed(1)} µs, p99 ${(p99 * 1000).toFixed(1)} µs, max ${(times[999] * 1000).toFixed(1)} µs`);
    expect(quads).toBeGreaterThan(0);
    expect(p99).toBeLessThan(1);
  });
});

describe('lines at other angles', () => {
  // A page 0.75 as wide as it is high, as a letter page is; one unit is its height.
  const aspect = 0.75;
  // A label read upward beside a figure, at x = 0.8 as shown, from y = 0.7 up to y = 0.3; its
  // letters stand to the left of its baseline. And a line of text at 45°, read up to the right.
  const upward: Line = { points: [[0.8, 0.7], [0.8, 0.3]], gaps: [[0.45, 0.5]], turned: true, ascent: 0.012, descent: 0.003 };
  const diagonal: Line = { points: [[0.2, 0.6], [0.2 + 0.2 / aspect, 0.4]], gaps: [[0.48, 0.52]], turned: true, ascent: 0.012, descent: 0.003 };
  const page = decodeLines(encodeLines([upward, diagonal]), aspect);
  const [up, slant] = page.lines;

  it('measures a line along its length, in one unit across the page', () => {
    expect(up.turned).toBe(true);
    expect(up.length).toBeCloseTo(0.4, 5);
    expect(slant.length).toBeCloseTo(0.2 * Math.SQRT2, 5);
    const middle = pointAt(page, slant, 0.5);
    expect(middle.x).toBeCloseTo(0.2 + 0.1 / aspect, 5);
    expect(middle.y).toBeCloseTo(0.5, 5);
    expect(middle.dx).toBeCloseTo(Math.SQRT1_2, 5);
    expect(middle.dy).toBeCloseTo(-Math.SQRT1_2, 5);
  });

  it('finds a turned line under a point on the side its letters stand', () => {
    // Its letters are to the left of x = 0.8, in heights of the page: 0.012 of them is 0.016 of
    // the width.
    expect(lineAt(page, 0.8 - 0.006 / aspect, 0.5, false)).toBe(up);
    expect(lineAt(page, 0.8 + 0.01 / aspect, 0.5, false)).toBeNull();
    expect(lineAt(page, 0.8 - 0.006 / aspect, 0.5, true)).toBe(up);
    const onSlant = pointAt(page, slant, 0.25);
    // Up across a line going up to the right is up and to the left.
    expect(lineAt(page, onSlant.x + (onSlant.dy * 0.006) / aspect, onSlant.y - onSlant.dx * 0.006, false)).toBe(slant);
  });

  it('snaps a drag along a turned line to its words', () => {
    const near = (t: number): { x: number; y: number } => {
      const at = pointAt(page, up, t);
      return { x: at.x + (at.dy * 0.005) / aspect, y: at.y - at.dx * 0.005 };
    };
    const pieces = snapRange(page, near(0.1), near(0.3), true);
    expect(pieces).toEqual([{ index: 0, from: 0, to: expect.closeTo(0.45, 5) as unknown as number }]);
    const backwards = snapRange(page, near(0.3), near(0.1), true);
    expect(backwards).toEqual(pieces);
  });

  it('draws a turned stretch as a quad turned with it', () => {
    const [quad] = pieceQuads(page, up, { index: 0, from: 0, to: 0.5 }, false);
    // Start of the stretch at the bottom, its end halfway up; the ink to the left of the baseline.
    const xs = [quad[0], quad[2], quad[4], quad[6]];
    const ys = [quad[1], quad[3], quad[5], quad[7]];
    expect(Math.min(...xs)).toBeCloseTo(0.8 - 0.012 / aspect, 5);
    expect(Math.max(...xs)).toBeCloseTo(0.8 + 0.003 / aspect, 5);
    expect(Math.min(...ys)).toBeCloseTo(0.5, 5);
    expect(Math.max(...ys)).toBeCloseTo(0.7, 5);
    // Top left, read as the line reads: at its start, on the side of its letters.
    expect(quad[0]).toBeCloseTo(0.8 - 0.012 / aspect, 5);
    expect(quad[1]).toBeCloseTo(0.7, 5);
    const follow = pieceQuads(page, slant, { index: 1, from: 0, to: 1 }, true);
    expect(follow).toHaveLength(1);
    // Its top edge runs parallel to the baseline, 0.012 above it.
    const [tlx, tly, trx, try_] = follow[0];
    expect(((trx - tlx) * aspect) / (try_ - tly)).toBeCloseTo(-1, 5);
    const start = pointAt(page, slant, 0);
    expect(Math.hypot((tlx - start.x) * aspect, tly - start.y)).toBeCloseTo(0.012, 5);
  });
});

describe('a page set on its side', () => {
  // The lines of a page turned the long way, read up the page, one after another from left to
  // right; and a scanner's mark at 0° under them, which the model lists between the second and
  // the third, as a block of its own.
  const aspect = 0.7;
  const up = (x: number): Line => ({
    points: [[x, 0.8], [x, 0.2]],
    gaps: [[0.3, 0.32], [0.6, 0.62]],
    turned: true,
    ascent: 0.012,
    descent: 0.003,
  });
  const mark: Line = { points: [[0.25, 0.83], [0.5, 0.83]], gaps: [[0.4, 0.45]], ascent: 0.012, descent: 0.003 };
  const page = decodeLines(encodeLines([up(0.3), up(0.35), mark, up(0.4), up(0.45)]), aspect);
  // Beside a line, on the side its letters stand: to its left.
  const near = (index: number, t: number): { x: number; y: number } => {
    const at = pointAt(page, page.lines[index], t);
    return { x: at.x + (at.dy * 0.005) / aspect, y: at.y - at.dx * 0.005 };
  };

  it('takes the turned lines a drag runs over, and not the level mark the model puts between them', () => {
    const pieces = snapRange(page, near(0, 0.1), near(4, 0.9), true);
    expect(pieces.map((piece) => piece.index)).toEqual([0, 1, 3, 4]);
    expect(pieces[0]).toEqual({ index: 0, from: 0, to: 1 });
    expect(pieces[3]).toEqual({ index: 4, from: 0, to: 1 });
    expect(snapRange(page, near(4, 0.9), near(0, 0.1), true)).toEqual(pieces);
  });

  it('ends a drag on the nearest line its own way, not on the mark under the pointer', () => {
    // The drag starts up the first line and ends on the mark: the mark is not taken, and the
    // nearest turned line, the second, ends the drag where the pointer is beside it.
    const onMark = pointAt(page, page.lines[2], 0.1);
    const pieces = snapRange(page, near(0, 0.5), { x: onMark.x, y: onMark.y - 0.004 }, true);
    expect(pieces.every((piece) => piece.index !== 2)).toBe(true);
    // A drag along the mark alone takes the mark.
    const along = snapRange(page, { x: onMark.x, y: onMark.y - 0.004 }, pointAt(page, page.lines[2], 0.9), true);
    expect(along.map((piece) => piece.index)).toEqual([2]);
  });
});

describe('a stroke and the hash', () => {
  it('draws a stroke as overlapping bands, as wide as asked', () => {
    const quads = strokeQuads([0.1, 0.1, 0.3, 0.1, 0.3, 0.3], 10, 500, 800);
    expect(quads).toHaveLength(2);
    // Ten points high, past the start by five.
    expect((quads[0][7] - quads[0][1]) * 800).toBeCloseTo(10);
    expect(quads[0][0] * 500).toBeCloseTo(0.1 * 500 - 5);
    expect(strokeQuads([0.5, 0.5], 10, 500, 800)).toHaveLength(1);
    expect(strokeQuads([], 10, 500, 800)).toHaveLength(0);
  });

  it('hashes a picture to the same bits as musaeum_vision::dhash', () => {
    const width = 37;
    const height = 23;
    const pixels = new Uint8Array(width * height * 4);
    for (let y = 0; y < height; y++) {
      for (let x = 0; x < width; x++) {
        const v = (x * 7 + y * 13 + ((x * y) % 17) * 5) % 256;
        pixels.set([v, (v * 3) % 256, 255 - v, 255], (y * width + x) * 4);
      }
    }
    // Pinned on the Rust side too, in hash.rs.
    expect(dhashRgba(pixels, width, height)).toBe('268e1db4d2979746');
  });
});
