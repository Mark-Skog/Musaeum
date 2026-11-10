import { describe, expect, it } from 'vitest';

import {
  BASE_FONT_PX,
  changesLayout,
  contentCss,
  DEFAULT_STYLE,
  isReadingStyle,
  typographyCss,
} from './typography';

describe('typographyCss', () => {
  it('overrides whatever size and spacing the book asks for', () => {
    expect(typographyCss({ fontSizePx: 20, lineHeight: 1.4 })).toBe(
      'html { font-size: 20px !important; line-height: 1.4 !important; }',
    );
  });
});

describe('the reading style', () => {
  it('turns into CSS that sizes the root and keeps code in its own font', () => {
    const css = contentCss({ ...DEFAULT_STYLE, size: 150, family: 'serif', lineHeight: 1.7 });
    expect(css).toContain(`html { font-size: ${BASE_FONT_PX * 1.5}px !important;`);
    expect(css).toContain('line-height: 1.7 !important');
    expect(css).toMatch(/font-family: Charter/);
    expect(css).not.toMatch(/\bcode\b[^{]*\{[^}]*font-family/);
  });

  it("keeps the book's own font and line height when asked to", () => {
    const css = contentCss({ ...DEFAULT_STYLE, family: 'publisher', lineHeight: 0 });
    expect(css).not.toContain('font-family');
    expect(css).not.toMatch(/line-height/);
  });

  it("overrides the book's colors only away from a white page", () => {
    expect(contentCss({ ...DEFAULT_STYLE, theme: 'light' })).not.toContain('background-color: transparent');
    const dark = contentCss({ ...DEFAULT_STYLE, theme: 'dark' });
    expect(dark).toContain('background-color: transparent !important');
    expect(dark).toContain('color-scheme: dark');
  });

  it('knows which changes lay the text out again', () => {
    expect(changesLayout(DEFAULT_STYLE, { ...DEFAULT_STYLE, theme: 'sepia' })).toBe(false);
    expect(changesLayout(DEFAULT_STYLE, { ...DEFAULT_STYLE, size: 110 })).toBe(true);
    expect(changesLayout(DEFAULT_STYLE, { ...DEFAULT_STYLE, margin: 'wide' })).toBe(true);
    expect(changesLayout(DEFAULT_STYLE, { ...DEFAULT_STYLE, family: 'sans' })).toBe(true);
    expect(changesLayout(DEFAULT_STYLE, { ...DEFAULT_STYLE, lineHeight: 2 })).toBe(true);
  });

  it('accepts only a style the panel could have made', () => {
    expect(isReadingStyle(DEFAULT_STYLE)).toBe(true);
    expect(isReadingStyle({ ...DEFAULT_STYLE, size: 10 })).toBe(false);
    expect(isReadingStyle({ ...DEFAULT_STYLE, theme: 'neon' })).toBe(false);
    expect(isReadingStyle({ ...DEFAULT_STYLE, lineHeight: 0.5 })).toBe(false);
    expect(isReadingStyle({ ...DEFAULT_STYLE, family: 'Comic Sans' })).toBe(false);
    expect(isReadingStyle(null)).toBe(false);
  });
});
