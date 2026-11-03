import { describe, expect, it } from 'vitest';

import { MAX_PAGE_PIXELS, MAX_SCALE, MIN_SCALE, SCALE_ONE } from '../protocol';
import {
  clampScaleToPage,
  pagePixels,
  pageScaleKey,
  placeholderScale,
  quantizeScale,
  tileCount,
  tileIndex,
} from './tiles';

describe('tile arithmetic', () => {
  it('matches crates/musaeum-shm/src/tile.rs at a scale no power of two lands on', () => {
    // The same numbers as \`any_scale_is_a_grid_of_its_own\` on the Rust side.
    const scale = quantizeScale(1.37);
    expect(scale).toBe(5612);
    expect(pagePixels(612, scale)).toBe(839);
    expect(pagePixels(792, scale)).toBe(1085);
    expect(tileCount(612, scale)).toBe(2);
    expect(tileCount(792, scale)).toBe(3);
  });

  it('keeps scales inside what the host accepts', () => {
    expect(quantizeScale(1e9)).toBe(MAX_SCALE);
    expect(quantizeScale(0)).toBe(MIN_SCALE);
    expect(quantizeScale(Number.NaN)).toBe(SCALE_ONE);
    expect(pagePixels(0, SCALE_ONE)).toBe(1);
  });

  it('gives a placeholder that fits one tile and is close to the finest that does', () => {
    for (const [width, height] of [
      [612, 792],
      [1224, 792],
      [500, 500],
      [3, 14_000],
    ]) {
      const scale = placeholderScale(width, height);
      expect(tileCount(width, scale)).toBe(1);
      expect(tileCount(height, scale)).toBe(1);
      const finer = Math.ceil(scale * 1.01);
      if (scale > MIN_SCALE) {
        expect(Math.max(tileCount(width, finer), tileCount(height, finer))).toBeGreaterThan(1);
      }
    }
  });

  it('brings the scale down for a page too large for a grid at it', () => {
    expect(clampScaleToPage(5612, 612, 792)).toBe(5612);
    const banner = clampScaleToPage(MAX_SCALE, 20_000, 100);
    expect(banner).toBeLessThan(MAX_SCALE);
    expect(pagePixels(20_000, banner)).toBeLessThanOrEqual(MAX_PAGE_PIXELS);
    expect(pagePixels(20_000, banner + 1)).toBeGreaterThan(MAX_PAGE_PIXELS);
  });

  it('builds keys that stay exact at their largest values', () => {
    const largest = pageScaleKey(4_194_303, MAX_SCALE);
    expect(Number.isSafeInteger(largest)).toBe(true);
    expect(pageScaleKey(4_194_303, MAX_SCALE - 1)).toBe(largest - 1);
    expect(pageScaleKey(1, 0)).not.toBe(pageScaleKey(0, MAX_SCALE));
    expect(tileIndex(1023, 1023)).toBeLessThan(2 ** 20);
    expect(tileIndex(0, 1)).not.toBe(tileIndex(1, 0));
  });
});
