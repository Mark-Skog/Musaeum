/**
 * Path B on the main thread: watches the host's delivery ring, copies each announced tile
 * from the WebView2 shared buffer into the `SharedArrayBuffer` ring, and acknowledges it.
 *
 * This is the one part of the tile pipeline that depends on the main thread. Experiment B
 * measures what that costs by also running with the main thread deliberately blocked.
 */

import {
  A_SLOT,
  ANNOUNCEMENT_WORDS,
  RING_CAPACITY_WORD,
  RING_CONSUMED_WORD,
  RING_ENTRIES_WORD,
  RING_HEAD_WORD,
  SLAB_PIXELS_OFFSET,
  SLAB_SLOT_COUNT_WORD,
  TILE_BYTES,
} from './protocol';
import type { SabRingProducer } from './sab-ring';

export class RingPump {
  private readonly ring: Uint32Array;
  private readonly mask: number;
  private readonly slots: Uint8Array[];
  private consumed = 0;
  /** Set when the host announces a slot that does not exist, which would be a bug. */
  badSlots = 0;

  constructor(
    slab: ArrayBuffer,
    ring: ArrayBuffer,
    private readonly output: SabRingProducer,
    private readonly recordCopy: (milliseconds: number) => void,
  ) {
    this.ring = new Uint32Array(ring);
    this.mask = this.ring[RING_CAPACITY_WORD] - 1;
    const slotCount = new Uint32Array(slab, 0, 16)[SLAB_SLOT_COUNT_WORD];
    // One view per slot, created up front. Each stays mapped for the whole session.
    this.slots = Array.from(
      { length: slotCount },
      (_, slot) => new Uint8Array(slab, SLAB_PIXELS_OFFSET + slot * TILE_BYTES, TILE_BYTES),
    );
  }

  /** Copies everything that is ready and that the output has room for. */
  poll(): number {
    // Atomics on this non-shared view give the ordering the producer's Release store needs.
    const head = Atomics.load(this.ring, RING_HEAD_WORD);
    let copied = 0;
    while (this.consumed !== head && this.output.hasRoom()) {
      const entry = RING_ENTRIES_WORD + (this.consumed & this.mask) * ANNOUNCEMENT_WORDS;
      const pixels = this.slots[this.ring[entry + A_SLOT]];
      if (pixels === undefined) {
        this.badSlots++;
      } else {
        const started = performance.now();
        this.output.push(pixels, this.ring, entry);
        this.recordCopy(performance.now() - started);
        copied++;
      }
      this.consumed = (this.consumed + 1) >>> 0;
      // Tells the host that this slot can be reused. Nothing else travels back to it.
      Atomics.store(this.ring, RING_CONSUMED_WORD, this.consumed);
    }
    return copied;
  }
}
