import { describe, expect, it } from 'vitest';

import { type DocumentShape, Reader, type ReadingSnapshot } from './reader';
import { PlacedPages } from './view';

// Three portrait pages, a landscape plate, two more portrait pages.
const widths = new Float32Array([600, 600, 600, 1000, 600, 600]);
const heights = new Float32Array([800, 800, 800, 700, 800, 800]);

function shape(direction: 'L2R' | 'R2L' | null = null): DocumentShape {
  return { widths, heights, pageLayout: null, direction };
}

interface Setup {
  reducedMotion?: boolean;
  saved?: ReadingSnapshot;
  direction?: 'L2R' | 'R2L';
}

function reader(options: Setup = {}): Reader {
  const created = new Reader(
    shape(options.direction ?? null),
    {
      devicePixelRatio: 1,
      comfortableScale: 1.5,
      prefersReducedMotion: options.reducedMotion ?? false,
    },
    options.saved ?? null,
  );
  created.resize(1000, 800);
  return created;
}

function shown(subject: Reader): { page: number; x: number; y: number; w: number; h: number }[] {
  const out = new PlacedPages();
  subject.placeCurrent(out);
  return Array.from({ length: out.count }, (_, at) => ({
    page: out.pages[at],
    x: out.x[at],
    y: out.y[at],
    w: widths[out.pages[at]] * out.scales[at],
    h: heights[out.pages[at]] * out.scales[at],
  }));
}

function runFor(subject: Reader, from: number, ms: number): number {
  let now = from;
  subject.advance(now);
  while (now < from + ms) {
    now += 8;
    subject.advance(now);
  }
  return now;
}

type WheelArgument = Parameters<Reader['wheel']>[0];

const wheel = (timeMs: number, deltaY: number, extra: Partial<WheelArgument> = {}) => ({
  deltaX: 0,
  deltaY,
  deltaMode: 0,
  ctrlKey: false,
  x: 500,
  y: 400,
  timeMs,
  ...extra,
});

describe('the reading mode', () => {
  it('is paginated with or without animation, or continuous with none at all', () => {
    const subject = reader();
    expect(subject.mode).toEqual({ kind: 'paginated', animate: true });

    subject.setMode('continuous');
    expect(subject.mode).toEqual({ kind: 'continuous' });
    expect('animate' in subject.mode).toBe(false);

    // Changing the preference meanwhile does not give continuous reading an animation...
    subject.setAnimation(false);
    expect(subject.mode).toEqual({ kind: 'continuous' });
    // ...and going back to paginated reading brings the preference back.
    subject.setMode('paginated');
    expect(subject.mode).toEqual({ kind: 'paginated', animate: false });
    subject.setMode('continuous');
    subject.setMode('paginated');
    expect(subject.mode).toEqual({ kind: 'paginated', animate: false });
  });

  it('starts without the page turn for someone who asked for less motion', () => {
    expect(reader({ reducedMotion: true }).mode).toEqual({ kind: 'paginated', animate: false });
  });

  it('comes back exactly as it was saved', () => {
    const saved: ReadingSnapshot = {
      page: 4,
      u: 0,
      v: 0.5,
      progress: 0,
      mode: 'continuous',
      pageTurnAnimation: false,
      zoom: 1.2,
      spreads: 'single',
    };
    const subject = reader({ saved });
    expect(subject.mode).toEqual({ kind: 'continuous' });
    expect(subject.animationPreference).toBe(false);
    expect(subject.zoomFactor).toBe(1.2);
    expect(subject.locator).toEqual({ page: 4, u: 0, v: 0.5 });
    expect(subject.snapshot()).toMatchObject({
      page: 4,
      v: 0.5,
      progress: 4.5 / 6,
      mode: 'continuous',
    });
  });
});

describe('turning pages', () => {
  it('swaps in the same frame when the animation is off', () => {
    const subject = reader();
    subject.setAnimation(false);
    subject.key('ArrowRight', false, false, 0);
    // No frame has advanced, and the next page is already the only one shown.
    expect(shown(subject).map((page) => page.page)).toEqual([1]);
    expect(subject.locator.page).toBe(1);
  });

  it('slides on a spring when the animation is on, and lands exactly', () => {
    const subject = reader();
    subject.key('ArrowRight', false, false, 0);
    expect(shown(subject).map((page) => page.page)).toEqual([0]);
    subject.advance(0);
    subject.advance(16);
    const during = shown(subject).map((page) => page.page);
    expect(during).toEqual([0, 1]);
    runFor(subject, 16, 1000);
    expect(subject.isMoving()).toBe(false);
    expect(subject.stripPosition).toBe(1016);
    expect(shown(subject).map((page) => page.page)).toEqual([1]);
  });

  it('follows a dragging finger exactly', () => {
    const subject = reader();
    subject.pointerDown(500, 400, 0);
    subject.pointerMove(400, 400, 16);
    expect(subject.stripPosition).toBe(100);
    subject.pointerMove(437.5, 400, 32);
    expect(subject.stripPosition).toBe(62.5);
  });

  it('settles back from a short slow drag and goes on from a long one', () => {
    const subject = reader();
    subject.pointerDown(500, 400, 0);
    for (let step = 1; step <= 10; step++) {
      subject.pointerMove(500 - 5 * step, 400, 16 * step);
    }
    subject.pointerUp(170);
    runFor(subject, 170, 1000);
    expect(subject.currentScreen).toBe(0);
    expect(subject.stripPosition).toBe(0);

    subject.pointerDown(500, 400, 2000);
    for (let step = 1; step <= 100; step++) {
      subject.pointerMove(500 - 4 * step, 400, 2000 + 16 * step);
    }
    subject.pointerUp(3610);
    runFor(subject, 3610, 1000);
    expect(subject.currentScreen).toBe(1);
    expect(subject.stripPosition).toBe(1016);
  });

  it('lets a finger catch a sliding page exactly where it is', () => {
    const subject = reader();
    subject.key('ArrowRight', false, false, 0);
    subject.advance(0);
    subject.advance(100);
    const caught = subject.stripPosition;
    expect(caught).toBeGreaterThan(0);
    expect(caught).toBeLessThan(1016);
    subject.pointerDown(500, 400, 100);
    expect(subject.stripPosition).toBe(caught);
    subject.advance(200);
    expect(subject.stripPosition).toBe(caught);
  });

  it('turns the other way when reading right to left', () => {
    const subject = reader({ direction: 'R2L' });
    subject.setAnimation(false);
    subject.key('ArrowLeft', false, false, 0);
    expect(subject.locator.page).toBe(1);

    const sliding = reader({ direction: 'R2L' });
    sliding.pointerDown(500, 400, 0);
    sliding.pointerMove(600, 400, 16);
    expect(sliding.stripPosition).toBe(100);
    // The next page comes in from the left.
    const next = shown(sliding).find((page) => page.page === 1);
    expect(next).toBeDefined();
    expect(next!.x + next!.w).toBeLessThan(200);
  });
});

