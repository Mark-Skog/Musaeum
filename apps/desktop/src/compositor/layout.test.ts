import { describe, expect, it } from 'vitest';

import { GEOMETRY_MAGIC, MAX_LOD, MIN_LOD } from '../protocol';
import {
  buildLayout,
  lodForScale,
  pageAt,
  parseGeometry,
  tileCount,
  tileKey,
  tileSpanPt,
  wholePageLod,
} from './layout';

function table(pages: Array<[number, number]>): ArrayBuffer {
  const bytes = new ArrayBuffer(8 + pages.length * 8);
  const view = new DataView(bytes);
  view.setUint32(0, GEOMETRY_MAGIC, true);
  view.setUint32(4, pages.length, true);
  pages.forEach(([width, height], index) => {
    view.setFloat32(8 + index * 8, width, true);
    view.setFloat32(12 + index * 8, height, true);
  });
  return bytes;
}

describe('parseGeometry', () => {
  it('reads the table the host produces', () => {
    expect(Array.from(parseGeometry(table([[612, 792], [842, 595]])))).toEqual([612, 792, 842, 595]);
  });

  it('rejects damaged tables', () => {
    expect(() => parseGeometry(new ArrayBuffer(4))).toThrow(/too short/);
    expect(() => parseGeometry(new ArrayBuffer(8))).toThrow(/magic/);
    expect(() => parseGeometry(table([[1, 1]]).slice(0, 12))).toThrow(/length/);
  });
});

describe('buildLayout', () => {
  const layout = buildLayout(new Float32Array([100, 200, 300, 50, 80, 100]), 10);

  it('stacks mixed page sizes with a gap between them', () => {
    expect(Array.from(layout.tops)).toEqual([0, 210, 270]);
    expect(layout.totalHeight).toBe(370);
    expect(layout.maxWidth).toBe(300);
  });

  it('handles an empty document', () => {
    const empty = buildLayout(new Float32Array(0), 10);
    expect(empty.pageCount).toBe(0);
    expect(empty.totalHeight).toBe(0);
  });

  it('finds the page at a position, including inside gaps and past the end', () => {
    expect(pageAt(layout, 0)).toBe(0);
    expect(pageAt(layout, 205)).toBe(0);
    expect(pageAt(layout, 210)).toBe(1);
    expect(pageAt(layout, 269)).toBe(1);
    expect(pageAt(layout, 9999)).toBe(2);
    expect(pageAt(layout, -5)).toBe(0);
  });
});

describe('levels of detail', () => {
  it('never picks a level blurrier than the screen', () => {
    expect(lodForScale(1)).toBe(0);
    expect(lodForScale(1.01)).toBe(1);
    expect(lodForScale(2)).toBe(1);
    expect(lodForScale(1.9)).toBe(1);
    expect(lodForScale(0.5)).toBe(-1);
    expect(lodForScale(0.26)).toBe(-1);
  });

  it('stays inside the protocol limits', () => {
    expect(lodForScale(1e6)).toBe(MAX_LOD);
    expect(lodForScale(1e-6)).toBe(MIN_LOD);
  });

  it('picks a whole-page level that really fits in one tile', () => {
    for (const [width, height] of [[612, 792], [842, 595], [500, 500], [2000, 3000]]) {
      const lod = wholePageLod(width, height);
      expect(tileCount(width, lod)).toBe(1);
      expect(tileCount(height, lod)).toBe(1);
      // One level finer would no longer fit.
      expect(Math.max(tileCount(width, lod + 1), tileCount(height, lod + 1))).toBeGreaterThan(1);
    }
  });
});

describe('tile arithmetic', () => {
  it('matches the Rust side, as asserted in musaeum-shm/src/tile.rs', () => {
    // 612 pt at 2 px/pt is 1224 px: two full tiles and a partial third.
    expect(tileCount(612, 1)).toBe(3);
    expect(tileCount(792, 1)).toBe(4);
    expect(tileCount(612, -1)).toBe(1);
    expect(tileCount(0, 0)).toBe(1);
  });

  it('computes how much of a page one tile spans', () => {
    expect(tileSpanPt(0)).toBe(512);
    expect(tileSpanPt(1)).toBe(256);
    expect(tileSpanPt(-1)).toBe(1024);
  });

  it('gives every tile a distinct key, even at the extremes', () => {
    const keys = new Set<number>();
    for (const page of [0, 1, 2000, 33_000_000]) {
      for (const lod of [MIN_LOD, -1, 0, 1, MAX_LOD]) {
        for (const x of [0, 1, 4095]) {
          for (const y of [0, 1, 4095]) {
            const key = tileKey(page, lod, x, y);
            expect(Number.isSafeInteger(key)).toBe(true);
            expect(keys.has(key)).toBe(false);
            keys.add(key);
          }
        }
      }
    }
  });
});
