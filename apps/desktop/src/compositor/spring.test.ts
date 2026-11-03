import { describe, expect, it } from 'vitest';

import { Spring } from './spring';

describe('Spring', () => {
  it('goes to its target without ever passing it, and comes to rest', () => {
    const spring = new Spring(0);
    spring.target = 1000;
    let previous = 0;
    let elapsed = 0;
    while (!spring.atRest && elapsed < 2000) {
      spring.step(8);
      elapsed += 8;
      expect(spring.position).toBeGreaterThanOrEqual(previous);
      expect(spring.position).toBeLessThanOrEqual(1000);
      previous = spring.position;
    }
    expect(spring.atRest).toBe(true);
    expect(spring.position).toBe(1000);
    expect(elapsed).toBeLessThan(700);
  });

  it('lands in the same place however the time is cut up', () => {
    const coarse = new Spring(0);
    const fine = new Spring(0);
    coarse.target = 500;
    fine.target = 500;
    coarse.step(120);
    for (let step = 0; step < 15; step++) {
      fine.step(8);
    }
    expect(fine.position).toBeCloseTo(coarse.position, 6);
    expect(fine.velocity).toBeCloseTo(coarse.velocity, 4);
  });

  it('changes course mid-flight without a jump in position or speed', () => {
    const spring = new Spring(0);
    spring.target = 1000;
    spring.step(100);
    const { position, velocity } = spring;
    expect(velocity).toBeGreaterThan(0);
    spring.target = 0;
    expect(spring.position).toBe(position);
    expect(spring.velocity).toBe(velocity);
    spring.step(1);
    expect(Math.abs(spring.position - position)).toBeLessThan(Math.abs(velocity) * 0.002 + 1e-9);
  });

  it('snaps when told to, as a turn with no animation does', () => {
    const spring = new Spring(0);
    spring.target = 1000;
    spring.step(50);
    spring.snap(1000);
    expect(spring.atRest).toBe(true);
    expect(spring.position).toBe(1000);
  });
});
