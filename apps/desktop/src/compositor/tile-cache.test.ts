import { describe, expect, it } from 'vitest';

import { TILE_BYTES } from '../protocol';
import { defaultBudgetBytes, layersForBudget, MIN_LAYERS, TileCache } from './tile-cache';

const MIB = 1024 * 1024;

describe('the budget', () => {
  it('defaults to four screens of tiles, within reason', () => {
    expect(defaultBudgetBytes(1920, 1080)).toBe(80 * TILE_BYTES);
    expect(defaultBudgetBytes(3840, 2160)).toBe(216 * TILE_BYTES);
    expect(defaultBudgetBytes(800, 600)).toBe(64 * TILE_BYTES);
    expect(defaultBudgetBytes(7680, 4320)).toBe(256 * TILE_BYTES);
  });

  it('is spent in whole tiles, never past what the GPU allows', () => {
    expect(TILE_BYTES).toBe(MIB);
    expect(layersForBudget(100 * MIB, 2048)).toBe(100);
    expect(layersForBudget(100.9 * MIB, 2048)).toBe(100);
    expect(layersForBudget(10 * MIB, 2048)).toBe(MIN_LAYERS);
    expect(layersForBudget(10_000 * MIB, 2048)).toBe(2048);
  });
});

describe('TileCache', () => {
  it('fills empty layers before evicting anything', () => {
    const cache = new TileCache(3);
    expect(cache.allocate(10, 4096, 0, 0, 1)).toBe(0);
    expect(cache.allocate(10, 4096, 1, 0, 1)).toBe(1);
    expect(cache.allocate(11, 5612, 0, 0, 1)).toBe(2);
    expect(cache.evictions).toBe(0);
    expect(cache.resident).toBe(3);
    expect(cache.find(10, 4096, 1, 0)).toBe(1);
    expect(cache.find(10, 4096, 0, 1)).toBe(-1);
    expect(cache.find(10, 5612, 1, 0)).toBe(-1);
  });

  it('evicts the least recently used tile, whatever its scale', () => {
    const cache = new TileCache(3);
    cache.allocate(1, 4096, 0, 0, 1);
    cache.allocate(2, 4096, 0, 0, 2);
    cache.allocate(3, 4096, 0, 0, 3);
    cache.touch(cache.find(1, 4096, 0, 0), 4);
    expect(cache.allocate(4, 8192, 0, 0, 5)).toBe(1);
    expect(cache.find(2, 4096, 0, 0)).toBe(-1);
    expect(cache.evictions).toBe(1);
    expect(cache.resident).toBe(3);
  });

  it('never evicts what the current frame draws', () => {
    const cache = new TileCache(2);
    cache.allocate(1, 4096, 0, 0, 7);
    cache.allocate(2, 4096, 0, 0, 7);
    expect(cache.allocate(3, 4096, 0, 0, 7)).toBe(-1);
    expect(cache.find(1, 4096, 0, 0)).toBe(0);
  });

  it('knows which scales each page has, as tiles come and go', () => {
    const cache = new TileCache(3);
    cache.allocate(5, 5612, 0, 0, 1);
    cache.allocate(5, 2048, 0, 0, 2);
    cache.allocate(5, 5612, 1, 0, 3);
    expect(cache.scalesOf(5)).toEqual([2048, 5612]);
    expect(cache.scalesOf(6)).toEqual([]);
    // The 5612 tile of frame 1 goes first; another 5612 tile stays, so the scale does too.
    cache.allocate(6, 4096, 0, 0, 4);
    expect(cache.scalesOf(5)).toEqual([2048, 5612]);
    // Then the 2048 one, which was the last at that scale.
    cache.allocate(6, 4096, 1, 0, 5);
    expect(cache.scalesOf(5)).toEqual([5612]);
    expect(cache.scalesOf(6)).toEqual([4096]);
    cache.clear();
    expect(cache.scalesOf(5)).toEqual([]);
    expect(cache.resident).toBe(0);
  });
});
