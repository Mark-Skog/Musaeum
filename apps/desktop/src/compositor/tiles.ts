/**
 * Tile arithmetic, mirroring `crates/musaeum-shm/src/tile.rs`: a tile's scale is in device pixels
 * per point, in units of 1/`SCALE_ONE`, and a page edge is `max(1, round(points × scale / 4096))`
 * pixels long. Any scale can be rendered, so what is on screen is rasterized at the size it is
 * shown. The GPU resamples a tile only while a zoom is under way, or for a page too large for a
 * grid at the scale it is shown at.
 */

import { MAX_PAGE_PIXELS, MAX_SCALE, MIN_SCALE, SCALE_ONE, TILE_SIZE } from '../protocol';

/** The scale that draws `pixelsPerPoint` device pixels per point, as close as the units allow. */
export function quantizeScale(pixelsPerPoint: number): number {
  const scale = Math.round(pixelsPerPoint * SCALE_ONE);
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, Number.isFinite(scale) ? scale : SCALE_ONE));
}

/** Device pixels per point at `scale`. */
export function scaleFactor(scale: number): number {
  return scale / SCALE_ONE;
}

/** Pixels along a page edge of `lengthPt` points at `scale`. */
export function pagePixels(lengthPt: number, scale: number): number {
  const pixels = Math.round((lengthPt * scale) / SCALE_ONE);
  return Number.isFinite(pixels) && pixels >= 1 ? pixels : 1;
}

/** Tiles needed along a page edge of `lengthPt` points at `scale`. */
export function tileCount(lengthPt: number, scale: number): number {
  return Math.ceil(pagePixels(lengthPt, scale) / TILE_SIZE);
}

/** The finest scale no finer than `scale` whose grid the host accepts for this page. */
export function clampScaleToPage(scale: number, widthPt: number, heightPt: number): number {
  const longest = Math.max(widthPt, heightPt, 1);
  const limit = Math.floor((MAX_PAGE_PIXELS * SCALE_ONE) / longest);
  let clamped = Math.max(MIN_SCALE, Math.min(scale, limit));
  // Rounding inside `pagePixels` can still land one pixel over.
  while (clamped > MIN_SCALE && pagePixels(longest, clamped) > MAX_PAGE_PIXELS) {
    clamped--;
  }
  return clamped;
}

/** The finest scale at which the whole page fits in one tile: a cheap stand-in for it. */
export function placeholderScale(widthPt: number, heightPt: number): number {
  const longest = Math.max(widthPt, heightPt, 1);
  const fits = Math.floor((TILE_SIZE * SCALE_ONE) / longest);
  let scale = Math.max(MIN_SCALE, Math.min(MAX_SCALE, fits));
  while (scale > MIN_SCALE && pagePixels(longest, scale) > TILE_SIZE) {
    scale--;
  }
  return scale;
}

/**
 * Half of a tile's identity, exact in a double: 22 bits of page and 18 of scale. The other half is
 * [`tileIndex`]. Two numbers rather than one, because one would need more than 53 bits.
 */
export function pageScaleKey(page: number, scale: number): number {
  return page * 262_144 + scale;
}

/** The other half: the tile's place in its grid, which is never more than 1024 tiles a side. */
export function tileIndex(x: number, y: number): number {
  return y * 1024 + x;
}
