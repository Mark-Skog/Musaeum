import { describe, expect, it } from 'vitest';

import { FlowEngine } from './flow-engine';
import { isFromReader, isToReader, SHORTCUT_KEYS } from './messages';

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
    expect(isFromReader({ type: 'shortcut', key: 'fullscreen' })).toBe(true);
    for (const key of SHORTCUT_KEYS) {
      expect(isFromReader({ type: 'shortcut', key })).toBe(true);
    }
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
    expect(isFromReader({ type: 'shortcut', key: 'tab-0' })).toBe(false);
    expect(isFromReader({ type: 'shortcut', key: 'tab-10' })).toBe(false);
    expect(isFromReader({ type: 'shortcut', key: 'close-window' })).toBe(false);
    expect(isFromReader({ type: 'shortcut', key: 1 })).toBe(false);
    expect(isFromReader({ type: 'link', href: 'javascript:alert(1)' })).toBe(false);
    expect(isFromReader({ type: 'link', href: 'file:///C:/Windows/win.ini' })).toBe(false);
    expect(isFromReader({ type: 'link', href: 'blob:http://book.x.localhost/1' })).toBe(false);
  });

  it('takes a selection and a click on a mark only in their own shapes', () => {
    const rect = { x: 10, y: 20, width: 100, height: 18 };
    const selection = { type: 'selection', start: 'epubcfi(/6/4!/4/2/1:3)', end: 'epubcfi(/6/4!/4/2/1:9)', text: 'olhos', rect };
    expect(isFromReader(selection)).toBe(true);
    expect(isFromReader({ type: 'selection', start: null, end: null, text: '', rect: null })).toBe(true);
    expect(isFromReader({ ...selection, end: null })).toBe(false);
    expect(isFromReader({ ...selection, start: 'javascript:1' })).toBe(false);
    expect(isFromReader({ ...selection, text: 'x'.repeat(70_000) })).toBe(false);
    expect(isFromReader({ ...selection, rect: { ...rect, x: Number.NaN } })).toBe(false);
    const id = '0192f3a4-5b6c-7d8e-9f01-23456789abcd';
    expect(isFromReader({ type: 'mark-click', id, rect })).toBe(true);
    expect(isFromReader({ type: 'mark-click', id: 'not-a-uuid', rect })).toBe(false);
    expect(isFromReader({ type: 'mark-click', id, rect: null })).toBe(false);
  });

  it('checks what the page sends the reader too', () => {
    expect(isToReader({ type: 'set-mode', mode: 'continuous' })).toBe(true);
    expect(isToReader({ type: 'set-mode', mode: 'scroll' })).toBe(false);
    expect(isToReader({ type: 'turn', step: 2 })).toBe(false);
    expect(isToReader({ type: 'set-style', style: { family: 'sans' } })).toBe(false);
  });

  it('forwards global tab and preferences shortcuts from FlowEngine.onKey while preserving copy and AltGr', () => {
    const sent: unknown[] = [];
    const host = { send: (msg: unknown) => sent.push(msg) };
    const fire = (init: Partial<KeyboardEvent>): { prevented: boolean } => {
      let prevented = false;
      const ev = {
        key: '',
        code: '',
        ctrlKey: false,
        shiftKey: false,
        altKey: false,
        metaKey: false,
        preventDefault: () => {
          prevented = true;
        },
        ...init,
      } as unknown as KeyboardEvent;
      FlowEngine.prototype.onKey.call(host, ev);
      return { prevented };
    };

    expect(fire({ key: 'Tab', ctrlKey: true }).prevented).toBe(true);
    expect(fire({ key: 'Tab', ctrlKey: true, shiftKey: true }).prevented).toBe(true);
    expect(fire({ key: 'w', ctrlKey: true }).prevented).toBe(true);
    expect(fire({ key: 'F4', ctrlKey: true }).prevented).toBe(true);
    expect(fire({ key: '1', ctrlKey: true }).prevented).toBe(true);
    expect(fire({ key: '9', code: 'Digit9', ctrlKey: true }).prevented).toBe(true);
    expect(fire({ key: ',', ctrlKey: true }).prevented).toBe(true);

    // Ctrl+C (copy) and AltGr combinations (ctrlKey + altKey on ABNT2) must not be hijacked
    expect(fire({ key: 'c', ctrlKey: true }).prevented).toBe(false);
    expect(fire({ key: '1', ctrlKey: true, altKey: true }).prevented).toBe(false);
    expect(fire({ key: 'w', ctrlKey: true, altKey: true }).prevented).toBe(false);

    expect(sent).toEqual([
      { type: 'shortcut', key: 'next-tab' },
      { type: 'shortcut', key: 'prev-tab' },
      { type: 'shortcut', key: 'close-tab' },
      { type: 'shortcut', key: 'close-tab' },
      { type: 'shortcut', key: 'tab-1' },
      { type: 'shortcut', key: 'tab-9' },
      { type: 'shortcut', key: 'preferences' },
    ]);
    expect(sent.every(isFromReader)).toBe(true);
  });
});
