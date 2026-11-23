/**
 * What waits for the person's word, and the merge of works. The library suggests; nothing here
 * happens without a click: two documents that may be one (a file that came back changed, one that
 * took another's place with other identifiers in it), two works that may be one book (the same
 * identifier, the same title by the same author). A merge keeps one work, chosen here, and moves
 * the others' documents into it whole, with their annotations, reading positions and history,
 * and their categories, tags and collections.
 */

import { useCallback, useEffect, useState } from 'react';
import { Button, Radio, RadioGroup } from 'react-aria-components';

import { api, type BookRow, type DuplicatePair, type Review, type SuggestionReview, type WorkMetadata } from '../api';
import { Frame, reasonOf } from './dialogs';

const NO_FILTER = { folder: null, category: null, tag: null, collection: null, smart: null };

/** The books of these works, as the listing shows them. */
async function booksOf(works: number[]): Promise<BookRow[]> {
  const page = await api.browse({ ...NO_FILTER, works, text: '', offset: 0, limit: 200 });
  return page.rows;
}

function BookLine({ row }: { row: BookRow }): React.JSX.Element {
  return (
    <div className="review-book" data-document={row.documentId}>
      <span className="book-title">{row.title}</span>
      <span className="muted">
        {row.creators ?? 'sem autor'} · {row.format.toUpperCase()} · {row.path.split(/[\\/]/).at(-1)}
      </span>
    </div>
  );
}

function reasonText(pair: DuplicatePair): string {
  return pair.reason.kind === 'same_identifier' ? `O mesmo ${pair.reason.scheme.toUpperCase()}: ${pair.reason.value}` : 'O mesmo título e o mesmo autor';
}

const SUGGESTION_KINDS: Record<SuggestionReview['kind'], string> = {
  same_internal_id: 'O mesmo identificador interno, com outro conteúdo',
  same_file_id: 'O mesmo arquivo do sistema, com outro conteúdo',
  same_path: 'Um arquivo no lugar de outro que sumiu',
  same_fingerprint: 'O mesmo começo de um arquivo que sumiu',
};

export interface MergeDialogProps {
  works: number[];
  onClose(): void;
  onMerged(keep: number): void;
}

/** Choosing the work that stays, and merging the others into it. */
export function MergeDialog({ works, onClose, onMerged }: MergeDialogProps): React.JSX.Element {
  const [loaded, setLoaded] = useState<WorkMetadata[] | null>(null);
  const [keep, setKeep] = useState<string>(String(Math.min(...works)));
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const key = works.join(',');
  useEffect(() => {
    let current = true;
    Promise.all(works.map((work) => api.workMetadata(work)))
      .then((found) => current && setLoaded(found))
      .catch((reason: unknown) => current && setError(reasonOf(reason)));
    return () => {
      current = false;
    };
    // `key` stands for `works`.
  }, [key]);
  const merge = (): void => {
    const kept = Number(keep);
    setBusy(true);
    setError(null);
    api
      .mergeWorks(kept, works.filter((work) => work !== kept))
      .then(() => onMerged(kept))
      .catch((reason: unknown) => setError(reasonOf(reason)))
      .finally(() => setBusy(false));
  };
  return (
    <Frame title="Fundir em um livro" onClose={onClose} onSubmit={merge} submitLabel="Fundir" error={error} busy={busy || loaded === null} wide>
      <p>
        Os {works.length} livros ficam um só. Os arquivos de todos passam para o que fica, cada um com as suas anotações, onde a leitura parou e o seu histórico; categorias, etiquetas e coleções
        também. O que o que fica não tem é tirado dos outros. Uma fusão não se desfaz.
      </p>
      {loaded === null ? (
        <p className="muted">Lendo…</p>
      ) : (
        <RadioGroup aria-label="O livro que fica" className="merge-choices" value={keep} onChange={setKeep}>
          {loaded.map((work) => (
            <Radio key={work.id} value={String(work.id)} className="merge-choice" data-work={work.id}>
              <span className="radio-dot" aria-hidden="true" />
              <span className="merge-what">
                <span className="book-title">{work.title}</span>
                <span className="muted">
                  {work.creators.map((credit) => credit.name).join('; ') || 'sem autor'} · {work.formats.map((format) => format.toUpperCase()).join(', ')}
                  {work.year !== null ? ` · ${work.year}` : ''}
                </span>
              </span>
            </Radio>
          ))}
        </RadioGroup>
      )}
    </Frame>
  );
}

