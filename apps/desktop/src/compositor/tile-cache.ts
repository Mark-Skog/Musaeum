/**
 * Tracks which tile lives in which layer of the GPU texture array, with least-recently-used
 * eviction inside a memory budget. The array is allocated once at its full size, so the budget is
 * exactly what the tiles cost the GPU process, whether they are in use or not, and nothing is
 * allocated while reading apart from the maps' own bookkeeping.
 *
 * Tiles of every scale share the budget. The ones a zoom left behind stop being drawn, so they are
 * the first to age out, and nothing the current frame draws is ever evicted.
 */

import { TILE_BYTES } from '../protocol';
import { pageScaleKey, tileIndex } from './tiles';

const EMPTY = -1;
/** The side of a tile, in device pixels. */
const TILE_PIXELS = 512;
/** Screens' worth of tiles kept by default. Going back a little should not mean rendering. */
const SCREENS_KEPT = 4;
/** Fewer layers than this could not even cover one screen while it changes scale. */
export const MIN_LAYERS = 64;
/** What the default budget never goes beyond, whatever the screen: 256 MiB. */
const MAX_DEFAULT_LAYERS = 256;

/** The default budget for a screen of this size, in bytes: a few screens of tiles. */
export function defaultBudgetBytes(screenWidth: number, screenHeight: number): number {
  // A tile grid that does not line up with the screen shows one more tile along each side.
  const across = Math.ceil(screenWidth / TILE_PIXELS) + 1;
  const down = Math.ceil(screenHeight / TILE_PIXELS) + 1;
  const layers = Math.min(MAX_DEFAULT_LAYERS, Math.max(MIN_LAYERS, across * down * SCREENS_KEPT));
  return layers * TILE_BYTES;
}

/** How many layers a budget pays for, within what the GPU allows. */
export function layersForBudget(budgetBytes: number, gpuMaxLayers: number): number {
  return Math.max(MIN_LAYERS, Math.min(gpuMaxLayers, Math.floor(budgetBytes / TILE_BYTES)));
}

export class TileCache {
  /** Layer of each resident tile, by page and scale, then by place in the grid. */
  private readonly layers = new Map<number, Map<number, number>>();
  /** The scales each page has tiles at, finest last. */
  private readonly scalesByPage = new Map<number, number[]>();
  private readonly layerPage: Int32Array;
  private readonly layerScale: Int32Array;
  private readonly layerTile: Int32Array;
  private readonly lastUsedFrame: Uint32Array;
  /** Tiles evicted to make room. High numbers with a big budget would mean thrashing. */
  evictions = 0;
  /** Layers holding a tile. */
  resident = 0;

  constructor(readonly capacity: number) {
    this.layerPage = new Int32Array(capacity).fill(EMPTY);
    this.layerScale = new Int32Array(capacity);
    this.layerTile = new Int32Array(capacity);
    this.lastUsedFrame = new Uint32Array(capacity);
  }

  /** Bytes the budget pays for. */
  get budgetBytes(): number {
    return this.capacity * TILE_BYTES;
  }

  /** The tile's layer, or -1 if it is not resident. */
  find(page: number, scale: number, x: number, y: number): number {
    return this.layers.get(pageScaleKey(page, scale))?.get(tileIndex(x, y)) ?? -1;
  }

  /** The scales a page has tiles at, coarsest first. Never modify the array. */
  scalesOf(page: number): readonly number[] {
    return this.scalesByPage.get(page) ?? NONE;
  }

  /** Marks a layer as used by this frame, which protects it from eviction. */
  touch(layer: number, frame: number): void {
    this.lastUsedFrame[layer] = frame;
  }

  /**
   * Picks a layer for a new tile, evicting the least recently used one if the budget is spent.
   * Never evicts anything the current frame uses, because that tile would turn into a hole on
   * screen. Returns -1 if every layer is in use.
   */
  allocate(page: number, scale: number, x: number, y: number, frame: number): number {
    let chosen = -1;
    let oldest = Number.POSITIVE_INFINITY;
    for (let layer = 0; layer < this.capacity; layer++) {
      if (this.layerPage[layer] === EMPTY) {
        chosen = layer;
        break;
      }
      const used = this.lastUsedFrame[layer];
      if (used !== frame && used < oldest) {
        oldest = used;
        chosen = layer;
      }
    }
    if (chosen < 0) {
      return -1;
    }
    if (this.layerPage[chosen] !== EMPTY) {
      this.forget(chosen);
      this.evictions++;
    }

    const key = pageScaleKey(page, scale);
    let tiles = this.layers.get(key);
    if (tiles === undefined) {
      tiles = new Map();
      this.layers.set(key, tiles);
      const scales = this.scalesByPage.get(page) ?? [];
      scales.push(scale);
      scales.sort((a, b) => a - b);
      this.scalesByPage.set(page, scales);
    }
    tiles.set(tileIndex(x, y), chosen);
    this.layerPage[chosen] = page;
    this.layerScale[chosen] = scale;
    this.layerTile[chosen] = tileIndex(x, y);
    this.lastUsedFrame[chosen] = frame;
    this.resident++;
    return chosen;
  }

  private forget(layer: number): void {
    const page = this.layerPage[layer];
    const scale = this.layerScale[layer];
    const key = pageScaleKey(page, scale);
    const tiles = this.layers.get(key);
    tiles?.delete(this.layerTile[layer]);
    if (tiles !== undefined && tiles.size === 0) {
      this.layers.delete(key);
      const scales = this.scalesByPage.get(page);
      if (scales !== undefined) {
        scales.splice(scales.indexOf(scale), 1);
        if (scales.length === 0) {
          this.scalesByPage.delete(page);
        }
      }
    }
    this.layerPage[layer] = EMPTY;
    this.resident--;
  }

  /** Forgets everything, as after a lost WebGL context. */
  clear(): void {
    this.layers.clear();
    this.scalesByPage.clear();
    this.layerPage.fill(EMPTY);
    this.lastUsedFrame.fill(0);
    this.resident = 0;
  }
}

const NONE: readonly number[] = Object.freeze([]);
