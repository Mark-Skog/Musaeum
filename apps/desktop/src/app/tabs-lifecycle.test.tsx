import { act, createContext, forwardRef, type ReactNode, useContext } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { ReaderStatus } from '../compositor/messages';
import type { FixedReaderConfig } from '../fixed/fixed-reader';
import type { SavedReadingState, StartupOptions } from '../protocol';
import { createAbortError, withReaderSessionLock } from '../reader-session-lock';
import type { FlowMode } from '../reader/messages';
import { DEFAULT_STYLE, type ReadingStyle } from '../reader/typography';
import type { BookRow, FlowInfo } from './api';
import { setLocale } from './i18n';
import type { FlowReaderEvents, FlowReaderRestore } from './reader/flow-reader';
import { ReaderView } from './reader/ReaderView';
import {
  activateTab,
  activeBookTab,
  bookTabId,
  createInitialTabsState,
  HOME_TAB_ID,
  openBookTab,
  type ReaderTabSnapshot,
  setTabSnapshot,
  type TabsState,
} from './tabs-model';

const harness = vi.hoisted(() => {
  return {
    activeFixedSessions: 0,
    peakFixedSessions: 0,
    activeWebglContexts: 0,
    peakWebglContexts: 0,
    activeFlowOrigins: new Set<string>(),
    peakFlowOrigins: 0,
    fixedOpenCalls: [] as {
      document: string;
      documentId?: number | null;
      initialState?: SavedReadingState | null;
      emitStatus: (status: ReaderStatus) => void;
    }[],
    flowOpenCalls: [] as {
      documentId: number;
      origin: string;
      restore?: FlowReaderRestore;
      emitRelocate: (cfi: string, section: number, progress: number) => void;
    }[],
    closeBookCalls: [] as (string | undefined)[],
    fixedDelayMs: 0,
    flowDelayMs: 0,
    failingDocuments: new Map<number, string>(),
    reset() {
      this.activeFixedSessions = 0;
      this.peakFixedSessions = 0;
      this.activeWebglContexts = 0;
      this.peakWebglContexts = 0;
      this.activeFlowOrigins.clear();
      this.peakFlowOrigins = 0;
      this.fixedOpenCalls = [];
      this.flowOpenCalls = [];
      this.closeBookCalls = [];
      this.fixedDelayMs = 0;
      this.flowDelayMs = 0;
      this.failingDocuments.clear();
    },
  };
});

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'document_outline') {
      return [{ label: 'Capítulo 1', page: 0, level: 0 }];
    }
    return null;
  }),
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => () => {}),
}));

vi.mock('./preferences/store', () => ({
  isMotionReduced: () => true,
  onMotionChange: () => () => {},
  currentPreferences: () => null,
  updatePreferences: async () => null,
}));

vi.mock('./api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./api')>();
  return {
    ...actual,
    api: {
      ...actual.api,
      setting: vi.fn(async () => DEFAULT_STYLE),
      setSetting: vi.fn(async () => {}),
      fixedAnnotations: vi.fn(async () => []),
      flowAnnotations: vi.fn(async () => []),
      searchAnnotations: vi.fn(async () => []),
      closeBook: vi.fn(async (origin?: string) => {
        harness.closeBookCalls.push(origin);
        if (origin !== undefined) {
          harness.activeFlowOrigins.delete(origin);
        } else {
          harness.activeFlowOrigins.clear();
        }
      }),
    },
  };
});

