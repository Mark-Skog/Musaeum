import { describe, expect, it } from 'vitest';

import { FramePlanner, type QuadSink, TileList } from './frame-planner';
import { TileCache } from './tile-cache';
import { placeholderScale, quantizeScale } from './tiles';
import { PlacedPages } from './view';

const widths = new Float32Array([612]);
const heights = new Float32Array([792]);
const EXACT = quantizeScale(1.37);

class Recorder implements QuadSink {
  plates: number[][] = [];
  tiles: number[][] = [];
  plate(x: number, y: number, width: number, height: number): void {
    this.plates.push([x, y, width, height]);
  }
  tile(...quad: number[]): void {
    this.tiles.push(quad);
  }
}

function at(x: number, y: number, scale: number): PlacedPages {
  const placed = new PlacedPages();
  placed.add(0, x, y, scale);
  return placed;
}

function setup(): { cache: TileCache; planner: FramePlanner; sink: Recorder } {
  const cache = new TileCache(64);
  return { cache, planner: new FramePlanner(widths, heights, cache), sink: new Recorder() };
}

describe('FramePlanner', () => {
  it('always draws the plate, and asks for the placeholder', () => {
    const { planner, sink } = setup();
    planner.plan(1, at(10.4, 20.6, 1.37), 2000, 2000, sink);
    expect(sink.plates).toHaveLength(1);
    expect(sink.plates[0].slice(0, 2)).toEqual([10, 21]);
    expect(planner.counters).toMatchObject({
      visiblePages: 1,
      plates: 1,
      tilesVisible: 6,
      uncovered: 6,
    });
    expect(planner.missingPlaceholders.length).toBe(1);
    expect(planner.missingPlaceholders.scales[0]).toBe(placeholderScale(612, 792));
  });

  it('draws tiles at exactly their scale on whole pixels', () => {
    const { cache, planner, sink } = setup();
    for (let y = 0; y < 3; y++) {
      for (let x = 0; x < 2; x++) {
        cache.allocate(0, EXACT, x, y, 0);
      }
    }
    planner.plan(1, at(10.4, 20.6, 1.37), 2000, 2000, sink);
    expect(planner.counters).toMatchObject({ exact: 6, fallback: 0, uncovered: 0 });
    const [first, second] = sink.tiles;
    expect(first.slice(0, 4)).toEqual([10, 21, 512, 512]);
    expect(first.slice(4, 8)).toEqual([0, 0, 1, 1]);
    expect(second[0]).toBe(10 + 512);
    // The last column is part of a tile, sampled texel for pixel.
    expect(second[2] / second[6]).toBeCloseTo(512, 9);
  });

  it('covers the page from a coarser scale that no power of two relates', () => {
    const { cache, planner, sink } = setup();
    const coarse = cache.allocate(0, 2048, 0, 0, 0);
    planner.plan(1, at(0, 0, 1.37), 2000, 2000, sink);
    expect(planner.counters).toMatchObject({ exact: 0, fallback: 6, uncovered: 0 });
    expect(sink.tiles.every((quad) => quad[8] === coarse)).toBe(true);
    // The second column of the 1.37 grid starts 373.7 pt in, which is 0.365 of the coarse tile.
    const second = sink.tiles[1];
    expect(second[4]).toBeCloseTo((512 / (EXACT / 4096)) / 1024, 9);
    expect(second[6]).toBeCloseTo(612 / 1024, 9);
  });

  it('covers the page from a finer scale when zooming out, cut to fit', () => {
    const { cache, planner, sink } = setup();
    for (let y = 0; y < 4; y++) {
      for (let x = 0; x < 3; x++) {
        cache.allocate(0, 8192, x, y, 0);
      }
    }
    planner.plan(1, at(0, 0, 1.37), 2000, 2000, sink);
    expect(planner.counters).toMatchObject({ exact: 0, fallback: 6, uncovered: 0 });
    for (const [x, y, width, height] of sink.tiles) {
      expect(x + width).toBeLessThanOrEqual(612 * (EXACT / 4096) + 1e-9);
      expect(y + height).toBeLessThanOrEqual(792 * (EXACT / 4096) + 1e-9);
    }
  });

  it('leaves the plate showing rather than half a fallback', () => {
    const { cache, planner, sink } = setup();
    for (let y = 0; y < 4; y++) {
      for (let x = 0; x < 3; x++) {
        if (x !== 1 || y !== 1) {
          cache.allocate(0, 8192, x, y, 0);
        }
      }
    }
    planner.plan(1, at(0, 0, 1.37), 2000, 2000, sink);
    expect(planner.counters.uncovered).toBeGreaterThan(0);
    expect(planner.counters.fallback + planner.counters.uncovered).toBe(6);
  });

  it('does not resample from a scale far finer than the one shown', () => {
    const { cache, planner, sink } = setup();
    cache.allocate(0, EXACT * 5, 0, 0, 0);
    planner.plan(1, at(0, 0, 1.37), 600, 600, sink);
    expect(planner.counters.fallback).toBe(0);
  });

  it('skips pages entirely off screen', () => {
    const { planner, sink } = setup();
    planner.plan(1, at(5000, 0, 1.37), 1000, 1000, sink);
    expect(planner.counters.visiblePages).toBe(0);
    expect(sink.plates).toHaveLength(0);
  });

  it('lists the tiles still wanted, nearest the centre first', () => {
    const { planner } = setup();
    const wanted = new TileList(16);
    planner.collectWanted(at(0, 0, 1.37), 600, 600, wanted);
    const order = Array.from({ length: wanted.length }, (_, n) => n).sort(
      (a, b) => wanted.priorities[a] - wanted.priorities[b],
    );
    const tiles = order.map((n) => [wanted.columns[n], wanted.rows[n], wanted.scales[n]]);
    expect(tiles).toHaveLength(4);
    expect(tiles[0]).toEqual([0, 0, EXACT]);
    expect(tiles[3]).toEqual([1, 1, EXACT]);
  });
});
