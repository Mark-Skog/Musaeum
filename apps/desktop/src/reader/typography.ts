/** The reader's settings, and the CSS they turn into. */

/** What the Phase 0 benchmark sets: a size and a line height. */
export interface Typography {
  fontSizePx: number;
  lineHeight: number;
}

export function typographyCss({ fontSizePx, lineHeight }: Typography): string {
  return `html { font-size: ${fontSizePx}px !important; line-height: ${lineHeight} !important; }`;
}

export type FontFamily = 'publisher' | 'serif' | 'sans';
export type Margin = 'narrow' | 'normal' | 'wide';
export type Theme = 'light' | 'sepia' | 'dark';

/** What a reader chooses for flowing books. */
export interface ReadingStyle {
  family: FontFamily;
  /** Percent of the base size, 100 being 18 px. */
  size: number;
  /** Line height as a multiple of the font size; 0 keeps the book's own. */
  lineHeight: number;
  margin: Margin;
  theme: Theme;
}

export const DEFAULT_STYLE: ReadingStyle = {
  family: 'publisher',
  size: 100,
  lineHeight: 1.5,
  margin: 'normal',
  theme: 'light',
};

export const BASE_FONT_PX = 18;
export const MIN_SIZE = 60;
export const MAX_SIZE = 300;
export const LINE_HEIGHTS = [0, 1.2, 1.35, 1.5, 1.7, 2] as const;
/** The widest line of text, in CSS pixels, before the column stops growing. */
export const MAX_LINE_PX = 760;

const FAMILIES: Record<Exclude<FontFamily, 'publisher'>, string> = {
  serif: "Charter, 'Bitstream Charter', 'Iowan Old Style', Georgia, Cambria, 'Times New Roman', serif",
  sans: "'Segoe UI', 'Noto Sans', Inter, Roboto, Arial, sans-serif",
};

export const THEMES: Record<Theme, { background: string; color: string; link: string }> = {
  light: { background: '#ffffff', color: '#1d1d1f', link: '#1f5fbf' },
  sepia: { background: '#f4ecd8', color: '#4a3b28', link: '#8a4b16' },
  dark: { background: '#1b1c1e', color: '#d8d6d1', link: '#8ab4f8' },
};

/** The part of the width given to the margins on each side, by setting. */
export const MARGIN_FRACTION: Record<Margin, number> = {
  narrow: 0.03,
  normal: 0.07,
  wide: 0.13,
};

export function isReadingStyle(value: unknown): value is ReadingStyle {
  if (typeof value !== 'object' || value === null) {
    return false;
  }
  const style = value as Record<string, unknown>;
  return (
    ['publisher', 'serif', 'sans'].includes(style.family as string) &&
    typeof style.size === 'number' &&
    style.size >= MIN_SIZE &&
    style.size <= MAX_SIZE &&
    typeof style.lineHeight === 'number' &&
    (style.lineHeight === 0 || (style.lineHeight >= 1 && style.lineHeight <= 3)) &&
    ['narrow', 'normal', 'wide'].includes(style.margin as string) &&
    ['light', 'sepia', 'dark'].includes(style.theme as string)
  );
}

/** Whether going from `a` to `b` lays the text out again, or only changes its colors. */
export function changesLayout(a: ReadingStyle, b: ReadingStyle): boolean {
  return (
    a.family !== b.family || a.size !== b.size || a.lineHeight !== b.lineHeight || a.margin !== b.margin
  );
}

/** Text elements whose look the reader's choices override. Code keeps its own font. */
const TEXT = 'body, p, div, span, li, dd, dt, blockquote, td, th, caption, figcaption, h1, h2, h3, h4, h5, h6, a, em, strong, i, b, u, small, cite, q, sup, sub, label';

/** The CSS of a section, in either way of reading. */
export function contentCss(style: ReadingStyle): string {
  const theme = THEMES[style.theme];
  const rules = [
    `html { font-size: ${(BASE_FONT_PX * style.size) / 100}px !important; color-scheme: ${style.theme === 'dark' ? 'dark' : 'light'}; }`,
    `html, body { background: transparent !important; }`,
    `body { color: ${theme.color}; }`,
    `a:link, a:visited { color: ${theme.link}; }`,
    // Long words and URLs wrap instead of widening the page.
    `body { overflow-wrap: break-word; }`,
  ];
  if (style.family !== 'publisher') {
    rules.push(`${TEXT} { font-family: ${FAMILIES[style.family]} !important; }`);
  }
  if (style.lineHeight > 0) {
    rules.push(`body, p, li, dd, dt, blockquote, td, th, div { line-height: ${style.lineHeight} !important; }`);
  }
  if (style.theme !== 'light') {
    // A book's own colors were chosen for a white page; on another one, the text takes the
    // theme's color, and boxes lose their light backgrounds.
    rules.push(
      `${TEXT} { color: ${theme.color} !important; background-color: transparent !important; }`,
      `a:link, a:visited, a * { color: ${theme.link} !important; }`,
    );
  }
  return rules.join('\n');
}