vi.mock('../fixed/fixed-reader', () => {
  let nextId = 1;
  class MockFixedReader {
    session: { id: number; pageCount: number; readingState: SavedReadingState | null } | null;
    private lastStatus: ReaderStatus | null = null;
    private closed = false;
    readonly toolsSet: string[] = [];
    readonly snapsSet: boolean[] = [];

    constructor(
      private readonly config: FixedReaderConfig,
      id: number,
    ) {
      this.session = {
        id,
        pageCount: 402,
        readingState: config.initialState ?? null,
      };
    }

    static async open(config: FixedReaderConfig): Promise<MockFixedReader> {
      if (config.signal?.aborted) {
        throw createAbortError();
      }
      return withReaderSessionLock(async () => {
        if (config.signal?.aborted) {
          throw createAbortError();
        }
        const failMsg =
          config.documentId != null ? harness.failingDocuments.get(config.documentId) : undefined;
        if (harness.fixedDelayMs > 0) {
          await new Promise<void>((resolve) => {
            const timer = setTimeout(resolve, harness.fixedDelayMs);
            config.signal?.addEventListener(
              'abort',
              () => {
                clearTimeout(timer);
                resolve();
              },
              { once: true },
            );
          });
        }
        if (config.signal?.aborted) {
          throw createAbortError();
        }
        if (failMsg !== undefined) {
          throw new Error(failMsg);
        }
        const id = nextId++;
        harness.activeFixedSessions++;
        harness.peakFixedSessions = Math.max(harness.peakFixedSessions, harness.activeFixedSessions);
        harness.activeWebglContexts++;
        harness.peakWebglContexts = Math.max(harness.peakWebglContexts, harness.activeWebglContexts);

        const reader = new MockFixedReader(config, id);
        const seed = config.initialState ?? {
          page: 0,
          u: 0,
          v: 0,
          progress: 0,
          mode: 'paginated' as const,
          pageTurnAnimation: true,
          zoom: 1,
          spreads: 'auto' as const,
        };
        const initialStatus: ReaderStatus = {
          type: 'status',
          mode: seed.mode,
          animating: seed.mode === 'paginated' && seed.pageTurnAnimation,
          animationPreference: seed.pageTurnAnimation,
          spreads: seed.spreads,
          zoom: seed.zoom,
          page: seed.page,
          pageCount: 402,
          u: seed.u,
          v: seed.v,
          progress: seed.progress,
        };
        reader.lastStatus = initialStatus;
        harness.fixedOpenCalls.push({
          document: config.document,
          documentId: config.documentId,
          initialState: config.initialState ?? null,
          emitStatus: (nextStatus) => {
            reader.lastStatus = nextStatus;
            config.onStatus?.(nextStatus);
          },
        });
        queueMicrotask(() => {
          if (!reader.closed && !config.signal?.aborted) {
            config.onStatus?.(initialStatus);
          }
        });
        return reader;
      });
    }

    ready(): Promise<void> {
      return Promise.resolve();
    }

    status(): ReaderStatus | null {
      return this.lastStatus;
    }

    saved(): SavedReadingState[] {
      return [];
    }

    currentReadingState(): SavedReadingState | null {
      if (this.lastStatus !== null) {
        return {
          page: this.lastStatus.page,
          u: this.lastStatus.u,
          v: this.lastStatus.v,
          progress: this.lastStatus.progress,
          mode: this.lastStatus.mode,
          pageTurnAnimation: this.lastStatus.animationPreference,
          zoom: this.lastStatus.zoom,
          spreads: this.lastStatus.spreads,
        };
      }
      return this.config.initialState ?? null;
    }

    send(): void {}
    setReducedMotion(): void {}
    setTool(tool: string): void {
      this.toolsSet.push(tool);
    }
    setSnap(on: boolean): void {
      this.snapsSet.push(on);
    }
    setMarks(): void {}
    clearSelection(): void {}

    async close(): Promise<void> {
      if (this.closed) {
        return;
      }
      this.closed = true;
      this.session = null;
      harness.activeFixedSessions = Math.max(0, harness.activeFixedSessions - 1);
      harness.activeWebglContexts = Math.max(0, harness.activeWebglContexts - 1);
    }
  }

  return { FixedReader: MockFixedReader };
});

