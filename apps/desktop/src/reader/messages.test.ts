import { describe, expect, it } from 'vitest';

import { isFromReader } from './messages';

describe('messages from the reader', () => {
  it('accepts the shapes the reader really sends', () => {
    expect(isFromReader({ type: 'ready', sections: 4, openMs: 12.5 })).toBe(true);
    expect(isFromReader({ type: 'resize-window', id: 1, width: 1200, height: 800 })).toBe(true);
    expect(isFromReader({ type: 'phase', name: 'reading' })).toBe(true);
    expect(isFromReader({ type: 'report', report: {} })).toBe(true);
    expect(isFromReader({ type: 'failed', message: 'no' })).toBe(true);
  });

  it('refuses anything else, because a hostile book may be the one writing', () => {
    for (const message of [
      null,
      undefined,
      'ready',
      42,
      {},
      { pwned: 'script-inline' },
      { type: 'open_session' },
      { type: 'ready', sections: '4', openMs: 1 },
      { type: 'phase', name: 7 },
      { type: 'phase', name: 'x'.repeat(65) },
      { type: 'report', report: null },
      { type: 'failed' },
    ]) {
      expect(isFromReader(message)).toBe(false);
    }
  });

  it('refuses a window size that would hide the window or fill the desktop', () => {
    const resize = (width: unknown, height: unknown): boolean =>
      isFromReader({ type: 'resize-window', id: 1, width, height });
    expect(resize(1, 800)).toBe(false);
    expect(resize(1200, 100_000)).toBe(false);
    expect(resize(1200.5, 800)).toBe(false);
    expect(resize('1200', 800)).toBe(false);
  });
});
