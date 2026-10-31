import { describe, expect, it } from 'vitest';

import {
  ANNOUNCEMENT_WORDS,
  BATCH_MAGIC,
  RING_CAPACITY_WORD,
  RING_CONSUMED_WORD,
  RING_ENTRIES_WORD,
  RING_HEAD_WORD,
  SLAB_PIXELS_OFFSET,
  SLAB_SLOT_COUNT_WORD,
  TILE_BYTES,
} from '../protocol';
import { createSabRing, SabRingConsumer, SabRingProducer } from '../sab-ring';
import { DirectDelivery, parseBatch, RingDelivery, type TileConsumer } from './delivery';

function batch(tiles: Array<[number, number, number, number, number]>): ArrayBuffer {
  const stride = ANNOUNCEMENT_WORDS * 4 + TILE_BYTES;
  const bytes = new ArrayBuffer(8 + tiles.length * stride);
  new Uint32Array(bytes, 0, 2).set([BATCH_MAGIC, tiles.length]);
  tiles.forEach(([page, lod, x, y, fill], index) => {
    const at = 8 + index * stride;
    new Uint32Array(bytes, at, ANNOUNCEMENT_WORDS).set([
      index,
      page,
      lod >>> 0,
      x,
      y,
      1,
      111,
      222,
    ]);
    new Uint8Array(bytes, at + ANNOUNCEMENT_WORDS * 4, TILE_BYTES).fill(fill);
  });
  return bytes;
}

describe('parseBatch', () => {
  it('decodes tiles and exposes pixels without copying them', () => {
    const bytes = batch([
      [3, -1, 0, 0, 0xaa],
      [4, 2, 5, 6, 0xbb],
    ]);
    const tiles = parseBatch(bytes);
    expect(tiles).toHaveLength(2);
    expect(tiles[0]).toMatchObject({ page: 3, lod: -1, x: 0, y: 0, renderUs: 111, doneUs: 222 });
    expect(tiles[1]).toMatchObject({ page: 4, lod: 2, x: 5, y: 6 });
    expect(tiles[0].pixels.length).toBe(TILE_BYTES);
    expect(tiles[0].pixels[0]).toBe(0xaa);
    expect(tiles[1].pixels[TILE_BYTES - 1]).toBe(0xbb);
    expect(tiles[1].pixels.buffer).toBe(bytes);
  });

  it('rejects damaged batches', () => {
    expect(() => parseBatch(new ArrayBuffer(8))).toThrow(/magic/);
    const truncated = batch([[0, 0, 0, 0, 0]]).slice(0, 100);
    expect(() => parseBatch(truncated)).toThrow(/length/);
  });
});