vi.mock('./reader/flow-reader', () => {
  let nextOrigin = 1;
  class MockFlowReader {
    readonly info: FlowInfo;
    private mode: FlowMode;
    private animated: boolean;
    private style: ReadingStyle;
    private reloc: {
      cfi: string;
      section: number;
      progress: number;
      atStart: boolean;
      atEnd: boolean;
      reason: string;
    } | null = null;
    private readonly frame: HTMLElement;
    private closed = false;
    readonly layouts: { ms: number; change: string }[] = [];

    constructor(
      container: HTMLElement,
      info: FlowInfo,
      mode: FlowMode,
      animated: boolean,
      style: ReadingStyle,
    ) {
      this.info = info;
      this.mode = mode;
      this.animated = animated;
      this.style = style;
      this.frame = container.ownerDocument.createElement('iframe');
      this.frame.className = 'reader-frame';
      container.appendChild(this.frame);
    }

    static async open(
      container: HTMLElement,
      documentId: number,
      defaults: { mode: FlowMode; animated: boolean; style: ReadingStyle },
      events: FlowReaderEvents,
      options?: { restore?: FlowReaderRestore; signal?: AbortSignal },
    ): Promise<MockFlowReader> {
      const signal = options?.signal;
      if (signal?.aborted) {
        throw createAbortError();
      }
      return withReaderSessionLock(async () => {
        if (signal?.aborted) {
          throw createAbortError();
        }
        const failMsg = harness.failingDocuments.get(documentId);
        if (harness.flowDelayMs > 0) {
          await new Promise<void>((resolve) => {
            const timer = setTimeout(resolve, harness.flowDelayMs);
            signal?.addEventListener(
              'abort',
              () => {
                clearTimeout(timer);
                resolve();
              },
              { once: true },
            );
          });
        }
        if (signal?.aborted) {
          throw createAbortError();
        }
        if (failMsg !== undefined) {
          throw new Error(failMsg);
        }
        const origin = `http://book.mock${nextOrigin++}.localhost`;
        harness.activeFlowOrigins.add(origin);
        harness.peakFlowOrigins = Math.max(harness.peakFlowOrigins, harness.activeFlowOrigins.size);

        const restore = options?.restore;
        const mode = restore?.mode ?? defaults.mode;
        const animated = restore?.animated ?? defaults.animated;
        const style = restore?.style ?? defaults.style;
        const info: FlowInfo = {
          origin,
          readerUrl: `${origin}/reader.html`,
          documentId,
          title: `Book ${documentId}`,
          creators: [],
          language: 'pt-BR',
          toc: [{ label: 'Parte I', href: 'part1.xhtml', cfi: null }],
          sections: [],
          totalText: 10000,
          saved: null,
          cached: true,
          openMs: 1,
        };
        const reader = new MockFlowReader(container, info, mode, animated, style);
        const initialCfi = restore?.readerCfi ?? 'epubcfi(/6/2!/4/2/1:0)';
        const initialProgress = restore?.progress ?? 0.01;
        reader.reloc = {
          cfi: initialCfi,
          section: 1,
          progress: initialProgress,
          atStart: false,
          atEnd: false,
          reason: 'open',
        };
        harness.flowOpenCalls.push({
          documentId,
          origin,
          restore,
          emitRelocate: (cfi, section, progress) => {
            reader.reloc = { cfi, section, progress, atStart: false, atEnd: false, reason: 'page' };
            events.relocate(reader.reloc);
          },
        });
        queueMicrotask(() => {
          if (!reader.closed && !signal?.aborted && reader.reloc !== null) {
            events.relocate(reader.reloc);
          }
        });
        return reader;
      });
    }

    get currentMode(): FlowMode {
      return this.mode;
    }
    get isAnimated(): boolean {
      return this.animated;
    }
    get currentStyle(): ReadingStyle {
      return this.style;
    }
    get relocation() {
      return this.reloc;
    }
    get savedPosition() {
      return null;
    }

    setMode(mode: FlowMode): void {
      this.mode = mode;
    }
    setAnimated(animated: boolean): void {
      this.animated = animated;
    }
    setReducedMotion(): void {}
    setLanguage(): void {}
    setStyle(style: ReadingStyle): void {
      this.style = style;
    }
    setMarks(): void {}
    clearSelection(): void {}
    goTo(): void {}
    turn(): void {}
    key(): void {}
    focus(): void {}

    async close(): Promise<void> {
      if (this.closed) {
        return;
      }
      this.closed = true;
      this.frame.remove();
      harness.closeBookCalls.push(this.info.origin);
      harness.activeFlowOrigins.delete(this.info.origin);
    }
  }

  return { FlowReader: MockFlowReader };
});

