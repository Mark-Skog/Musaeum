import { describe, expect, it } from 'vitest';

import { nextVelocity } from './camera';

const VIEW_HEIGHT = 500;

describe('nextVelocity', () => {
  it('converges on a steady speed', () => {
    let velocity = 0;
    for (let frame = 0; frame < 60; frame++) {
      velocity = nextVelocity(velocity, 10, 10, VIEW_HEIGHT);
    }
    expect(velocity).toBeCloseTo(1000, 3);
  });

  it('is smoothed, so one uneven frame does not swing it fully', () => {
    const velocity = nextVelocity(1000, 20, 10, VIEW_HEIGHT);
    expect(velocity).toBeGreaterThan(1000);
    expect(velocity).toBeLessThan(2000);
  });

  it('drops to zero at once when the scroll stops', () => {
    expect(nextVelocity(15_000, 0, 13, VIEW_HEIGHT)).toBe(0);
  });

  it('does not treat a jump as a speed', () => {
    expect(nextVelocity(100, VIEW_HEIGHT + 1, 13, VIEW_HEIGHT)).toBe(0);
    expect(nextVelocity(100, -50_000, 13, VIEW_HEIGHT)).toBe(0);
  });

  it('keeps its sign when scrolling up', () => {
    expect(nextVelocity(0, -10, 10, VIEW_HEIGHT)).toBeLessThan(0);
  });

  it('ignores the first frame, which has no interval to measure', () => {
    expect(nextVelocity(42, 10, 0, VIEW_HEIGHT)).toBe(42);
  });
});
