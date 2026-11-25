import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  attachOverlayScrollbar,
  canConsumeVerticalWheel,
  computeThumbMetrics,
  expDamp,
  isScrollCancelKey,
  normalizeWheelDelta,
  SCROLL_HIDE_DELAY_MS,
  thumbOffsetToScrollTop,
  WHEEL_LINE_PX,
} from './scrollbar';

class FakeClassList {
  private set = new Set<string>();
  contains(name: string): boolean {
    return this.set.has(name);
  }
  add(...names: string[]): void {
    for (const name of names) {
      this.set.add(name);
    }
  }
  remove(...names: string[]): void {
    for (const name of names) {
      this.set.delete(name);
    }
  }
  toggle(name: string, force?: boolean): boolean {
    if (force === true) {
      this.set.add(name);
      return true;
    }
    if (force === false) {
      this.set.delete(name);
      return false;
    }
    if (this.set.has(name)) {
      this.set.delete(name);
      return false;
    }
    this.set.add(name);
    return true;
  }
}

class FakeStyle {
  [key: string]: unknown;
  height = '';
  setProperty(name: string, value: string): void {
    this[name] = value;
  }
}

class FakeElement extends EventTarget {
  classList = new FakeClassList();
  style = new FakeStyle();
  scrollTop = 0;
  scrollHeight = 1000;
  clientHeight = 200;
  firstElementChild: Element | null = null;
  private rectTop = 0;
  private rectHeight = 200;

  constructor(rectTop = 0, rectHeight = 200) {
    super();
    this.rectTop = rectTop;
    this.rectHeight = rectHeight;
    this.clientHeight = rectHeight;
  }

  contains(node: unknown): boolean {
    return node === this;
  }

  getBoundingClientRect(): DOMRect {
    return {
      top: this.rectTop,
      bottom: this.rectTop + this.rectHeight,
      left: 0,
      right: 20,
      width: 20,
      height: this.rectHeight,
      x: 0,
      y: this.rectTop,
      toJSON: () => ({}),
    } as DOMRect;
  }
}

describe('exponential damping and thumb geometry', () => {
  it('reaches equivalent progress regardless of refresh rate and settles cleanly', () => {
    // 120 Hz: two 1/120s steps should match one 1/60s step.
    const step60 = expDamp(0, 100, 1 / 60, 20, 0);
    const half = expDamp(0, 100, 1 / 120, 20, 0);
    const step120 = expDamp(half, 100, 1 / 120, 20, 0);
    expect(step120).toBeCloseTo(step60, 6);

    // Clamps large dt spikes (e.g. background tab resume) and snaps inside epsilon.
    const spike = expDamp(0, 100, 5, 20);
    const maxClamped = expDamp(0, 100, 0.05, 20);
    expect(spike).toBeCloseTo(maxClamped, 6);
    expect(expDamp(99.7, 100, 1 / 60, 20)).toBe(100);
  });

  it('hides thumb without overflow and maps thumb offset to scrollTop proportionally', () => {
    expect(computeThumbMetrics(0, 200, 200, 200).visible).toBe(false);
    expect(computeThumbMetrics(0, 201, 200, 200).visible).toBe(false);

    const metrics = computeThumbMetrics(400, 1000, 200, 200);
    expect(metrics.visible).toBe(true);
    expect(metrics.thumbHeight).toBe(40);
    expect(metrics.maxScroll).toBe(800);
    expect(metrics.maxOffset).toBe(160);
    expect(metrics.thumbOffset).toBe(80);

    expect(thumbOffsetToScrollTop(80, metrics.maxOffset, metrics.maxScroll)).toBe(400);
    expect(thumbOffsetToScrollTop(-10, metrics.maxOffset, metrics.maxScroll)).toBe(0);
    expect(thumbOffsetToScrollTop(500, metrics.maxOffset, metrics.maxScroll)).toBe(800);

    // Enforces minimum thumb height on very tall content.
    const tall = computeThumbMetrics(0, 10_000, 200, 200);
    expect(tall.thumbHeight).toBe(28);
  });

  it('recognizes scroll keys that cancel active thumb animation', () => {
    expect(isScrollCancelKey('ArrowDown')).toBe(true);
    expect(isScrollCancelKey('PageDown')).toBe(true);
    expect(isScrollCancelKey(' ')).toBe(true);
    expect(isScrollCancelKey('a')).toBe(false);
  });
});