vi.mock('react-aria-components', () => {
  const ToggleCtx = createContext<{
    selectedKeys?: Iterable<string>;
    onChange?: (keys: Set<string>) => void;
  }>({});
  return {
    Button: forwardRef<
      HTMLButtonElement,
      {
        id?: string;
        className?: string;
        'aria-label'?: string;
        'aria-expanded'?: boolean;
        isDisabled?: boolean;
        children?: ReactNode;
        onPress?: () => void;
      }
    >(function MockBtn(props, ref) {
      return (
        <button
          ref={ref}
          type="button"
          id={props.id}
          className={props.className}
          aria-label={props['aria-label']}
          aria-expanded={props['aria-expanded']}
          disabled={props.isDisabled}
          onClick={() => {
            if (!props.isDisabled) {
              props.onPress?.();
            }
          }}
        >
          {props.children}
        </button>
      );
    }),
    Dialog: ({ className, children }: { className?: string; children?: ReactNode }) => (
      <div className={className}>{children}</div>
    ),
    DialogTrigger: ({ children }: { children?: ReactNode }) => <>{children}</>,
    Heading: ({ className, children }: { className?: string; children?: ReactNode }) => (
      <h2 className={className}>{children}</h2>
    ),
    Input: forwardRef<
      HTMLInputElement,
      { id?: string; placeholder?: string; value?: string; onChange?: (e: { target: { value: string } }) => void }
    >(function MockInput(props, ref) {
      return <input ref={ref} id={props.id} placeholder={props.placeholder} value={props.value} onChange={props.onChange as never} />;
    }),
    Label: ({ children }: { children?: ReactNode }) => <label>{children}</label>,
    Modal: ({ className, children }: { className?: string; children?: ReactNode }) => (
      <div className={className}>{children}</div>
    ),
    ModalOverlay: ({ isOpen, children }: { isOpen?: boolean; children?: ReactNode }) =>
      isOpen ? <div className="modal-overlay">{children}</div> : null,
    Popover: ({ className, children }: { className?: string; children?: ReactNode }) => (
      <div className={className}>{children}</div>
    ),
    Radio: ({ value, children }: { value: string; children?: ReactNode }) => <div data-value={value}>{children}</div>,
    RadioGroup: ({ children }: { children?: ReactNode }) => <div>{children}</div>,
    SearchField: ({
      value,
      onChange,
      children,
      className,
    }: {
      value?: string;
      onChange?: (val: string) => void;
      children?: ReactNode;
      className?: string;
    }) => (
      <div className={className} data-search-value={value ?? ''} onClick={() => onChange?.('dialética')}>
        {children}
      </div>
    ),
    Switch: ({
      id,
      isSelected,
      onChange,
      children,
    }: {
      id?: string;
      isSelected?: boolean;
      onChange?: (val: boolean) => void;
      children?: ReactNode;
    }) => (
      <button type="button" id={id} aria-pressed={isSelected} onClick={() => onChange?.(!isSelected)}>
        {children}
      </button>
    ),
    TextArea: () => <textarea />,
    TextField: ({ children }: { children?: ReactNode }) => <div>{children}</div>,
    ToggleButtonGroup: ({
      selectedKeys,
      onSelectionChange,
      children,
      className,
    }: {
      selectedKeys?: Iterable<string>;
      onSelectionChange?: (keys: Set<string>) => void;
      children?: ReactNode;
      className?: string;
    }) => (
      <ToggleCtx.Provider value={{ selectedKeys, onChange: onSelectionChange }}>
        <div className={className}>{children}</div>
      </ToggleCtx.Provider>
    ),
    ToggleButton: ({ id, className, children }: { id?: string; className?: string; children?: ReactNode }) => {
      const ctx = useContext(ToggleCtx);
      return (
        <button
          type="button"
          id={id}
          className={className}
          onClick={() => {
            if (id !== undefined) {
              ctx.onChange?.(new Set([id]));
            }
          }}
        >
          {children}
        </button>
      );
    },
  };
});

function makeOptions(): StartupOptions {
  return {
    document: null,
    e2e: false,
    epub: null,
    bookProtection: 'full',
    delivery: 'auto',
    deliveryFaults: [],
    workers: 2,
    pullBatch: 8,
    gpuLayers: null,
    tileBudgetMb: null,
    noSandbox: false,
    width: 1280,
    height: 800,
    bench: null,
  };
}

function makeBook(id: number, title: string, format: 'pdf' | 'epub'): BookRow {
  return {
    documentId: id,
    workId: id * 10,
    title,
    creators: 'Autor',
    layout: format === 'epub' ? 'flow' : 'fixed',
    format,
    instanceId: id,
    revisionId: id,
    folderId: 1,
    path: `books/${title.toLowerCase()}.${format}`,
    state: 'present',
    blake3: null,
    size: 2048,
    progress: null,
    lastReadAt: null,
  };
}

class MiniEvent {
  defaultPrevented = false;
  cancelBubble = false;
  target: unknown = null;
  currentTarget: unknown = null;
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
  textValue = '';
  listeners = new Map<string, Set<(ev: MiniEvent) => void>>();

