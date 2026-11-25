/**
 * The library: every book in the folders it follows, as covers or as a table, found by a search
 * as it is typed, in the titles or in the books' text, or by what the side chooses: a folder, a
 * category, a tag, a collection, a smart collection, or what waits for review. A click chooses a
 * book and shows its details beside the books, to edit; Ctrl+click, Shift+click and Ctrl+A choose
 * several, and the details then change all of them at once. A double click or Enter opens a book.
 */

import { memo, type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Button,
  Cell,
  Collection,
  Column,
  GridList,
  GridListItem,
  GridListLoadMoreItem,
  GridLayout,
  Header,
  Input,
  Menu,
  MenuItem,
  MenuSection,
  MenuTrigger,
  Popover,
  Row,
  SearchField,
  Separator,
  Size,
  Table,
  TableBody,
  TableHeader,
  TableLayout,
  TableLoadMoreItem,
  ToggleButton,
  ToggleButtonGroup,
  Virtualizer,
  type Key,
  type Selection,
} from 'react-aria-components';

import { type BookRow, coverUrl, type FolderEntry, type LibraryInfo, type ReaderTarget, type SelectedBook, api } from '../api';
import { tn, useLocale, useT } from '../i18n';
import type { ExchangeAction } from '../exchange';
import { useExchange } from '../ExchangeSection';
import { onEscape, useFocusReturn } from '../focus';
import { OverlayScrollbar } from '../OverlayScrollbar';
import { isMotionReduced, updatePreferences, usePreferences } from '../preferences/store';
import { expDamp } from '../scrollbar';
import { BatchPanel } from './BatchPanel';
import { CalibreDialog } from './CalibreDialog';
import { FieldsDialog, FoldersDialog, reasonOf } from './dialogs';
import { type CollectionPlace, Inspector } from './Inspector';
import { MergeDialog, ReviewView } from './Review';
import { Sidebar, SidebarShell } from './Sidebar';
import {
  cardHeightForCoverWidth,
  clampCoverWidth,
  clampSidebarWidth,
  COVER_DEFAULT_WIDTH,
  formatAddRootsNotice,
  isEditableTarget,
  resizeSidebarWidth,
  SIDEBAR_DEFAULT_WIDTH,
  SIDEBAR_MAX_WIDTH,
  SIDEBAR_MIN_WIDTH,
  SIDEBAR_STEP,
  stepCoverWidth,
} from './sidebar-model';
import { type Filter, filterArgs } from './taxonomy-model';
import { TextResults } from './TextResults';
import { type Shown, useBooks, useCoverVersions, useTaxonomy } from './use-books';

/** How long typing has to pause before the search runs. Short: the search answers in milliseconds. */
const SEARCH_AFTER_MS = 90;
const RESIZE_DAMPING_K = 20;
const RESIZE_SETTLE_EPSILON = 0.5;
const GRID_REFLOW_MS = 340;
const INSPECTOR_CLOSE_MS = 280;

const TABLE_LAYOUT_OPTIONS = { rowHeight: 36, headingHeight: 34 };

const FORMAT_NAMES: Record<string, string> = {
  pdf: 'PDF',
  epub: 'EPUB',
  mobi: 'MOBI',
  azw: 'AZW',
  azw3: 'AZW3',
  fb2: 'FB2',
  cbz: 'CBZ',
  cbr: 'CBR',
  djvu: 'DjVu',
  djv: 'DjVu',
  txt: 'TXT',
  md: 'Markdown',
  markdown: 'Markdown',
};

export function formatName(format: string): string {
  return FORMAT_NAMES[format] ?? format.toUpperCase();
}

function hueOf(text: string): number {
  let hash = 0;
  for (const character of text) {
    hash = (hash * 31 + character.charCodeAt(0)) >>> 0;
  }
  return hash % 360;
}

/** A cover the host drew, or, until there is one, a plain one with the title. */
const Cover = memo(function Cover({ book, version }: { book: BookRow; version: number }): React.JSX.Element {
  const [missing, setMissing] = useState(false);
  useEffect(() => setMissing(false), [version]);
  if (book.blake3 !== null && !missing) {
    return (
      <img
        className="cover"
        src={coverUrl(book.blake3, version)}
        alt=""
        loading="lazy"
        decoding="async"
        draggable={false}
        onError={() => setMissing(true)}
      />
    );
  }
  return (
    <div className="cover cover-plain" style={{ '--hue': hueOf(book.title) } as React.CSSProperties} aria-hidden="true">
      <span className="cover-plain-title">{book.title}</span>
      <span className="cover-plain-format">{formatName(book.format)}</span>
    </div>
  );
});

const Progress = memo(function Progress({ value }: { value: number | null }): React.JSX.Element | null {
  const t = useT();
  if (value === null || value <= 0) {
    return null;
  }
  return (
    <div className="progress" aria-label={t('library.progress', { percent: Math.round(value * 100) })}>
      <div className="progress-fill" style={{ width: `${Math.max(2, value * 100)}%` }} />
    </div>
  );
});

interface BookShelfProps {
  rows: BookRow[];
  loading: boolean;
  view: 'grid' | 'table';
  coverWidth: number;
  selection: Selection;
  versions: Map<string, number>;
  locale: string;
  onSelect(keys: Selection): void;
  onOpenBook(key: Key): void;
  onLoadMore(): void;
}

