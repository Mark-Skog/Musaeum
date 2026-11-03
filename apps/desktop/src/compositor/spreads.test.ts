import { describe, expect, it } from 'vitest';

import { BLANK, buildRows, isLandscape, planSpreads } from './spreads';

// The same pages as \`write_spread_fixture\` in tools/gen-fixtures: a cover, a pair, a landscape
// plate, a page that needs a blank beside it, a pair, and a square page.
const widths = new Float32Array([595, 595, 595, 1190, 595, 595, 595, 600]);
const heights = new Float32Array([842, 842, 842, 842, 842, 842, 842, 600]);

const slots = (plan: ReturnType<typeof planSpreads>): number[][] =>
  buildRows(widths, heights, plan).map((row) => row.slots);

describe('planSpreads', () => {
  it('follows /PageLayout when set to auto, and the reader otherwise', () => {
    expect(planSpreads('auto', null, null)).toMatchObject({ double: false, direction: 'ltr' });
    expect(planSpreads('auto', 'OneColumn', null).double).toBe(false);
    expect(planSpreads('auto', 'TwoPageRight', 'R2L')).toEqual({
      double: true,
      firstPageAlone: true,
      direction: 'rtl',
    });
    expect(planSpreads('auto', 'TwoColumnLeft', null).firstPageAlone).toBe(false);
    expect(planSpreads('single', 'TwoPageRight', null).double).toBe(false);
    // Facing pages asked for by the reader: as in a printed book, the first page alone.
    expect(planSpreads('double', null, null)).toMatchObject({ double: true, firstPageAlone: true });
  });
});

describe('buildRows', () => {
  it('keeps printed parity with blanks, and landscape pages alone', () => {
    expect(slots(planSpreads('double', null, 'L2R'))).toEqual([
      [BLANK, 0],
      [1, 2],
      [3],
      // Without this blank, 4 and 5 would pair up and every spread after the plate would be wrong.
      [BLANK, 4],
      [5, 6],
      [7, BLANK],
    ]);
  });

  it('mirrors the order of facing pages for right-to-left reading', () => {
    expect(slots(planSpreads('auto', 'TwoPageRight', 'R2L'))).toEqual([
      [0, BLANK],
      [2, 1],
      [3],
      [4, BLANK],
      [6, 5],
      [BLANK, 7],
    ]);
  });

  it('pairs from the first page when odd pages go on the left', () => {
    expect(slots(planSpreads('auto', 'TwoColumnLeft', null))).toEqual([
      [0, 1],
      [2, BLANK],
      [3],
      [4, 5],
      [6, 7],
    ]);
  });

  it('shows one page per row otherwise', () => {
    const one = [[0], [1], [2], [3], [4], [5], [6], [7]];
    expect(slots(planSpreads('auto', null, null))).toEqual(one);
  });

  it('sizes rows by their slots, a blank being as large as the page it faces', () => {
    const rows = buildRows(widths, heights, planSpreads('double', null, null));
    expect(rows[0]).toMatchObject({ widthPt: 1190, heightPt: 842, pages: [0] });
    expect(rows[2]).toMatchObject({ widthPt: 1190, heightPt: 842, pages: [3] });
    expect(rows[5]).toMatchObject({ widthPt: 1200, heightPt: 600, pages: [7] });
  });

  it('counts a nearly square page as square, not landscape', () => {
    expect(isLandscape(1190, 842)).toBe(true);
    expect(isLandscape(600, 600)).toBe(false);
    expect(isLandscape(510, 500)).toBe(false);
    expect(isLandscape(595, 842)).toBe(false);
  });
});
