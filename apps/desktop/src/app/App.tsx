/**
 * The app: unified Acrobat-style tab strip at the top of the Win32 window, the Library permanently
 * mounted in memory (`'home'`) so scroll position, filters, search, selection and Details return in
 * 0ms, and open book tabs managed by `tabs-model.ts`.
 */

import { invoke } from '@tauri-apps/api/core';
import { useCallback, useEffect, useRef, useState } from 'react';

import type { StartupOptions } from '../protocol';
import type { Shortcut } from '../reader/messages';
import { api, type BookRow, type ReaderTarget, type SelectedBook } from './api';
import { exchangeHooks } from './ExchangeSection';
import type { ExchangeAction } from './exchange';
import { useT } from './i18n';
import { FRESH_LIBRARY, type LibraryMemory, type LibraryMenuAction, LibraryView } from './library/LibraryView';
import { useLibrary } from './library/use-books';
import { PreferencesDialog } from './preferences/PreferencesDialog';
import { currentPreferences, updatePreferences } from './preferences/store';
import type { PdfAction } from './reader/annotations/AnnotationUi';
import { type ReaderProbe, ReaderView } from './reader/ReaderView';
import {
  activateTab,
  activeBookTab,
  closeActiveBookTab,
  closeTab,
  closeTabsForFolders,
  createInitialTabsState,
  HOME_TAB_ID,
  mergeWorksInTabs,
  nextTab,
  openBookTab,
  prevTab,
  selectTabByIndex,
  type TabId,
  type TabsState,
  updateWorkTitleInTabs,
} from './tabs-model';
import { TitleBar, type TitleBarMenuAction, useWindowChrome } from './TitleBar';

function shortcutDigitOf(event: KeyboardEvent): number | null {
  if (/^[1-9]$/.test(event.key)) {
    return Number(event.key);
  }
  const match = /^Digit([1-9])$/.exec(event.code ?? '');
  return match !== null ? Number(match[1]) : null;
}

