/** The books the library view lists, read page by page, and refreshed when the library changes. */

import { listen } from '@tauri-apps/api/event';
import { useCallback, useEffect, useRef, useState } from 'react';

import { api, type BookRow, type FolderEntry, type LibraryInfo } from '../api';

const PAGE = 400;
/** How long the library has to stay quiet before what is shown is read again. */
const REFRESH_AFTER_MS = 400;

export interface BookList {
  rows: BookRow[];
  total: number;
  loading: boolean;
  /** The search and the folder the rows shown are for, once they arrived. */
  shown: { text: string; folder: number | null };
  loadMore(): void;
}

/** The books matching `text` in `folder`, best first when searching, by title otherwise. */
export function useBooks(text: string, folder: number | null, generation: number): BookList {
  const [rows, setRows] = useState<BookRow[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(false);
  const [shown, setShown] = useState<{ text: string; folder: number | null }>({ text: '', folder: null });
  const request = useRef(0);
  const loaded = useRef<BookRow[]>([]);

  useEffect(() => {
    const id = ++request.current;
    setLoading(true);
    api
      .browse({ text, folder, offset: 0, limit: PAGE })
      .then((page) => {
        if (id === request.current) {
          loaded.current = page.rows;
          setRows(page.rows);
          setTotal(page.total);
          setShown({ text, folder });
        }
      })
      .catch((error: unknown) => console.warn('the library could not be read', error))
      .finally(() => {
        if (id === request.current) {
          setLoading(false);
        }
      });
  }, [text, folder, generation]);

  const loadMore = useCallback(() => {
    if (loading || loaded.current.length >= total) {
      return;
    }
    const id = request.current;
    setLoading(true);
    api
      .browse({ text, folder, offset: loaded.current.length, limit: PAGE })
      .then((page) => {
        if (id === request.current) {
          loaded.current = loaded.current.concat(page.rows);
          setRows(loaded.current);
          setTotal(page.total);
        }
      })
      .catch((error: unknown) => console.warn('the library could not be read', error))
      .finally(() => {
        if (id === request.current) {
          setLoading(false);
        }
      });
  }, [text, folder, total, loading]);

  return { rows, total, loading, shown, loadMore };
}

/** What the library is, and a number that changes whenever what it holds may have. */
export function useLibrary(): {
  info: LibraryInfo | null;
  folders: FolderEntry[];
  generation: number;
  refresh(): void;
} {
  const [info, setInfo] = useState<LibraryInfo | null>(null);
  const [folders, setFolders] = useState<FolderEntry[]>([]);
  const [generation, setGeneration] = useState(0);
  const timer = useRef(0);

  const refresh = useCallback(() => {
    void api.libraryInfo().then((next) => {
      setInfo(next);
      if (next.open) {
        void api.folders().then(setFolders).catch(() => setFolders([]));
      }
      setGeneration((value) => value + 1);
    });
  }, []);

  useEffect(() => {
    refresh();
    const unlisten = listen('library-changed', () => {
      clearTimeout(timer.current);
      timer.current = window.setTimeout(refresh, REFRESH_AFTER_MS);
    });
    return () => {
      clearTimeout(timer.current);
      void unlisten.then((stop) => stop());
    };
  }, [refresh]);

  return { info, folders, generation, refresh };
}

/** A version per cover, raised when the host announces a new one, so the image is asked again. */
export function useCoverVersions(): Map<string, number> {
  const [versions, setVersions] = useState(() => new Map<string, number>());
  useEffect(() => {
    const unlisten = listen<string[]>('covers-changed', (event) => {
      setVersions((current) => {
        const next = new Map(current);
        for (const hash of event.payload) {
          next.set(hash, (next.get(hash) ?? 0) + 1);
        }
        return next;
      });
    });
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, []);
  return versions;
}
