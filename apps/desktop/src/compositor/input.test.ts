import { describe, expect, it } from 'vitest';

import { type WheelInput, WheelClassifier } from './input';

const event = (timeMs: number, deltaY: number, extra: Partial<WheelInput> = {}): WheelInput => ({
  deltaX: 0,
  deltaY,
  deltaMode: 0,
  timeMs,
  ...extra,
});

describe('WheelClassifier', () => {
  it('takes notches for a mouse wheel', () => {
    const classifier = new WheelClassifier();
    expect(classifier.classify(event(0, 100))).toBe('mouse');
    expect(classifier.classify(event(1000, -120, { wheelDeltaY: 120 }))).toBe('mouse');
    expect(classifier.classify(event(2000, 3, { deltaMode: 1 }))).toBe('mouse');
  });

  it('takes small, fractional or diagonal deltas for a trackpad', () => {
    const classifier = new WheelClassifier();
    expect(classifier.classify(event(0, 2.5))).toBe('trackpad');
    expect(classifier.classify(event(1000, 12, { deltaX: 3 }))).toBe('trackpad');
    expect(classifier.classify(event(2000, 30))).toBe('trackpad');
  });

  it('keeps a gesture in its class, unless the trackpad shows itself', () => {
    const classifier = new WheelClassifier();
    expect(classifier.classify(event(0, 100))).toBe('mouse');
    // A small whole step in the middle of notches is still the wheel.
    expect(classifier.classify(event(30, 4))).toBe('mouse');
    // A fractional one is not something a wheel sends.
    expect(classifier.classify(event(60, 1.25))).toBe('trackpad');
    // Momentum's tail is part of the same gesture.
    expect(classifier.classify(event(90, 100))).toBe('trackpad');
    // After a pause, a new gesture is judged afresh.
    expect(classifier.classify(event(500, 100))).toBe('mouse');
  });
});
