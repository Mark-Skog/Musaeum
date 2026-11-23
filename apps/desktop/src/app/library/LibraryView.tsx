/**
 * The library: every book in the folders it follows, as covers or as a table, found by a search
 * as it is typed, in the titles or in the books' text, or by what the side chooses: a folder, a
 * category, a tag, a collection, a smart collection, or what waits for review. A click chooses a
 * book and shows its details beside the books, to edit; Ctrl+click, Shift+click and Ctrl+A choose
 * several, and the details then change all of them at once. A double click or Enter opens a book.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Button,
  Cell,
  Collection,
  Column,
  GridList,
  GridListItem,
  GridListLoadMoreItem,
  GridLayout,
  Input,
  Row,
  SearchField,
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
import { BatchPanel } from './BatchPanel';
import { CalibreDialog } from './CalibreDialog';
import { reasonOf } from './dialogs';
import { type CollectionPlace, Inspector } from './Inspector';
import { MergeDialog, ReviewView } from './Review';
import { Sidebar } from './Sidebar';
import { type Filter, filterArgs } from './taxonomy-model';
import { TextResults } from './TextResults';
import { type Shown, useBooks, useCoverVersions, useTaxonomy } from './use-books';

/** How long typing has to pause before the search runs. Short: the search answers in milliseconds. */
const SEARCH_AFTER_MS = 90;

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
function Cover({ book, version }: { book: BookRow; version: number }): React.JSX.Element {
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
}

function Progress({ value }: { value: number | null }): React.JSX.Element | null {
  if (value === null || value <= 0) {
    return null;
  }
  return (
    <div className="progress" aria-label={`Lido ${Math.round(value * 100)}%`}>
      <div className="progress-fill" style={{ width: `${Math.max(2, value * 100)}%` }} />
    </div>
  );
}

