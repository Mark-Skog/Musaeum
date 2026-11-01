import { describe, expect, it } from 'vitest';

import { typographyCss } from './typography';

describe('typographyCss', () => {
  it('overrides whatever size and spacing the book asks for', () => {
    expect(typographyCss({ fontSizePx: 20, lineHeight: 1.4 })).toBe(
      'html { font-size: 20px !important; line-height: 1.4 !important; }',
    );
  });
});
