/**
 * The app's colors keep WCAG 2.2 AA in both themes: text against what it is read on, 4.5:1; the
 * boundary of a control, the focus ring and a state shown by color alone, 3:1 (1.4.11). The colors
 * are read from `app.css` itself, the light ones from `:root` and the dark ones from both of its
 * dark blocks, which must say the same.
 */

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

const CSS = readFileSync(fileURLToPath(new URL('../src/app/app.css', import.meta.url)), 'utf8');

/** The `--name: #rrggbb` of the first block that `selector` opens. */
function tokens(selector) {
  const at = CSS.indexOf(selector);
  if (at < 0) {
    throw new Error(`no ${selector}`);
  }
  const body = CSS.slice(CSS.indexOf('{', at) + 1, CSS.indexOf('}', at));
  return Object.fromEntries([...body.matchAll(/--([\w-]+):\s*(#[0-9a-f]{6})\s*;/gi)].map((match) => [match[1], match[2].toLowerCase()]));
}

const light = tokens(':root {');
const darkMedia = tokens(":root:not([data-theme='light'])");
const dark = tokens(":root[data-theme='dark']");

function luminance(hex) {
  const channel = (at) => {
    const value = parseInt(hex.slice(at, at + 2), 16) / 255;
    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5);
}

function contrast(a, b) {
  const [high, low] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (high + 0.05) / (low + 0.05);
}

const GROUNDS = ['bg', 'surface', 'surface-2'];
/** Text, and what it is read on. */
const TEXT = [
  ...['text', 'muted', 'accent', 'danger'].flatMap((fg) => GROUNDS.map((bg) => [fg, bg])),
  ['accent-text', 'accent'],
  ['text', 'mark'],
];
/** What is not text but has to be seen: 3:1 against every ground it stands on. */
const NON_TEXT = ['focus', 'control-line', 'star', 'accent', 'scrollbar-thumb-hover', 'scrollbar-thumb-active'].flatMap((fg) =>
  GROUNDS.map((bg) => [fg, bg]),
);

describe('the colors', () => {
  it('are the same in both dark blocks, and every one has a dark value', () => {
    expect(darkMedia).toEqual(dark);
    expect(Object.keys(dark).sort()).toEqual(Object.keys(light).sort());
  });

  for (const [name, theme] of [['light', light], ['dark', dark]]) {
    it(`keep text at 4.5:1 or more in the ${name} theme`, () => {
      const low = TEXT.map(([fg, bg]) => [`${fg} on ${bg}`, contrast(theme[fg], theme[bg])]).filter(([, ratio]) => ratio < 4.5);
      expect(low).toEqual([]);
    });

    it(`keep controls, focus and states at 3:1 or more in the ${name} theme`, () => {
      const low = NON_TEXT.map(([fg, bg]) => [`${fg} on ${bg}`, contrast(theme[fg], theme[bg])]).filter(([, ratio]) => ratio < 3);
      expect(low).toEqual([]);
    });
  }

  it('are worked out as WCAG says', () => {
    expect(contrast('#000000', '#ffffff')).toBeCloseTo(21, 5);
    expect(contrast('#767676', '#ffffff')).toBeCloseTo(4.54, 2);
  });
});
