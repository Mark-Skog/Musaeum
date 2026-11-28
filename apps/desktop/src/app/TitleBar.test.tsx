import { act, createContext, forwardRef, type ReactNode, useContext } from 'react';
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

vi.mock('./preferences/store', () => ({
  isMotionReduced: () => false,
}));

vi.mock('react-aria-components', () => {
  const MenuActionCtx = createContext<((key: string) => void) | undefined>(undefined);
  return {
    MenuTrigger: ({ children }: { children?: ReactNode }) => <>{children}</>,
    Button: forwardRef<
      HTMLButtonElement,
      {
        id?: string;
        className?: string;
        'aria-label'?: string;
        children?: ReactNode;
        onPress?: () => void;
      }
    >(function MockButton(props, ref) {
      return (
        <button
          ref={ref}
          type="button"
          id={props.id}
          className={props.className}
          aria-label={props['aria-label']}
          onClick={() => props.onPress?.()}
        >
          {props.children}
        </button>
      );
    }),
    Popover: ({ className, children }: { className?: string; children?: ReactNode }) => (
      <div className={className}>{children}</div>
    ),
    Menu: ({
      className,
      'aria-label': ariaLabel,
      onAction,
      children,
    }: {
      className?: string;
      'aria-label'?: string;
      onAction?: (key: string) => void;
      children?: ReactNode;
    }) => (
      <MenuActionCtx.Provider value={onAction}>
        <div role="menu" className={className} aria-label={ariaLabel}>
          {children}
        </div>
      </MenuActionCtx.Provider>
    ),
    MenuSection: ({ children }: { children?: ReactNode }) => <div>{children}</div>,
    Header: ({ className, children }: { className?: string; children?: ReactNode }) => (
      <div className={className}>{children}</div>
    ),
    MenuItem: ({
      id,
      className,
      isDisabled,
      'data-action': dataAction,
      children,
    }: {
      id?: string;
      className?: string;
      isDisabled?: boolean;
      'data-action'?: string;
      children?: ReactNode;
    }) => {
      const onAction = useContext(MenuActionCtx);
      return (
        <div
          role="menuitem"
          id={id}
          className={className}
          data-action={dataAction}
          aria-disabled={isDisabled || undefined}
          onClick={() => {
            if (!isDisabled && id !== undefined) {
              onAction?.(id);
            }
          }}
        >
          {children}
        </div>
      );
    },
    Separator: ({ className }: { className?: string }) => <hr className={className} />,
  };
});

import { setLocale } from './i18n';
import { type BookTab, bookTabId, HOME_TAB_ID } from './tabs-model';
import { TitleBar, type TitleBarMenuAction, useWindowChrome } from './TitleBar';

function makeBookTab(id: number, title: string, format = 'pdf'): BookTab {
  return {
    id: bookTabId(id),
    documentId: id,
    workId: id * 10,
    title,
    format,
    target: null,
    targetSeq: 0,
  };
}

