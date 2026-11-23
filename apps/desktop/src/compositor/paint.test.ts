import { describe, expect, it } from 'vitest';

import { PALETTE, paintOf } from './annotator';

describe('paintOf', () => {
  it('paints palette names with their fill and their ink', () => {
    const yellow = paintOf('yellow');
    expect(yellow.fill).toEqual(PALETTE.yellow);
    expect(yellow.ink).toEqual([0.85, 0.6, 0.0]);
  });

  it('paints a color a PDF gave as itself, over white as much as its opacity says', () => {
    expect(paintOf('#ff0000')).toEqual({ fill: [1, 0, 0], ink: [1, 0, 0] });
    const half = paintOf('#0000ff80');
    const alpha = 0x80 / 255;
    expect(half.fill[0]).toBeCloseTo(1 - alpha, 6);
    expect(half.fill[2]).toBeCloseTo(1, 6);
    expect(paintOf('#ABCDEF').fill[1]).toBeCloseTo(0xcd / 255, 6);
  });

  it('paints anything else yellow', () => {
    expect(paintOf('#12').fill).toEqual(PALETTE.yellow);
    expect(paintOf('red').fill).toEqual(PALETTE.yellow);
  });
});