describe('DirectDelivery', () => {
  const SLOTS = 2;
  const CAPACITY = 4;

  function setup() {
    const slab = new ArrayBuffer(SLAB_PIXELS_OFFSET + SLOTS * TILE_BYTES);
    new Uint32Array(slab, 0, 16)[SLAB_SLOT_COUNT_WORD] = SLOTS;
    new Uint8Array(slab, SLAB_PIXELS_OFFSET + TILE_BYTES, TILE_BYTES).fill(0x77);

    const ring = new ArrayBuffer((RING_ENTRIES_WORD + CAPACITY * ANNOUNCEMENT_WORDS) * 4);
    const words = new Uint32Array(ring);
    words[RING_CAPACITY_WORD] = CAPACITY;
    let head = 0;
    const announce = (slot: number, page: number, lod: number): void => {
      const entry = RING_ENTRIES_WORD + (head & (CAPACITY - 1)) * ANNOUNCEMENT_WORDS;
      words.set([slot, page, lod >>> 0, 5, 6, 1, 111, 222], entry);
      head++;
      Atomics.store(words, RING_HEAD_WORD, head);
    };
    return { delivery: new DirectDelivery(slab, ring), words, announce };
  }

  it('reads tiles in place and acknowledges each only after its upload', () => {
    const { delivery, words, announce } = setup();
    announce(1, 40, -1);

    let consumedDuringUpload = -1;
    let firstByte = 0;
    const consumer: TileConsumer = (page, lod, x, y, pixels) => {
      consumedDuringUpload = Atomics.load(words, RING_CONSUMED_WORD);
      firstByte = pixels[0];
      expect([page, lod, x, y]).toEqual([40, -1, 5, 6]);
    };
    expect(delivery.drain(consumer, 1000)).toBe(1);
    expect(firstByte).toBe(0x77);
    expect(consumedDuringUpload).toBe(0);
    expect(Atomics.load(words, RING_CONSUMED_WORD)).toBe(1);
    expect(delivery.drain(consumer, 1000)).toBe(0);
  });

  it('stops at the time budget and continues on the next frame', () => {
    const { delivery, words, announce } = setup();
    announce(0, 1, 0);
    announce(1, 2, 0);
    const consumer: TileConsumer = () => {};
    expect(delivery.drain(consumer, 0)).toBe(1);
    expect(Atomics.load(words, RING_CONSUMED_WORD)).toBe(1);
    expect(delivery.drain(consumer, 0)).toBe(1);
    expect(Atomics.load(words, RING_CONSUMED_WORD)).toBe(2);
  });

  it('skips a bad slot number without stalling', () => {
    const { delivery, words, announce } = setup();
    announce(99, 1, 0);
    announce(0, 2, 0);
    let delivered = 0;
    delivery.drain(() => delivered++, 1000);
    expect(delivered).toBe(1);
    expect(Atomics.load(words, RING_CONSUMED_WORD)).toBe(2);
  });

  it('wakes an idle compositor when the host announces a tile', async () => {
    const { delivery, announce } = setup();
    const woken = new Promise<void>((resolve) => delivery.notifyOnArrival(resolve));
    announce(0, 1, 0);
    await woken;
    delivery.stop();
  });
});

describe('RingDelivery', () => {
  function setup(capacity = 4) {
    const buffers = createSabRing(capacity);
    return {
      producer: new SabRingProducer(buffers),
      delivery: new RingDelivery(new SabRingConsumer(buffers)),
    };
  }

  const pixels = new Uint8Array(TILE_BYTES);
  function words(page: number, lod: number): Uint32Array {
    return new Uint32Array([0, page, lod >>> 0, 7, 8, 1, 111, 222]);
  }

  it('hands tiles over in order and decodes negative levels', () => {
    const { producer, delivery } = setup();
    producer.push(pixels, words(1, -2), 0);
    producer.push(pixels, words(2, 3), 0);

    const received: number[][] = [];
    const consumer: TileConsumer = (page, lod, x, y, _pixels, doneUs, renderUs) => {
      received.push([page, lod, x, y, doneUs, renderUs]);
    };
    expect(delivery.drain(consumer, 1000)).toBe(2);
    expect(received).toEqual([
      [1, -2, 7, 8, 222, 111],
      [2, 3, 7, 8, 222, 111],
    ]);
    expect(delivery.drain(consumer, 1000)).toBe(0);
  });

  it('stops once the time budget is spent, leaving the rest for the next frame', () => {
    const { producer, delivery } = setup();
    producer.push(pixels, words(1, 0), 0);
    producer.push(pixels, words(2, 0), 0);
    const consumer: TileConsumer = () => {};
    expect(delivery.drain(consumer, 0)).toBe(1);
    expect(delivery.drain(consumer, 0)).toBe(1);
  });

  it('wakes an idle compositor when a tile arrives', async () => {
    const { producer, delivery } = setup();
    let woken = false;
    const arrived = new Promise<void>((resolve) => {
      delivery.notifyOnArrival(() => {
        woken = true;
        resolve();
      });
    });
    expect(woken).toBe(false);
    producer.push(pixels, words(1, 0), 0);
    await arrived;
    expect(woken).toBe(true);
  });

  it('wakes at once if a tile is already waiting', () => {
    const { producer, delivery } = setup();
    producer.push(pixels, words(1, 0), 0);
    let woken = false;
    delivery.notifyOnArrival(() => {
      woken = true;
    });
    expect(woken).toBe(true);
  });
});
