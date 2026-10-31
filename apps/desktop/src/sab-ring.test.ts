import { describe, expect, it } from 'vitest';

import { TILE_BYTES } from './protocol';
import { createSabRing, SabRingConsumer, SabRingProducer } from './sab-ring';

function words(page: number): Uint32Array {
  return new Uint32Array([0, page, 0, 0, 0, 0, 0, 0]);
}

describe('SAB ring', () => {
  it('rejects capacities that are not powers of two', () => {
    expect(() => createSabRing(3)).toThrow(RangeError);
    expect(() => createSabRing(0)).toThrow(RangeError);
  });

  it('uses shared memory, which is what lets it cross to the worker', () => {
    const buffers = createSabRing(2);
    expect(buffers.control).toBeInstanceOf(SharedArrayBuffer);
    expect(buffers.pixels).toBeInstanceOf(SharedArrayBuffer);
    expect(buffers.pixels.byteLength).toBe(2 * TILE_BYTES);
  });

  it('refuses to overwrite tiles the consumer has not read', () => {
    const buffers = createSabRing(2);
    const producer = new SabRingProducer(buffers);
    const consumer = new SabRingConsumer(buffers);
    const pixels = new Uint8Array(TILE_BYTES);

    expect(producer.hasRoom()).toBe(true);
    producer.push(pixels, words(1), 0);
    producer.push(pixels, words(2), 0);
    expect(producer.hasRoom()).toBe(false);

    consumer.advance();
    expect(producer.hasRoom()).toBe(true);
  });

  it('keeps pixels and metadata together as it wraps around', () => {
    const buffers = createSabRing(2);
    const producer = new SabRingProducer(buffers);
    const consumer = new SabRingConsumer(buffers);

    for (let page = 0; page < 9; page++) {
      producer.push(new Uint8Array(TILE_BYTES).fill(page), words(page), 0);
      const index = consumer.peek();
      expect(consumer.word(index, 1)).toBe(page);
      expect(consumer.pixels(index)[1000]).toBe(page);
      consumer.advance();
      expect(consumer.peek()).toBe(-1);
    }
  });

  it('reads announcement words from anywhere in a larger array', () => {
    const buffers = createSabRing(2);
    const producer = new SabRingProducer(buffers);
    const consumer = new SabRingConsumer(buffers);
    const array = new Uint32Array(20);
    array[10 + 1] = 77;
    producer.push(new Uint8Array(TILE_BYTES), array, 10);
    expect(consumer.word(consumer.peek(), 1)).toBe(77);
  });

  it('returns values above 2^31 as unsigned', () => {
    const buffers = createSabRing(2);
    const producer = new SabRingProducer(buffers);
    const consumer = new SabRingConsumer(buffers);
    const array = new Uint32Array(8);
    array[7] = 4_000_000_000;
    producer.push(new Uint8Array(TILE_BYTES), array, 0);
    expect(consumer.word(consumer.peek(), 7)).toBe(4_000_000_000);
  });

  it('survives the counters overflowing 32 bits', () => {
    const buffers = createSabRing(4);
    const producer = new SabRingProducer(buffers);
    const consumer = new SabRingConsumer(buffers);
    const nearWrap = 2 ** 31 - 2;
    // Reaching into private state, because getting here honestly takes billions of tiles.
    (producer as unknown as { head: number }).head = nearWrap;
    (consumer as unknown as { tail: number }).tail = nearWrap;
    new Int32Array(buffers.control).set([nearWrap, nearWrap], 0);

    for (let page = 0; page < 8; page++) {
      expect(producer.hasRoom()).toBe(true);
      producer.push(new Uint8Array(TILE_BYTES).fill(page), words(page), 0);
      expect(consumer.word(consumer.peek(), 1)).toBe(page);
      consumer.advance();
    }
  });
});
