/**
 * The library: every book in the folders it follows, as covers or as a table, found by a search
 * as it is typed, or by the folder it is in. A book opens with a click, or Enter.
 */

import { useEffect, useMemo, useState } from 'react';
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
  Tree,
  TreeItem,
  TreeItemContent,
  Virtualizer,
  type Key,
  type Selection,
} from 'react-aria-components';

import { type BookRow, coverUrl, type FolderEntry, type LibraryInfo, api } from '../api';
import { useBooks, useCoverVersions } from './use-books';

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

interface FolderNode {
  entry: FolderEntry;
  children: FolderNode[];
  total: number;
}

function buildTree(folders: FolderEntry[]): FolderNode[] {
  const nodes = new Map<number, FolderNode>();
  for (const entry of folders) {
    nodes.set(entry.id, { entry, children: [], total: entry.books });
  }
  const roots: FolderNode[] = [];
  for (const node of nodes.values()) {
    const parent = node.entry.parentId === null ? undefined : nodes.get(node.entry.parentId);
    if (parent === undefined) {
      roots.push(node);
    } else {
      parent.children.push(node);
    }
  }
  const sum = (node: FolderNode): number => {
    node.children.sort((a, b) => a.entry.name.localeCompare(b.entry.name, 'pt-BR'));
    node.total = node.entry.books + node.children.reduce((total, child) => total + sum(child), 0);
    return node.total;
  };
  roots.forEach(sum);
  // Folders with no books anywhere below them are only noise in a filter.
  const prune = (list: FolderNode[]): FolderNode[] =>
    list.filter((node) => node.total > 0).map((node) => ({ ...node, children: prune(node.children) }));
  return prune(roots);
}

/** A root's last folder name, which is what tells roots apart; the whole path is its tooltip. */
function shortName(path: string): string {
  const parts = path.split(/[\\/]/).filter((part) => part.length > 0);
  return parts.at(-1) ?? path;
}

function FolderItems({ nodes }: { nodes: FolderNode[] }): React.JSX.Element {
  return (
    <>
      {nodes.map((node) => (
        <TreeItem key={node.entry.id} id={node.entry.id} textValue={node.entry.name} className="folder-item">
          <TreeItemContent>
            {({ hasChildItems, isExpanded }) => (
              <div className="folder-row">
                {hasChildItems ? (
                  <Button slot="chevron" className="folder-chevron" aria-label={isExpanded ? 'Recolher' : 'Expandir'}>
                    {isExpanded ? '▾' : '▸'}
                  </Button>
                ) : (
                  <span className="folder-chevron" />
                )}
                <span className="folder-name" title={node.entry.name}>
                  {node.entry.parentId === null ? shortName(node.entry.name) : node.entry.name}
                </span>
                <span className="folder-count">{node.total}</span>
              </div>
            )}
          </TreeItemContent>
          {node.children.length > 0 ? <FolderItems nodes={node.children} /> : null}
        </TreeItem>
      ))}
    </>
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
  onRows?(rows: BookRow[], total: number, shown: { text: string; folder: number | null }): void;
}

export function LibraryView({ info, folders, generation, onChanged, onOpen, onRows }: LibraryViewProps): React.JSX.Element {
  const [typed, setTyped] = useState('');
  const [query, setQuery] = useState('');
  const [folder, setFolder] = useState<number | null>(null);
  const [view, setView] = useState<'grid' | 'table'>('grid');
  const books = useBooks(query, folder, generation);
  const versions = useCoverVersions();
  const tree = useMemo(() => buildTree(folders), [folders]);
  // The folders a root holds show from the start; deeper ones open on demand.
  const [expanded, setExpanded] = useState<Set<Key>>(() => new Set());
  useEffect(() => {
    setExpanded((current) => {
      const missing = tree.filter((node) => !current.has(node.entry.id));
      if (missing.length === 0) {
        return current;
      }
      const next = new Set(current);
      for (const node of missing) {
        next.add(node.entry.id);
      }
      return next;
    });
  }, [tree]);
  const byId = useMemo(() => new Map(books.rows.map((row) => [row.documentId, row])), [books.rows]);

  useEffect(() => {
    const timer = window.setTimeout(() => setQuery(typed), SEARCH_AFTER_MS);
    return () => clearTimeout(timer);
  }, [typed]);
  useEffect(() => onRows?.(books.rows, books.total, books.shown), [books.rows, books.total, books.shown, onRows]);

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
  const selectedFolder: Selection = new Set<Key>(folder === null ? ['all'] : [folder]);

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
          {books.total === info.books || query !== '' || folder !== null
            ? `${books.total} ${books.total === 1 ? 'livro' : 'livros'}`
            : `${books.total} de ${info.books} livros`}
          {info.indexing ? ' · lendo a pasta…' : ''}
        </span>
      </header>
      <div className="library-body">
        <nav className="sidebar" aria-label="Pastas">
          <Tree
            aria-label="Filtrar por pasta"
            className="folders"
            selectionMode="single"
            disallowEmptySelection
            selectedKeys={selectedFolder}
            expandedKeys={expanded}
            onExpandedChange={setExpanded}
            onSelectionChange={(keys) => {
              const key = [...keys][0];
              setFolder(key === undefined || key === 'all' ? null : Number(key));
            }}
          >
            <TreeItem id="all" textValue="Todos os livros" className="folder-item">
              <TreeItemContent>
                <div className="folder-row">
                  <span className="folder-chevron" />
                  <span className="folder-name">Todos os livros</span>
                  <span className="folder-count">{info.books}</span>
                </div>
              </TreeItemContent>
            </TreeItem>
            <FolderItems nodes={tree} />
          </Tree>
        </nav>
        <main className="shelf">
          {books.rows.length === 0 && !books.loading ? (
            <p className="empty">
              {query !== '' ? `Nada encontrado para “${query}”.` : info.indexing ? 'Lendo a pasta…' : 'Nenhum livro aqui.'}
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
              <Table aria-label="Livros" className="book-table" onRowAction={open}>
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
      </div>
    </div>
  );
}