function Welcome({ info, onChanged }: { info: LibraryInfo | null; onChanged(): void }): React.JSX.Element {
  const [error, setError] = useState<string | null>(info?.error ?? null);
  const run = (action: () => Promise<unknown>): void => {
    setError(null);
    action()
      .then(onChanged)
      .catch((reason: unknown) => setError(String(reason)));
  };
  return (
    <div className="welcome">
      <h1>Musaeum</h1>
      <p>Escolha a pasta onde estão os seus livros. Eles ficam onde estão: a biblioteca só os lê.</p>
      <div className="welcome-actions">
        <Button className="button primary" onPress={() => run(api.chooseRoot)}>
          Escolher pasta de livros…
        </Button>
        <Button className="button" onPress={() => run(api.chooseExisting)}>
          Abrir biblioteca existente…
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
}

export function LibraryView({ info, folders, generation, onChanged, onOpen, memory, onRows, onSelection, calibreRequest = null }: LibraryViewProps): React.JSX.Element {
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
      .catch(() => {});
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

  if (info === null) {
    return <div className="loading">Abrindo a biblioteca…</div>;
  }
  if (!info.open) {
    return <Welcome info={info} onChanged={onChanged} />;
  }

  const openBook = (key: Key): void => {
    const book = byId.get(Number(key));
    if (book !== undefined) {
      onOpen(book);
    }
  };
  const last = given.current;
  const selection: Selection = last !== null && last.size === chosen.size && [...last].every((key) => chosen.has(Number(key))) ? last : new Set<Key>(chosen.keys());
  const select = (keys: Selection): void => {
    const asked = ++allAsked.current;
    given.current = keys === 'all' ? null : keys;
    if (keys === 'all') {
      // Every book the list holds, not only those read so far: the host lists them.
      const listed = byId;
      api
        .selectAll({ ...filterArgs(filter), text: inText ? '' : query })
        .then((all) => {
          if (asked === allAsked.current) {
            setChosen(new Map(all.map((book) => [book.documentId, { ...book, row: listed.get(book.documentId) ?? null }])));
          }
        })
        .catch((reason: unknown) => setNotice(reasonOf(reason)));
      return;
    }
    setChosen((before) => {
      const next = new Map<number, Chosen>();
      for (const key of keys) {
        const id = Number(key);
        const row = byId.get(id);
        const known = before.get(id);
        if (row !== undefined) {
          next.set(id, { documentId: id, workId: row.workId, row });
        } else if (known !== undefined) {
          next.set(id, known);
        }
      }
      return next;
    });
  };
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
  const works = [...new Set(current.map((book) => book.workId))];
  // One work chosen (one book, or its formats) is edited as itself; more, all at once.
  const single = works.length === 1 ? (current.find((book) => book.row !== null)?.row ?? null) : null;
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
  const reviewing = filter.kind === 'review';
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
    setNotice('Os livros foram fundidos em um.');
    changed();
  };
  const count = `${books.total} ${books.total === 1 ? 'livro' : 'livros'}`;

  return (
    <div className="library">
      <header className="toolbar">
        <h1 className="toolbar-title">Biblioteca</h1>
        <SearchField className="search" aria-label="Buscar livros" value={typed} onChange={setTyped} onSubmit={setQuery}>
          <Input placeholder={searchIn === 'text' ? 'Buscar no texto dos livros' : 'Buscar por título, autor ou série'} />
          <Button className="search-clear" aria-label="Limpar busca">
            ×
          </Button>
        </SearchField>
        {typed !== '' || searchIn === 'text' ? (
          <ToggleButtonGroup
            className="segmented"
            aria-label="Buscar em"
            selectionMode="single"
            disallowEmptySelection
            selectedKeys={[searchIn]}
            onSelectionChange={(keys) => setSearchIn([...keys][0] as SearchIn)}
          >
            <ToggleButton id="titles" className="segment">
              Títulos
            </ToggleButton>
            <ToggleButton id="text" className="segment">
              No texto
            </ToggleButton>
          </ToggleButtonGroup>
        ) : null}
        <ToggleButtonGroup
          className="segmented"
          aria-label="Modo de exibição"
          selectionMode="single"
          disallowEmptySelection
          selectedKeys={[view]}
          onSelectionChange={(keys) => setView([...keys][0] as 'grid' | 'table')}
        >
          <ToggleButton id="grid" className="segment">
            Capas
          </ToggleButton>
          <ToggleButton id="table" className="segment">
            Lista
          </ToggleButton>
        </ToggleButtonGroup>
        <Button className="button" onPress={() => void api.chooseRoot().then(onChanged)}>
          Adicionar pasta…
        </Button>
        <Button className="button" onPress={() => setCalibre({ folder: null })}>
          Importar do Calibre…
        </Button>
        <span className="toolbar-status" aria-live="polite">
          {chosen.size > 1 ? `${chosen.size} escolhidos · ` : ''}
          {books.total === info.books || filtered ? count : `${books.total} de ${info.books} livros`}
          {info.indexing ? ' · lendo a pasta…' : ''}
        </span>
      </header>
      {notice !== null ? (
        <p className="library-notice" role="alert">
          {notice}
        </p>
      ) : null}
      <div className={panel === null ? 'library-body' : 'library-body with-inspector'}>
        <Sidebar info={info} folders={folders} taxonomy={taxonomy} filter={filter} onFilter={setFilter} onChanged={changed} notice={setNotice} waiting={waiting} />
        {/* Caught on the way down: the items of a grid stop some keys before they reach the shelf. */}
        <main className="shelf" onKeyDownCapture={putAway}>
          {reviewing ? (
            <ReviewView generation={generation + version} onChanged={changed} />
          ) : inText ? (
            <TextResults query={query} filter={filterArgs(filter)} generation={generation + version} onOpenAt={(row, target) => onOpen(row, target)} />
          ) : books.rows.length === 0 && !books.loading ? (
            <p className="empty">
              {books.error !== null
                ? `Esta coleção inteligente não pôde ser lida: ${books.error.replace(/^Error:\s*/, '')}`
                : query !== ''
                  ? `Nada encontrado para “${query}”.`
                  : info.indexing
                    ? 'Lendo a pasta…'
                    : 'Nenhum livro aqui.'}
            </p>
          ) : view === 'grid' ? (
            <Virtualizer
              layout={GridLayout}
              layoutOptions={{
                minItemSize: new Size(148, 268),
                maxItemSize: new Size(196, 330),
                minSpace: new Size(20, 22),
                preserveAspectRatio: true,
              }}
            >
              <GridList
                aria-label="Livros"
                className="book-grid"
                layout="grid"
                selectionMode="multiple"
                selectionBehavior="replace"
                disallowEmptySelection
                selectedKeys={selection}
                onSelectionChange={select}
                onAction={openBook}
              >
                <Collection items={books.rows} dependencies={[versions]}>
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
                <GridListLoadMoreItem onLoadMore={books.loadMore} isLoading={books.loading}>
                  <span className="loading-more">Carregando…</span>
                </GridListLoadMoreItem>
              </GridList>
            </Virtualizer>
          ) : (
            <Virtualizer layout={TableLayout} layoutOptions={{ rowHeight: 36, headingHeight: 34 }}>
              <Table
                aria-label="Livros"
                className="book-table"
                selectionMode="multiple"
                selectionBehavior="replace"
                disallowEmptySelection
                selectedKeys={selection}
                onSelectionChange={select}
                onRowAction={openBook}
              >
                <TableHeader>
                  <Column isRowHeader id="title" width="2fr">
                    Título
                  </Column>
                  <Column id="creators" width="1.2fr">
                    Autor
                  </Column>
                  <Column id="format" width={96}>
                    Formato
                  </Column>
                  <Column id="progress" width={90}>
                    Lido
                  </Column>
                  <Column id="path" width="2fr">
                    Arquivo
                  </Column>
                </TableHeader>
                <TableBody>
                  <Collection items={books.rows}>
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
                  <TableLoadMoreItem onLoadMore={books.loadMore} isLoading={books.loading}>
                    <span className="loading-more">Carregando…</span>
                  </TableLoadMoreItem>
                </TableBody>
              </Table>
            </Virtualizer>
          )}
        </main>
        {panel}
      </div>
      {merging !== null ? <MergeDialog works={merging} onClose={() => setMerging(null)} onMerged={merged} /> : null}
      {calibre !== null ? (
        <CalibreDialog
          folder={calibre.folder}
          onClose={() => setCalibre(null)}
          onImported={(report) => {
            setNotice(`Do Calibre: ${report.imported} de ${report.books} livros trazidos.`);
            changed();
            onChanged();
          }}
        />
      ) : null}
    </div>
  );
}
