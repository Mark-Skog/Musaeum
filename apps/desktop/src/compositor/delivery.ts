/**
 * The two ways a finished tile can reach the compositor worker. Everything else in the
 * pipeline is identical, so this file is the only variable in Experiment B.
 */

import {
  A_DONE_US,
  A_SCALE,
  A_PAGE,
  A_RENDER_US,
  A_SLOT,
  A_X,
  A_Y,
  ANNOUNCEMENT_WORDS,
  BATCH_HEADER_BYTES,
  BATCH_MAGIC,
  RING_CAPACITY_WORD,
  RING_CONSUMED_WORD,
  RING_ENTRIES_WORD,
  RING_HEAD_WORD,
  SLAB_PIXELS_OFFSET,
  SLAB_SLOT_COUNT_WORD,
  TILE_BYTES,
} from '../protocol';
import type { SabRingConsumer } from '../sab-ring';

export type TileConsumer = (
  page: number,
  scale: number,
  x: number,
  y: number,
  pixels: Uint8Array,
  doneUs: number,
  renderUs: number,
) => void;

export interface Delivery {
  /** Hands over waiting tiles until `budgetMs` has been spent. Returns how many. */
  drain(consumer: TileConsumer, budgetMs: number): number;
  /** Calls `wake` once when a tile arrives while the compositor is idle. */
  notifyOnArrival(wake: () => void): void;
  stop(): void;
}

const CONCURRENT_POLLS = 3;
const ERROR_BACKOFF_MS = 50;
/** How often an idle Path C compositor looks at the ring. See `notifyOnArrival` there. */
const DIRECT_IDLE_POLL_MS = 16;

interface ReceivedTile {
  page: number;
  scale: number;
  x: number;
  y: number;
  pixels: Uint8Array;
  doneUs: number;
  renderUs: number;
}

/** Decodes a Path A batch. The pixels are views into `bytes`, not copies. */
export function parseBatch(bytes: ArrayBuffer): ReceivedTile[] {
  const header = new Uint32Array(bytes, 0, 2);
  if (header[0] !== BATCH_MAGIC) {
    throw new Error('tile batch has a bad magic number');
  }
  const count = header[1];
  const stride = ANNOUNCEMENT_WORDS * 4 + TILE_BYTES;
  if (bytes.byteLength !== BATCH_HEADER_BYTES + count * stride) {
    throw new Error('tile batch length does not match its tile count');
  }
  const tiles: ReceivedTile[] = [];
  for (let index = 0; index < count; index++) {
    const at = BATCH_HEADER_BYTES + index * stride;
    const words = new Uint32Array(bytes, at, ANNOUNCEMENT_WORDS);
    tiles.push({
      page: words[A_PAGE],
      scale: words[A_SCALE],
      x: words[A_X],
      y: words[A_Y],
      pixels: new Uint8Array(bytes, at + ANNOUNCEMENT_WORDS * 4, TILE_BYTES),
      doneUs: words[A_DONE_US],
      renderUs: words[A_RENDER_US],
    });
  }
  return tiles;
}

/**
 * Path A: `fetch` from the worker straight to the host's custom protocol. The main thread
 * is not involved at all. Each response parks on the host until a tile is ready.
 */
export class PullDelivery implements Delivery {
  private readonly queue: ReceivedTile[] = [];
  private running = true;
  private wake: (() => void) | null = null;
  responses = 0;
  errors = 0;

  constructor(private readonly url: string) {
    for (let poll = 0; poll < CONCURRENT_POLLS; poll++) {
      void this.poll();
    }
  }

  private async poll(): Promise<void> {
    while (this.running) {
      try {
        const response = await fetch(this.url, { cache: 'no-store', credentials: 'omit' });
        if (response.status === 204 || response.status === 410) {
          // The session is closing.
          return;
        }
        if (!response.ok) {
          throw new Error(`the host answered ${response.status}`);
        }
        const tiles = parseBatch(await response.arrayBuffer());
        this.responses++;
        this.queue.push(...tiles);
        this.fireWake();
      } catch (error) {
        this.errors++;
        console.warn('polling for tiles failed', error);
        await new Promise((resolve) => setTimeout(resolve, ERROR_BACKOFF_MS));
      }
    }
  }

  private fireWake(): void {
    const wake = this.wake;
    if (wake !== null) {
      this.wake = null;
      wake();
    }
  }

