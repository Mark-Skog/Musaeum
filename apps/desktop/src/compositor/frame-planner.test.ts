import { describe, expect, it } from 'vitest';

import { FramePlanner, type QuadSink } from './frame-planner';
import { buildLayout, tileKey } from './layout';
import { TileCache } from './tile-cache';

interface Quad {
  x: number;
  y: number;
  width: number;
  height: number;
  u0?: number;
  v0?: number;
  u1?: number;
  v1?: number;
  layer?: number;
}

class RecordingSink implements QuadSink {
  plates: Quad[] = [];
  tiles: Quad[] = [];

  plate(x: number, y: number, width: number, height: number): void {
    this.plates.push({ x, y, width, height });
  }

  tile(
    x: number,
    y: number,
    width: number,
    height: number,
    u0: number,
    v0: number,
    u1: number,
    v1: number,
    layer: number,
  ): void {
    this.tiles.push({ x, y, width, height, u0, v0, u1, v1, layer });
  }
}

const VIEW_WIDTH = 1600;
const VIEW_HEIGHT = 900;

/** Letter pages: at scale 2 and level 1, each one is 3 × 4 tiles of 512 device pixels. */
function setup(pages = 10) {
  const sizes = new Float32Array(pages * 2);
  for (let page = 0; page < pages; page++) {
    sizes[page * 2] = 612;
    sizes[page * 2 + 1] = 792;
  }
  const layout = buildLayout(sizes, 10);
  const cache = new TileCache(64);
  return { layout, cache, planner: new FramePlanner(layout, cache), sink: new RecordingSink() };
}