describe('the position', () => {
  it('survives switching modes back and forth, exactly', () => {
    const subject = reader();
    subject.setMode('continuous');
    subject.scrollTo(1234.567);
    const top = subject.scrollPt;
    const locator = { ...subject.locator };
    expect(locator.page).toBe(1);
    expect(locator.v).toBeGreaterThan(0);

    subject.setMode('paginated');
    expect(shown(subject).map((page) => page.page)).toEqual([1]);
    subject.setMode('continuous');
    expect(subject.locator).toEqual(locator);
    expect(subject.scrollPt).toBeCloseTo(top, 9);
  });

  it('never cuts a landscape page off in continuous reading', () => {
    const subject = reader();
    subject.setMode('continuous');
    subject.scrollTo(2000);
    const plate = shown(subject).find((page) => page.page === 3)!;
    expect(plate.x).toBeGreaterThanOrEqual(0);
    expect(plate.x + plate.w).toBeLessThanOrEqual(1000);
  });

  it('keeps the point under the pointer still while zooming', () => {
    const subject = reader();
    subject.setMode('continuous');
    subject.scrollTo(500);
    const pointUnder = (x: number, y: number): { page: number; u: number; v: number } => {
      const inside = (p: { x: number; y: number; w: number; h: number }): boolean =>
        x >= p.x && x <= p.x + p.w && y >= p.y && y <= p.y + p.h;
      const page = shown(subject).find(inside)!;
      return { page: page.page, u: (x - page.x) / page.w, v: (y - page.y) / page.h };
    };
    const before = pointUnder(300, 200);
    subject.setZoom(2, 0, { x: 300, y: 200 }, false);
    const after = pointUnder(300, 200);
    expect(after.page).toBe(before.page);
    expect(after.u).toBeCloseTo(before.u, 6);
    expect(after.v).toBeCloseTo(before.v, 6);
  });
});

describe('continuous scrolling', () => {
  it('smooths a mouse wheel, and follows a trackpad exactly', () => {
    const subject = reader();
    subject.setMode('continuous');
    subject.advance(0);
    subject.wheel(wheel(0, 100));
    expect(subject.scrollPt).toBe(0);
    subject.advance(16);
    expect(subject.scrollPt).toBeGreaterThan(0);
    expect(subject.scrollPt).toBeLessThan(100 / subject.currentScale);
    runFor(subject, 16, 800);
    expect(subject.scrollPt).toBeCloseTo(100 / subject.currentScale, 6);

    const before = subject.scrollPt;
    subject.wheel(wheel(5000, 3.5));
    // No smoothing on top of the trackpad's own: it moved by exactly that, at once.
    expect(subject.scrollPt).toBeCloseTo(before + 3.5 / subject.currentScale, 9);
  });

  it('counts a steady scroll as speed, and a stop or a jump as none', () => {
    const subject = reader();
    subject.setMode('continuous');
    let now = 0;
    subject.advance(now);
    for (let frame = 0; frame < 20; frame++) {
      subject.scrollTo(subject.scrollPt + 40);
      now += 16;
      subject.advance(now);
    }
    // 40 points every 16 ms, through a column of 6 pages: 4700 points of them and 5 gaps of 12.
    const expected = (40 / 16) * 1000 * (6 / 4760);
    expect(subject.pagesPerSecond).toBeGreaterThan(expected * 0.9);
    expect(subject.pagesPerSecond).toBeLessThan(expected * 1.01);

    subject.scrollTo(subject.scrollPt + 2000);
    subject.advance((now += 16));
    expect(subject.pagesPerSecond).toBe(0);

    subject.scrollTo(subject.scrollPt - 40);
    subject.advance((now += 16));
    subject.advance((now += 16));
    expect(subject.pagesPerSecond).toBe(0);
  });

  it('turns one page per notch in paginated reading', () => {
    const subject = reader();
    subject.setAnimation(false);
    subject.wheel(wheel(0, 100));
    expect(subject.locator.page).toBe(1);
    subject.wheel(wheel(200, 100));
    expect(subject.locator.page).toBe(2);
  });
});
