/**
 * The app: the library, and a book open in the reader over it. Going back to the library closes
 * the book, keeping where the reader was, and leaves the library as it was left.
 */

import { invoke } from '@tauri-apps/api/core';
import { useCallback, useEffect, useRef, useState } from 'react';

import type { StartupOptions } from '../protocol';
import { api, type BookRow } from './api';
import { LibraryView } from './library/LibraryView';
import { useLibrary } from './library/use-books';
import { type ReaderProbe, ReaderView } from './reader/ReaderView';

export function App({ options }: { options: StartupOptions }): React.JSX.Element {
  const library = useLibrary();
  const [book, setBook] = useState<BookRow | null>(null);
  const rows = useRef<{ rows: BookRow[]; total: number; text: string; folder: number | null }>({
    rows: [],
    total: 0,
    text: '',
    folder: null,
  });
  const probe = useRef<ReaderProbe | null>(null);
  const opened = useRef(0);

  const onRows = useCallback(
    (list: BookRow[], total: number, shown: { text: string; folder: number | null }) => {
      rows.current = { rows: list, total, ...shown };
    },
    [],
  );

  const back = useCallback(() => {
    probe.current = null;
    setBook(null);
    library.refresh();
  }, [library]);

  const open = useCallback((next: BookRow) => {
    opened.current++;
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
          selection: current.selection,
          annotations: current.annotations.map((entry) => ({
            uuid: entry.annotation.uuid,
            kind: entry.annotation.kind,
            color: entry.annotation.color,
            note: entry.annotation.note,
            quote: entry.annotation.quote,
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
      rects: () => ({
        canvas: document.querySelector('.reader-canvas')?.getBoundingClientRect().toJSON() ?? null,
        frame: document.querySelector('.reader-frame')?.getBoundingClientRect().toJSON() ?? null,
        dpr: devicePixelRatio,
      }),
    };
  }, [options.e2e, book, library]);

  return (
    <div className="app">
      {book === null ? (
        <LibraryView
          info={library.info}
          folders={library.folders}
          generation={library.generation}
          onChanged={library.refresh}
          onOpen={open}
          onRows={onRows}
        />
      ) : (
        <ReaderView key={`${book.documentId}-${opened.current}`} book={book} options={options} onBack={back} probe={probe} />
      )}
    </div>
  );
}
