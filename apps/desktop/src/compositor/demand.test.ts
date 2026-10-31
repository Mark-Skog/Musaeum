import { describe, expect, it } from 'vitest';

import { buildDemand, DemandEncoder, type DemandInputs } from './demand';
import { TileList } from './frame-planner';
import { buildLayout, tileKey } from './layout';
import { TileCache } from './tile-cache';

function decode(bytes: Uint8Array): number[] {
  return Array.from(new Uint32Array(bytes.buffer, bytes.byteOffset, bytes.byteLength / 4));
}

describe('DemandEncoder', () => {
  it('produces exactly the bytes Rust parses in musaeum-core/src/demand.rs', () => {
    const encoder = new DemandEncoder();
    encoder.reset();
    encoder.add(0, -2, 0, 0);
    encoder.add(0, 1, 2, 3);
    expect(decode(encoder.finish(7))).toEqual([
      0x444d444d, 7, 2, 0, 0, 0xfffffffe, 0, 0, 0, 1, 2, 3,
    ]);
  });

  it('keeps only the first request for a tile, which holds its best priority', () => {
    const encoder = new DemandEncoder();
    encoder.reset();
    encoder.add(1, 0, 0, 0);
    encoder.add(2, 0, 0, 0);
    encoder.add(1, 0, 0, 0);
    expect(encoder.tileCount).toBe(2);
  });

  it('never exceeds what the host accepts', () => {
    const encoder = new DemandEncoder();
    encoder.reset();
    for (let page = 0; page < 5000; page++) {
      encoder.add(page, 0, 0, 0);
    }
    expect(encoder.tileCount).toBe(4096);
  });

  it('gives a stable fingerprint that follows the contents and their order', () => {
    const encoder = new DemandEncoder();
    const fingerprintOf = (pages: number[]): number => {
      encoder.reset();
      pages.forEach((page) => encoder.add(page, 0, 0, 0));
      return encoder.fingerprint();
    };
    expect(fingerprintOf([1, 2])).toBe(fingerprintOf([1, 2]));
    expect(fingerprintOf([1, 2])).not.toBe(fingerprintOf([2, 1]));
    expect(fingerprintOf([1])).not.toBe(fingerprintOf([1, 2]));
  });

  it('hands fetch a copy that later frames cannot change', () => {
    const encoder = new DemandEncoder();
    encoder.reset();
    encoder.add(1, 0, 0, 0);
    const first = encoder.finish(1);
    encoder.reset();
    encoder.add(2, 0, 0, 0);
    encoder.finish(2);
    expect(decode(first)[4]).toBe(1);
  });
});

describe('buildDemand', () => {
  const pages = 100;
  const sizes = new Float32Array(pages * 2);
  for (let page = 0; page < pages; page++) {
    sizes[page * 2] = 612;
    sizes[page * 2 + 1] = 792;
  }
  const layout = buildLayout(sizes, 10);

  function inputs(overrides: Partial<DemandInputs> = {}): DemandInputs {
    const placeholders = new TileList(8);
    placeholders.add(50, -1, 0, 0, 0);
    const sharp = new TileList(8);
    sharp.add(50, 1, 1, 1, 300);
    sharp.add(50, 1, 0, 0, 100);
    return {
      layout,
      cache: new TileCache(8),
      missingPlaceholders: placeholders,
      missingSharp: sharp,
      firstVisiblePage: 50,
      lastVisiblePage: 50,
      targetLod: 1,
      velocityPt: 0,
      viewHeightPt: 450,
      ...overrides,
    };
  }

  function listed(encoder: DemandEncoder): Array<[number, number, number, number]> {
    const words = decode(encoder.finish(1));
    const tiles: Array<[number, number, number, number]> = [];
    for (let at = 4; at < words.length; at += 4) {
      tiles.push([words[at], words[at + 1] | 0, words[at + 2], words[at + 3]]);
    }
    return tiles;
  }

  it('asks for placeholders first, then sharp tiles from the centre outwards', () => {
    const encoder = new DemandEncoder();
    buildDemand(encoder, inputs());
    const tiles = listed(encoder);
    expect(tiles[0]).toEqual([50, -1, 0, 0]);
    expect(tiles[1]).toEqual([50, 1, 0, 0]);
    expect(tiles[2]).toEqual([50, 1, 1, 1]);
  });

  it('skips sharp tiles during fast scrolling, because they would arrive too late', () => {
    const encoder = new DemandEncoder();
    buildDemand(encoder, inputs({ velocityPt: 450 * 10 }));
    expect(listed(encoder).some(([, lod]) => lod === 1)).toBe(false);
  });

  it('prepares placeholders in the direction of travel, further when moving faster', () => {
    const slow = new DemandEncoder();
    buildDemand(slow, inputs({ velocityPt: 450 }));
    const fast = new DemandEncoder();
    buildDemand(fast, inputs({ velocityPt: 450 * 20 }));

    const placeholderPages = (encoder: DemandEncoder): number[] =>
      listed(encoder)
        .filter(([page, lod]) => lod === -1 && page !== 50)
        .map(([page]) => page);

    expect(Math.max(...placeholderPages(fast))).toBeGreaterThan(Math.max(...placeholderPages(slow)));
    expect(placeholderPages(slow)).toContain(51);
  });

  it('looks up the document when scrolling up', () => {
    const encoder = new DemandEncoder();
    buildDemand(encoder, inputs({ velocityPt: -450 * 20 }));
    const pagesAhead = listed(encoder)
      .filter(([page, lod]) => lod === -1 && page < 50)
      .map(([page]) => page);
    expect(pagesAhead).toContain(49);
    expect(Math.min(...pagesAhead)).toBeLessThan(45);
  });

  it('stays inside the document at both ends', () => {
    const encoder = new DemandEncoder();
    buildDemand(
      encoder,
      inputs({ firstVisiblePage: 99, lastVisiblePage: 99, velocityPt: 450 * 30 }),
    );
    for (const [page] of listed(encoder)) {
      expect(page).toBeGreaterThanOrEqual(0);
      expect(page).toBeLessThan(pages);
    }
  });

  it('does not ask for what the cache already holds', () => {
    const cache = new TileCache(8);
    cache.allocate(tileKey(51, -1, 0, 0), 1);
    const encoder = new DemandEncoder();
    buildDemand(encoder, inputs({ cache, velocityPt: 450 }));
    expect(listed(encoder).some(([page, lod]) => page === 51 && lod === -1)).toBe(false);
  });

  it('prepares the neighbouring pages in full while the reader is still', () => {
    const encoder = new DemandEncoder();
    buildDemand(encoder, inputs());
    // 3 × 4 tiles on each of the two neighbouring pages.
    const neighbours = listed(encoder).filter(([page, lod]) => lod === 1 && page !== 50);
    expect(neighbours).toHaveLength(24);
  });
});