const BookShelf = memo(function BookShelf({
  rows,
  loading,
  view,
  coverWidth,
  selection,
  versions,
  locale,
  onSelect,
  onOpenBook,
  onLoadMore,
}: BookShelfProps): React.JSX.Element {
  const t = useT();
  const collectionDeps = useMemo(() => [versions, locale], [versions, locale]);
  const gridLayoutOptions = useMemo(() => {
    const width = clampCoverWidth(coverWidth);
    const size = new Size(width, cardHeightForCoverWidth(width));
    return {
      minItemSize: size,
      maxItemSize: size,
      minSpace: new Size(20, 22),
      preserveAspectRatio: true,
    };
  }, [coverWidth]);

  return view === 'grid' ? (
    <Virtualizer layout={GridLayout} layoutOptions={gridLayoutOptions}>
      <GridList
        aria-label={t('library.books.label')}
        className="book-grid"
        layout="grid"
        selectionMode="multiple"
        selectionBehavior="replace"
        disallowEmptySelection
        selectedKeys={selection}
        onSelectionChange={onSelect}
        onAction={onOpenBook}
      >
        <Collection items={rows} dependencies={collectionDeps}>
          {(book) => (
            <GridListItem id={book.documentId} textValue={book.title} className="book-card" data-document={book.documentId}>
              <Cover book={book} version={book.blake3 !== null ? (versions.get(book.blake3) ?? 0) : 0} />
              <div className="book-meta">
                <span className="book-title" title={book.title}>
                  {book.title}
                </span>
                <span className="book-creators">{book.creators ?? formatName(book.format)}</span>
              </div>
              <Progress value={book.progress} />
            </GridListItem>
          )}
        </Collection>
        <GridListLoadMoreItem onLoadMore={onLoadMore} isLoading={loading}>
          <span className="loading-more">{t('common.loading')}</span>
        </GridListLoadMoreItem>
      </GridList>
    </Virtualizer>
  ) : (
    <Virtualizer layout={TableLayout} layoutOptions={TABLE_LAYOUT_OPTIONS}>
      <Table
        aria-label={t('library.books.label')}
        className="book-table"
        selectionMode="multiple"
        selectionBehavior="replace"
        disallowEmptySelection
        selectedKeys={selection}
        onSelectionChange={onSelect}
        onRowAction={onOpenBook}
      >
        <TableHeader>
          <Column isRowHeader id="title" width="2fr">
            {t('library.column.title')}
          </Column>
          <Column id="creators" width="1.2fr">
            {t('library.column.creators')}
          </Column>
          <Column id="format" width={96}>
            {t('library.column.format')}
          </Column>
          <Column id="progress" width={90}>
            {t('library.column.progress')}
          </Column>
          <Column id="path" width="2fr">
            {t('library.column.path')}
          </Column>
        </TableHeader>
        <TableBody>
          <Collection items={rows}>
            {(book) => (
              <Row id={book.documentId} className="book-row" data-document={book.documentId}>
                <Cell>{book.title}</Cell>
                <Cell>{book.creators ?? ''}</Cell>
                <Cell>{formatName(book.format)}</Cell>
                <Cell>{book.progress !== null ? `${Math.round(book.progress * 100)}%` : ''}</Cell>
                <Cell>
                  <span className="path" title={book.path}>
                    {book.path}
                  </span>
                </Cell>
              </Row>
            )}
          </Collection>
          <TableLoadMoreItem onLoadMore={onLoadMore} isLoading={loading}>
            <span className="loading-more">{t('common.loading')}</span>
          </TableLoadMoreItem>
        </TableBody>
      </Table>
    </Virtualizer>
  );
});

function Welcome({ info, onChanged, onPreferences }: { info: LibraryInfo | null; onChanged(): void; onPreferences?(): void }): React.JSX.Element {
  const t = useT();
  const [error, setError] = useState<string | null>(info?.error ?? null);
  const chooseFolders = (): void => {
    setError(null);
    api
      .chooseRoot()
      .then((result) => {
        if (result !== null) {
          if (result.added.length > 0) {
            onChanged();
          }
          const problem = formatAddRootsNotice(result);
          if (problem !== null) {
            setError(problem);
          }
        }
      })
      .catch((reason: unknown) => setError(reasonOf(reason)));
  };
  const openExisting = (): void => {
    setError(null);
    api
      .chooseExisting()
      .then(onChanged)
      .catch((reason: unknown) => setError(reasonOf(reason)));
  };
  return (
    <div className="welcome">
      <h1>Musaeum</h1>
      <p>{t('library.welcome.text')}</p>
      <div className="welcome-actions">
        <Button className="button primary" onPress={chooseFolders}>
          {t('library.welcome.choose')}
        </Button>
        <Button className="button" onPress={openExisting}>
          {t('library.welcome.existing')}
        </Button>
        <Button id="open-preferences" className="button" onPress={onPreferences} aria-keyshortcuts="Control+Comma">
          {t('prefs.open')}
        </Button>
      </div>
      {error !== null ? <p className="error">{error}</p> : null}
    </div>
  );
}

/** A book chosen: kept by itself, since a choice of all the books holds many not read yet. */
interface Chosen {
  documentId: number;
  workId: number;
  /** The row, when it was read; a book chosen with Ctrl+A further down has none yet. */
  row: BookRow | null;
}

export type SearchIn = 'titles' | 'text';

export interface InspectorShellProps {
  open: boolean;
  onKeyDown?: (event: React.KeyboardEvent<HTMLDivElement>) => void;
  children: ReactNode;
}

/**
 * Animated right-hand details host kept mounted in the DOM so open/close glides smoothly without
 * snapping grid columns; keeps last rendered content briefly during close while `inert` +
 * `aria-hidden` prevent interaction or focus.
 */