describe('TitleBar unified Acrobat-style tab bar & Win32 chrome', () => {
  afterEach(() => {
    windowMock.reset();
    vi.unstubAllGlobals();
  });

  it('renders unified top-left options menu, aligned Home tab, drag spacer, windowed top resize strip, and pt-BR controls', () => {
    setLocale('pt-BR');
    const html = renderToStaticMarkup(
      <TitleBar
        tabs={[]}
        activeTabId={HOME_TAB_ID}
        maximized={false}
        onMinimize={() => {}}
        onToggleMaximize={() => {}}
        onClose={() => {}}
      />,
    );

    expect(html).toContain('class="titlebar"');
    expect(html).toContain('class="titlebar-resize-top"');
    expect(html).toContain('id="library-actions"');
    expect(html).toContain('aria-label="Ações da biblioteca"');
    expect(html).toContain('Adicionar pastas…');
    expect(html).toContain('Gerenciar pastas…');
    expect(html).toContain('Importar do Calibre…');
    expect(html).toContain('Exportar todas as anotações (JSON-LD)…');
    expect(html).toContain('Importar anotações (JSON-LD)…');
    expect(html).toContain('Campos personalizados…');
    expect(html).toContain('Preferências…');
    expect(html).toContain('role="tablist"');
    expect(html).toContain('aria-label="Abas abertas"');
    expect(html).toContain('id="tab-home"');
    expect(html).toContain('role="tab"');
    expect(html).toContain('aria-selected="true"');
    expect(html).toContain('aria-label="Biblioteca"');
    expect(html).toContain('class="titlebar-drag" data-tauri-drag-region=""');
    expect(html).toContain('id="titlebar-minimize"');
    expect(html).toContain('aria-label="Minimizar"');
    expect(html).toContain('id="titlebar-maximize"');
    expect(html).toContain('aria-label="Maximizar"');
    // Windowed square glyph (□)
    expect(html).toContain('<rect x="1.5" y="1.5" width="7" height="7"></rect>');
    expect(html).toContain('id="titlebar-close"');
    expect(html).toContain('aria-label="Fechar"');
  });

  it('renders book tabs, keeps drag spacer with 15+ open tabs, omits top resize strip for Fitts’s Law, and localizes in English when maximized', () => {
    setLocale('en');
    const tabs = Array.from({ length: 15 }, (_, idx) => makeBookTab(idx + 1, idx === 0 ? 'Dune' : `Book ${idx + 1}`));
    const html = renderToStaticMarkup(
      <TitleBar
        tabs={tabs}
        activeTabId={bookTabId(1)}
        maximized={true}
        onMinimize={() => {}}
        onToggleMaximize={() => {}}
        onClose={() => {}}
      />,
    );

    expect(html).toContain('class="titlebar maximized"');
    expect(html).not.toContain('titlebar-resize-top');
    expect(html).toContain('aria-label="Library actions"');
    expect(html).toContain('Add folders…');
    expect(html).toContain('Preferences…');
    expect(html).toContain('aria-label="Open tabs"');
    expect(html).toContain('aria-label="Library"');
    expect(html).toContain('data-tab-id="doc:1"');
    expect(html).toContain('data-tab-id="doc:15"');
    expect(html).toContain('>Dune</span>');
    expect(html).toContain('aria-label="Close Dune"');
    // Guaranteed drag spacer remains present even with 15 open tabs
    expect(html).toContain('class="titlebar-drag" data-tauri-drag-region=""');
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
        tabs={[makeBookTab(1, 'Dune')]}
        activeTabId={bookTabId(1)}
        maximized={false}
        hidden={true}
        onMinimize={() => {}}
        onToggleMaximize={() => {}}
        onClose={() => {}}
      />,
    );
    expect(html).toBe('');
  });

  it('handles top-left menu actions, tab selection, close button stopPropagation, middle-click close, wheel scroll, and useWindowChrome sync', async () => {
    class MiniEvent {
      defaultPrevented = false;
      cancelBubble = false;
      target: unknown = null;
      currentTarget: unknown = null;
      button = 0;
      deltaX = 0;
      deltaY = 0;
      constructor(
        public readonly type: string,
        init?: Record<string, unknown>,
      ) {
        if (init) {
          Object.assign(this, init);
        }
      }
      preventDefault(): void {
        this.defaultPrevented = true;
      }
      stopPropagation(): void {
        this.cancelBubble = true;
      }
    }

    class MiniNode {
      parentNode: MiniNode | null = null;
      childNodes: MiniNode[] = [];
      ownerDocument: unknown = null;
      nodeType = 1;
      nodeName = 'DIV';
      listeners = new Map<string, Set<(ev: MiniEvent) => void>>();
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
      addEventListener(type: string, cb: (ev: MiniEvent) => void): void {
        let set = this.listeners.get(type);
        if (!set) {
          set = new Set();
          this.listeners.set(type, set);
        }
        set.add(cb);
      }
      removeEventListener(type: string, cb: (ev: MiniEvent) => void): void {
        this.listeners.get(type)?.delete(cb);
      }
      dispatchEvent(ev: MiniEvent): boolean {
        if (ev.target == null) {
          ev.target = this;
        }
        let cur: MiniNode | null = this;
        while (cur !== null) {
          ev.currentTarget = cur;
          for (const cb of [...(cur.listeners.get(ev.type) ?? [])]) {
            cb(ev);
          }
          if (ev.cancelBubble) {
            break;
          }
          cur = cur.parentNode;
        }
        return !ev.defaultPrevented;
      }
    }

    class MiniEl extends MiniNode {
      tagName: string;
      style: Record<string, unknown> = {};
      attrs = new Map<string, string>();
      scrollLeft = 0;
      scrollWidth = 900;
      clientWidth = 300;
      scrollIntoView = vi.fn();
      focus = vi.fn();
      constructor(tag = 'DIV') {
        super();
        this.tagName = tag.toUpperCase();
        this.nodeName = this.tagName;
      }
      get id(): string {
        return this.attrs.get('id') ?? '';
      }
      set id(v: string) {
        this.attrs.set('id', v);
      }
      get className(): string {
        return this.attrs.get('class') ?? '';
      }
      set className(v: string) {
        this.attrs.set('class', v);
      }
      setAttribute(k: string, v: string): void {
        this.attrs.set(k, String(v));
      }
      getAttribute(k: string): string | null {
        return this.attrs.get(k) ?? null;
      }
      removeAttribute(k: string): void {
        this.attrs.delete(k);
      }
      querySelectorAll(selector: string): MiniEl[] {
        const out: MiniEl[] = [];
        const walk = (node: MiniNode): void => {
          for (const ch of node.childNodes) {
            if (ch instanceof MiniEl) {
              const matches =
                selector === '[data-tab-id]'
                  ? ch.attrs.has('data-tab-id')
                  : selector.startsWith('#')
                    ? ch.id === selector.slice(1)
                    : selector.startsWith('.')
                      ? ch.className.split(/\s+/).includes(selector.slice(1))
                      : false;
              if (matches) {
                out.push(ch);
              }
              walk(ch);
            }
          }
        };
        walk(this);
        return out;
      }
      querySelector(selector: string): MiniEl | null {
        return this.querySelectorAll(selector)[0] ?? null;
      }
    }
    class MiniIFrame extends MiniEl {}
    const invokeSpy = vi.fn(async () => undefined);
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
      __TAURI_INTERNALS__: { invoke: invokeSpy },
      document: doc,
      HTMLIFrameElement: MiniIFrame,
      HTMLElement: MiniEl,
      SVGElement: MiniEl,
      Element: MiniEl,
      Node: MiniNode,
      addEventListener: () => {},
      removeEventListener: () => {},
    };
    doc.defaultView = win;

    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
    vi.stubGlobal('document', doc);
    vi.stubGlobal('window', win);
    vi.stubGlobal('HTMLElement', MiniEl);
    vi.stubGlobal('SVGElement', MiniEl);
    vi.stubGlobal('Element', MiniEl);
    vi.stubGlobal('Node', MiniNode);
    vi.stubGlobal('HTMLIFrameElement', MiniIFrame);

    const onSelectTab = vi.fn();
    const onCloseTab = vi.fn();
    const onMenuAction = vi.fn<(action: TitleBarMenuAction) => void>();
    const sampleTabs = [makeBookTab(1, 'Solaris'), makeBookTab(2, 'Dune')];

    let latest: ReturnType<typeof useWindowChrome> | null = null;
    function Harness({ title, activeTabId }: { title: string; activeTabId: string }): React.JSX.Element | null {
      const chrome = useWindowChrome(title);
      latest = chrome;
      return (
        <TitleBar
          tabs={sampleTabs}
          activeTabId={activeTabId}
          maximized={chrome.maximized}
          hidden={chrome.fullscreen}
          onSelectTab={onSelectTab}
          onCloseTab={onCloseTab}
          onMenuAction={onMenuAction}
          onMinimize={chrome.minimize}
          onToggleMaximize={chrome.toggleMaximize}
          onClose={chrome.close}
        />
      );
    }

    const container = doc.createElement();
    const root = createRoot(container as unknown as HTMLElement);

    await act(async () => {
      root.render(<Harness title="Solaris — Musaeum" activeTabId="doc:1" />);
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(doc.title).toBe('Solaris — Musaeum');
    expect(invokeSpy.mock.calls[0]?.slice(0, 2)).toEqual(['set_window_title', { title: 'Solaris — Musaeum' }]);
    expect(latest!.maximized).toBe(false);
    expect(latest!.fullscreen).toBe(false);

    // Top-left gear menu dispatches library options and Preferences
    const actionsBtn = container.querySelector('#library-actions')!;
    act(() => {
      container.querySelector('#add-folder')!.dispatchEvent(new MiniEvent('click', { bubbles: true }));
      container.querySelector('#preferences')!.dispatchEvent(new MiniEvent('click', { bubbles: true }));
    });
    expect(onMenuAction).toHaveBeenNthCalledWith(1, 'add-folder');
    expect(onMenuAction).toHaveBeenNthCalledWith(2, 'preferences');
    expect(actionsBtn.focus).toHaveBeenCalled();

    // Clicking background tab's close button closes it WITHOUT firing onSelectTab
    const tabItems = container.querySelectorAll('[data-tab-id]');
    expect(tabItems).toHaveLength(2);
    expect(tabItems[0].scrollIntoView).toHaveBeenCalled();

    const secondCloseBtn = tabItems[1].querySelector('.titlebar-tab-close')!;
    act(() => {
      secondCloseBtn.dispatchEvent(new MiniEvent('click', { bubbles: true }));
    });
    expect(onCloseTab).toHaveBeenCalledWith('doc:2');
    expect(onSelectTab).not.toHaveBeenCalled();

    // Clicking a book tab trigger activates that tab; clicking Home activates 'home'
    const secondTrigger = tabItems[1].querySelector('.titlebar-tab-trigger')!;
    act(() => {
      secondTrigger.dispatchEvent(new MiniEvent('click', { bubbles: true }));
      container.querySelector('#tab-home')!.dispatchEvent(new MiniEvent('click', { button: 0, bubbles: true }));
    });
    expect(onSelectTab).toHaveBeenNthCalledWith(1, 'doc:2');
    expect(onSelectTab).toHaveBeenNthCalledWith(2, 'home');

    // Middle-clicking a book tab closes it; middle-clicking Home never closes Home
    onCloseTab.mockClear();
    act(() => {
      tabItems[0].dispatchEvent(new MiniEvent('auxclick', { button: 1, bubbles: true }));
      container.querySelector('#tab-home')!.dispatchEvent(new MiniEvent('auxclick', { button: 1, bubbles: true }));
    });
    expect(onCloseTab).toHaveBeenCalledOnce();
    expect(onCloseTab).toHaveBeenCalledWith('doc:1');

    // Mouse wheel over overflowing tab strip scrolls horizontally
    const strip = container.querySelector('.titlebar-tab-strip')!;
    act(() => {
      strip.dispatchEvent(new MiniEvent('wheel', { deltaY: 64, deltaX: 0, bubbles: true }));
    });
    expect(strip.scrollLeft).toBe(64);

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
