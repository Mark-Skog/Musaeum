import { act, createContext, useContext, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { BookRow, FolderEntry, LibraryInfo, Preferences, Taxonomy, WorkMetadata } from '../api';
import { setLocale } from '../i18n';

class MiniClassList {
  private readonly set = new Set<string>();
  constructor(private readonly onSync: () => void) {}

  contains(name: string): boolean {
    return this.set.has(name);
  }

  add(...names: string[]): void {
    for (const name of names) {
      if (name) {
        this.set.add(name);
      }
    }
    this.onSync();
  }

  remove(...names: string[]): void {
    for (const name of names) {
      this.set.delete(name);
    }
    this.onSync();
  }

  toggle(name: string, force?: boolean): boolean {
    if (force === true) {
      this.add(name);
      return true;
    }
    if (force === false) {
      this.remove(name);
      return false;
    }
    if (this.set.has(name)) {
      this.remove(name);
      return false;
    }
    this.add(name);
    return true;
  }

  reset(value: string): void {
    this.set.clear();
    for (const part of value.split(/\s+/)) {
      if (part) {
        this.set.add(part);
      }
    }
  }

  toString(): string {
    return [...this.set].join(' ');
  }
}

class MiniStyle {
  [key: string]: unknown;
  setProperty(name: string, value: string): void {
    this[name] = value;
  }
  removeProperty(name: string): void {
    delete this[name];
  }
  getPropertyValue(name: string): string {
    const value = this[name];
    return typeof value === 'string' ? value : '';
  }
}

class MiniEvent {
  readonly type: string;
  defaultPrevented = false;
  cancelBubble = false;
  target: unknown = null;
  currentTarget: unknown = null;

  constructor(type: string, init?: { bubbles?: boolean; cancelable?: boolean }) {
    this.type = type;
    void init;
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
  private listeners = new Map<string, Set<(event: unknown) => void>>();

  constructor(
    public readonly nodeType: number,
    public readonly nodeName: string,
    public ownerDocument: MiniDocument | null,
  ) {}

  get parentElement(): MiniElement | null {
    return this.parentNode instanceof MiniElement ? this.parentNode : null;
  }

  get children(): MiniElement[] {
    return this.childNodes.filter((node): node is MiniElement => node instanceof MiniElement);
  }

  get firstChild(): MiniNode | null {
    return this.childNodes[0] ?? null;
  }

  get lastChild(): MiniNode | null {
    return this.childNodes[this.childNodes.length - 1] ?? null;
  }

  get firstElementChild(): MiniElement | null {
    return this.children[0] ?? null;
  }

  get lastElementChild(): MiniElement | null {
    const list = this.children;
    return list[list.length - 1] ?? null;
  }

  get nextSibling(): MiniNode | null {
    if (this.parentNode === null) {
      return null;
    }
    const idx = this.parentNode.childNodes.indexOf(this);
    return idx >= 0 ? (this.parentNode.childNodes[idx + 1] ?? null) : null;
  }

  get previousSibling(): MiniNode | null {
    if (this.parentNode === null) {
      return null;
    }
    const idx = this.parentNode.childNodes.indexOf(this);
    return idx > 0 ? (this.parentNode.childNodes[idx - 1] ?? null) : null;
  }

  appendChild<T extends MiniNode>(child: T): T {
    return this.insertBefore(child, null);
  }

  insertBefore<T extends MiniNode>(child: T, ref: MiniNode | null): T {
    if (child instanceof MiniFragment) {
      const items = [...child.childNodes];
      for (const item of items) {
        this.insertBefore(item, ref);
      }
      return child;
    }
    if (child.parentNode !== null) {
      child.parentNode.removeChild(child);
    }
    child.parentNode = this;
    child.ownerDocument = this.ownerDocument;
    if (ref === null) {
      this.childNodes.push(child);
    } else {
      const idx = this.childNodes.indexOf(ref);
      if (idx === -1) {
        this.childNodes.push(child);
      } else {
        this.childNodes.splice(idx, 0, child);
      }
    }
    return child;
  }

  removeChild<T extends MiniNode>(child: T): T {
    const idx = this.childNodes.indexOf(child);
    if (idx !== -1) {
      this.childNodes.splice(idx, 1);
      child.parentNode = null;
    }
    return child;
  }

  replaceChild<T extends MiniNode>(nextChild: T, oldChild: MiniNode): MiniNode {
    this.insertBefore(nextChild, oldChild);
    return this.removeChild(oldChild);
  }

  contains(other: MiniNode | null): boolean {
    let cursor: MiniNode | null = other;
    while (cursor !== null) {
      if (cursor === this) {
        return true;
      }
      cursor = cursor.parentNode;
    }
    return false;
  }

  addEventListener(type: string, listener: (event: unknown) => void): void {
    let set = this.listeners.get(type);
    if (set === undefined) {
      set = new Set();
      this.listeners.set(type, set);
    }
    set.add(listener);
  }

  removeEventListener(type: string, listener: (event: unknown) => void): void {
    this.listeners.get(type)?.delete(listener);
  }

  dispatchEvent(event: unknown): boolean {
    const ev = event as MiniEvent & Record<string, unknown>;
    if (ev.target === null || ev.target === undefined) {
      ev.target = this;
    }
    let cursor: MiniNode | null = this;
    while (cursor !== null) {
      ev.currentTarget = cursor;
      const set = cursor.listeners.get(ev.type);
      if (set !== undefined) {
        for (const listener of [...set]) {
          listener.call(cursor, ev);
        }
      }
      if (ev.cancelBubble) {
        break;
      }
      cursor = cursor.parentNode;
    }
    return !ev.defaultPrevented;
  }

  get textContent(): string {
    return this.childNodes.map((child) => child.textContent).join('');
  }

  set textContent(value: string) {
    for (const child of [...this.childNodes]) {
      child.parentNode = null;
    }
    this.childNodes = [];
    if (value !== '') {
      this.appendChild(new MiniText(value, this.ownerDocument));
    }
  }
}

class MiniText extends MiniNode {
  constructor(
    public data: string,
    ownerDocument: MiniDocument | null,
  ) {
    super(3, '#text', ownerDocument);
  }

  get nodeValue(): string {
    return this.data;
  }

  set nodeValue(value: string) {
    this.data = value ?? '';
  }

  override get textContent(): string {
    return this.data;
  }

  override set textContent(value: string) {
    this.data = value ?? '';
  }
}

class MiniComment extends MiniNode {
  constructor(
    public data: string,
    ownerDocument: MiniDocument | null,
  ) {
    super(8, '#comment', ownerDocument);
  }
}

class MiniFragment extends MiniNode {
  constructor(ownerDocument: MiniDocument | null) {
    super(11, '#document-fragment', ownerDocument);
  }
}

function matchesSingleSelector(el: MiniElement, selector: string): boolean {
  const s = selector.trim();
  if (!s) {
    return false;
  }
  const attrMatch = s.match(/^([a-zA-Z0-9_-]*)?\[([^=\]]+)(?:=["']?([^"'\]]*)["']?)?\]$/);
  if (attrMatch !== null) {
    const [, tag, attrName, attrVal] = attrMatch;
    if (tag && el.tagName.toLowerCase() !== tag.toLowerCase()) {
      return false;
    }
    const actual = el.getAttribute(attrName);
    if (actual === null) {
      return false;
    }
    return attrVal === undefined ? true : actual === attrVal;
  }
  if (s.startsWith('#')) {
    return el.id === s.slice(1);
  }
  if (s.startsWith('.')) {
    return s
      .slice(1)
      .split('.')
      .every((cls) => el.classList.contains(cls));
  }
  return el.tagName.toLowerCase() === s.toLowerCase();
}

function matchesSelector(el: MiniElement, selector: string): boolean {
  return selector
    .split(',')
    .some((part) => matchesSingleSelector(el, part));
}

class MiniElement extends MiniNode {
  readonly tagName: string;
  readonly style = new MiniStyle();
  readonly attributes = new Map<string, string>();
  readonly dataset: Record<string, string> = {};
  scrollTop = 0;
  scrollHeight = 800;
  clientHeight = 200;
  tabIndex = 0;
  private _className = '';
  readonly classList: MiniClassList;

  constructor(tagName: string, ownerDocument: MiniDocument | null) {
    const upper = tagName.toUpperCase();
    super(1, upper, ownerDocument);
    this.tagName = upper;
    this.classList = new MiniClassList(() => {
      this._className = this.classList.toString();
      this.attributes.set('class', this._className);
    });
  }

  get className(): string {
    return this._className;
  }

  set className(value: string) {
    this._className = value ?? '';
    this.classList.reset(this._className);
    this.attributes.set('class', this._className);
  }

  get id(): string {
    return this.attributes.get('id') ?? '';
  }

  set id(value: string) {
    if (value) {
      this.attributes.set('id', value);
    } else {
      this.attributes.delete('id');
    }
  }

  get inert(): boolean {
    return this.hasAttribute('inert');
  }

  set inert(value: boolean) {
    if (value) {
      this.setAttribute('inert', '');
    } else {
      this.removeAttribute('inert');
    }
  }

  setAttribute(name: string, value: string): void {
    const str = String(value);
    this.attributes.set(name, str);
    if (name === 'class') {
      this._className = str;
      this.classList.reset(str);
    } else if (name.startsWith('data-')) {
      this.dataset[name.slice(5)] = str;
    }
  }

  getAttribute(name: string): string | null {
    return this.attributes.get(name) ?? null;
  }

  removeAttribute(name: string): void {
    this.attributes.delete(name);
    if (name === 'class') {
      this._className = '';
      this.classList.reset('');
    }
  }

  hasAttribute(name: string): boolean {
    return this.attributes.has(name);
  }

  focus(): void {
    if (this.ownerDocument !== null) {
      this.ownerDocument.activeElement = this;
    }
  }

  blur(): void {
    if (this.ownerDocument !== null && this.ownerDocument.activeElement === this) {
      this.ownerDocument.activeElement = null;
    }
  }

  closest(selector: string): MiniElement | null {
    let cursor: MiniNode | null = this;
    while (cursor instanceof MiniElement) {
      if (matchesSelector(cursor, selector)) {
        return cursor;
      }
      cursor = cursor.parentNode;
    }
    return null;
  }

  querySelector(selector: string): MiniElement | null {
    return this.querySelectorAll(selector)[0] ?? null;
  }

  querySelectorAll(selector: string): MiniElement[] {
    const out: MiniElement[] = [];
    const visit = (node: MiniNode): void => {
      for (const child of node.childNodes) {
        if (child instanceof MiniElement) {
          if (matchesSelector(child, selector)) {
            out.push(child);
          }
          visit(child);
        }
      }
    };
    visit(this);
    return out;
  }

  getBoundingClientRect(): { top: number; left: number; width: number; height: number; bottom: number; right: number } {
    return { top: 0, left: 0, width: 200, height: 200, bottom: 200, right: 200 };
  }
}

class MiniDocument extends MiniNode {
  body: MiniElement;
  documentElement: MiniElement;
  activeElement: MiniElement | null = null;
  defaultView: unknown = null;

  constructor() {
    super(9, '#document', null);
    this.ownerDocument = this;
    this.documentElement = new MiniElement('html', this);
    this.body = new MiniElement('body', this);
    this.documentElement.appendChild(this.body);
    this.appendChild(this.documentElement);
  }

  createElement(tag: string): MiniElement {
    return new MiniElement(tag, this);
  }

  createElementNS(_ns: string, tag: string): MiniElement {
    return new MiniElement(tag, this);
  }

  createTextNode(text: string): MiniText {
    return new MiniText(text, this);
  }

  createComment(text: string): MiniComment {
    return new MiniComment(text, this);
  }

  createDocumentFragment(): MiniFragment {
    return new MiniFragment(this);
  }

  getElementById(id: string): MiniElement | null {
    return this.documentElement.querySelector(`#${id}`);
  }

  querySelector(selector: string): MiniElement | null {
    return this.documentElement.querySelector(selector);
  }

  querySelectorAll(selector: string): MiniElement[] {
    return this.documentElement.querySelectorAll(selector);
  }
}

const mocks = vi.hoisted(() => {
  const sampleBooks: BookRow[] = [
    {
      documentId: 101,
      workId: 201,
      title: 'Dune',
      creators: 'Frank Herbert',
      layout: 'flow',
      format: 'epub',
      instanceId: 1,
      revisionId: 1,
      folderId: 1,
      path: 'C:\\Books\\Dune.epub',
      state: 'ok',
      blake3: null,
      size: 1024,
      progress: 0.4,
      lastReadAt: null,
    },
    {
      documentId: 102,
      workId: 202,
      title: 'Solaris',
      creators: 'Stanislaw Lem',
      layout: 'fixed',
      format: 'pdf',
      instanceId: 2,
      revisionId: 2,
      folderId: 1,
      path: 'C:\\Books\\Solaris.pdf',
      state: 'ok',
      blake3: null,
      size: 2048,
      progress: null,
      lastReadAt: null,
    },
  ];
  const sampleWork: WorkMetadata = {
    id: 201,
    title: 'Dune',
    creators: [{ name: 'Frank Herbert', role: 'author' }],
    series: null,
    seriesPosition: null,
    year: 1965,
    language: 'en',
    identifiers: [],
    rating: 5,
    status: 'reading',
    description: null,
    categories: [],
    tags: [],
    tagNames: [],
    collections: [],
    custom: [],
    provenance: {},
    documents: [101],
    formats: ['epub'],
  };
  const prefs: Preferences = {
    language: 'en',
    theme: 'light',
    motion: 'system',
    annotationAuthor: '',
    ocrLanguages: ['por', 'eng'],
    sidebarWidth: 240,
    sidebarCollapsed: false,
    showEmptySidebarSections: false,
    coverWidth: 168,
    showSidebarCounts: false,
  };
  const updatePreferences = vi.fn(async (patch: Partial<Preferences>) => {
    Object.assign(prefs, patch);
    return { preferences: { ...prefs }, systemTheme: 'light', systemAuthor: null };
  });
  return { sampleBooks, sampleWork, prefs, updatePreferences };
});

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => () => {}),
}));