describe('attachOverlayScrollbar', () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('stays hidden without overflow and fades out 900ms after wheel activity', () => {
    vi.useFakeTimers();
    const scrollEl = new FakeElement();
    const trackEl = new FakeElement();
    const thumbEl = new FakeElement(0, 40);
    scrollEl.scrollHeight = 200;

    const dispose = attachOverlayScrollbar(
      scrollEl as unknown as HTMLElement,
      trackEl as unknown as HTMLElement,
      thumbEl as unknown as HTMLElement,
    );

    expect(trackEl.classList.contains('has-overflow')).toBe(false);
    expect(trackEl.classList.contains('visible')).toBe(false);

    scrollEl.dispatchEvent(new Event('wheel'));
    expect(trackEl.classList.contains('visible')).toBe(false);

    // Enable overflow and simulate wheel scroll.
    scrollEl.scrollHeight = 1000;
    scrollEl.dispatchEvent(new Event('wheel'));
    expect(trackEl.classList.contains('has-overflow')).toBe(true);
    expect(trackEl.classList.contains('visible')).toBe(true);

    vi.advanceTimersByTime(SCROLL_HIDE_DELAY_MS - 100);
    expect(trackEl.classList.contains('visible')).toBe(true);

    vi.advanceTimersByTime(100);
    expect(trackEl.classList.contains('visible')).toBe(false);
    dispose();
  });

  it('shows on proximity hover and glides exponentially on track click/drag until cancelled by wheel', () => {
    vi.useFakeTimers();
    const rafCallbacks = new Map<number, FrameRequestCallback>();
    let nextRafId = 1;
    vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => {
      const id = nextRafId++;
      rafCallbacks.set(id, cb);
      return id;
    });
    vi.stubGlobal('cancelAnimationFrame', (id: number) => {
      rafCallbacks.delete(id);
    });

    const originalWindow = globalThis.window;
    const windowTarget = new EventTarget();
    vi.stubGlobal('window', {
      addEventListener: windowTarget.addEventListener.bind(windowTarget),
      removeEventListener: windowTarget.removeEventListener.bind(windowTarget),
      dispatchEvent: windowTarget.dispatchEvent.bind(windowTarget),
    });

    try {
      const scrollEl = new FakeElement();
      const trackEl = new FakeElement();
      const thumbEl = new FakeElement(0, 40);
      scrollEl.scrollHeight = 1000;

      const dispose = attachOverlayScrollbar(
        scrollEl as unknown as HTMLElement,
        trackEl as unknown as HTMLElement,
        thumbEl as unknown as HTMLElement,
      );

      // Proximity enter reveals the scrollbar without timer hide.
      trackEl.dispatchEvent(new Event('pointerenter'));
      expect(trackEl.classList.contains('visible')).toBe(true);
      vi.advanceTimersByTime(2000);
      expect(trackEl.classList.contains('visible')).toBe(true);

      // Click bottom area of track (clientY = 180 -> desired offset = 160 -> targetScrollTop = 800).
      const down = new Event('pointerdown');
      Object.assign(down, { button: 0, clientY: 180, preventDefault: vi.fn(), stopPropagation: vi.fn() });
      trackEl.dispatchEvent(down);
      expect(trackEl.classList.contains('dragging')).toBe(true);
      expect(rafCallbacks.size).toBe(1);

      // Advance one rAF frame (16ms): scrollTop glides exponentially partway toward 800.
      const firstBatch = [...rafCallbacks.entries()];
      rafCallbacks.clear();
      for (const [, cb] of firstBatch) {
        cb(16);
      }
      expect(scrollEl.scrollTop).toBeGreaterThan(0);
      expect(scrollEl.scrollTop).toBeLessThan(800);

      // Wheel cancels rAF immediately to prevent control fighting.
      scrollEl.dispatchEvent(new Event('wheel'));
      expect(rafCallbacks.size).toBe(0);

      const up = new Event('pointerup');
      windowTarget.dispatchEvent(up);
      expect(trackEl.classList.contains('dragging')).toBe(false);

      trackEl.dispatchEvent(new Event('pointerleave'));
      vi.advanceTimersByTime(SCROLL_HIDE_DELAY_MS);
      expect(trackEl.classList.contains('visible')).toBe(false);

      dispose();
      expect(trackEl.classList.contains('has-overflow')).toBe(false);
    } finally {
      vi.stubGlobal('window', originalWindow);
    }
  });

  it('jumps immediately without rAF when reduced motion is enabled', () => {
    const rafSpy = vi.fn(() => 1);
    vi.stubGlobal('requestAnimationFrame', rafSpy);

    const scrollEl = new FakeElement();
    const trackEl = new FakeElement();
    const thumbEl = new FakeElement(0, 40);
    scrollEl.scrollHeight = 1000;

    const dispose = attachOverlayScrollbar(
      scrollEl as unknown as HTMLElement,
      trackEl as unknown as HTMLElement,
      thumbEl as unknown as HTMLElement,
      { isReducedMotion: () => true },
    );

    const down = new Event('pointerdown');
    Object.assign(down, { button: 0, clientY: 180, preventDefault: vi.fn(), stopPropagation: vi.fn() });
    trackEl.dispatchEvent(down);
    expect(scrollEl.scrollTop).toBe(800);
    expect(rafSpy).not.toHaveBeenCalled();

    dispose();
  });

  it('smoothly glides vertical wheel events via rAF and yields on ctrl/meta or scroll boundaries', () => {
    expect(normalizeWheelDelta(3, 1, 200)).toBe(3 * WHEEL_LINE_PX);
    expect(normalizeWheelDelta(1, 2, 200)).toBeCloseTo(170, 5);
    expect(canConsumeVerticalWheel(0, 1000, 200, -100)).toBe(false);
    expect(canConsumeVerticalWheel(800, 1000, 200, 100)).toBe(false);
    expect(canConsumeVerticalWheel(0, 1000, 200, 100)).toBe(true);

    vi.useFakeTimers();
    let nextRafId = 1;
    const rafCallbacks = new Map<number, FrameRequestCallback>();
    vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => {
      const id = nextRafId++;
      rafCallbacks.set(id, cb);
      return id;
    });
    vi.stubGlobal('cancelAnimationFrame', (id: number) => {
      rafCallbacks.delete(id);
    });

    const scrollEl = new FakeElement();
    const trackEl = new FakeElement();
    const thumbEl = new FakeElement(0, 40);
    scrollEl.scrollHeight = 1000;
    scrollEl.clientHeight = 200;

    const dispose = attachOverlayScrollbar(
      scrollEl as unknown as HTMLElement,
      trackEl as unknown as HTMLElement,
      thumbEl as unknown as HTMLElement,
      { isReducedMotion: () => false },
    );

    const boundaryWheel = Object.assign(new Event('wheel'), {
      deltaY: -120,
      deltaX: 0,
      deltaMode: 0,
      ctrlKey: false,
      metaKey: false,
      preventDefault: vi.fn(),
    });
    scrollEl.dispatchEvent(boundaryWheel);
    expect(boundaryWheel.preventDefault).not.toHaveBeenCalled();
    expect(rafCallbacks.size).toBe(0);

    const ctrlWheel = Object.assign(new Event('wheel'), {
      deltaY: 120,
      deltaX: 0,
      deltaMode: 0,
      ctrlKey: true,
      metaKey: false,
      preventDefault: vi.fn(),
    });
    scrollEl.dispatchEvent(ctrlWheel);
    expect(ctrlWheel.preventDefault).not.toHaveBeenCalled();
    expect(rafCallbacks.size).toBe(0);

    const downWheel = Object.assign(new Event('wheel'), {
      deltaY: 200,
      deltaX: 0,
      deltaMode: 0,
      ctrlKey: false,
      metaKey: false,
      preventDefault: vi.fn(),
    });
    scrollEl.dispatchEvent(downWheel);
    expect(downWheel.preventDefault).toHaveBeenCalledOnce();
    expect(trackEl.classList.contains('visible')).toBe(true);
    expect(rafCallbacks.size).toBe(1);

    const first = [...rafCallbacks.values()];
    rafCallbacks.clear();
    first.forEach((cb) => cb(16));
    expect(scrollEl.scrollTop).toBeGreaterThan(0);
    expect(scrollEl.scrollTop).toBeLessThan(200);

    dispose();
  });
});
