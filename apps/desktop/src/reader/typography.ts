/** The reader's settings, and the CSS they turn into. */

export interface Typography {
  fontSizePx: number;
  lineHeight: number;
}

export function typographyCss({ fontSizePx, lineHeight }: Typography): string {
  return `html { font-size: ${fontSizePx}px !important; line-height: ${lineHeight} !important; }`;
}
