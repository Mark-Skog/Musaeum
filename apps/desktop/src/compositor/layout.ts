/**
 * Page layout and tile arithmetic. The tile rules mirror `crates/musaeum-shm/src/tile.rs`:
 * level of detail `lod` means `2^lod` device pixels per point, and a page edge is
 * `max(1, round(points × 2^lod))` pixels long.
 */

import { GEOMETRY_MAGIC, MAX_LOD, MIN_LOD, TILE_SIZE } from '../protocol';

export interface DocumentLayout {
  pageCount: number;
  /** Page widths, in points. */
  widths: Float32Array;
  /** Page heights, in points. */
  heights: Float32Array;
  /** Y of each page's top edge in the continuous column, in points. */
  tops: Float64Array;
  totalHeight: number;
  maxWidth: number;
}

/** Decodes the host's page-size table into `[width, height]` pairs. */
export function parseGeometry(bytes: ArrayBuffer): Float32Array {
  if (bytes.byteLength < 8) {
    throw new Error('geometry table is too short');
  }
  const header = new DataView(bytes);
  if (header.getUint32(0, true) !== GEOMETRY_MAGIC) {
    throw new Error('geometry table has a bad magic number');
  }
  const count = header.getUint32(4, true);
  if (bytes.byteLength !== 8 + count * 8) {
    throw new Error('geometry table length does not match its page count');
  }
  return new Float32Array(bytes, 8, count * 2);
}

/** Lays pages out in one centred column. The page sizes come from the file, unmodified. */
export function buildLayout(sizes: Float32Array, gapPt: number): DocumentLayout {
  const pageCount = sizes.length / 2;
  const widths = new Float32Array(pageCount);
  const heights = new Float32Array(pageCount);
  const tops = new Float64Array(pageCount);
  let y = 0;
  let maxWidth = 0;
  for (let page = 0; page < pageCount; page++) {
    widths[page] = sizes[page * 2];
    heights[page] = sizes[page * 2 + 1];
    tops[page] = y;
    y += heights[page] + gapPt;
    maxWidth = Math.max(maxWidth, widths[page]);
  }
  return {
    pageCount,
    widths,
    heights,
    tops,
    totalHeight: pageCount > 0 ? y - gapPt : 0,
    maxWidth,
  };
}

/** The last page whose top edge is at or above `y`, by binary search. */
export function pageAt(layout: DocumentLayout, y: number): number {
  let low = 0;
  let high = layout.pageCount - 1;
  while (low < high) {
    const middle = (low + high + 1) >>> 1;
    if (layout.tops[middle] <= y) {
      low = middle;
    } else {
      high = middle - 1;
    }
  }
  return low;
}

/** The coarsest level at least as sharp as the screen. Tiles are only ever scaled down. */
export function lodForScale(scale: number): number {
  // Adding zero turns the -0 that `ceil` gives for scales just above 1 into a plain 0.
  return Math.min(MAX_LOD, Math.max(MIN_LOD, Math.ceil(Math.log2(scale) - 1e-9) + 0));
}

/** The finest level at which a whole page fits in one tile: a cheap placeholder. */
export function wholePageLod(widthPt: number, heightPt: number): number {
  const longest = Math.max(widthPt, heightPt, 1);
  return Math.min(MAX_LOD, Math.max(MIN_LOD, Math.floor(Math.log2(TILE_SIZE / longest))));
}

/** Points covered by one tile's edge at `lod`. */
export function tileSpanPt(lod: number): number {
  return TILE_SIZE / 2 ** lod;
}

/** Tiles needed along a page edge of `lengthPt` points at `lod`. */
export function tileCount(lengthPt: number, lod: number): number {
  return Math.ceil(Math.max(1, Math.round(lengthPt * 2 ** lod)) / TILE_SIZE);
}

/**
 * A map key for a tile. Multiplication rather than bit shifts, because 32-bit shifts could
 * not hold it: 25 bits of page, 4 of level, 12 each of row and column fit a double exactly.
 */
export function tileKey(page: number, lod: number, x: number, y: number): number {
  return page * 268_435_456 + (lod - MIN_LOD) * 16_777_216 + y * 4096 + x;
}
