import { describe, expect, it } from 'vitest';

import { layersForScreen, TileCache } from './tile-cache';

describe('layersForScreen', () => {
  it('keeps four screens of tiles', () => {
    expect(layersForScreen(1920, 1080)).toBe(80);
    expect(layersForScreen(2560, 1440)).toBe(96);
    expect(layersForScreen(3840, 2160)).toBe(216);
  });

  it('stays within what is reasonable', () => {
    expect(layersForScreen(800, 600)).toBe(64);
    expect(layersForScreen(7680, 4320)).toBe(256);
  });
});

describe('TileCache', () => {
  it('fills empty layers before evicting anything', () => {
    const cache = new TileCache(3);
    expect(cache.allocate(10, 1)).toBe(0);
    expect(cache.allocate(11, 1)).toBe(1);
    expect(cache.allocate(12, 1)).toBe(2);
    expect(cache.evictions).toBe(0);
    expect(cache.find(11)).toBe(1);
    expect(cache.find(99)).toBe(-1);
  });

  it('evicts the least recently used tile', () => {
    const cache = new TileCache(3);
    cache.allocate(10, 1);
    cache.allocate(11, 2);
    cache.allocate(12, 3);
    cache.touch(cache.find(10), 4);

    const layer = cache.allocate(13, 5);
    expect(layer).toBe(1);
    expect(cache.find(11)).toBe(-1);
    expect(cache.find(13)).toBe(1);
    expect(cache.find(10)).toBe(0);
    expect(cache.evictions).toBe(1);
  });

  it('never evicts what the current frame is using', () => {
    const cache = new TileCache(2);
    cache.allocate(10, 5);
    cache.allocate(11, 5);
    expect(cache.allocate(12, 5)).toBe(-1);
    expect(cache.find(10)).toBe(0);
    expect(cache.find(11)).toBe(1);
    expect(cache.find(12)).toBe(-1);
  });

  it('can reuse tile key 0, which must not be mistaken for an empty layer', () => {
    const cache = new TileCache(2);
    expect(cache.allocate(0, 1)).toBe(0);
    expect(cache.find(0)).toBe(0);
    expect(cache.allocate(1, 1)).toBe(1);
    expect(cache.allocate(2, 2)).toBe(0);
    expect(cache.find(0)).toBe(-1);
  });

  it('forgets everything after a lost context', () => {
    const cache = new TileCache(2);
    cache.allocate(10, 1);
    cache.clear();
    expect(cache.find(10)).toBe(-1);
    expect(cache.allocate(11, 1)).toBe(0);
    expect(cache.evictions).toBe(0);
  });
});
