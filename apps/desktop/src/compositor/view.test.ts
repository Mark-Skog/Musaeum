import { describe, expect, it } from 'vitest';

import { buildRows, planSpreads } from './spreads';
import { ContinuousView, PaginatedView, PlacedPages } from './view';

// Portrait, a landscape plate twice as wide, a small square, portrait.
const widths = new Float32Array([612, 1224, 300, 612]);
const heights = new Float32Array([792, 792, 300, 792]);
const single = buildRows(widths, heights, planSpreads('single', null, null));

interface Shown {
  page: number;
  x: number;
  y: number;
  w: number;
  h: number;
}

function placed(fill: (out: PlacedPages) => void): Shown[] {
  const out = new PlacedPages();
  fill(out);
  return Array.from({ length: out.count }, (_, at) => ({
    page: out.pages[at],
    x: out.x[at],
    y: out.y[at],
    w: widths[out.pages[at]] * out.scales[at],
    h: heights[out.pages[at]] * out.scales[at],
  }));
}

describe('ContinuousView', () => {
  const view = new ContinuousView(single, widths, heights, 12);

  it('uses one physical scale, limited so the widest page is never cut off', () => {
    const window = { width: 1000, height: 3000 };
    const scale = view.scaleFor(window, 1.5, 1, 16);
    expect(scale).toBeCloseTo(968 / 1224, 10);
    const pages = placed((out) => view.place(0, 0, scale, window, out));
    expect(pages.map((page) => page.page)).toEqual([0, 1, 2, 3]);
    for (const page of pages) {
      expect(page.x).toBeGreaterThanOrEqual(0);
      expect(page.x + page.w).toBeLessThanOrEqual(1000);
    }
    // Physical: the plate is exactly twice as wide as the portrait page.
    expect(pages[1].w).toBeCloseTo(pages[0].w * 2, 9);
    // Narrower pages are centred, and the rest is margin.
    expect(pages[0].x + pages[0].w / 2).toBeCloseTo(500, 9);
  });

  it('reads at a comfortable scale on a window wide enough', () => {
    const wide = { width: 4000, height: 1000 };
    expect(view.scaleFor(wide, 1.5, 1, 16)).toBe(1.5);
    expect(view.scaleFor(wide, 1.5, 2, 16)).toBe(3);
  });

  it('turns a position into a Locator and back exactly', () => {
    const window = { width: 2000, height: 500 };
    for (const top of [100.25, 900.5, 1700]) {
      const locator = view.locatorAt(top, 0, 1, window);
      expect(locator.v).toBeGreaterThan(0);
      expect(view.positionOf(locator, 1, window).topPt).toBeCloseTo(top, 9);
    }
    // In the gap after a page, the reader is looking at the next one.
    expect(view.locatorAt(795, 0, 1, window)).toMatchObject({ page: 1, v: 0 });
  });
});

describe('PaginatedView', () => {
  const view = new PaginatedView(single, widths, heights);
  const window = { width: 1000, height: 700 };

  it('fits every row whole and undistorted, whatever its proportions', () => {
    for (let screen = 0; screen < 4; screen++) {
      const scale = view.scaleOf(screen, window, 16, 1);
      const origin = { x: 0, y: 0 };
      const [page] = placed((out) => view.placeScreen(screen, 0, scale, origin, window, out));
      expect(page.x).toBeGreaterThanOrEqual(16 - 1e-9);
      expect(page.y).toBeGreaterThanOrEqual(16 - 1e-9);
      expect(page.x + page.w).toBeLessThanOrEqual(1000 - 16 + 1e-9);
      expect(page.y + page.h).toBeLessThanOrEqual(700 - 16 + 1e-9);
      // One side touches the margins: as large as it can be without cropping.
      const fillsWidth = Math.abs(page.w - 968) < 1e-6;
      const fillsHeight = Math.abs(page.h - 668) < 1e-6;
      expect(fillsWidth || fillsHeight).toBe(true);
    }
  });

  it('pans a zoomed row, and keeps a Locator through it', () => {
    const scale = view.scaleOf(0, window, 16, 3);
    const range = view.panRange(0, scale, window);
    expect(range.x).toBeGreaterThan(0);
    expect(range.y).toBeGreaterThan(0);
    const locator = view.locatorAt(0, scale, { x: range.x / 2, y: range.y / 3 }, window);
    const pan = view.panFor(locator, scale, window);
    expect(pan.x).toBeCloseTo(range.x / 2, 9);
    expect(pan.y).toBeCloseTo(range.y / 3, 9);
  });
});