vi.mock('../preferences/store', () => ({
  usePreferences: () => ({ preferences: mocks.prefs, systemTheme: 'light', systemAuthor: null }),
  updatePreferences: mocks.updatePreferences,
  isMotionReduced: () => false,
  systemReducesMotion: () => false,
}));

vi.mock('../api', async () => {
  const actual = await vi.importActual<typeof import('../api')>('../api');
  return {
    ...actual,
    coverUrl: (hash: string, version: number) => `cover://${hash}?v=${version}`,
    api: {
      browse: vi.fn(async () => ({ rows: mocks.sampleBooks, total: mocks.sampleBooks.length })),
      taxonomy: vi.fn(async (): Promise<Taxonomy> => ({
        categories: [],
        tags: [],
        collections: [],
        smart: [],
        fields: [],
      })),
      review: vi.fn(async () => ({ suggestions: [], duplicates: [] })),
      workMetadata: vi.fn(async () => mocks.sampleWork),
      selectAll: vi.fn(async () => []),
      addRoots: vi.fn(async () => null),
      removeRoot: vi.fn(async () => ({ open: true, location: null, roots: [], books: 2, indexing: false, error: null })),
      chooseRoot: vi.fn(async () => null),
      chooseExisting: vi.fn(async () => ({ open: true, location: null, roots: [], books: 2, indexing: false, error: null })),
    },
  };
});

