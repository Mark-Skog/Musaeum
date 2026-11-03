/**
 * Which pages are shown together. A row is one page, or two facing pages, and both reading modes
 * are built from rows: the continuous mode stacks them, the paginated mode shows one at a time.
 *
 * Facing pages keep the parity they have in print. A recto never moves to the left just because a
 * landscape plate or a missing page came before it: a blank takes the place of its partner instead,
 * so that pages printed facing each other still face each other. A landscape page is a spread on
 * its own and stands alone.
 */

/** Pages whose sides differ by less than this fraction count as square, as in `musaeum-core`. */
const SQUARE_TOLERANCE = 0.03;

/** Where no page goes, to keep the one beside it on its side. */
export const BLANK = -1;

export type SpreadsSetting = 'auto' | 'single' | 'double';
export type ReadingDirection = 'ltr' | 'rtl';

/** `/PageLayout` as the host reports it, or null without one. */
export type PageLayoutHint =
  | 'SinglePage'
  | 'OneColumn'
  | 'TwoColumnLeft'
  | 'TwoColumnRight'
  | 'TwoPageLeft'
  | 'TwoPageRight'
  | null;

export interface SpreadPlan {
  double: boolean;
  /** The first page stands alone, as a book's first recto does, so pairs are (2, 3), (4, 5)… */
  firstPageAlone: boolean;
  direction: ReadingDirection;
}

/**
 * What to do with a document. `auto` is what the file asks for; without a `/PageLayout`, one page
 * at a time. Facing pages put the first page alone unless the file says odd pages go on the left.
 */
export function planSpreads(
  setting: SpreadsSetting,
  pageLayout: PageLayoutHint,
  direction: 'L2R' | 'R2L' | null,
): SpreadPlan {
  const twoUp = pageLayout !== null && pageLayout.startsWith('Two');
  return {
    double: setting === 'double' || (setting === 'auto' && twoUp),
    firstPageAlone: !(pageLayout === 'TwoColumnLeft' || pageLayout === 'TwoPageLeft'),
    direction: direction === 'R2L' ? 'rtl' : 'ltr',
  };
}

export function isLandscape(widthPt: number, heightPt: number): boolean {
  const longest = Math.max(widthPt, heightPt);
  return longest > 0 && widthPt - heightPt >= longest * SQUARE_TOLERANCE;
}

export interface Row {
  /** Pages from left to right as seen, with [`BLANK`] where a facing page is missing. */
  slots: number[];
  /** Pages in reading order, without blanks. */
  pages: number[];
  /** Width of each slot, in points. A blank is as wide as the page it faces. */
  slotWidths: number[];
  /** Height of each slot, in points. */
  slotHeights: number[];
  widthPt: number;
  heightPt: number;
}

function makeRow(
  leading: number,
  trailing: number,
  plan: SpreadPlan,
  widths: Float32Array,
  heights: Float32Array,
): Row {
  const inOrder = [leading, trailing].filter((page) => page !== BLANK || plan.double);
  const solo = inOrder.length === 1;
  // In reading order, the leading page comes first: on the left, or on the right in RTL.
  const facing = plan.direction === 'rtl' ? [trailing, leading] : [leading, trailing];
  const visual = solo ? inOrder : facing;
  const partner = (page: number): number => (page === leading ? trailing : leading);
  const slotWidths = visual.map((page) => widths[page === BLANK ? partner(page) : page]);
  const slotHeights = visual.map((page) => heights[page === BLANK ? partner(page) : page]);
  return {
    slots: visual,
    pages: [leading, trailing].filter((page) => page !== BLANK),
    slotWidths,
    slotHeights,
    widthPt: slotWidths.reduce((sum, width) => sum + width, 0),
    heightPt: Math.max(...slotHeights),
  };
}

/** Lays pages out in rows. `widths` and `heights` are in points, after `/Rotate`. */
export function buildRows(widths: Float32Array, heights: Float32Array, plan: SpreadPlan): Row[] {
  const rows: Row[] = [];
  const count = widths.length;
  if (!plan.double) {
    for (let page = 0; page < count; page++) {
      rows.push(makeRow(page, BLANK, plan, widths, heights));
    }
    return rows;
  }

  // A leading page waiting for the page that faces it.
  let waiting = BLANK;
  const flush = (): void => {
    if (waiting !== BLANK) {
      rows.push(makeRow(waiting, BLANK, plan, widths, heights));
      waiting = BLANK;
    }
  };
  for (let page = 0; page < count; page++) {
    if (isLandscape(widths[page], heights[page])) {
      flush();
      rows.push(makeRow(page, BLANK, { ...plan, double: false }, widths, heights));
      continue;
    }
    // Parity comes from the page's number, never from what came before it.
    const leads = plan.firstPageAlone ? page % 2 === 1 : page % 2 === 0;
    if (leads) {
      flush();
      waiting = page;
    } else if (waiting === page - 1) {
      rows.push(makeRow(waiting, page, plan, widths, heights));
      waiting = BLANK;
    } else {
      flush();
      rows.push(makeRow(BLANK, page, plan, widths, heights));
    }
  }
  flush();
  return rows;
}

/** The row holding each page. */
export function rowOfPage(rows: Row[], pageCount: number): Int32Array {
  const rowOf = new Int32Array(pageCount);
  rows.forEach((row, index) => {
    for (const page of row.pages) {
      rowOf[page] = index;
    }
  });
  return rowOf;
}
