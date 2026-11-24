/**
 * What a search finds in the books' text: each book with the pages or sections where every word
 * asked for is, a stretch of text around the first match with the matches marked, and a click
 * that opens the book there, on the page or at the very place in a flowing book. Above them, how
 * far the index is: the books are read into it in the background, a few at a time.
 */

import { listen } from '@tauri-apps/api/event';
import { useEffect, useState } from 'react';
import { Button } from 'react-aria-components';

import { api, type BookHit, type BookRow, type BrowseFilter, type FullTextResults, type FullTextStatus, type ReaderTarget, type UnitHit } from '../api';
import { t, tn, useT } from '../i18n';
import { reasonOf } from './dialogs';

/** The snippet, with what matched marked. */
function Snippet({ hit }: { hit: UnitHit }): React.JSX.Element {
  const parts: React.ReactNode[] = [];
  let at = 0;
  hit.marks.forEach(([from, to], index) => {
    if (from > at) {
      parts.push(hit.snippet.slice(at, from));
    }
    parts.push(<mark key={index}>{hit.snippet.slice(from, to)}</mark>);
    at = to;
  });
  parts.push(hit.snippet.slice(at));
  return <span className="snippet">{parts}</span>;
}

function unitLabel(hit: UnitHit): string {
  if (hit.kind === 'page') {
    const page = t('library.text.page', { page: hit.index + 1 });
    return hit.source === 'ocr' ? `${page} · ${t('library.text.ocr')}` : page;
  }
  return hit.label ?? t('library.text.section', { number: hit.index + 1 });
}

/** Where to open a book for a match. */
export function targetOf(row: BookRow, hit: UnitHit): ReaderTarget {
  return hit.kind === 'page'
    ? { kind: 'page', page: hit.index, position: hit.position }
    : { kind: 'flow', document: row.documentId, unit: hit.index, position: hit.position, length: hit.matched.length };
}

export function statusLine(status: FullTextStatus): string {
  const read = status.indexed + status.empty + status.failed;
  const all = read + status.pending;
  if (status.pending === 0) {
    return tn('library.text.indexed', status.indexed);
  }
  const progress = t('library.text.progress', { read, total: all });
  return status.running && status.current !== null ? `${progress} ${t('library.text.reading', { title: status.current })}` : progress;
}

export interface TextResultsProps {
  query: string;
  /** What the side has chosen: only its books are shown. */
  filter: BrowseFilter;
  generation: number;
  onOpenAt(row: BookRow, target: ReaderTarget): void;
}

interface Found {
  results: FullTextResults;
  rows: Map<number, BookRow>;
}

export function TextResults({ query, filter, generation, onOpenAt }: TextResultsProps): React.JSX.Element {
  const t = useT();
  const [status, setStatus] = useState<FullTextStatus | null>(null);
  const [found, setFound] = useState<Found | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [progress, setProgress] = useState(0);

  useEffect(() => {
    const stop = listen('fulltext-progress', () => setProgress((value) => value + 1));
    return () => {
      void stop.then((unlisten) => unlisten());
    };
  }, []);
  useEffect(() => {
    let current = true;
    api
      .fulltextStatus()
      .then((loaded) => current && setStatus(loaded))
      .catch(() => {});
    return () => {
      current = false;
    };
  }, [progress, generation]);
  // Searched again as more books are read, but not more often than the index grows.
  const read = status === null ? 0 : status.indexed + status.empty;
  const within = JSON.stringify(filter);
  useEffect(() => {
    let current = true;
    setError(null);
    (async () => {
      const chosen = JSON.parse(within) as BrowseFilter;
      const narrowed = Object.values(chosen).some((value) => value !== null && value !== undefined);
      // Under a filter, more books are asked for, since the filter leaves only some of them.
      const results = await api.fulltextSearch(query, narrowed ? 200 : 50);
      const works = [...new Set(results.books.flatMap((book) => book.works))];
      const page = works.length === 0 ? { rows: [] as BookRow[] } : await api.browse({ ...chosen, works, text: '', offset: 0, limit: 500 });
      const rows = new Map<number, BookRow>(page.rows.map((row) => [row.documentId, row]));
      if (current) {
        setFound({ results, rows });
      }
    })().catch((reason: unknown) => current && setError(reasonOf(reason)));
    return () => {
      current = false;
    };
  }, [query, within, read, generation]);

  const rowOf = (book: BookHit): BookRow | undefined => {
    for (const document of book.documents) {
      const row = found?.rows.get(document);
      if (row !== undefined) {
        return row;
      }
    }
    return undefined;
  };

  return (
    <div className="text-results" aria-label={t('library.text.label')}>
      {status !== null ? (
        <p className="muted fulltext-status" aria-live="polite" data-pending={status.pending}>
          {statusLine(status)}
        </p>
      ) : null}
      {error !== null ? (
        <p className="form-error" role="alert">
          {error}
        </p>
      ) : null}
      {found !== null && found.results.books.every((book) => rowOf(book) === undefined) ? <p className="empty">{t('library.text.nothing', { query })}</p> : null}
      {found?.results.books.map((book) => {
        const row = rowOf(book);
        if (row === undefined) {
          return null;
        }
        return (
          <article key={book.blake3} className="text-book" data-document={row.documentId}>
            <h3 className="text-book-title">
              {row.title} <span className="muted">{row.creators ?? ''}</span>
            </h3>
            <ul className="text-hits">
              {book.units.map((hit) => (
                <li key={`${hit.kind}-${hit.index}`}>
                  <Button className="text-hit" data-kind={hit.kind} data-unit={hit.index} onPress={() => onOpenAt(row, targetOf(row, hit))}>
                    <span className="text-hit-where">{unitLabel(hit)}</span>
                    <Snippet hit={hit} />
                  </Button>
                </li>
              ))}
            </ul>
          </article>
        );
      })}
    </div>
  );
}
