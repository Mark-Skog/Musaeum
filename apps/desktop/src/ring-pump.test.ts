import { describe, expect, it } from 'vitest';

import {
  ANNOUNCEMENT_WORDS,
  RING_CAPACITY_WORD,
  RING_CONSUMED_WORD,
  RING_ENTRIES_WORD,
  RING_HEAD_WORD,
  SLAB_PIXELS_OFFSET,
  SLAB_SLOT_COUNT_WORD,
  TILE_BYTES,
} from './protocol';
import { RingPump } from './ring-pump';
import { createSabRing, SabRingConsumer, SabRingProducer } from './sab-ring';

const SLOTS = 3;
const CAPACITY = 4;

function setup(sabCapacity = 4) {
  const slab = new ArrayBuffer(SLAB_PIXELS_OFFSET + SLOTS * TILE_BYTES);
  new Uint32Array(slab, 0, 16)[SLAB_SLOT_COUNT_WORD] = SLOTS;
  for (let slot = 0; slot < SLOTS; slot++) {
    new Uint8Array(slab, SLAB_PIXELS_OFFSET + slot * TILE_BYTES, TILE_BYTES).fill(0x10 + slot);
  }

  const ring = new ArrayBuffer((RING_ENTRIES_WORD + CAPACITY * ANNOUNCEMENT_WORDS) * 4);
  const ringWords = new Uint32Array(ring);
  ringWords[RING_CAPACITY_WORD] = CAPACITY;

  let head = 0;
  const announce = (slot: number, page: number): void => {
    const entry = RING_ENTRIES_WORD + (head & (CAPACITY - 1)) * ANNOUNCEMENT_WORDS;
    ringWords.set([slot, page, 0, 0, 0, 1, 2, 3], entry);
    head++;
    Atomics.store(ringWords, RING_HEAD_WORD, head);
  };

  const buffers = createSabRing(sabCapacity);
  const copies: number[] = [];
  const pump = new RingPump(slab, ring, new SabRingProducer(buffers), (ms) => copies.push(ms));
  return { pump, announce, ringWords, consumer: new SabRingConsumer(buffers), copies };
}

describe('RingPump', () => {
  it('copies announced tiles into the worker ring and acknowledges them', () => {
    const { pump, announce, ringWords, consumer, copies } = setup();
    announce(2, 40);
    announce(0, 41);

    expect(pump.poll()).toBe(2);
    expect(Atomics.load(ringWords, RING_CONSUMED_WORD)).toBe(2);
    expect(copies).toHaveLength(2);

    expect(consumer.word(consumer.peek(), 1)).toBe(40);
    expect(consumer.pixels(consumer.peek())[0]).toBe(0x12);
    consumer.advance();
    expect(consumer.word(consumer.peek(), 1)).toBe(41);
    expect(consumer.pixels(consumer.peek())[TILE_BYTES - 1]).toBe(0x10);
  });

  it('does nothing when no tile has been announced', () => {
    const { pump, ringWords } = setup();
    expect(pump.poll()).toBe(0);
    expect(Atomics.load(ringWords, RING_CONSUMED_WORD)).toBe(0);
  });

  it('applies backpressure by leaving tiles unacknowledged while the worker is behind', () => {
    const { pump, announce, ringWords, consumer } = setup(2);
    announce(0, 1);
    announce(1, 2);
    announce(2, 3);

    expect(pump.poll()).toBe(2);
    expect(Atomics.load(ringWords, RING_CONSUMED_WORD)).toBe(2);

    consumer.advance();
    expect(pump.poll()).toBe(1);
    expect(Atomics.load(ringWords, RING_CONSUMED_WORD)).toBe(3);
  });

  it('survives a bad slot number without stalling the ring', () => {
    const { pump, announce, ringWords } = setup();
    announce(99, 1);
    announce(1, 2);

    expect(pump.poll()).toBe(1);
    expect(pump.badSlots).toBe(1);
    expect(Atomics.load(ringWords, RING_CONSUMED_WORD)).toBe(2);
  });
});
