/**
 * The app: the library, and a book open in the reader over it. Going back to the library closes
 * the book, keeping where the reader was, and shows the library as it was left: the same search,
 * in the titles or the text, the same side chosen, the same view.
 */

import { invoke } from '@tauri-apps/api/core';
import { useCallback, useEffect, useRef, useState } from 'react';

import type { StartupOptions } from '../protocol';
import { api, type BookRow, type ReaderTarget, type SelectedBook } from './api';
import { FRESH_LIBRARY, type LibraryMemory, LibraryView } from './library/LibraryView';
import { useLibrary } from './library/use-books';
import { exchangeHooks } from './ExchangeSection';
import type { ExchangeAction } from './exchange';
import type { PdfAction } from './reader/annotations/AnnotationUi';
import { type ReaderProbe, ReaderView } from './reader/ReaderView';
import { PreferencesDialog } from './preferences/PreferencesDialog';
import { currentPreferences, updatePreferences } from './preferences/store';
import { TitleBar, useWindowChrome } from './TitleBar';

export function App({ options }: { options: StartupOptions }): React.JSX.Element {
  const library = useLibrary();
  const [book, setBook] = useState<BookRow | null>(null);
  const [target, setTarget] = useState<ReaderTarget | null>(null);
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
  const [preferencesOpen, setPreferencesOpen] = useState(false);
  const openPreferences = useCallback(() => setPreferencesOpen(true), []);
  const windowTitle = book === null ? 'Musaeum' : book.title;
  const chrome = useWindowChrome(windowTitle);

  // Ctrl+, opens the Preferences from anywhere; F11 toggles fullscreen in the reader.
  useEffect(() => {
    const onKey = (event: KeyboardEvent): void => {
      if (event.key === ',' && (event.ctrlKey || event.metaKey) && !event.altKey && !event.shiftKey) {
        event.preventDefault();
        setPreferencesOpen(true);
        return;
      }
      if (event.key === 'F11' && !event.ctrlKey && !event.altKey && !event.metaKey && (book !== null || chrome.fullscreen)) {
        event.preventDefault();
        void chrome.toggleFullscreen();
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [book, chrome]);

  const onRows = useCallback(
    (list: BookRow[], total: number, shown: { text: string; folder: number | null; filter: unknown }) => {
      rows.current = { rows: list, total, ...shown };
    },
    [],
  );

  const onSelection = useCallback((books: SelectedBook[]) => {
    chosen.current = books;
  }, []);

  const back = useCallback(() => {
    probe.current = null;
    if (chrome.fullscreen) {
      void chrome.setFullscreen(false);
    }
    setBook(null);
    setTarget(null);
    setCalibreRequest(null);
    library.refresh();
  }, [chrome, library]);

  const open = useCallback((next: BookRow, at?: ReaderTarget) => {
    opened.current++;
    setTarget(at ?? null);
    setBook(next);
  }, []);

  useEffect(() => {
    if (!options.e2e) {
      return;
    }
    // For tests that drive this window from outside, through the WebView's debugging protocol:
    // what the app shows, read exactly; what they do, they do with the input a person would give,
    // except adding a folder, which goes through the system's own dialog.
    (window as unknown as { musaeumApp: unknown }).musaeumApp = {
      view: () => (book === null ? 'library' : 'reader'),
      library: () => library.info,
      rows: () => rows.current,
      folders: () => library.folders,
      opened: () => opened.current,
      addRoot: (path: string) => api.addRoot(path).then((info) => (library.refresh(), info)),
      reader: () => {
        const current = probe.current;
        if (current === null) {
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
      target: () => target,
      /** Where a match of the text search is, as the flowing reader names places. */
      flowPlace: (document: number, unit: number, position: number, length: number) => api.fulltextFlowPlace(document, unit, position, length),
      /** Marks a range of the flowing book open now with a note, as the selection toolbar would. */
      annotateFlow: (start: string, end: string, note: string) =>
        book === null ? Promise.reject(new Error('no book is open')) : api.createFlowAnnotation(book.documentId, start, end, { kind: 'highlight', color: 'yellow', note }),
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
  }, [options.e2e, book, library, target]);

  return (
    <div className="app">
      <TitleBar
        title={windowTitle}
        maximized={chrome.maximized}
        hidden={chrome.fullscreen}
        onMinimize={chrome.minimize}
        onToggleMaximize={chrome.toggleMaximize}
        onClose={chrome.close}
      />
      {book === null ? (
        <LibraryView
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
        />
      ) : (
        <ReaderView
          key={`${book.documentId}-${opened.current}`}
          book={book}
          options={options}
          onBack={back}
          probe={probe}
          target={target}
          onPreferences={openPreferences}
          isFullscreen={chrome.fullscreen}
          onToggleFullscreen={() => void chrome.toggleFullscreen()}
        />
      )}
      {preferencesOpen ? <PreferencesDialog onClose={() => setPreferencesOpen(false)} /> : null}
    </div>
  );
}