vi.mock('react-aria-components', () => {
  const SelectionCtx = createContext<{ onSelectionChange?: (keys: Set<string | number>) => void }>({});

  const divWrapper = (props: { children?: ReactNode; className?: string; id?: string; 'aria-label'?: string }) => (
    <div id={props.id} className={props.className} aria-label={props['aria-label']}>
      {props.children}
    </div>
  );

  return {
    Size: class Size {
      constructor(
        public width: number,
        public height: number,
      ) {}
    },
    GridLayout: class GridLayout {},
    TableLayout: class TableLayout {},
    Virtualizer: (props: { children?: ReactNode }) => <>{props.children}</>,
    Button: (props: {
      id?: string;
      className?: string;
      children?: ReactNode;
      onPress?: () => void;
      onClick?: () => void;
      'aria-label'?: string;
      'aria-pressed'?: boolean;
    }) => (
      <button
        type="button"
        id={props.id}
        className={props.className}
        aria-label={props['aria-label']}
        aria-pressed={props['aria-pressed']}
        onClick={() => {
          props.onPress?.();
          props.onClick?.();
        }}
      >
        {props.children}
      </button>
    ),
    ToggleButton: (props: { id?: string; className?: string; children?: ReactNode; onPress?: () => void }) => (
      <button type="button" id={props.id} className={props.className} onClick={() => props.onPress?.()}>
        {props.children}
      </button>
    ),
    ToggleButtonGroup: divWrapper,
    SearchField: divWrapper,
    Input: (props: { className?: string; placeholder?: string; value?: string; onChange?: (e: unknown) => void }) => (
      <input
        className={props.className}
        placeholder={props.placeholder}
        value={props.value ?? ''}
        onChange={(props.onChange as never) ?? (() => {})}
      />
    ),
    TextField: divWrapper,
    TextArea: (props: { className?: string; value?: string }) => <textarea className={props.className} value={props.value ?? ''} readOnly />,
    Label: (props: { className?: string; children?: ReactNode }) => <label className={props.className}>{props.children}</label>,
    Collection: <T,>(props: { items: T[]; children: (item: T) => ReactNode }) => (
      <>{props.items.map((item, idx) => <div key={idx} role="presentation">{props.children(item)}</div>)}</>
    ),
    GridList: (props: {
      className?: string;
      children?: ReactNode | ((item: unknown) => ReactNode);
      items?: unknown[];
      onSelectionChange?: (keys: Set<string | number>) => void;
    }) => (
      <SelectionCtx.Provider value={{ onSelectionChange: props.onSelectionChange }}>
        <div className={props.className} role="grid">
          {typeof props.children === 'function' ? (props.items ?? []).map((item) => (props.children as (x: unknown) => ReactNode)(item)) : props.children}
        </div>
      </SelectionCtx.Provider>
    ),
    GridListItem: (props: { id: string | number; className?: string; children?: ReactNode; 'data-document'?: number }) => {
      const ctx = useContext(SelectionCtx);
      return (
        <div
          className={props.className}
          data-document={props['data-document'] === undefined ? undefined : String(props['data-document'])}
          onClick={() => ctx.onSelectionChange?.(new Set([props.id]))}
        >
          {props.children}
        </div>
      );
    },
    GridListLoadMoreItem: () => null,
    Table: divWrapper,
    TableHeader: divWrapper,
    Column: divWrapper,
    TableBody: divWrapper,
    Row: divWrapper,
    Cell: divWrapper,
    TableLoadMoreItem: () => null,
    Tree: divWrapper,
    TreeItem: divWrapper,
    TreeItemContent: (props: { children?: ReactNode | ((state: { hasChildItems: boolean; isExpanded: boolean }) => ReactNode) }) => (
      <>{typeof props.children === 'function' ? props.children({ hasChildItems: false, isExpanded: false }) : props.children}</>
    ),
    MenuTrigger: (props: { children?: ReactNode }) => <>{props.children}</>,
    Popover: divWrapper,
    Menu: divWrapper,
    MenuSection: divWrapper,
    MenuItem: divWrapper,
    Separator: () => <hr />,
    Header: divWrapper,
    ComboBox: divWrapper,
    ComboBoxStateContext: createContext(null),
    Select: divWrapper,
    SelectValue: () => <span />,
    ListBox: divWrapper,
    ListBoxItem: divWrapper,
    TagGroup: divWrapper,
    TagList: (props: { className?: string; items?: unknown[]; children?: ReactNode | ((item: unknown) => ReactNode) }) => (
      <div className={props.className}>
        {typeof props.children === 'function' ? (props.items ?? []).map((item) => (props.children as (x: unknown) => ReactNode)(item)) : props.children}
      </div>
    ),
    Tag: divWrapper,
    ModalOverlay: divWrapper,
    Modal: divWrapper,
    Dialog: divWrapper,
    Heading: (props: { children?: ReactNode }) => <h2>{props.children}</h2>,
    Checkbox: divWrapper,
    RadioGroup: divWrapper,
    Radio: divWrapper,
  };
});