export function InspectorShell({ open, onKeyDown, children }: InspectorShellProps): React.JSX.Element {
  const [retained, setRetained] = useState<ReactNode>(open ? children : null);

  useEffect(() => {
    if (open) {
      setRetained(children);
      return;
    }
    if (isMotionReduced()) {
      setRetained(null);
      return;
    }
    const timer = setTimeout(() => setRetained(null), INSPECTOR_CLOSE_MS);
    return () => clearTimeout(timer);
  }, [open, children]);

  return (
    <div className={open ? 'inspector-shell' : 'inspector-shell closed'} inert={!open || undefined} aria-hidden={!open || undefined} onKeyDown={onKeyDown}>
      <div className="inspector-inner">{open ? children : retained}</div>
    </div>
  );
}

/** What the library keeps while a book is open, so it comes back as it was left. */
export interface LibraryMemory {
  typed: string;
  searchIn: SearchIn;
  filter: Filter;
  view: 'grid' | 'table';
}

export const FRESH_LIBRARY: LibraryMemory = { typed: '', searchIn: 'titles', filter: { kind: 'all' }, view: 'grid' };

export interface LibraryViewProps {
  info: LibraryInfo | null;
  folders: FolderEntry[];
  generation: number;
  onChanged(): void;
  /** Opens a book, where its reading stopped or at a match found in its text. */
  onOpen(book: BookRow, target?: ReaderTarget): void;
  /** Where the library was left, read when it shows and written as it changes. */
  memory?: React.MutableRefObject<LibraryMemory>;
  /** For tests: the books shown, as they are listed. */
  onRows?(rows: BookRow[], total: number, shown: Shown): void;
  /** For tests: the books chosen. */
  onSelection?(books: SelectedBook[]): void;
  /** For tests: a Calibre library to bring in, without the system's folder dialog. */
  calibreRequest?: { folder: string; at: number } | null;
  /** Opens the Preferences. */
  onPreferences?(): void;
}

