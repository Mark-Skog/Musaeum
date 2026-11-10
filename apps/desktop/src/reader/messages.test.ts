import { describe, expect, it } from 'vitest';

import { isFromReader, isToReader } from './messages';

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

describe('messages of the app reader', () => {
  const relocate = {
    type: 'relocate',
    cfi: 'epubcfi(/6/4[d0p1]!/4/2/1:10)',
    section: 1,
    progress: 0.25,
    atStart: false,
    atEnd: false,
    reason: 'page',
  };

  it('accepts a position, a layout, a shortcut and a link', () => {
    expect(isFromReader(relocate)).toBe(true);
    expect(isFromReader({ type: 'opened', openMs: 12 })).toBe(true);
    expect(isFromReader({ type: 'layout', ms: 40, change: 'typography' })).toBe(true);
    expect(isFromReader({ type: 'shortcut', key: 'mode' })).toBe(true);
    expect(isFromReader({ type: 'link', href: 'https://example.org/a' })).toBe(true);
    expect(isFromReader({ type: 'link', href: 'mailto:a@example.org' })).toBe(true);
  });

  it('refuses what a broken-out book could try to send', () => {
    expect(isFromReader({ ...relocate, cfi: 'javascript:alert(1)' })).toBe(false);
    expect(isFromReader({ ...relocate, cfi: `epubcfi(${'/4'.repeat(3000)})` })).toBe(false);
    expect(isFromReader({ ...relocate, progress: 2 })).toBe(false);
    expect(isFromReader({ ...relocate, section: -1 })).toBe(false);
    expect(isFromReader({ ...relocate, reason: 'x'.repeat(100) })).toBe(false);
    expect(isFromReader({ type: 'shortcut', key: 'quit' })).toBe(false);
    expect(isFromReader({ type: 'link', href: 'javascript:alert(1)' })).toBe(false);
    expect(isFromReader({ type: 'link', href: 'file:///C:/Windows/win.ini' })).toBe(false);
    expect(isFromReader({ type: 'link', href: 'blob:http://book.x.localhost/1' })).toBe(false);
  });

  it('checks what the page sends the reader too', () => {
    expect(isToReader({ type: 'set-mode', mode: 'continuous' })).toBe(true);
    expect(isToReader({ type: 'set-mode', mode: 'scroll' })).toBe(false);
    expect(isToReader({ type: 'turn', step: 2 })).toBe(false);
    expect(isToReader({ type: 'set-style', style: { family: 'sans' } })).toBe(false);
  });
});
