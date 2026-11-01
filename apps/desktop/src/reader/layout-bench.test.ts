import { describe, expect, it } from 'vitest';

import { pickSections } from './layout-bench';

const KIB = 1024;

describe('pickSections', () => {
  it('finds the sizes it wants, the median and the largest', () => {
    const sizes = [20, 200, 400, 1024, 3072, ...Array<number>(40).fill(30)].map((kib) => kib * KIB);
    expect(pickSections(sizes).map((section) => section.name)).toEqual([
      '20 KiB',
      '30 KiB (median)',
      '200 KiB',
      '400 KiB',
      '1.0 MiB',
      '3.0 MiB',
    ]);
  });

  it('keeps each section at its place in the book', () => {
    const picked = pickSections([500 * KIB, 20 * KIB]);
    const twenty = picked.find((section) => section.bytes === 20 * KIB);
    expect(twenty?.index).toBe(1);
  });

  it('measures what an ordinary novel really has', () => {
    const sizes = [2, 9, 12, 14, 15, 16, 18, 19, 22, 90].map((kib) => kib * KIB);
    const names = pickSections(sizes).map((section) => section.name);
    expect(names).toEqual(['16 KiB (median)', '19 KiB', '90 KiB (largest)']);
  });

  it('does not measure the same section twice', () => {
    const picked = pickSections([20 * KIB]);
    expect(picked).toHaveLength(1);
  });

  it('skips sections with nothing in them, and survives an empty book', () => {
    expect(pickSections([])).toEqual([]);
    expect(pickSections([0, 0])).toEqual([]);
    expect(pickSections([0, 20 * KIB]).map((section) => section.index)).toEqual([1]);
  });
});
