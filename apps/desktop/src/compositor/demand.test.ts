import { describe, expect, it } from 'vitest';

import { buildDemand, DemandEncoder, type DemandInputs } from './demand';
import { TileList } from './frame-planner';
import { TileCache } from './tile-cache';
import { placeholderScale } from './tiles';

function decode(bytes: Uint8Array): number[] {
  return Array.from(new Uint32Array(bytes.buffer, bytes.byteOffset, bytes.byteLength / 4));
}

describe('DemandEncoder', () => {
  it('produces exactly the bytes Rust parses in musaeum-core/src/demand.rs', () => {
    const encoder = new DemandEncoder();
    encoder.reset();
    encoder.add(0, 5611, 1, 0);
    encoder.add(0, 8192, 2, 3);
    expect(decode(encoder.finish(7))).toEqual([0x444d444d, 7, 2, 0, 0, 5611, 1, 0, 0, 8192, 2, 3]);
  });

  it('keeps only the first request for a tile, which holds its best priority', () => {
    const encoder = new DemandEncoder();
    encoder.reset();
    encoder.add(1, 4096, 0, 0);
    encoder.add(2, 4096, 0, 0);
    encoder.add(1, 4096, 0, 0);
    encoder.add(1, 4097, 0, 0);
    expect(encoder.tileCount).toBe(3);
  });

  it('never exceeds what the host accepts', () => {
    const encoder = new DemandEncoder();
    encoder.reset();
    for (let page = 0; page < 5000; page++) {
      encoder.add(page, 4096, 0, 0);
    }
    expect(encoder.tileCount).toBe(4096);
  });

  it('gives a stable fingerprint that follows the contents and their order', () => {
    const encoder = new DemandEncoder();
    const fingerprintOf = (pages: number[]): number => {
      encoder.reset();
      pages.forEach((page) => encoder.add(page, 4096, 0, 0));
      return encoder.fingerprint();
    };
    expect(fingerprintOf([1, 2])).toBe(fingerprintOf([1, 2]));
    expect(fingerprintOf([1, 2])).not.toBe(fingerprintOf([2, 1]));
    expect(fingerprintOf([1])).not.toBe(fingerprintOf([1, 2]));
  });

  it('hands fetch a copy that later frames cannot change', () => {
    const encoder = new DemandEncoder();
    encoder.reset();
    encoder.add(1, 4096, 0, 0);
    const first = encoder.finish(1);
    encoder.reset();
    encoder.add(2, 4096, 0, 0);
    encoder.finish(2);
    expect(decode(first)[4]).toBe(1);
  });
});

describe('buildDemand', () => {
  const pages = 100;
  const widths = new Float32Array(pages).fill(612);
  const heights = new Float32Array(pages).fill(792);
  const placeholder = placeholderScale(612, 792);

  function inputs(overrides: Partial<DemandInputs> = {}): DemandInputs {
    const missingPlaceholders = new TileList(8);
    missingPlaceholders.add(50, placeholder, 0, 0, 0);
    const wanted = new TileList(8);
    wanted.add(50, 5612, 1, 1, 300);
    wanted.add(50, 5612, 0, 0, 100);
    const neighbours = new TileList(8);
    neighbours.add(51, 5612, 0, 0, 10);
    return {
      widths,
      heights,
      cache: new TileCache(64),
      missingPlaceholders,
      wanted,
      neighbours,
      firstVisiblePage: 50,
      lastVisiblePage: 50,
      pagesPerSecond: 0,
      ...overrides,
    };
  }

  function listed(encoder: DemandEncoder): number[][] {
    const words = decode(encoder.finish(1)).slice(4);
    const tiles: number[][] = [];
    for (let at = 0; at < words.length; at += 4) {
      tiles.push(words.slice(at, at + 4));
    }
    return tiles;
  }

  it('asks for placeholders, then sharp tiles from the centre out, then what is around', () => {
    const encoder = new DemandEncoder();
    buildDemand(encoder, inputs());
    const tiles = listed(encoder);
    expect(tiles[0]).toEqual([50, placeholder, 0, 0]);
    expect(tiles[1]).toEqual([50, 5612, 0, 0]);
    expect(tiles[2]).toEqual([50, 5612, 1, 1]);
    const placeholders = tiles.filter(([, scale]) => scale === placeholder).map(([page]) => page);
    expect(placeholders).toEqual([50, 51, 52, 49, 48]);
    expect(tiles[tiles.length - 1]).toEqual([51, 5612, 0, 0]);
  });

  it('asks only for placeholders while moving fast, further ahead the faster', () => {
    const encoder = new DemandEncoder();
    buildDemand(encoder, inputs({ pagesPerSecond: 10 }));
    const tiles = listed(encoder);
    expect(tiles.every(([, scale]) => scale === placeholder)).toBe(true);
    expect(tiles.map(([page]) => page)).toEqual([50, 51, 52, 53, 54, 55, 56, 57, 49, 48]);

    buildDemand(encoder, inputs({ pagesPerSecond: -10 }));
    expect(listed(encoder).map(([page]) => page)).toEqual([50, 49, 48, 47, 46, 45, 44, 43, 51, 52]);
  });

  it('leaves out placeholders it already has', () => {
    const cache = new TileCache(64);
    cache.allocate(51, placeholder, 0, 0, 0);
    const encoder = new DemandEncoder();
    buildDemand(encoder, inputs({ cache, pagesPerSecond: 10 }));
    expect(listed(encoder).some(([page]) => page === 51)).toBe(false);
  });
});