  drain(consumer: TileConsumer, budgetMs: number): number {
    const started = performance.now();
    let drained = 0;
    while (this.queue.length > 0) {
      const tile = this.queue.shift()!;
      consumer(tile.page, tile.scale, tile.x, tile.y, tile.pixels, tile.doneUs, tile.renderUs);
      drained++;
      if (performance.now() - started >= budgetMs) {
        break;
      }
    }
    return drained;
  }

  notifyOnArrival(wake: () => void): void {
    if (this.queue.length > 0) {
      wake();
    } else {
      this.wake = wake;
    }
  }

  stop(): void {
    this.running = false;
  }
}

/**
 * Path C: the WebView2 buffers themselves were transferred to this worker, so it reads the
 * host's ring directly and uploads pixels from where PDFium wrote them. From the rasterizer
 * to the GPU upload there are no CPU copies, and the main thread takes no part in it.
 */
export class DirectDelivery implements Delivery {
  private readonly ring: Uint32Array;
  private readonly mask: number;
  private readonly slots: Uint8Array[];
  private consumed = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(slab: ArrayBuffer, ring: ArrayBuffer) {
    this.ring = new Uint32Array(ring);
    this.mask = this.ring[RING_CAPACITY_WORD] - 1;
    const slotCount = new Uint32Array(slab, 0, 16)[SLAB_SLOT_COUNT_WORD];
    this.slots = Array.from(
      { length: slotCount },
      (_, slot) => new Uint8Array(slab, SLAB_PIXELS_OFFSET + slot * TILE_BYTES, TILE_BYTES),
    );
  }

  drain(consumer: TileConsumer, budgetMs: number): number {
    const started = performance.now();
    const head = Atomics.load(this.ring, RING_HEAD_WORD);
    let drained = 0;
    while (this.consumed !== head) {
      const entry = RING_ENTRIES_WORD + (this.consumed & this.mask) * ANNOUNCEMENT_WORDS;
      const pixels = this.slots[this.ring[entry + A_SLOT]];
      if (pixels !== undefined) {
        consumer(
          this.ring[entry + A_PAGE],
          this.ring[entry + A_SCALE],
          this.ring[entry + A_X],
          this.ring[entry + A_Y],
          pixels,
          this.ring[entry + A_DONE_US],
          this.ring[entry + A_RENDER_US],
        );
        drained++;
      }
      this.consumed = (this.consumed + 1) >>> 0;
      // Only now may the host reuse the slot, since the upload above read from it.
      Atomics.store(this.ring, RING_CONSUMED_WORD, this.consumed);
      if (performance.now() - started >= budgetMs) {
        break;
      }
    }
    return drained;
  }

  /**
   * This memory is not a `SharedArrayBuffer`, so `Atomics.waitAsync` does not work on it
   * and the only option is to look at it now and then. That only happens while idle.
   */
  notifyOnArrival(wake: () => void): void {
    if (this.timer !== null) {
      return;
    }
    const check = (): void => {
      if (Atomics.load(this.ring, RING_HEAD_WORD) !== this.consumed) {
        this.timer = null;
        wake();
      } else {
        this.timer = setTimeout(check, DIRECT_IDLE_POLL_MS);
      }
    };
    check();
  }

  stop(): void {
    if (this.timer !== null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
  }
}

/**
 * Path B: the main thread has already copied the tile into a `SharedArrayBuffer` ring,
 * so all this side does is read it. The worker never has to wait for anything.
 */
export class RingDelivery implements Delivery {
  private waiting = false;

  constructor(private readonly ring: SabRingConsumer) {}

  drain(consumer: TileConsumer, budgetMs: number): number {
    const started = performance.now();
    let drained = 0;
    for (;;) {
      const index = this.ring.peek();
      if (index < 0) {
        break;
      }
      consumer(
        this.ring.word(index, A_PAGE),
        this.ring.word(index, A_SCALE),
        this.ring.word(index, A_X),
        this.ring.word(index, A_Y),
        this.ring.pixels(index),
        this.ring.word(index, A_DONE_US),
        this.ring.word(index, A_RENDER_US),
      );
      this.ring.advance();
      drained++;
      if (performance.now() - started >= budgetMs) {
        break;
      }
    }
    return drained;
  }

  notifyOnArrival(wake: () => void): void {
    if (this.waiting) {
      return;
    }
    const wait = this.ring.waitForData();
    if (wait === null) {
      wake();
      return;
    }
    this.waiting = true;
    void wait.then(() => {
      this.waiting = false;
      wake();
    });
  }

  stop(): void {}
}