import { LibraryView } from './LibraryView';

describe('LibraryView Rules of Hooks & stateful lifecycle regression', () => {
  const restoreGlobals: Array<() => void> = [];
  let root: Root | null = null;

  const installTestDom = (): MiniElement => {
    class MiniIFrameElement extends MiniElement {}
    const doc = new MiniDocument();
    const container = doc.createElement('div');
    doc.body.appendChild(container);
    const winListeners = new Map<string, Set<(ev: unknown) => void>>();
    const win = {
      document: doc,
      HTMLIFrameElement: MiniIFrameElement,
      HTMLElement: MiniElement,
      SVGElement: MiniElement,
      Element: MiniElement,
      Node: MiniNode,
      setTimeout: globalThis.setTimeout.bind(globalThis),
      clearTimeout: globalThis.clearTimeout.bind(globalThis),
      addEventListener: (type: string, cb: (ev: unknown) => void) => {
        let set = winListeners.get(type);
        if (set === undefined) {
          set = new Set();
          winListeners.set(type, set);
        }
        set.add(cb);
      },
      removeEventListener: (type: string, cb: (ev: unknown) => void) => {
        winListeners.get(type)?.delete(cb);
      },
      dispatchEvent: (ev: MiniEvent) => {
        for (const cb of [...(winListeners.get(ev.type) ?? [])]) {
          cb(ev);
        }
        return !ev.defaultPrevented;
      },
      matchMedia: () => ({ matches: false, addEventListener: () => {}, removeEventListener: () => {} }),
    };
    doc.defaultView = win;

    const prevAct = (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT;
    const prevDoc = globalThis.document;
    const prevWin = globalThis.window;
    const prevNode = globalThis.Node;
    const prevElement = globalThis.Element;
    const prevHTMLElement = globalThis.HTMLElement;
    const prevIFrame = globalThis.HTMLIFrameElement;
    const prevSvg = globalThis.SVGElement;
    const prevEvent = globalThis.Event;
    const prevRaf = globalThis.requestAnimationFrame;
    const prevCancelRaf = globalThis.cancelAnimationFrame;

    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
    vi.stubGlobal('document', doc);
    vi.stubGlobal('window', win);
    vi.stubGlobal('Node', MiniNode);
    vi.stubGlobal('Element', MiniElement);
    vi.stubGlobal('HTMLElement', MiniElement);
    vi.stubGlobal('HTMLIFrameElement', MiniIFrameElement);
    vi.stubGlobal('SVGElement', MiniElement);
    vi.stubGlobal('Event', MiniEvent);
    vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => setTimeout(() => cb(16), 0) as unknown as number);
    vi.stubGlobal('cancelAnimationFrame', (id: number) => clearTimeout(id));

    restoreGlobals.push(() => {
      (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = prevAct;
      vi.stubGlobal('document', prevDoc);
      vi.stubGlobal('window', prevWin);
      vi.stubGlobal('Node', prevNode);
      vi.stubGlobal('Element', prevElement);
      vi.stubGlobal('HTMLElement', prevHTMLElement);
      vi.stubGlobal('HTMLIFrameElement', prevIFrame);
      vi.stubGlobal('SVGElement', prevSvg);
      vi.stubGlobal('Event', prevEvent);
      vi.stubGlobal('requestAnimationFrame', prevRaf);
      vi.stubGlobal('cancelAnimationFrame', prevCancelRaf);
    });

    return container;
  };

  afterEach(async () => {
    if (root !== null) {
      const toUnmount = root;
      root = null;
      await act(async () => {
        toUnmount.unmount();
      });
    }
    while (restoreGlobals.length > 0) {
      restoreGlobals.pop()?.();
    }
  });

  it('transitions from null -> closed welcome -> open library -> Inspector open/close -> sidebar toggle without violating Rules of Hooks', async () => {
    setLocale('en');
    const container = installTestDom();
    root = createRoot(container as unknown as HTMLElement);

    const folders: FolderEntry[] = [{ id: 1, parentId: null, rootId: 1, name: 'C:\\Books', books: 2 }];
    const closedInfo: LibraryInfo = { open: false, location: null, roots: [], books: 0, indexing: false, error: null };
    const openInfo: LibraryInfo = {
      open: true,
      location: 'C:\\Books',
      roots: [{ id: 1, path: 'C:\\Books', status: 'available' }],
      books: 2,
      indexing: false,
      error: null,
    };

    const renderView = async (info: LibraryInfo | null, generation = 0): Promise<void> => {
      await act(async () => {
        root!.render(<LibraryView info={info} folders={folders} generation={generation} onChanged={() => {}} onOpen={() => {}} />);
      });
    };

    // 1. Initial loading state (info === null)
    await renderView(null, 0);
    expect(container.querySelector('.loading')).not.toBeNull();

    // 2. Closed library welcome screen (!info.open)
    await renderView(closedInfo, 0);
    expect(container.querySelector('.welcome')).not.toBeNull();

    // 3. Open library with books loaded (triggers transition past early returns)
    await renderView(openInfo, 1);
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 10));
    });
    expect(container.querySelector('.library-body')).not.toBeNull();
    expect(container.querySelector('#library-actions')).toBeNull();
    expect(container.querySelectorAll('.book-card')).toHaveLength(2);
    expect(container.querySelector('.inspector-shell.closed')).not.toBeNull();

    // 4. Select a book -> opens Details in InspectorShell
    const firstCard = container.querySelector('.book-card');
    expect(firstCard).not.toBeNull();
    await act(async () => {
      firstCard!.dispatchEvent(new MiniEvent('click'));
      await new Promise((resolve) => setTimeout(resolve, 10));
    });
    const openShell = container.querySelector('.inspector-shell');
    expect(openShell).not.toBeNull();
    expect(openShell!.classList.contains('closed')).toBe(false);
    expect(container.querySelector('.library-body')!.classList.contains('with-inspector')).toBe(true);

    // 5. Close Details via Escape on the shelf -> closes InspectorShell
    const shelf = container.querySelector('.shelf');
    expect(shelf).not.toBeNull();
    await act(async () => {
      shelf!.dispatchEvent(Object.assign(new MiniEvent('keydown'), { key: 'Escape' }));
    });
    expect(container.querySelector('.inspector-shell.closed')).not.toBeNull();

    // 6. Toggle sidebar collapse and expand via #toggle-sidebar
    const toggleBtn = container.querySelector('#toggle-sidebar');
    expect(toggleBtn).not.toBeNull();
    await act(async () => {
      toggleBtn!.dispatchEvent(new MiniEvent('click'));
    });
    expect(container.querySelector('.library-body')!.classList.contains('sidebar-collapsed')).toBe(true);

    await act(async () => {
      toggleBtn!.dispatchEvent(new MiniEvent('click'));
    });
    expect(container.querySelector('.library-body')!.classList.contains('sidebar-collapsed')).toBe(false);

    // 7. Ctrl + Wheel cover zoom works without errors
    const wheel = new MiniEvent('wheel');
    Object.assign(wheel, { ctrlKey: true, deltaY: -120 });
    await act(async () => {
      shelf!.dispatchEvent(wheel);
    });
    expect(wheel.defaultPrevented).toBe(true);

    // 8. When kept mounted behind an open book tab (`active={false}`), Ctrl+B is ignored while state stays intact
    await act(async () => {
      root!.render(<LibraryView active={false} info={openInfo} folders={folders} generation={1} onChanged={() => {}} onOpen={() => {}} />);
    });
    const ctrlBWhileHidden = Object.assign(new MiniEvent('keydown'), { key: 'b', ctrlKey: true });
    await act(async () => {
      (window as unknown as { dispatchEvent(ev: MiniEvent): boolean }).dispatchEvent(ctrlBWhileHidden);
    });
    expect(ctrlBWhileHidden.defaultPrevented).toBe(false);
    expect(container.querySelector('.library-body')!.classList.contains('sidebar-collapsed')).toBe(false);

    // Returning to Home (`active={true}`) restores Ctrl+B immediately without remounting
    await act(async () => {
      root!.render(<LibraryView active={true} info={openInfo} folders={folders} generation={1} onChanged={() => {}} onOpen={() => {}} />);
    });
    const ctrlBWhileActive = Object.assign(new MiniEvent('keydown'), { key: 'b', ctrlKey: true });
    await act(async () => {
      (window as unknown as { dispatchEvent(ev: MiniEvent): boolean }).dispatchEvent(ctrlBWhileActive);
    });
    expect(ctrlBWhileActive.defaultPrevented).toBe(true);
    expect(container.querySelector('.library-body')!.classList.contains('sidebar-collapsed')).toBe(true);
  });
});