export function App({ options }: { options: StartupOptions }): React.JSX.Element {
  const t = useT();
  const library = useLibrary();
  const [tabs, setTabs] = useState<TabsState>(createInitialTabsState);
  const consumedTargets = useRef(new Map<string, number>());
  const memory = useRef<LibraryMemory>(FRESH_LIBRARY);
  const chosen = useRef<SelectedBook[]>([]);
  const [calibreRequest, setCalibreRequest] = useState<{ folder: string; at: number } | null>(null);
  const rows = useRef<{ rows: BookRow[]; total: number; text: string; folder: number | null }>({
    rows: [],
    total: 0,
    text: '',
    folder: null,
  });
  const probe = useRef<ReaderProbe | null>(null);
  const opened = useRef(0);
  const libraryActionsRef = useRef<((action: LibraryMenuAction) => void) | null>(null);
  const [exchangeBusy, setExchangeBusy] = useState(false);
  const [preferencesOpen, setPreferencesOpen] = useState(false);
  const openPreferences = useCallback(() => setPreferencesOpen(true), []);

  const activeTab = activeBookTab(tabs);
  const activeBook = activeTab?.book ?? null;
  const isHomeActive = activeTab === null;
  const windowTitle = activeTab === null ? 'Musaeum' : t('common.window.bookTitle', { title: activeTab.title });
  const chrome = useWindowChrome(windowTitle);
  const chromeRef = useRef(chrome);
  chromeRef.current = chrome;

  const transitionTabs = useCallback(
    (updater: (prev: TabsState) => TabsState) => {
      setTabs((prev) => {
        const next = updater(prev);
        if (next === prev) {
          return prev;
        }
        if (next.activeTabId !== prev.activeTabId) {
          probe.current = null;
          if (next.activeTabId === HOME_TAB_ID) {
            if (chromeRef.current.fullscreen) {
              void chromeRef.current.setFullscreen(false);
            }
            setCalibreRequest(null);
            library.refresh();
          }
        }
        return next;
      });
    },
    [library],
  );

  const selectTab = useCallback(
    (tabId: TabId) => {
      transitionTabs((prev) => activateTab(prev, tabId));
    },
    [transitionTabs],
  );

  const closeTabById = useCallback(
    (tabId: string) => {
      consumedTargets.current.delete(tabId);
      transitionTabs((prev) => closeTab(prev, tabId));
    },
    [transitionTabs],
  );

  /** Switches back to the Library (`'home'`) tab while keeping the open book's tab in the strip. */
  const goHome = useCallback(() => {
    transitionTabs((prev) => activateTab(prev, HOME_TAB_ID));
  }, [transitionTabs]);

  const handleTitleBarMenuAction = useCallback(
    (action: TitleBarMenuAction) => {
      if (action === 'preferences') {
        setPreferencesOpen(true);
        return;
      }
      if (action === 'manage-folders' || action === 'calibre' || action === 'fields') {
        transitionTabs((prev) => activateTab(prev, HOME_TAB_ID));
      }
      libraryActionsRef.current?.(action);
    },
    [transitionTabs],
  );

  const open = useCallback((next: BookRow, at?: ReaderTarget) => {
    opened.current++;
    probe.current = null;
    setTabs((prev) => openBookTab(prev, next, at ?? null));
  }, []);

  const handleWorkSaved = useCallback((workId: number, nextTitle: string) => {
    setTabs((prev) => updateWorkTitleInTabs(prev, workId, nextTitle));
  }, []);

  const handleRootRemoved = useCallback(
    (removedFolderIds: ReadonlySet<number>) => {
      transitionTabs((prev) => closeTabsForFolders(prev, removedFolderIds));
    },
    [transitionTabs],
  );

  const handleWorksMerged = useCallback((keepWorkId: number, mergedWorkIds: readonly number[]) => {
    setTabs((prev) => mergeWorksInTabs(prev, keepWorkId, mergedWorkIds));
    void api
      .workMetadata(keepWorkId)
      .then((work) => setTabs((prev) => updateWorkTitleInTabs(prev, keepWorkId, work.title)))
      .catch(() => {});
  }, []);

  const handleReaderShortcut = useCallback(
    (key: Shortcut) => {
      if (preferencesOpen || document.querySelector('.modal-overlay') !== null) {
        return;
      }
      if (key === 'preferences') {
        setPreferencesOpen(true);
      } else if (key === 'next-tab') {
        transitionTabs(nextTab);
      } else if (key === 'prev-tab') {
        transitionTabs(prevTab);
      } else if (key === 'close-tab') {
        transitionTabs((prev) => {
          if (prev.activeTabId !== HOME_TAB_ID) {
            consumedTargets.current.delete(prev.activeTabId);
          }
          return closeActiveBookTab(prev);
        });
      } else if (key.startsWith('tab-')) {
        const digit = Number(key.slice(4));
        transitionTabs((prev) => selectTabByIndex(prev, digit));
      }
    },
    [preferencesOpen, transitionTabs],
  );

  // Global shortcuts: Ctrl+, (Preferences), F11 (reader fullscreen), Ctrl+Tab / Ctrl+Shift+Tab,
  // Ctrl+W / Ctrl+F4, and Ctrl+1..Ctrl+9.
  useEffect(() => {
    const onKey = (event: KeyboardEvent): void => {
      if (event.key === ',' && (event.ctrlKey || event.metaKey) && !event.altKey && !event.shiftKey) {
        event.preventDefault();
        setPreferencesOpen(true);
        return;
      }
      if (event.key === 'F11' && !event.ctrlKey && !event.altKey && !event.metaKey && (!isHomeActive || chrome.fullscreen)) {
        event.preventDefault();
        void chrome.toggleFullscreen();
        return;
      }
      if (preferencesOpen || document.querySelector('.modal-overlay') !== null) {
        return;
      }
      if (event.ctrlKey && !event.altKey && !event.metaKey) {
        if (event.key === 'Tab') {
          event.preventDefault();
          transitionTabs(event.shiftKey ? prevTab : nextTab);
          return;
        }
        if (!event.shiftKey && (event.key === 'w' || event.key === 'W' || event.key === 'F4')) {
          event.preventDefault();
          transitionTabs((prev) => {
            if (prev.activeTabId !== HOME_TAB_ID) {
              consumedTargets.current.delete(prev.activeTabId);
            }
            return closeActiveBookTab(prev);
          });
          return;
        }
        if (!event.shiftKey) {
          const digit = shortcutDigitOf(event);
          if (digit !== null) {
            event.preventDefault();
            transitionTabs((prev) => selectTabByIndex(prev, digit));
          }
        }
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [isHomeActive, chrome, preferencesOpen, transitionTabs]);

  const onRows = useCallback(
    (list: BookRow[], total: number, shown: { text: string; folder: number | null; filter: unknown }) => {
      rows.current = { rows: list, total, ...shown };
    },
    [],
  );

  const onSelection = useCallback((books: SelectedBook[]) => {
    chosen.current = books;
  }, []);

  const currentTarget = activeTab?.target ?? null;

  useEffect(() => {
    if (!options.e2e) {
      return;
    }
    // For tests that drive this window from outside, through the WebView's debugging protocol:
    // what the app shows, read exactly; what they do, they do with the input a person would give,
    // except adding a folder, which goes through the system's own dialog.
    (window as unknown as { musaeumApp: unknown }).musaeumApp = {
      view: () => (activeBook === null ? 'library' : 'reader'),
      library: () => library.info,
      rows: () => rows.current,
      folders: () => library.folders,
      opened: () => opened.current,
      addRoot: (path: string) => api.addRoot(path).then((info) => (library.refresh(), info)),
      reader: () => {
        const current = probe.current;
        if (activeBook === null || current === null) {
          return null;
        }
        return {
          kind: current.kind,
          documentId: current.documentId,
          ready: current.ready,
          error: current.error,
          status: current.fixed?.status() ?? null,
          outline: current.outline,
          saved: current.fixed?.saved() ?? null,
          exchangeReport: current.exchangeReport,
          selection: current.selection,
          annotations: current.annotations.map((entry) => ({
            uuid: entry.annotation.uuid,
            kind: entry.annotation.kind,
            color: entry.annotation.color,
            note: entry.annotation.note,
            quote: entry.annotation.quote,
            quoteSource: entry.annotation.quoteSource,
            anchorState: entry.annotation.anchorState,
            progression: entry.annotation.progression,
            pending: entry.pending,
            marks: entry.marks.map((mark) => ({ page: mark.page, start: mark.start, end: mark.end, quads: mark.quads.length })),
            ranges: entry.ranges,
          })),
          flow:
            current.flow === null
              ? null
              : {
                  origin: current.flow.info.origin,
                  mode: current.flow.currentMode,
                  animated: current.flow.isAnimated,
                  relocation: current.flow.relocation,
                  saved: current.flow.savedPosition,
                  restored: current.flow.info.saved,
                  layouts: current.flow.layouts.slice(),
                  sections: current.flow.info.sections.length,
                  cached: current.flow.info.cached,
                  openMs: current.flow.info.openMs,
                },
        };
      },
      inspectFixed: () => probe.current?.fixed?.inspect() ?? null,
      /** The categories, tags, collections, smart collections and custom fields, as the side has them. */
      taxonomy: () => api.taxonomy(),
      /** A work as the editor reads it. */
      workMetadata: (work: number) => api.workMetadata(work),
      /** The books chosen in the library, as the details panel edits them. */
      selection: () => chosen.current,
      /** Opens the Calibre import for this folder, as the folder dialog would have given it. */
      openCalibre: (path: string) => setCalibreRequest({ folder: path, at: Date.now() }),
      /** What waits for review: suggestions and works that may be one. */
      review: () => api.review(),
      fulltextStatus: () => api.fulltextStatus(),
      fulltextSearch: (query: string) => api.fulltextSearch(query),
      /** Where the book open now was asked to be shown, if at a match. */
      target: () => currentTarget,
      /** Where a match of the text search is, as the flowing reader names places. */
      flowPlace: (document: number, unit: number, position: number, length: number) => api.fulltextFlowPlace(document, unit, position, length),
      /** Marks a range of the flowing book open now with a note, as the selection toolbar would. */
      annotateFlow: (start: string, end: string, note: string) =>
        activeBook === null ? Promise.reject(new Error('no book is open')) : api.createFlowAnnotation(activeBook.documentId, start, end, { kind: 'highlight', color: 'yellow', note }),
      /** Shows a page of the open fixed document, as an entry of its outline does. */
      goToPage: (page: number) => probe.current?.fixed?.send({ type: 'go-to-page', page }),
      /** The open fixed document's annotations, whole, with their segments and selectors. */
      fixedAnnotations: () => {
        const session = probe.current?.fixed?.session;
        return session === null || session === undefined ? null : api.fixedAnnotations(session.id);
      },
      /** One of a PDF's actions, with the path the dialog would have given. */
      pdf: (action: string, path?: string) => probe.current?.pdf?.(action as PdfAction, path) ?? null,
      /** What PDFium reads in a file. */
      pdfAnnotations: (path: string) => api.e2ePdfAnnotations(path),
      /**
       * JSON-LD and the reports, run as a button of `surface` runs them (`reader`, `details`, `batch`,
       * `library`), with the path the dialog would have given and, for a report, a fixed time.
       */
      exchange: (surface: string, action: string, path?: string, generatedAt?: number) => {
        const run = exchangeHooks.get(surface);
        // i18n: not for people (only the end-to-end test reads it)
        return run === undefined ? Promise.resolve({ ok: false, error: `no ${surface} here` }) : run(action as ExchangeAction, path, generatedAt);
      },
      /** Each page's text of a PDF, as PDFium reads it. */
      pdfText: (path: string) => api.e2ePdfText(path),
      resizeWindow: (width: number, height: number) => invoke('e2e_resize_window', { width, height }),
      openedLinks: () => invoke<string[]>('e2e_opened_links'),
      /** A page's characters as the compositor gets them, as bytes, for a test to aim a drag with. */
      pageChars: async (page: number) => {
        const session = probe.current?.fixed?.session;
        if (session === null || session === undefined) {
          return null;
        }
        const bytes = await invoke<ArrayBuffer>('page_chars', { session: session.id, page });
        return Array.from(new Uint8Array(bytes));
      },
      /** A page's line model as the host has it now: empty while it is being traced. */
      lineModel: async (page: number) => {
        const session = probe.current?.fixed?.session;
        if (session === null || session === undefined) {
          return null;
        }
        const bytes = await invoke<ArrayBuffer>('line_model', { session: session.id, page });
        return Array.from(new Uint8Array(bytes));
      },
      /** Every preference, as the interface applies them. */
      preferences: () => currentPreferences(),
      /** Changes preferences as the Preferences dialog would. */
      setPreferences: (patch: Record<string, unknown>) => updatePreferences(patch),
      /** Opens the Preferences, as Ctrl+, does. */
      openPreferences: () => setPreferencesOpen(true),
      /** What the open library's cache holds. */
      cacheReport: () => api.cacheReport(),
      rects: () => ({
        canvas: document.querySelector('.reader-canvas')?.getBoundingClientRect().toJSON() ?? null,
        frame: document.querySelector('.reader-frame')?.getBoundingClientRect().toJSON() ?? null,
        dpr: devicePixelRatio,
      }),
    };
  }, [options.e2e, activeBook, library, currentTarget]);

  const unconsumedTarget =
    activeTab !== null &&
    activeTab.target !== null &&
    consumedTargets.current.get(activeTab.id) !== activeTab.targetSeq
      ? activeTab.target
      : null;

  return (
    <div className="app">
      <TitleBar
        tabs={tabs.bookTabs}
        activeTabId={tabs.activeTabId}
        libraryOpen={library.info?.open === true}
        exchangeBusy={exchangeBusy}
        maximized={chrome.maximized}
        hidden={chrome.fullscreen}
        onSelectTab={selectTab}
        onCloseTab={closeTabById}
        onMenuAction={handleTitleBarMenuAction}
        onOpenPreferences={openPreferences}
        onMinimize={chrome.minimize}
        onToggleMaximize={chrome.toggleMaximize}
        onClose={chrome.close}
      />
      <div className="app-stage">
        <div
          className="library-host"
          hidden={!isHomeActive}
          inert={!isHomeActive || undefined}
          aria-hidden={!isHomeActive || undefined}
        >
          <LibraryView
            active={isHomeActive}
            info={library.info}
            folders={library.folders}
            generation={library.generation}
            onChanged={library.refresh}
            onOpen={open}
            memory={memory}
            onRows={onRows}
            onSelection={onSelection}
            calibreRequest={calibreRequest}
            onPreferences={openPreferences}
            actionsRef={libraryActionsRef}
            onExchangeBusyChange={setExchangeBusy}
            onWorkSaved={handleWorkSaved}
            onRootRemoved={handleRootRemoved}
            onWorksMerged={handleWorksMerged}
          />
        </div>
        {activeTab !== null && activeBook !== null ? (
          <ReaderView
            key={activeTab.id}
            book={activeBook}
            options={options}
            onBack={goHome}
            onSessionClosed={library.refresh}
            probe={probe}
            target={unconsumedTarget}
            targetSeq={activeTab.targetSeq}
            onTargetReached={() => {
              consumedTargets.current.set(activeTab.id, activeTab.targetSeq);
            }}
            onPreferences={openPreferences}
            isFullscreen={chrome.fullscreen}
            onToggleFullscreen={() => void chrome.toggleFullscreen()}
            onReaderShortcut={handleReaderShortcut}
          />
        ) : null}
      </div>
      {preferencesOpen ? <PreferencesDialog onClose={() => setPreferencesOpen(false)} /> : null}
    </div>
  );
}
