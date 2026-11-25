import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, describe, expect, it, vi } from 'vitest';

const windowMock = vi.hoisted(() => {
  let maximized = false;
  let fullscreen = false;
  const resizeListeners = new Set<() => void>();
  return {
    get maximized() {
      return maximized;
    },
    set maximized(val: boolean) {
      maximized = val;
    },
    get fullscreen() {
      return fullscreen;
    },
    set fullscreen(val: boolean) {
      fullscreen = val;
    },
    minimize: vi.fn(async () => {}),
    toggleMaximize: vi.fn(async () => {
      maximized = !maximized;
    }),
    close: vi.fn(async () => {}),
    isMaximized: vi.fn(async () => maximized),
    isFullscreen: vi.fn(async () => fullscreen),
    setFullscreen: vi.fn(async (next: boolean) => {
      fullscreen = next;
    }),
    onResized: vi.fn(async (cb: () => void) => {
      resizeListeners.add(cb);
      return () => {
        resizeListeners.delete(cb);
      };
    }),
    emitResize() {
      for (const cb of [...resizeListeners]) {
        cb();
      }
    },
    reset() {
      maximized = false;
      fullscreen = false;
      resizeListeners.clear();
      this.minimize.mockClear();
      this.toggleMaximize.mockClear();
      this.close.mockClear();
      this.isMaximized.mockClear();
      this.isFullscreen.mockClear();
      this.setFullscreen.mockClear();
      this.onResized.mockClear();
    },
  };
});

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => windowMock,
}));

import { setLocale } from './i18n';
import { TitleBar, useWindowChrome } from './TitleBar';