  appendChild<T extends MiniNode>(child: T): T {
    if (child.parentNode !== null) {
      child.parentNode.removeChild(child);
    }
    child.parentNode = this;
    child.ownerDocument = this.ownerDocument;
    this.childNodes.push(child);
    return child;
  }
  append(...nodes: MiniNode[]): void {
    for (const node of nodes) {
      this.appendChild(node);
    }
  }
  removeChild<T extends MiniNode>(child: T): T {
    this.childNodes = this.childNodes.filter((n) => n !== child);
    child.parentNode = null;
    return child;
  }
  insertBefore<T extends MiniNode>(child: T, refNode: MiniNode | null): T {
    if (child.parentNode !== null) {
      child.parentNode.removeChild(child);
    }
    child.parentNode = this;
    child.ownerDocument = this.ownerDocument;
    const idx = refNode !== null ? this.childNodes.indexOf(refNode) : -1;
    if (idx >= 0) {
      this.childNodes.splice(idx, 0, child);
    } else {
      this.childNodes.push(child);
    }
    return child;
  }
  remove(): void {
    this.parentNode?.removeChild(this);
  }
  get textContent(): string {
    if (this.nodeType === 3) {
      return this.textValue;
    }
    return this.childNodes.map((c) => c.textContent).join('');
  }
  set textContent(val: string) {
    this.childNodes = [];
    const str = String(val ?? '');
    if (this.nodeType === 3) {
      this.textValue = str;
    } else {
      this.textValue = '';
      if (str !== '') {
        const tn = new MiniNode();
        tn.nodeType = 3;
        tn.textValue = str;
        tn.parentNode = this;
        tn.ownerDocument = this.ownerDocument;
        this.childNodes.push(tn);
      }
    }
  }
  contains(other: MiniNode | null): boolean {
    let cur: MiniNode | null = other;
    while (cur !== null) {
      if (cur === this) {
        return true;
      }
      cur = cur.parentNode;
    }
    return false;
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
  style: Record<string, unknown> & {
    setProperty: (k: string, v: string) => void;
    removeProperty: (k: string) => void;
    getPropertyValue: (k: string) => string;
  } = {
    setProperty(k: string, v: string) {
      (this as Record<string, unknown>)[k] = v;
    },
    removeProperty(k: string) {
      delete (this as Record<string, unknown>)[k];
    },
    getPropertyValue(k: string) {
      return String((this as Record<string, unknown>)[k] ?? '');
    },
  };
  attrs = new Map<string, string>();
  disabled = false;
  width = 800;
  height = 600;
  clientWidth = 800;
  clientHeight = 600;
  scrollTop = 0;
  scrollHeight = 600;
  focus = vi.fn();
  readonly classList = {
    add: (...tokens: string[]) => {
      const set = new Set(this.className.split(/\s+/).filter(Boolean));
      for (const token of tokens) {
        if (token) {
          set.add(token);
        }
      }
      this.className = [...set].join(' ');
    },
    remove: (...tokens: string[]) => {
      const set = new Set(this.className.split(/\s+/).filter(Boolean));
      for (const token of tokens) {
        set.delete(token);
      }
      this.className = [...set].join(' ');
    },
    toggle: (token: string, force?: boolean) => {
      const set = new Set(this.className.split(/\s+/).filter(Boolean));
      const next = force !== undefined ? force : !set.has(token);
      if (next) {
        set.add(token);
      } else {
        set.delete(token);
      }
      this.className = [...set].join(' ');
      return next;
    },
    contains: (token: string) => this.className.split(/\s+/).filter(Boolean).includes(token),
  };

  constructor(tag = 'DIV') {
    super();
    this.tagName = tag.toUpperCase();
    this.nodeName = this.tagName;
  }
  get firstElementChild(): MiniEl | null {
    for (const ch of this.childNodes) {
      if (ch instanceof MiniEl) {
        return ch;
      }
    }
    return null;
  }
  get id(): string {
    return this.attrs.get('id') ?? '';
  }
  set id(val: string) {
    this.attrs.set('id', val);
  }
  get className(): string {
    return this.attrs.get('class') ?? '';
  }
  set className(val: string) {
    this.attrs.set('class', val);
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
  getBoundingClientRect() {
    return { left: 0, top: 0, width: 800, height: 600, right: 800, bottom: 600 };
  }
  querySelectorAll(selector: string): MiniEl[] {
    const out: MiniEl[] = [];
    const matchesSelector = (el: MiniEl, sel: string): boolean => {
      if (sel.startsWith('#')) {
        return el.id === sel.slice(1);
      }
      if (sel.startsWith('.')) {
        return el.className.split(/\s+/).includes(sel.slice(1));
      }
      if (sel === '[role="alert"]') {
        return el.getAttribute('role') === 'alert';
      }
      return el.tagName.toLowerCase() === sel.toLowerCase();
    };
    const walk = (node: MiniNode): void => {
      for (const ch of node.childNodes) {
        if (ch instanceof MiniEl) {
          if (matchesSelector(ch, selector)) {
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

function setupMiniDom(): { doc: { createElement: (tag?: string) => MiniEl; getElementById: (id: string) => MiniEl | null }; rootHost: MiniEl } {
  const rootHost = new MiniEl('div');
  const doc = {
    title: '',
    documentElement: { lang: 'pt-BR' },
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
    createTextNode: (text = '') => {
      const n = new MiniNode();
      n.nodeType = 3;
      n.textValue = String(text);
      n.ownerDocument = doc;
      return n;
    },
    getElementById: (id: string) => rootHost.querySelector(`#${id}`),
    querySelector: (sel: string) => rootHost.querySelector(sel),
    addEventListener: () => {},
    removeEventListener: () => {},
  };
  rootHost.ownerDocument = doc;

  class FakeResizeObserver {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  }
  class FakeMutationObserver {
    observe(): void {}
    disconnect(): void {}
  }
  class MiniIFrame extends MiniEl {}

  const win = {
    document: doc,
    HTMLIFrameElement: MiniIFrame,
    HTMLElement: MiniEl,
    SVGElement: MiniEl,
    Element: MiniEl,
    Node: MiniNode,
    ResizeObserver: FakeResizeObserver,
    MutationObserver: FakeMutationObserver,
    setTimeout: globalThis.setTimeout.bind(globalThis),
    clearTimeout: globalThis.clearTimeout.bind(globalThis),
    requestAnimationFrame: (cb: FrameRequestCallback) => setTimeout(() => cb(performance.now()), 0) as unknown as number,
    cancelAnimationFrame: (id: number) => clearTimeout(id),
    addEventListener: () => {},
    removeEventListener: () => {},
  };
  doc.defaultView = win;

  (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
  vi.stubGlobal('document', doc);
  vi.stubGlobal('window', win);
  vi.stubGlobal('HTMLIFrameElement', MiniIFrame);
  vi.stubGlobal('HTMLElement', MiniEl);
  vi.stubGlobal('SVGElement', MiniEl);
  vi.stubGlobal('Element', MiniEl);
  vi.stubGlobal('Node', MiniNode);
  vi.stubGlobal('ResizeObserver', FakeResizeObserver);
  vi.stubGlobal('MutationObserver', FakeMutationObserver);

  return { doc, rootHost };
}

async function flushAsync(ticks = 6): Promise<void> {
  for (let i = 0; i < ticks; i++) {
    await Promise.resolve();
  }
}

describe('Etapa 2B tab suspension, rich ReaderTabSnapshot rehydration, stress switching, and isolated errors', () => {
  beforeEach(() => {
    harness.reset();
    setLocale('pt-BR');
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('captures and restores rich Fixed & Flow snapshots (page, offsets, zoom, panels, search) without default overwrite during async mount', async () => {
    const { rootHost } = setupMiniDom();
    const root = createRoot(rootHost as unknown as HTMLElement);
    const options = makeOptions();
    const pdfBook = makeBook(1, 'Solaris', 'pdf');
    const epubBook = makeBook(2, 'Duna', 'epub');

    let tabsState: TabsState = createInitialTabsState();
    tabsState = openBookTab(tabsState, pdfBook);
    tabsState = openBookTab(tabsState, epubBook);
    tabsState = activateTab(tabsState, bookTabId(1));

    const snapshotGetterRef = { current: null as (() => ReaderTabSnapshot | undefined) | null };

    const renderCurrentTab = () => {
      const current = activeBookTab(tabsState);
      if (current === null || current.book === undefined) {
        return <div id="home-screen">Home</div>;
      }
      return (
        <ReaderView
          key={current.id}
          book={current.book}
          options={options}
          snapshot={current.snapshot}
          snapshotGetterRef={snapshotGetterRef}
          onSnapshotCapture={(snap) => {
            tabsState = setTabSnapshot(tabsState, current.id, snap);
          }}
          onBack={() => {
            const snap = snapshotGetterRef.current?.();
            if (snap !== undefined) {
              tabsState = setTabSnapshot(tabsState, current.id, snap);
            }
            tabsState = activateTab(tabsState, HOME_TAB_ID);
          }}
          onCloseTab={() => {}}
        />
      );
    };

    // 1. Open Tab 1 (PDF)
    await act(async () => {
      root.render(renderCurrentTab());
      await flushAsync();
    });
    expect(harness.activeFixedSessions).toBe(1);
    expect(harness.activeWebglContexts).toBe(1);

    // Simulate user navigating to page 37 (shown as 38), offset (0.22, 0.74), zoom 1.5x, continuous, double spreads
    act(() => {
      harness.fixedOpenCalls[0].emitStatus({
        type: 'status',
        mode: 'continuous',
        animating: false,
        animationPreference: false,
        spreads: 'double',
        zoom: 1.5,
        page: 37,
        pageCount: 402,
        u: 0.22,
        v: 0.74,
        progress: 0.094,
      });
    });

    // Open TOC and Annotations panels + enter search query 'dialética' + switch tool to 'lines'
    act(() => {
      rootHost.querySelector('#toc-toggle')!.dispatchEvent(new MiniEvent('click', { bubbles: true }));
      rootHost.querySelector('#annotations-toggle')!.dispatchEvent(new MiniEvent('click', { bubbles: true }));
      rootHost.querySelector('#lines')!.dispatchEvent(new MiniEvent('click', { bubbles: true }));
    });
    act(() => {
      rootHost.querySelector('.panel-search')!.dispatchEvent(new MiniEvent('click', { bubbles: true }));
    });

    // 2. Suspend Tab 1 -> Home (`🏠`): verify 0 active WebGL2 contexts & 0 backend sessions
    await act(async () => {
      const snap = snapshotGetterRef.current?.();
      if (snap !== undefined) {
        tabsState = setTabSnapshot(tabsState, bookTabId(1), snap);
      }
      tabsState = activateTab(tabsState, HOME_TAB_ID);
      root.render(renderCurrentTab());
      await flushAsync();
    });

    expect(harness.activeFixedSessions).toBe(0);
    expect(harness.activeWebglContexts).toBe(0);

    const savedPdfSnap = tabsState.bookTabs[0].snapshot!;
    expect(savedPdfSnap).toMatchObject({
      layout: 'fixed',
      mode: 'continuous',
      pageTurnAnimation: false,
      page: 37,
      pageCount: 402,
      u: 0.22,
      v: 0.74,
      scrollOffset: 0.74,
      zoom: 1.5,
      fitMode: 'manual',
      spreads: 'double',
      tool: 'lines',
      tocOpen: true,
      annotationsOpen: true,
      annotationSearch: 'dialética',
    });

    // 3. Re-activate Tab 1 with a slow async load (60ms), and switch away AFTER 5ms BEFORE open resolves:
    //    Snapshot MUST NOT be overwritten with defaults (`page = 0`, `zoom = 1`)!
    harness.fixedDelayMs = 60;
    await act(async () => {
      tabsState = activateTab(tabsState, bookTabId(1));
      root.render(renderCurrentTab());
      await flushAsync(2);
    });
    // Reading bar already reflects saved snapshot immediately during async open (Page 38 of 402, 150%)!
    expect(rootHost.querySelector('#reading-position')?.textContent).toBe('Página 38 de 402');
    expect(rootHost.querySelector('.zoom')?.textContent).toBe('150%');

    // Switch to Tab 2 (EPUB) before Tab 1's slow open finishes!
    harness.fixedDelayMs = 0;
    await act(async () => {
      const snapMidLoad = snapshotGetterRef.current?.();
      if (snapMidLoad !== undefined) {
        tabsState = setTabSnapshot(tabsState, bookTabId(1), snapMidLoad);
      }
      tabsState = activateTab(tabsState, bookTabId(2));
      root.render(renderCurrentTab());
      await new Promise((resolve) => setTimeout(resolve, 20));
      await flushAsync();
    });

    // Verify Tab 1's snapshot was NOT clobbered by defaults during the aborted mount!
    expect(tabsState.bookTabs[0].snapshot).toMatchObject({
      page: 37,
      u: 0.22,
      v: 0.74,
      zoom: 1.5,
      mode: 'continuous',
      spreads: 'double',
      tocOpen: true,
      annotationsOpen: true,
      annotationSearch: 'dialética',
    });

    // Tab 2 (EPUB) is now active -> 0 fixed sessions, 1 flow origin
    expect(harness.activeFixedSessions).toBe(0);
    expect(harness.activeWebglContexts).toBe(0);
    expect(harness.activeFlowOrigins.size).toBe(1);
    const firstEpubOrigin = harness.flowOpenCalls[0].origin;

    // Move EPUB to chapter 4 CFI & switch to continuous mode
    act(() => {
      harness.flowOpenCalls[0].emitRelocate('epubcfi(/6/10!/4/6/1:19)', 4, 0.42);
      rootHost.querySelector('#continuous')!.dispatchEvent(new MiniEvent('click', { bubbles: true }));
    });

    // 4. Switch back from Tab 2 (EPUB) -> Tab 1 (PDF):
    //    EPUB iframe unmounts + close_book called with exact origin; PDF opens with exact `initialState`!
    await act(async () => {
      const epubSnap = snapshotGetterRef.current?.();
      if (epubSnap !== undefined) {
        tabsState = setTabSnapshot(tabsState, bookTabId(2), epubSnap);
      }
      tabsState = activateTab(tabsState, bookTabId(1));
      root.render(renderCurrentTab());
      await flushAsync();
    });

    expect(harness.closeBookCalls).toContain(firstEpubOrigin);
    expect(harness.activeFlowOrigins.size).toBe(0);
    expect(rootHost.querySelector('.reader-frame')).toBeNull();
    expect(harness.activeFixedSessions).toBe(1);
    expect(harness.activeWebglContexts).toBe(1);

    const rehydratedFixedOpen = harness.fixedOpenCalls[harness.fixedOpenCalls.length - 1];
    expect(rehydratedFixedOpen.initialState).toEqual({
      page: 37,
      u: 0.22,
      v: 0.74,
      progress: 0.094,
      mode: 'continuous',
      pageTurnAnimation: false,
      zoom: 1.5,
      spreads: 'double',
    });
    expect(rootHost.querySelector('.toc-panel')).not.toBeNull();
    expect(rootHost.querySelector('.panel-search')?.getAttribute('data-search-value')).toBe('dialética');

    // Verify EPUB snapshot captured exact CFI, section, progress and mode, and restores them
    expect(tabsState.bookTabs[1].snapshot).toMatchObject({
      layout: 'flow',
      mode: 'continuous',
      readerCfi: 'epubcfi(/6/10!/4/6/1:19)',
      section: 4,
      progress: 0.42,
    });

    await act(async () => {
      tabsState = activateTab(tabsState, bookTabId(2));
      root.render(renderCurrentTab());
      await flushAsync();
    });
    const rehydratedFlowOpen = harness.flowOpenCalls[harness.flowOpenCalls.length - 1];
    expect(rehydratedFlowOpen.restore).toMatchObject({
      readerCfi: 'epubcfi(/6/10!/4/6/1:19)',
      progress: 0.42,
      mode: 'continuous',
    });

    await act(async () => {
      root.unmount();
      await flushAsync();
    });
    expect(harness.activeFixedSessions).toBe(0);
    expect(harness.activeFlowOrigins.size).toBe(0);
  });

  it('maintains hard ceiling <= 1 session across rapid mixed stress switching (Tab A -> Tab B -> Tab C -> Home -> Tab A)', async () => {
    const { rootHost } = setupMiniDom();
    const root = createRoot(rootHost as unknown as HTMLElement);
    const options = makeOptions();
    const bookA = makeBook(10, 'Alpha', 'pdf');
    const bookB = makeBook(20, 'Beta', 'epub');
    const bookC = makeBook(30, 'Gamma', 'pdf');

    harness.fixedDelayMs = 30;
    harness.flowDelayMs = 30;

    // Rapid sequence without waiting for intermediate opens to complete: A -> B -> C -> Home
    await act(async () => {
      root.render(<ReaderView key="doc:10" book={bookA} options={options} onBack={() => {}} />);
      await flushAsync(2);
      root.render(<ReaderView key="doc:20" book={bookB} options={options} onBack={() => {}} />);
      await flushAsync(2);
      root.render(<ReaderView key="doc:30" book={bookC} options={options} onBack={() => {}} />);
      await flushAsync(2);
      root.render(<div id="home">Home</div>);
      await new Promise((resolve) => setTimeout(resolve, 60));
      await flushAsync();
    });

    // At rest on Home: 0 fixed sessions, 0 WebGL2 contexts, 0 flow origins, peak <= 1
    expect(harness.activeFixedSessions).toBe(0);
    expect(harness.activeWebglContexts).toBe(0);
    expect(harness.activeFlowOrigins.size).toBe(0);
    expect(harness.peakFixedSessions).toBeLessThanOrEqual(1);
    expect(harness.peakWebglContexts).toBeLessThanOrEqual(1);
    expect(harness.peakFlowOrigins).toBeLessThanOrEqual(1);

    // Reopen Tab A after settling: opens cleanly with 0 false errors
    harness.fixedDelayMs = 0;
    await act(async () => {
      root.render(<ReaderView key="doc:10" book={bookA} options={options} onBack={() => {}} />);
      await flushAsync();
    });
    expect(harness.activeFixedSessions).toBe(1);
    expect(rootHost.querySelector('.reader-error')).toBeNull();

    await act(async () => {
      root.unmount();
      await flushAsync();
    });
    expect(harness.activeFixedSessions).toBe(0);
  });

  it('renders isolated localized error with Close Tab action in pt-BR and en when a suspended tab file is moved or deleted', async () => {
    const { rootHost } = setupMiniDom();
    const root = createRoot(rootHost as unknown as HTMLElement);
    const options = makeOptions();
    const missingBook = makeBook(77, 'Perdido', 'pdf');
    harness.failingDocuments.set(77, 'o arquivo deste livro não está mais na pasta');

    const onCloseTab = vi.fn();
    const onBack = vi.fn();

    setLocale('pt-BR');
    await act(async () => {
      root.render(
        <ReaderView
          key="doc:77"
          book={missingBook}
          options={options}
          onBack={onBack}
          onCloseTab={onCloseTab}
        />,
      );
      await flushAsync();
    });

    const alertPt = rootHost.querySelector('.reader-error');
    expect(alertPt).not.toBeNull();
    expect(alertPt!.textContent).toContain('Este arquivo não foi encontrado.');
    expect(alertPt!.textContent).toContain('Fechar aba');

    // Switch interface language to English -> localized error updates cleanly
    await act(async () => {
      setLocale('en');
      await flushAsync();
    });
    expect(alertPt!.textContent).toContain('This file could not be found.');
    expect(alertPt!.textContent).toContain('Close tab');

    // Clicking Close Tab invokes isolated tab close without affecting other tabs
    await act(async () => {
      rootHost.querySelector('#close-error-tab')!.dispatchEvent(new MiniEvent('click', { bubbles: true }));
      await flushAsync();
    });
    expect(onCloseTab).toHaveBeenCalledOnce();
    expect(harness.activeFixedSessions).toBe(0);

    await act(async () => {
      root.unmount();
    });
  });
});
