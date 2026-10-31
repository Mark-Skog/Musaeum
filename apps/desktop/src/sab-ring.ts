/**
 * The Path B hand-off between the main thread and the compositor worker: a pre-allocated
 * single-producer, single-consumer ring of tiles in `SharedArrayBuffer`s.
 *
 * Nothing is allocated per tile, so this path adds no garbage-collection pressure.
 */

import { ANNOUNCEMENT_WORDS, TILE_BYTES } from './protocol';

const HEAD = 0;
const TAIL = 1;
/** Head and tail on separate cache lines, so the two threads do not slow each other down. */
const ENTRIES_WORD = 32;

export interface SabRingBuffers {
  control: SharedArrayBuffer;
  pixels: SharedArrayBuffer;
  capacity: number;
}

export function createSabRing(capacity: number): SabRingBuffers {
  if (capacity <= 0 || (capacity & (capacity - 1)) !== 0) {
    throw new RangeError(`SAB ring capacity must be a power of two, got ${capacity}`);
  }
  return {
    control: new SharedArrayBuffer((ENTRIES_WORD + capacity * ANNOUNCEMENT_WORDS) * 4),
    pixels: new SharedArrayBuffer(capacity * TILE_BYTES),
    capacity,
  };
}

/** Lives on the main thread. */
export class SabRingProducer {
  private readonly control: Int32Array;
  private readonly slots: Uint8Array[];
  private readonly mask: number;
  private head = 0;

  constructor(buffers: SabRingBuffers) {
    this.control = new Int32Array(buffers.control);
    this.mask = buffers.capacity - 1;
    this.slots = Array.from(
      { length: buffers.capacity },
      (_, index) => new Uint8Array(buffers.pixels, index * TILE_BYTES, TILE_BYTES),
    );
  }

  hasRoom(): boolean {
    const used = (this.head - Atomics.load(this.control, TAIL)) | 0;
    return used <= this.mask;
  }

  /**
   * Copies a tile in and wakes the worker. The caller must have checked `hasRoom`.
   * `words` holds the tile's announcement starting at `at`.
   */
  push(pixels: Uint8Array, words: Uint32Array, at: number): void {
    const index = this.head & this.mask;
    this.slots[index].set(pixels);
    const entry = ENTRIES_WORD + index * ANNOUNCEMENT_WORDS;
    for (let word = 0; word < ANNOUNCEMENT_WORDS; word++) {
      this.control[entry + word] = words[at + word];
    }
    this.head = (this.head + 1) | 0;
    // Publishes the writes above. A worker that went idle is parked on this word.
    Atomics.store(this.control, HEAD, this.head);
    Atomics.notify(this.control, HEAD);
  }
}

/** Lives on the compositor worker. */
export class SabRingConsumer {
  private readonly control: Int32Array;
  private readonly slots: Uint8Array[];
  private readonly mask: number;
  private tail = 0;

  constructor(buffers: SabRingBuffers) {
    this.control = new Int32Array(buffers.control);
    this.mask = buffers.capacity - 1;
    this.slots = Array.from(
      { length: buffers.capacity },
      (_, index) => new Uint8Array(buffers.pixels, index * TILE_BYTES, TILE_BYTES),
    );
  }

  /** The oldest unread entry's index, or -1 if the ring is empty. */
  peek(): number {
    return Atomics.load(this.control, HEAD) === this.tail ? -1 : this.tail & this.mask;
  }

  /** One word of the entry returned by `peek`, as an unsigned value. */
  word(index: number, word: number): number {
    return this.control[ENTRIES_WORD + index * ANNOUNCEMENT_WORDS + word] >>> 0;
  }

  pixels(index: number): Uint8Array {
    return this.slots[index];
  }

  /** Releases the entry returned by `peek`, making room for the producer. */
  advance(): void {
    this.tail = (this.tail + 1) | 0;
    Atomics.store(this.control, TAIL, this.tail);
  }

  /** Resolves when the producer publishes something, for a worker with nothing to do. */
  waitForData(): Promise<unknown> | null {
    const head = Atomics.load(this.control, HEAD);
    if (head !== this.tail) {
      return null;
    }
    const wait = Atomics.waitAsync(this.control, HEAD, head);
    return wait.async ? wait.value : null;
  }
}