describe('TitleBar custom window chrome', () => {
  afterEach(() => {
    windowMock.reset();
    vi.unstubAllGlobals();
  });

  it('renders Musaeum seal, dynamic title, drag region, windowed top resize strip, and localized pt-BR controls', () => {
    setLocale('pt-BR');
    const html = renderToStaticMarkup(
      <TitleBar
        title="Musaeum"
        maximized={false}
        onMinimize={() => {}}
        onToggleMaximize={() => {}}
        onClose={() => {}}
      />,
    );

    expect(html).toContain('class="titlebar"');
    expect(html).toContain('class="titlebar-resize-top"');
    expect(html).toContain('class="titlebar-drag" data-tauri-drag-region=""');
    expect(html).toContain('class="titlebar-seal"');
    expect(html).toContain('>Musaeum</span>');
    expect(html).toContain('id="titlebar-minimize"');
    expect(html).toContain('aria-label="Minimizar"');
    expect(html).toContain('id="titlebar-maximize"');
    expect(html).toContain('aria-label="Maximizar"');
    // Windowed square glyph (□)
    expect(html).toContain('<rect x="1.5" y="1.5" width="7" height="7"></rect>');
    expect(html).toContain('id="titlebar-close"');
    expect(html).toContain('aria-label="Fechar"');
  });

  it('switches to restore icon (❐), omits top resize strip for Fitts’s Law, and localizes in English when maximized', () => {
    setLocale('en');
    const html = renderToStaticMarkup(
      <TitleBar
        title="Dune"
        maximized={true}
        onMinimize={() => {}}
        onToggleMaximize={() => {}}
        onClose={() => {}}
      />,
    );

    expect(html).toContain('class="titlebar maximized"');
    expect(html).not.toContain('titlebar-resize-top');
    expect(html).toContain('>Dune</span>');
    expect(html).toContain('aria-label="Minimize"');
    expect(html).toContain('aria-label="Restore"');
    expect(html).toContain('aria-label="Close"');
    // Overlapping restore squares glyph (❐)
    expect(html).toContain('<rect x="1.5" y="3.5" width="5.5" height="5.5"></rect>');
    expect(html).toContain('<path d="M3.5 3.5V1.5h5v5H6.5"></path>');
  });

  it('hides completely when fullscreen hidden prop is true', () => {
    const html = renderToStaticMarkup(
      <TitleBar
        title="Dune"
        maximized={false}
        hidden={true}
        onMinimize={() => {}}
        onToggleMaximize={() => {}}
        onClose={() => {}}
      />,
    );
    expect(html).toBe('');
  });

  it('synchronizes maximize and fullscreen state with Tauri window events in useWindowChrome', async () => {
    class MiniNode {
      parentNode: MiniNode | null = null;
      childNodes: MiniNode[] = [];
      ownerDocument: unknown = null;
      nodeType = 1;
      nodeName = 'DIV';
      appendChild<T extends MiniNode>(c: T): T {
        c.parentNode = this;
        c.ownerDocument = this.ownerDocument;
        this.childNodes.push(c);
        return c;
      }
      removeChild<T extends MiniNode>(c: T): T {
        this.childNodes = this.childNodes.filter((n) => n !== c);
        c.parentNode = null;
        return c;
      }
      insertBefore<T extends MiniNode>(c: T): T {
        return this.appendChild(c);
      }
      addEventListener(): void {}
      removeEventListener(): void {}
    }
    class MiniEl extends MiniNode {
      tagName: string;
      style: Record<string, unknown> = {};
      constructor(tag = 'DIV') {
        super();
        this.tagName = tag.toUpperCase();
        this.nodeName = this.tagName;
      }
      setAttribute(): void {}
      removeAttribute(): void {}
    }
    class MiniIFrame extends MiniEl {}
    const doc = {
      title: '',
      fullscreenElement: null,
      activeElement: null,
      defaultView: null as unknown,
      createElement: (tag = 'div') => {
        const el = new MiniEl(tag);
        el.ownerDocument = doc;
        return el;
      },
      createElementNS: (_ns: string, tag = 'svg') => {
        const el = new MiniEl(tag);
        el.ownerDocument = doc;
        return el;
      },
      createTextNode: () => {
        const n = new MiniNode();
        n.nodeType = 3;
        n.ownerDocument = doc;
        return n;
      },
      addEventListener: () => {},
      removeEventListener: () => {},
    };
    const win = {
      __TAURI_INTERNALS__: {},
      document: doc,
      HTMLIFrameElement: MiniIFrame,
      addEventListener: () => {},
      removeEventListener: () => {},
    };
    doc.defaultView = win;

    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
    vi.stubGlobal('document', doc);
    vi.stubGlobal('window', win);
    vi.stubGlobal('HTMLIFrameElement', MiniIFrame);

    let latest: ReturnType<typeof useWindowChrome> | null = null;
    function Harness({ title }: { title: string }): React.JSX.Element | null {
      const chrome = useWindowChrome(title);
      latest = chrome;
      return (
        <TitleBar
          title={title}
          maximized={chrome.maximized}
          hidden={chrome.fullscreen}
          onMinimize={chrome.minimize}
          onToggleMaximize={chrome.toggleMaximize}
          onClose={chrome.close}
        />
      );
    }

    const container = doc.createElement();
    const root = createRoot(container as unknown as HTMLElement);

    await act(async () => {
      root.render(<Harness title="Solaris" />);
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(doc.title).toBe('Solaris');
    expect(latest!.maximized).toBe(false);
    expect(latest!.fullscreen).toBe(false);

    // Simulate OS Aero Snap maximizing the window
    windowMock.maximized = true;
    await act(async () => {
      windowMock.emitResize();
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(latest!.maximized).toBe(true);

    // Toggle fullscreen via F11 handler
    await act(async () => {
      await latest!.toggleFullscreen();
    });
    expect(windowMock.setFullscreen).toHaveBeenCalledWith(true);
    expect(latest!.fullscreen).toBe(true);

    // Minimize and close forward to Tauri window
    act(() => {
      latest!.minimize();
      latest!.close();
    });
    expect(windowMock.minimize).toHaveBeenCalledOnce();
    expect(windowMock.close).toHaveBeenCalledOnce();

    await act(async () => {
      root.unmount();
    });
  });
});
