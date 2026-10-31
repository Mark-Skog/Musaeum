/**
 * Tracks which tile lives in which layer of the GPU texture array, with LRU eviction.
 * The layout of the array is fixed, so nothing is allocated while scrolling apart from
 * the map's own bookkeeping.
 */

const EMPTY = -1;

export class TileCache {
  private readonly layerByKey = new Map<number, number>();
  private readonly keyByLayer: Float64Array;
  private readonly lastUsedFrame: Uint32Array;
  /** Tiles evicted to make room. High numbers with a big cache would mean thrashing. */
  evictions = 0;

  constructor(readonly layers: number) {
    this.keyByLayer = new Float64Array(layers).fill(EMPTY);
    this.lastUsedFrame = new Uint32Array(layers);
  }

  /** The tile's layer, or -1 if it is not resident. */
  find(key: number): number {
    return this.layerByKey.get(key) ?? -1;
  }

  /** Marks a layer as used by this frame, which protects it from eviction. */
  touch(layer: number, frame: number): void {
    this.lastUsedFrame[layer] = frame;
  }

  /**
   * Picks a layer for a new tile. Never evicts anything the current frame uses, because
   * that tile would turn into a hole on screen. Returns -1 if every layer is in use.
   */
  allocate(key: number, frame: number): number {
    let chosen = -1;
    let oldest = Number.POSITIVE_INFINITY;
    for (let layer = 0; layer < this.layers; layer++) {
      if (this.keyByLayer[layer] === EMPTY) {
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
    const previous = this.keyByLayer[chosen];
    if (previous !== EMPTY) {
      this.layerByKey.delete(previous);
      this.evictions++;
    }
    this.keyByLayer[chosen] = key;
    this.layerByKey.set(key, chosen);
    this.lastUsedFrame[chosen] = frame;
    return chosen;
  }

  /** Forgets everything, as after a lost WebGL context. */
  clear(): void {
    this.layerByKey.clear();
    this.keyByLayer.fill(EMPTY);
    this.lastUsedFrame.fill(0);
  }
}