describe('FramePlanner', () => {
  it('draws a plate for every visible page even when no tile has arrived', () => {
    const { planner, sink } = setup();
    planner.plan(1, 0, 2, VIEW_WIDTH, VIEW_HEIGHT, sink);

    expect(sink.plates).toHaveLength(1);
    expect(sink.plates[0]).toEqual({ x: 188, y: 0, width: 1224, height: 1584 });
    expect(sink.tiles).toHaveLength(0);
    expect(planner.counters.plates).toBe(planner.counters.visiblePages);
    expect(planner.counters.uncovered).toBe(planner.counters.tilesVisible);
    expect(planner.counters.exact).toBe(0);
  });

  it('asks for what is missing, at both levels', () => {
    const { planner, sink } = setup();
    planner.plan(1, 0, 2, VIEW_WIDTH, VIEW_HEIGHT, sink);

    expect(planner.missingPlaceholders.length).toBe(1);
    expect(planner.missingPlaceholders.lods[0]).toBe(-1);
    // 900 device pixels show two rows of three tiles.
    expect(planner.missingSharp.length).toBe(6);
  });

  it('covers every page in view, across page boundaries', () => {
    const { planner, sink } = setup();
    // Halfway through page 0's bottom, with page 1 coming into view.
    planner.plan(1, 700, 2, VIEW_WIDTH, VIEW_HEIGHT, sink);

    expect(planner.counters.visiblePages).toBe(2);
    expect(sink.plates).toHaveLength(2);
    expect(planner.firstVisiblePage).toBe(0);
    expect(planner.lastVisiblePage).toBe(1);
  });

  it('skips a page when only the gap below it is in view', () => {
    const { planner, sink } = setup();
    planner.plan(1, 795, 2, VIEW_WIDTH, VIEW_HEIGHT, sink);
    expect(planner.firstVisiblePage).toBe(0);
    expect(planner.counters.visiblePages).toBe(1);
    expect(sink.plates[0].y).toBe(Math.round((802 - 795) * 2));
  });

  it('draws a sharp tile exactly where it belongs', () => {
    const { planner, cache, sink } = setup();
    const layer = cache.allocate(tileKey(0, 1, 1, 0), 1);
    planner.plan(2, 0, 2, VIEW_WIDTH, VIEW_HEIGHT, sink);

    expect(planner.counters.exact).toBe(1);
    expect(sink.tiles).toEqual([
      { x: 188 + 512, y: 0, width: 512, height: 512, u0: 0, v0: 0, u1: 1, v1: 1, layer },
    ]);
  });

  it('samples only the part of an edge tile that lies on the page', () => {
    const { planner, cache, sink } = setup();
    // Column 2 of 1224 device pixels covers x = 1024..1224, or 200 of 512 pixels.
    const layer = cache.allocate(tileKey(0, 1, 2, 0), 1);
    planner.plan(2, 0, 2, VIEW_WIDTH, VIEW_HEIGHT, sink);

    const [tile] = sink.tiles;
    expect(tile.layer).toBe(layer);
    expect(tile.width).toBeCloseTo(200);
    expect(tile.height).toBe(512);
    expect(tile.u1).toBeCloseTo(200 / 512);
    expect(tile.v1).toBe(1);
  });

  it('falls back to the matching part of the whole-page tile', () => {
    const { planner, cache, sink } = setup();
    const layer = cache.allocate(tileKey(0, -1, 0, 0), 1);
    planner.plan(2, 0, 2, VIEW_WIDTH, VIEW_HEIGHT, sink);

    expect(planner.counters.exact).toBe(0);
    expect(planner.counters.uncovered).toBe(0);
    expect(planner.counters.fallback).toBe(6);
    expect(sink.tiles).toHaveLength(6);
    expect(sink.tiles.every((tile) => tile.layer === layer)).toBe(true);

    // At level -1 one tile spans 1024 pt, and at level 1 one spans 256 pt, so the tile at
    // column 1, row 1 occupies the quarter starting at (0.25, 0.25) of the coarse texture.
    const tile = sink.tiles.find((quad) => quad.x === 188 + 512 && quad.y === 512)!;
    expect(tile.u0).toBeCloseTo(0.25);
    expect(tile.v0).toBeCloseTo(0.25);
    expect(tile.u1).toBeCloseTo(0.5);
    expect(tile.v1).toBeCloseTo(0.5);
  });

  it('prefers the finest coarser tile available', () => {
    const { planner, cache, sink } = setup();
    const whole = cache.allocate(tileKey(0, -1, 0, 0), 1);
    const middle = cache.allocate(tileKey(0, 0, 0, 0), 1);
    planner.plan(2, 0, 2, VIEW_WIDTH, VIEW_HEIGHT, sink);

    const first = sink.tiles.find((quad) => quad.x === 188 && quad.y === 0)!;
    expect(first.layer).toBe(middle);
    expect(first.layer).not.toBe(whole);
    expect(first.u1).toBeCloseTo(0.5);
  });

  it('keeps tiles it uses from being evicted', () => {
    const { planner, cache, sink } = setup();
    cache.allocate(tileKey(0, 1, 0, 0), 1);
    planner.plan(7, 0, 2, VIEW_WIDTH, VIEW_HEIGHT, sink);
    const small = new TileCache(1);
    small.allocate(tileKey(0, 1, 0, 0), 1);
    const planned = new FramePlanner(buildLayout(new Float32Array([612, 792]), 0), small);
    planned.plan(7, 0, 2, VIEW_WIDTH, VIEW_HEIGHT, new RecordingSink());
    expect(small.allocate(123, 7)).toBe(-1);
  });

  it('leaves no gaps between neighbouring tiles at a fractional scale', () => {
    const { planner, cache, sink } = setup();
    for (let y = 0; y < 4; y++) {
      for (let x = 0; x < 3; x++) {
        cache.allocate(tileKey(0, 1, x, y), 1);
      }
    }
    planner.plan(2, 0, 1.9, VIEW_WIDTH, VIEW_HEIGHT, sink);

    const row = sink.tiles.filter((tile) => tile.y === sink.tiles[0].y).sort((a, b) => a.x - b.x);
    expect(row.length).toBeGreaterThan(1);
    for (let at = 1; at < row.length; at++) {
      expect(row[at].x).toBeCloseTo(row[at - 1].x + row[at - 1].width, 3);
    }
  });

  it('handles an empty document without drawing anything', () => {
    const layout = buildLayout(new Float32Array(0), 10);
    const sink = new RecordingSink();
    const planner = new FramePlanner(layout, new TileCache(4));
    planner.plan(1, 0, 2, VIEW_WIDTH, VIEW_HEIGHT, sink);
    expect(sink.plates).toHaveLength(0);
    expect(planner.lastVisiblePage).toBe(-1);
  });

  it('gives each page its own geometry in a mixed document', () => {
    const layout = buildLayout(new Float32Array([612, 792, 842, 595]), 10);
    const sink = new RecordingSink();
    const planner = new FramePlanner(layout, new TileCache(4));
    planner.plan(1, 0, 1, VIEW_WIDTH, 2000, sink);

    expect(sink.plates).toHaveLength(2);
    expect(sink.plates[0]).toEqual({ x: 494, y: 0, width: 612, height: 792 });
    expect(sink.plates[1].width).toBe(842);
    expect(sink.plates[1].height).toBe(595);
    expect(sink.plates[1].x).toBe(379);
  });
});