export interface ReviewViewProps {
  /** Something was merged or turned down: the books and the side are read again. */
  onChanged(): void;
  generation: number;
}

interface Loaded {
  review: Review;
  books: Map<number, BookRow[]>;
}

export function ReviewView({ onChanged, generation }: ReviewViewProps): React.JSX.Element {
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [merging, setMerging] = useState<number[] | null>(null);
  const [version, setVersion] = useState(0);
  useEffect(() => {
    let current = true;
    (async () => {
      const review = await api.review();
      const works = new Set<number>();
      for (const suggestion of review.suggestions) {
        works.add(suggestion.workId);
        works.add(suggestion.candidateWorkId);
      }
      for (const pair of review.duplicates) {
        works.add(pair.works[0]);
        works.add(pair.works[1]);
      }
      const rows = works.size === 0 ? [] : await booksOf([...works]);
      const books = new Map<number, BookRow[]>();
      for (const row of rows) {
        books.set(row.workId, [...(books.get(row.workId) ?? []), row]);
      }
      if (current) {
        setLoaded({ review, books });
      }
    })().catch((reason: unknown) => current && setError(reasonOf(reason)));
    return () => {
      current = false;
    };
  }, [generation, version]);
  const done = useCallback(() => {
    setVersion((value) => value + 1);
    onChanged();
  }, [onChanged]);
  const act = (work: Promise<unknown>): void => {
    setError(null);
    work.then(done).catch((reason: unknown) => setError(reasonOf(reason)));
  };
  if (loaded === null) {
    return <p className="empty">{error ?? 'Lendo…'}</p>;
  }
  const { review, books } = loaded;
  const lines = (work: number): React.JSX.Element[] => (books.get(work) ?? []).map((row) => <BookLine key={row.documentId} row={row} />);
  return (
    <div className="review" aria-label="Revisar">
      {error !== null ? (
        <p className="form-error" role="alert">
          {error}
        </p>
      ) : null}
      {review.suggestions.length === 0 && review.duplicates.length === 0 ? <p className="empty">Nada esperando revisão.</p> : null}
      {review.duplicates.length > 0 ? (
        <section className="review-section" aria-label="Livros que podem ser um só">
          <h2>Livros que podem ser um só</h2>
          {review.duplicates.map((pair) => (
            <article key={pair.works.join('-')} className="review-item" data-pair={pair.works.join('-')}>
              <p className="review-reason">{reasonText(pair)}</p>
              <div className="review-books">
                {lines(pair.works[0])}
                {lines(pair.works[1])}
              </div>
              <div className="modal-actions">
                <Button className="button" onPress={() => act(api.dismissDuplicate(pair.works[0], pair.works[1]))}>
                  São livros diferentes
                </Button>
                <Button className="button primary" onPress={() => setMerging([...pair.works])}>
                  Fundir…
                </Button>
              </div>
            </article>
          ))}
        </section>
      ) : null}
      {review.suggestions.length > 0 ? (
        <section className="review-section" aria-label="Arquivos que podem ser o mesmo">
          <h2>Arquivos que podem ser o mesmo</h2>
          {review.suggestions.map((suggestion) => (
            <article key={suggestion.id} className="review-item" data-suggestion={suggestion.id}>
              <p className="review-reason">{SUGGESTION_KINDS[suggestion.kind]}</p>
              <p className="muted">{suggestion.evidence}</p>
              <div className="review-books">
                {lines(suggestion.candidateWorkId)}
                {lines(suggestion.workId)}
              </div>
              <div className="modal-actions">
                <Button className="button" onPress={() => act(api.dismissSuggestion(suggestion.id))}>
                  São diferentes
                </Button>
                <Button className="button primary" onPress={() => act(api.acceptSuggestion(suggestion.id))}>
                  É o mesmo arquivo
                </Button>
              </div>
            </article>
          ))}
        </section>
      ) : null}
      {merging !== null ? (
        <MergeDialog
          works={merging}
          onClose={() => setMerging(null)}
          onMerged={() => {
            setMerging(null);
            done();
          }}
        />
      ) : null}
    </div>
  );
}