export function LibraryView({ info, folders, generation, onChanged, onOpen, memory, onRows, onSelection, calibreRequest = null, onPreferences }: LibraryViewProps): React.JSX.Element {
  const t = useT();
  const locale = useLocale();
  const prefs = usePreferences();
  const prefWidth = clampSidebarWidth(prefs?.preferences.sidebarWidth);
  const prefCollapsed = prefs?.preferences.sidebarCollapsed ?? false;
  const showEmptySections = prefs?.preferences.showEmptySidebarSections ?? false;
  const showSidebarCounts = prefs?.preferences.showSidebarCounts ?? false;
  const prefCoverWidth = clampCoverWidth(prefs?.preferences.coverWidth ?? COVER_DEFAULT_WIDTH);
  const [sidebarWidth, setSidebarWidth] = useState(prefWidth);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(prefCollapsed);
  const [coverWidth, setCoverWidth] = useState(prefCoverWidth);
  const [fieldsOpen, setFieldsOpen] = useState(false);
  const [foldersOpen, setFoldersOpen] = useState(false);

  const zoomSaveTimerRef = useRef<number | null>(null);
  const reflowTimerRef = useRef<number | null>(null);
  const prevReflowRef = useRef<{ collapsed: boolean; panelOpen: boolean; coverWidth: number } | null>(null);

  const bodyRef = useRef<HTMLDivElement>(null);
  const resizerRef = useRef<HTMLDivElement>(null);
  const sidebarScrollRef = useRef<HTMLElement>(null);
  const actionsButtonRef = useRef<HTMLButtonElement>(null);
  const toggleButtonRef = useRef<HTMLButtonElement>(null);

  const currentWidthRef = useRef(prefWidth);
  const targetWidthRef = useRef(prefWidth);
  const resizeRafRef = useRef<number | null>(null);
  const resizeLastTimeRef = useRef(0);
  const isDraggingRef = useRef(false);
  const persistPendingRef = useRef(false);
  const dragCleanupRef = useRef<(() => void) | null>(null);

  const remembered = memory?.current ?? FRESH_LIBRARY;
  const [typed, setTyped] = useState(remembered.typed);
  const [query, setQuery] = useState(remembered.typed);
  const [searchIn, setSearchIn] = useState<SearchIn>(remembered.searchIn);
  const [filter, setFilter] = useState<Filter>(remembered.filter);
  const [view, setView] = useState<'grid' | 'table'>(remembered.view);
  // Raised by every change made here, so the books and the side are read again.
  const [version, setVersion] = useState(0);
  const inText = searchIn === 'text' && query !== '';
  // Searching the text, the listing behind is the side's books, not the titles that match.
  const books = useBooks(inText ? '' : query, filter, generation + version);
  const { taxonomy, refresh: refreshTaxonomy } = useTaxonomy(generation);
  const versions = useCoverVersions();
  // The chosen books are kept by themselves, not looked up in the rows: a list read again after a
  // save brings back only its first page, and the books being edited may be further down.
  const [chosen, setChosen] = useState<ReadonlyMap<number, Chosen>>(() => new Map());
  const [notice, setNotice] = useState<string | null>(null);
  const [merging, setMerging] = useState<number[] | null>(null);
  const [calibre, setCalibre] = useState<{ folder: string | null } | null>(null);
  const [waiting, setWaiting] = useState(0);
  const byId = useMemo(() => new Map(books.rows.map((row) => [row.documentId, row])), [books.rows]);
  // A choice of all the books answers after the host listed them; a click since then wins.
  const allAsked = useRef(0);
  // The selection as React Aria last gave it: it also knows where a Shift+click extends from (its
  // anchor), which a set made again from the chosen books would forget.
  const given = useRef<Set<Key> | null>(null);
  const changed = useCallback(() => {
    refreshTaxonomy();
    setVersion((value) => value + 1);
  }, [refreshTaxonomy]);
  // The whole library's annotations, out and in, from the toolbar's menu.
  const everything = useExchange({ kind: 'library' }, () => changed(), 'library');
  useEffect(() => {
    if (everything.report !== null) {
      setNotice(everything.report.text);
    }
  }, [everything.report]);

  const applyWidthDom = useCallback((width: number) => {
    bodyRef.current?.style.setProperty('--sidebar-width', `${width}px`);
    resizerRef.current?.setAttribute('aria-valuenow', String(Math.round(width)));
  }, []);

  const cancelResizeRaf = useCallback(() => {
    if (resizeRafRef.current !== null) {
      cancelAnimationFrame(resizeRafRef.current);
      resizeRafRef.current = null;
    }
    resizeLastTimeRef.current = 0;
  }, []);

  const finishWidthSettled = useCallback(() => {
    const finalWidth = clampSidebarWidth(targetWidthRef.current);
    currentWidthRef.current = finalWidth;
    targetWidthRef.current = finalWidth;
    applyWidthDom(finalWidth);
    bodyRef.current?.classList.remove('is-resizing');
    cancelResizeRaf();
    setSidebarWidth(finalWidth);
    if (persistPendingRef.current) {
      persistPendingRef.current = false;
      void updatePreferences({ sidebarWidth: finalWidth }).catch(() => { });
    }
  }, [applyWidthDom, cancelResizeRaf]);

  const stepResizeRaf = useCallback(
    (now: number) => {
      resizeRafRef.current = null;
      const dt = resizeLastTimeRef.current === 0 ? 1 / 60 : (now - resizeLastTimeRef.current) / 1000;
      resizeLastTimeRef.current = now;

      const target = targetWidthRef.current;
      const next = expDamp(currentWidthRef.current, target, dt, RESIZE_DAMPING_K, RESIZE_SETTLE_EPSILON);
      currentWidthRef.current = next;
      applyWidthDom(next);

      if (Math.abs(target - next) >= RESIZE_SETTLE_EPSILON) {
        resizeRafRef.current = requestAnimationFrame(stepResizeRaf);
      } else if (!isDraggingRef.current) {
        finishWidthSettled();
      } else {
        resizeLastTimeRef.current = 0;
      }
    },
    [applyWidthDom, finishWidthSettled],
  );

  const animateWidthTo = useCallback(
    (target: number, persist: boolean) => {
      const clamped = clampSidebarWidth(target);
      targetWidthRef.current = clamped;
      persistPendingRef.current ||= persist;

      if (isMotionReduced()) {
        cancelResizeRaf();
        currentWidthRef.current = clamped;
        applyWidthDom(clamped);
        if (!isDraggingRef.current) {
          bodyRef.current?.classList.remove('is-resizing');
          setSidebarWidth(clamped);
          if (persistPendingRef.current) {
            persistPendingRef.current = false;
            void updatePreferences({ sidebarWidth: clamped }).catch(() => { });
          }
        }
        return;
      }

      bodyRef.current?.classList.add('is-resizing');
      if (Math.abs(clamped - currentWidthRef.current) < RESIZE_SETTLE_EPSILON) {
        if (!isDraggingRef.current) {
          finishWidthSettled();
        }
        return;
      }

      if (resizeRafRef.current === null) {
        resizeLastTimeRef.current = 0;
        resizeRafRef.current = requestAnimationFrame(stepResizeRaf);
      }
    },
    [applyWidthDom, cancelResizeRaf, finishWidthSettled, stepResizeRaf],
  );

  useEffect(() => {
    if (!isDraggingRef.current && resizeRafRef.current === null) {
      currentWidthRef.current = prefWidth;
      targetWidthRef.current = prefWidth;
      setSidebarWidth(prefWidth);
      applyWidthDom(prefWidth);
    }
  }, [prefWidth, applyWidthDom]);

  useEffect(() => {
    setSidebarCollapsed(prefCollapsed);
  }, [prefCollapsed]);

  useEffect(() => {
    if (zoomSaveTimerRef.current === null) {
      setCoverWidth(prefCoverWidth);
    }
  }, [prefCoverWidth]);

  useEffect(
    () => () => {
      if (zoomSaveTimerRef.current !== null) {
        window.clearTimeout(zoomSaveTimerRef.current);
        zoomSaveTimerRef.current = null;
      }
      if (reflowTimerRef.current !== null) {
        window.clearTimeout(reflowTimerRef.current);
        reflowTimerRef.current = null;
      }
    },
    [],
  );

  useEffect(
    () => () => {
      dragCleanupRef.current?.();
      cancelResizeRaf();
    },
    [cancelResizeRaf],
  );

  const toggleSidebar = useCallback(() => {
    setSidebarCollapsed((current) => {
      const next = !current;
      if (next && document.activeElement?.closest('.sidebar-shell') != null) {
        toggleButtonRef.current?.focus();
      }
      void updatePreferences({ sidebarCollapsed: next }).catch(() => { });
      return next;
    });
  }, []);

  useEffect(() => {
    if (info?.open !== true) {
      return;
    }
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.defaultPrevented || event.altKey || event.shiftKey || !(event.ctrlKey || event.metaKey)) {
        return;
      }
      if (event.key.toLowerCase() !== 'b') {
        return;
      }
      if (isEditableTarget(event.target) || isEditableTarget(document.activeElement)) {
        return;
      }
      if (document.querySelector('.modal-overlay') !== null) {
        return;
      }
      event.preventDefault();
      toggleSidebar();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [info?.open, toggleSidebar]);

  const startResize = useCallback(
    (event: React.PointerEvent<HTMLDivElement>) => {
      if (event.button !== 0 || sidebarCollapsed) {
        return;
      }
      event.preventDefault();
      dragCleanupRef.current?.();

      const startX = event.clientX;
      const startWidth = currentWidthRef.current;
      isDraggingRef.current = true;
      bodyRef.current?.classList.add('is-resizing');
      resizerRef.current?.classList.add('dragging');

      const onMove = (moveEvent: PointerEvent): void => {
        const next = resizeSidebarWidth(startWidth, moveEvent.clientX - startX);
        animateWidthTo(next, false);
      };

      const finish = (): void => {
        cleanup();
        isDraggingRef.current = false;
        resizerRef.current?.classList.remove('dragging');
        const changedWidth = Math.round(targetWidthRef.current) !== Math.round(startWidth);
        if (changedWidth) {
          persistPendingRef.current = true;
        }
        if (resizeRafRef.current === null) {
          finishWidthSettled();
        }
      };

      const cleanup = (): void => {
        window.removeEventListener('pointermove', onMove);
        window.removeEventListener('pointerup', finish);
        window.removeEventListener('pointercancel', finish);
        dragCleanupRef.current = null;
      };

      dragCleanupRef.current = cleanup;
      window.addEventListener('pointermove', onMove);
      window.addEventListener('pointerup', finish);
      window.addEventListener('pointercancel', finish);
    },
    [animateWidthTo, finishWidthSettled, sidebarCollapsed],
  );

  const onResizerDoubleClick = useCallback(() => {
    if (!sidebarCollapsed) {
      animateWidthTo(SIDEBAR_DEFAULT_WIDTH, true);
    }
  }, [animateWidthTo, sidebarCollapsed]);

  const onResizerKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLDivElement>) => {
      if (sidebarCollapsed) {
        return;
      }
      let next: number | null = null;
      if (event.key === 'ArrowLeft') {
        next = clampSidebarWidth(targetWidthRef.current - SIDEBAR_STEP);
      } else if (event.key === 'ArrowRight') {
        next = clampSidebarWidth(targetWidthRef.current + SIDEBAR_STEP);
      } else if (event.key === 'Home') {
        next = SIDEBAR_MIN_WIDTH;
      } else if (event.key === 'End') {
        next = SIDEBAR_MAX_WIDTH;
      }
      if (next !== null) {
        event.preventDefault();
        animateWidthTo(next, true);
      }
    },
    [animateWidthTo, sidebarCollapsed],
  );

  const addFolders = useCallback(async () => {
    try {
      const result = await api.chooseRoot();
      if (result === null) {
        return;
      }
      if (result.added.length > 0) {
        onChanged();
      }
      const message = formatAddRootsNotice(result);
      if (message !== null) {
        setNotice(message);
      }
    } catch (reason: unknown) {
      setNotice(reasonOf(reason));
    }
  }, [onChanged]);

  const removeRoot = useCallback(
    async (id: number) => {
      await api.removeRoot(id);
      if (filter.kind === 'folder') {
        const active = folders.find((entry) => entry.id === filter.id);
        if (active?.rootId === id) {
          setFilter({ kind: 'all' });
        }
      }
      onChanged();
      changed();
    },
    [changed, filter, folders, onChanged],
  );

  useEffect(() => {
    if (memory !== undefined) {
      memory.current = { typed, searchIn, filter, view };
    }
  }, [memory, typed, searchIn, filter, view]);

  // Gone to another filter or search, only the chosen books listed there stay chosen, so the
  // others do not come back by themselves on returning. The same list read again keeps them all.
  const shownKey = JSON.stringify([books.shown.text, books.shown.filter]);
  const lastShown = useRef(shownKey);
  useEffect(() => {
    if (lastShown.current === shownKey) {
      return;
    }
    lastShown.current = shownKey;
    allAsked.current++;
    setChosen((current) => {
      if (current.size === 0) {
        return current;
      }
      const listed = new Set(books.rows.map((row) => row.documentId));
      const kept = [...current.values()].filter((book) => listed.has(book.documentId));
      return kept.length === current.size ? current : new Map(kept.map((book) => [book.documentId, book]));
    });
  }, [shownKey, books.rows]);

  useEffect(() => {
    const timer = window.setTimeout(() => setQuery(typed), SEARCH_AFTER_MS);
    return () => clearTimeout(timer);
  }, [typed]);
  useEffect(() => onRows?.(books.rows, books.total, books.shown), [books.rows, books.total, books.shown, onRows]);
  useEffect(() => {
    if (notice === null) {
      return;
    }
    const timer = window.setTimeout(() => setNotice(null), 6000);
    return () => clearTimeout(timer);
  }, [notice]);
  // How much waits for review, beside its entry on the side.
  const open = info?.open === true;
  useEffect(() => {
    if (!open) {
      return;
    }
    let current = true;
    api
      .review()
      .then((review) => current && setWaiting(review.suggestions.length + review.duplicates.length))
      .catch(() => { });
    return () => {
      current = false;
    };
  }, [open, generation, version]);
  useEffect(() => {
    if (calibreRequest !== null) {
      setCalibre({ folder: calibreRequest.folder });
    }
  }, [calibreRequest]);

  // The chosen books with what was read of them last: a merge moves a book to another work.
  const current = useMemo(
    () =>
      [...chosen.values()].map((book): Chosen => {
        const row = byId.get(book.documentId);
        return row === undefined ? book : { documentId: book.documentId, workId: row.workId, row };
      }),
    [chosen, byId],
  );
  useEffect(() => onSelection?.(current.map(({ documentId, workId }) => ({ documentId, workId }))), [current, onSelection]);
  // The details beside the books: when they close with the focus in them, it goes back to the books.
  const shelf = useRef<HTMLElement>(null);
  const focusShelf = useCallback(() => {
    const root = shelf.current;
    (root?.querySelector<HTMLElement>('[aria-selected="true"]') ?? root?.querySelector<HTMLElement>('[role="grid"], [role="treegrid"]'))?.focus();
  }, []);
  useFocusReturn(current.length > 0 && filter.kind !== 'review', focusShelf);

  const byIdRef = useRef(byId);
  byIdRef.current = byId;
  const onOpenRef = useRef(onOpen);
  onOpenRef.current = onOpen;
  const filterRef = useRef(filter);
  filterRef.current = filter;
  const searchQueryRef = useRef(inText ? '' : query);
  searchQueryRef.current = inText ? '' : query;

  const openBook = useCallback((key: Key) => {
    const book = byIdRef.current.get(Number(key));
    if (book !== undefined) {
      onOpenRef.current(book);
    }
  }, []);

  const select = useCallback((keys: Selection) => {
    const asked = ++allAsked.current;
    given.current = keys === 'all' ? null : keys;
    if (keys === 'all') {
      // Every book the list holds, not only those read so far: the host lists them.
      const listed = byIdRef.current;
      api
        .selectAll({ ...filterArgs(filterRef.current), text: searchQueryRef.current })
        .then((all) => {
          if (asked === allAsked.current) {
            setChosen(new Map(all.map((book) => [book.documentId, { ...book, row: listed.get(book.documentId) ?? null }])));
          }
        })
        .catch((reason: unknown) => setNotice(reasonOf(reason)));
      return;
    }
    const listed = byIdRef.current;
    setChosen((before) => {
      const next = new Map<number, Chosen>();
      for (const key of keys) {
        const id = Number(key);
        const row = listed.get(id);
        const known = before.get(id);
        if (row !== undefined) {
          next.set(id, { documentId: id, workId: row.workId, row });
        } else if (known !== undefined) {
          next.set(id, known);
        }
      }
      return next;
    });
  }, []);

  const openFolders = useCallback(() => setFoldersOpen(true), []);
  const openFields = useCallback(() => setFieldsOpen(true), []);

  const works = [...new Set(current.map((book) => book.workId))];
  // One work chosen (one book, or its formats) is edited as itself; more, all at once.
  const single = works.length === 1 ? (current.find((book) => book.row !== null)?.row ?? null) : null;
  const reviewing = filter.kind === 'review';
  const panelOpen = info?.open === true && !reviewing && (works.length >= 2 || single !== null);

  useEffect(() => {
    const host = shelf.current;
    if (host === null || info?.open !== true || reviewing || inText || view !== 'grid') {
      return;
    }
    const onWheel = (event: WheelEvent): void => {
      if (!(event.ctrlKey || event.metaKey) || event.deltaY === 0) {
        return;
      }
      event.preventDefault();
      const deltaSteps = event.deltaY < 0 ? 1 : -1;
      setCoverWidth((currentWidth) => {
        const next = stepCoverWidth(currentWidth, deltaSteps);
        if (next === currentWidth) {
          return currentWidth;
        }
        if (zoomSaveTimerRef.current !== null) {
          window.clearTimeout(zoomSaveTimerRef.current);
        }
        zoomSaveTimerRef.current = window.setTimeout(() => {
          zoomSaveTimerRef.current = null;
          void updatePreferences({ coverWidth: next }).catch(() => { });
        }, 300);
        return next;
      });
    };
    host.addEventListener('wheel', onWheel, { passive: false });
    return () => host.removeEventListener('wheel', onWheel);
  }, [info?.open, reviewing, inText, view]);

  useEffect(() => {
    const grid = shelf.current?.querySelector<HTMLElement>('.book-grid') ?? null;
    if (info?.open !== true || reviewing || inText || view !== 'grid' || grid === null) {
      prevReflowRef.current = null;
      return;
    }
    const stopReflow = (): void => {
      grid.classList.remove('is-reflowing');
      if (reflowTimerRef.current !== null) {
        window.clearTimeout(reflowTimerRef.current);
        reflowTimerRef.current = null;
      }
    };
    grid.addEventListener('scroll', stopReflow, { passive: true });
    const prev = prevReflowRef.current;
    prevReflowRef.current = { collapsed: sidebarCollapsed, panelOpen, coverWidth };
    if (
      prev !== null &&
      !isMotionReduced() &&
      (prev.collapsed !== sidebarCollapsed || prev.panelOpen !== panelOpen || prev.coverWidth !== coverWidth)
    ) {
      grid.classList.add('is-reflowing');
      if (reflowTimerRef.current !== null) {
        window.clearTimeout(reflowTimerRef.current);
      }
      reflowTimerRef.current = window.setTimeout(() => {
        reflowTimerRef.current = null;
        grid.classList.remove('is-reflowing');
      }, GRID_REFLOW_MS);
    }
    return () => {
      grid.removeEventListener('scroll', stopReflow);
    };
  }, [info?.open, reviewing, inText, view, sidebarCollapsed, panelOpen, coverWidth]);

  if (info === null) {
    return <div className="loading">{t('library.loading')}</div>;
  }
  if (!info.open) {
    return <Welcome info={info} onChanged={onChanged} onPreferences={onPreferences} />;
  }

  const last = given.current;
  const selection: Selection = last !== null && last.size === chosen.size && [...last].every((key) => chosen.has(Number(key))) ? last : new Set<Key>(chosen.keys());
  const forget = (): void => {
    allAsked.current++;
    setChosen(new Map());
  };
  // A click on the chosen book keeps it chosen, as in a file manager; Escape puts the details away.
  const putAway = (event: React.KeyboardEvent): void => {
    if (event.key === 'Escape' && chosen.size > 0) {
      forget();
    }
  };
  const filtered = query !== '' || filter.kind !== 'all';
  // In a collection, the books just before and after the chosen one, for moving it there.
  let place: CollectionPlace | null = null;
  if (single !== null && chosen.size === 1 && filter.kind === 'collection' && query === '') {
    const at = books.rows.findIndex((row) => row.documentId === single.documentId);
    const collection = taxonomy.collections.find((candidate) => candidate.id === filter.id);
    if (at >= 0 && collection !== undefined) {
      place = {
        collection: collection.id,
        name: collection.name,
        before: books.rows[at - 1]?.workId ?? null,
        after: books.rows[at + 1]?.workId ?? null,
      };
    }
  }
  const panel = reviewing ? null : works.length >= 2 ? (
    <BatchPanel works={works} books={current.length} taxonomy={taxonomy} onSaved={changed} onClose={forget} onMerge={setMerging} />
  ) : single !== null ? (
    <Inspector key={single.workId} book={single} taxonomy={taxonomy} place={place} onSaved={changed} onClose={forget} />
  ) : null;
  const merged = (keep: number): void => {
    setMerging(null);
    // The merged books are one work now: it is what stays chosen, to see it whole.
    const kept = current.find((book) => book.workId === keep) ?? current[0];
    allAsked.current++;
    setChosen(kept === undefined ? new Map() : new Map([[kept.documentId, { ...kept, workId: keep }]]));
    setNotice(t('library.merged'));
    changed();
  };
  const status = [
    ...(chosen.size > 1 ? [t('library.chosen', { count: chosen.size })] : []),
    books.total === info.books || filtered ? tn('library.books', books.total) : t('library.booksOf', { shown: books.total, total: info.books }),
    ...(info.indexing ? [t('library.indexing')] : []),
    ...(everything.busy !== null ? [t('library.exchanging')] : []),
  ].join(' · ');

  const runAction = (key: Key): void => {
    switch (String(key)) {
      case 'add-folder':
        void addFolders();
        break;
      case 'manage-folders':
        actionsButtonRef.current?.focus();
        setFoldersOpen(true);
        break;
      case 'calibre':
        actionsButtonRef.current?.focus();
        setCalibre({ folder: null });
        break;
      case 'export-jsonld':
      case 'import-jsonld':
        void everything.run(String(key) as ExchangeAction);
        break;
      case 'fields':
        actionsButtonRef.current?.focus();
        setFieldsOpen(true);
        break;
      case 'preferences':
        actionsButtonRef.current?.focus();
        onPreferences?.();
        break;
    }
  };
  const bodyClasses = ['library-body'];
  if (panelOpen) {
    bodyClasses.push('with-inspector');
  }
  if (sidebarCollapsed) {
    bodyClasses.push('sidebar-collapsed');
  }
  const shelfWatchKey = reviewing ? 'review' : inText ? 'text' : view;

  return (
    <div className="library">
      <header className="toolbar">
        <h1 className="sr-only">{t('library.title')}</h1>
        <Button
          ref={toggleButtonRef}
          id="toggle-sidebar"
          className="button icon-button"
          aria-label={sidebarCollapsed ? t('sidebar.show') : t('sidebar.hide')}
          aria-pressed={!sidebarCollapsed}
          aria-keyshortcuts="Control+B"
          onPress={toggleSidebar}
        >
          <span className="icon-box" title={sidebarCollapsed ? t('sidebar.show') : t('sidebar.hide')}>
            <svg className="icon" viewBox="0 0 16 16" width={16} height={16} fill="none" stroke="currentColor" aria-hidden="true">
              <rect x={2} y={2.5} width={12} height={11} rx={1.5} />
              <line x1={6} y1={2.5} x2={6} y2={13.5} />
            </svg>
          </span>
        </Button>
        <SearchField className="search" aria-label={t('library.search.label')} value={typed} onChange={setTyped} onSubmit={setQuery}>
          <Input placeholder={searchIn === 'text' ? t('library.search.text') : t('library.search.titles')} />
          <Button className="search-clear" aria-label={t('library.search.clear')}>
            ×
          </Button>
        </SearchField>
        {typed !== '' || searchIn === 'text' ? (
          <ToggleButtonGroup
            className="segmented"
            aria-label={t('library.search.in')}
            selectionMode="single"
            disallowEmptySelection
            selectedKeys={[searchIn]}
            onSelectionChange={(keys) => setSearchIn([...keys][0] as SearchIn)}
          >
            <ToggleButton id="titles" className="segment">
              {t('library.search.inTitles')}
            </ToggleButton>
            <ToggleButton id="text" className="segment">
              {t('library.search.inText')}
            </ToggleButton>
          </ToggleButtonGroup>
        ) : null}
        <div className="toolbar-end">
          <span className="toolbar-status" aria-live="polite" aria-busy={info.indexing || everything.busy !== null}>
            {status}
          </span>
          <ToggleButtonGroup
            className="segmented icon-segmented"
            aria-label={t('library.view.label')}
            selectionMode="single"
            disallowEmptySelection
            selectedKeys={[view]}
            onSelectionChange={(keys) => setView([...keys][0] as 'grid' | 'table')}
          >
            <ToggleButton id="grid" className="segment icon-segment" aria-label={t('library.view.grid')}>
              <span className="icon-box" title={t('library.view.grid')}>
                <svg className="icon" viewBox="0 0 16 16" width={15} height={15} fill="none" stroke="currentColor" aria-hidden="true">
                  <rect x={2.5} y={2.5} width={4.5} height={4.5} rx={0.75} />
                  <rect x={9} y={2.5} width={4.5} height={4.5} rx={0.75} />
                  <rect x={2.5} y={9} width={4.5} height={4.5} rx={0.75} />
                  <rect x={9} y={9} width={4.5} height={4.5} rx={0.75} />
                </svg>
              </span>
            </ToggleButton>
            <ToggleButton id="table" className="segment icon-segment" aria-label={t('library.view.table')}>
              <span className="icon-box" title={t('library.view.table')}>
                <svg className="icon" viewBox="0 0 16 16" width={15} height={15} fill="none" stroke="currentColor" aria-hidden="true">
                  <line x1={3} y1={4} x2={13} y2={4} />
                  <line x1={3} y1={8} x2={13} y2={8} />
                  <line x1={3} y1={12} x2={13} y2={12} />
                </svg>
              </span>
            </ToggleButton>
          </ToggleButtonGroup>
          <MenuTrigger>
            <Button ref={actionsButtonRef} id="library-actions" className="button icon-button" aria-label={t('library.actions.label')}>
              <span className="icon-box" title={t('library.actions.label')}>
                <svg className="icon icon-gear" viewBox="0 0 24 24" width={16} height={16} fill="none" stroke="currentColor" aria-hidden="true">
                  <path d="M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6Z" />
                  <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9c.26.6.77 1.02 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1Z" />
                </svg>
              </span>
            </Button>
            <Popover className="popover menu-popover" placement="bottom end">
              <Menu className="menu" aria-label={t('library.actions.label')} onAction={runAction}>
                <MenuItem id="add-folder" className="menu-item">
                  {t('library.addFolder')}
                </MenuItem>
                <MenuItem id="manage-folders" className="menu-item">
                  {t('sidebar.folders.manage')}
                </MenuItem>
                <MenuItem id="calibre" className="menu-item">
                  {t('library.calibre')}
                </MenuItem>
                <Separator className="menu-separator" />
                <MenuSection>
                  <Header className="menu-section-header">{t('library.annotations.label')}</Header>
                  <MenuItem id="export-jsonld" className="menu-item" data-action="export-jsonld" isDisabled={everything.busy !== null}>
                    {t('library.annotations.export')}
                  </MenuItem>
                  <MenuItem id="import-jsonld" className="menu-item" data-action="import-jsonld" isDisabled={everything.busy !== null}>
                    {t('library.annotations.import')}
                  </MenuItem>
                </MenuSection>
                <Separator className="menu-separator" />
                <MenuItem id="fields" className="menu-item">
                  {t('sidebar.fields')}
                </MenuItem>
                <Separator className="menu-separator" />
                <MenuItem id="preferences" className="menu-item" data-action="preferences">
                  {t('prefs.open')}
                </MenuItem>
              </Menu>
            </Popover>
          </MenuTrigger>
        </div>
      </header>
      {notice !== null ? (
        <p className="library-notice" role="alert">
          {notice}
        </p>
      ) : null}
      <div ref={bodyRef} className={bodyClasses.join(' ')} style={{ '--sidebar-width': `${sidebarWidth}px` } as React.CSSProperties}>
        <SidebarShell
          collapsed={sidebarCollapsed}
          width={sidebarWidth}
          resizeLabel={t('sidebar.resize')}
          scrollRef={sidebarScrollRef}
          resizerRef={resizerRef}
          onPointerDown={startResize}
          onDoubleClick={onResizerDoubleClick}
          onKeyDown={onResizerKeyDown}
        >
          <Sidebar
            info={info}
            folders={folders}
            taxonomy={taxonomy}
            filter={filter}
            showEmptySections={showEmptySections}
            showCounts={showSidebarCounts}
            onFilter={setFilter}
            onChanged={changed}
            notice={setNotice}
            waiting={waiting}
            onManageFolders={openFolders}
            onOpenFields={openFields}
            scrollRef={sidebarScrollRef}
          />
        </SidebarShell>
        {/* Caught on the way down: the items of a grid stop some keys before they reach the shelf. */}
        <main ref={shelf} className="shelf" onKeyDownCapture={putAway}>
          {reviewing ? (
            <ReviewView generation={generation + version} onChanged={changed} />
          ) : inText ? (
            <TextResults query={query} filter={filterArgs(filter)} generation={generation + version} onOpenAt={(row, target) => onOpen(row, target)} />
          ) : books.rows.length === 0 && !books.loading ? (
            <p className="empty">
              {books.error !== null
                ? t('library.smartFailed', { reason: books.error.replace(/^Error:\s*/, '') })
                : query !== ''
                  ? t('library.nothingFound', { query })
                  : info.indexing
                    ? t('library.readingFolder')
                    : t('library.empty')}
            </p>
          ) : (
            <BookShelf
              rows={books.rows}
              loading={books.loading}
              view={view}
              coverWidth={coverWidth}
              selection={selection}
              versions={versions}
              locale={locale}
              onSelect={select}
              onOpenBook={openBook}
              onLoadMore={books.loadMore}
            />
          )}
          <OverlayScrollbar hostRef={shelf} target=".book-grid, .book-table, .review, .text-results" watchKey={shelfWatchKey} />
        </main>
        {/* Escape in the details goes back to the books; Escape there puts the details away. */}
        <InspectorShell open={panel !== null} onKeyDown={onEscape(focusShelf)}>
          {panel}
        </InspectorShell>
      </div>
      {merging !== null ? <MergeDialog works={merging} onClose={() => setMerging(null)} onMerged={merged} /> : null}
      {fieldsOpen ? <FieldsDialog fields={taxonomy.fields} onClose={() => setFieldsOpen(false)} onChanged={changed} /> : null}
      {foldersOpen ? <FoldersDialog roots={info.roots} onClose={() => setFoldersOpen(false)} onAddFolders={addFolders} onRemoveRoot={removeRoot} /> : null}
      {calibre !== null ? (
        <CalibreDialog
          folder={calibre.folder}
          onClose={() => setCalibre(null)}
          onImported={(report) => {
            setNotice(t('library.calibreDone', { imported: report.imported, books: report.books }));
            changed();
            onChanged();
          }}
        />
      ) : null}
    </div>
  );
}
