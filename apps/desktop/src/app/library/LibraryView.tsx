/**
 * The library: every book in the folders it follows, as covers or as a table, found by a search
 * as it is typed, or by what the side chooses: a folder, a category, a tag, a collection, a smart
 * collection. A click chooses a book and shows its details beside the books, to edit; a double
 * click or Enter opens it.
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

import { type BookRow, coverUrl, type FolderEntry, type LibraryInfo, api } from '../api';
import { type CollectionPlace, Inspector } from './Inspector';
import { Sidebar } from './Sidebar';
import type { Filter } from './taxonomy-model';
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

export interface LibraryViewProps {
  info: LibraryInfo | null;
  folders: FolderEntry[];
  generation: number;
  onChanged(): void;
  onOpen(book: BookRow): void;
  /** For tests: the books shown, as they are listed. */
  onRows?(rows: BookRow[], total: number, shown: Shown): void;
}

export function LibraryView({ info, folders, generation, onChanged, onOpen, onRows }: LibraryViewProps): React.JSX.Element {
  const [typed, setTyped] = useState('');
  const [query, setQuery] = useState('');
  const [filter, setFilter] = useState<Filter>({ kind: 'all' });
  const [view, setView] = useState<'grid' | 'table'>('grid');
  // Raised by every change made here, so the books and the side are read again.
  const [version, setVersion] = useState(0);
  const books = useBooks(query, filter, generation + version);
  const { taxonomy, refresh: refreshTaxonomy } = useTaxonomy(generation);
  const versions = useCoverVersions();
  // The chosen book is kept by itself, not looked up in the rows: a list read again after a save
  // brings back only its first page, and the book being edited may be further down.
  const [selected, setSelected] = useState<BookRow | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const byId = useMemo(() => new Map(books.rows.map((row) => [row.documentId, row])), [books.rows]);
  const changed = useCallback(() => {
    refreshTaxonomy();
    setVersion((value) => value + 1);
  }, [refreshTaxonomy]);
  // Gone to another filter or search where the chosen book is not listed, it is no longer chosen,
  // so it does not come back by itself on returning. The same list read again keeps it.
  const shownKey = JSON.stringify([books.shown.text, books.shown.filter]);
  const lastShown = useRef(shownKey);
  useEffect(() => {
    if (lastShown.current === shownKey) {
      return;
    }
    lastShown.current = shownKey;
    setSelected((current) => (current !== null && !books.rows.some((row) => row.documentId === current.documentId) ? null : current));
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

  if (info === null) {
    return <div className="loading">Abrindo a biblioteca…</div>;
  }
  if (!info.open) {
    return <Welcome info={info} onChanged={onChanged} />;
  }

  const open = (key: Key): void => {
    const book = byId.get(Number(key));
    if (book !== undefined) {
      onOpen(book);
    }
  };
  // The row as last read, when it is among those read, for what a save changed in it.
  const chosen = selected === null ? undefined : (byId.get(selected.documentId) ?? selected);
  const selection: Selection = new Set<Key>(chosen === undefined ? [] : [chosen.documentId]);
  const select = (keys: Selection): void => {
    const key = keys === 'all' ? undefined : [...keys][0];
    setSelected(key === undefined ? null : (byId.get(Number(key)) ?? null));
  };
  // A click on the chosen book keeps it chosen, as in a file manager; Escape puts the details away.
  const putAway = (event: React.KeyboardEvent): void => {
    if (event.key === 'Escape' && selected !== null) {
      setSelected(null);
    }
  };
  const filtered = query !== '' || filter.kind !== 'all';
  // In a collection, the books just before and after the chosen one, for moving it there.
  let place: CollectionPlace | null = null;
  if (chosen !== undefined && filter.kind === 'collection' && query === '') {
    const at = books.rows.findIndex((row) => row.documentId === chosen.documentId);
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

  return (
    <div className="library">
      <header className="toolbar">
        <h1 className="toolbar-title">Biblioteca</h1>
        <SearchField
          className="search"
          aria-label="Buscar livros"
          value={typed}
          onChange={setTyped}
          onSubmit={setQuery}
        >
          <Input placeholder="Buscar por título, autor ou série" />
          <Button className="search-clear" aria-label="Limpar busca">
            ×
          </Button>
        </SearchField>
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
        <span className="toolbar-status" aria-live="polite">
          {books.total === info.books || filtered
            ? `${books.total} ${books.total === 1 ? 'livro' : 'livros'}`
            : `${books.total} de ${info.books} livros`}
          {info.indexing ? ' · lendo a pasta…' : ''}
        </span>
      </header>
      {notice !== null ? (
        <p className="library-notice" role="alert">
          {notice}
        </p>
      ) : null}
      <div className={chosen === undefined ? 'library-body' : 'library-body with-inspector'}>
        <Sidebar
          info={info}
          folders={folders}
          taxonomy={taxonomy}
          filter={filter}
          onFilter={setFilter}
          onChanged={changed}
          notice={setNotice}
        />
        {/* Caught on the way down: the items of a grid stop some keys before they reach the shelf. */}
        <main className="shelf" onKeyDownCapture={putAway}>
          {books.rows.length === 0 && !books.loading ? (
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
                selectionMode="single"
                selectionBehavior="replace"
                disallowEmptySelection
                selectedKeys={selection}
                onSelectionChange={select}
                onAction={open}
              >
                <Collection items={books.rows} dependencies={[versions]}>
                {(book) => (
                  <GridListItem
                    id={book.documentId}
                    textValue={book.title}
                    className="book-card"
                    data-document={book.documentId}
                  >
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
                selectionMode="single"
                selectionBehavior="replace"
                disallowEmptySelection
                selectedKeys={selection}
                onSelectionChange={select}
                onRowAction={open}
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
        {chosen !== undefined ? <Inspector key={chosen.workId} book={chosen} taxonomy={taxonomy} place={place} onSaved={changed} onClose={() => setSelected(null)} /> : null}
      </div>
    </div>
  );
}
