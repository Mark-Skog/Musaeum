/**
 * The side of the library: every book, its folders on the disk, the person's categories (a tree),
 * tags, collections and smart collections. One of them is chosen at a time, and the books shown
 * are those it holds. Each item has a menu of what can be done to it; each section, a button to
 * add one.
 */

import { useEffect, useMemo, useState } from 'react';
import {
  Button,
  GridList,
  GridListItem,
  type Key,
  Menu,
  MenuItem,
  MenuTrigger,
  Popover,
  Separator,
  Tree,
  TreeItem,
  TreeItemContent,
} from 'react-aria-components';

import { api, type Category, type FolderEntry, type LibraryInfo, type SmartCollection, type Tag, type Taxonomy } from '../api';
import { CollectionDialog, ConfirmDialog, FieldsDialog, MoveCategoryDialog, NameDialog, SmartDialog, TagDialog } from './dialogs';
import { type CategoryNode, categoryTree, type Filter, sameFilter, siblingsAround } from './taxonomy-model';

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
  const prune = (list: FolderNode[]): FolderNode[] => list.filter((node) => node.total > 0).map((node) => ({ ...node, children: prune(node.children) }));
  return prune(roots);
}

/** A root's last folder name, which is what tells roots apart; the whole path is its tooltip. */
function shortName(path: string): string {
  const parts = path.split(/[\\/]/).filter((part) => part.length > 0);
  return parts.at(-1) ?? path;
}

function Chevron({ hasChildItems, isExpanded }: { hasChildItems: boolean; isExpanded: boolean }): React.JSX.Element {
  return hasChildItems ? (
    <Button slot="chevron" className="folder-chevron" aria-label={isExpanded ? 'Recolher' : 'Expandir'}>
      {isExpanded ? '▾' : '▸'}
    </Button>
  ) : (
    <span className="folder-chevron" />
  );
}

function FolderItems({ nodes }: { nodes: FolderNode[] }): React.JSX.Element {
  return (
    <>
      {nodes.map((node) => (
        <TreeItem key={node.entry.id} id={`folder:${node.entry.id}`} textValue={node.entry.name} className="folder-item">
          <TreeItemContent>
            {({ hasChildItems, isExpanded }) => (
              <div className="folder-row">
                <Chevron hasChildItems={hasChildItems} isExpanded={isExpanded} />
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

/** A menu of actions behind a "⋯" button. */
function Actions({ label, actions, run }: { label: string; actions: [string, string, boolean?][]; run(id: string): void }): React.JSX.Element {
  return (
    <MenuTrigger>
      <Button className="row-menu" aria-label={`Ações de ${label}`}>
        ⋯
      </Button>
      <Popover className="popover menu-popover" placement="bottom end">
        <Menu className="menu" onAction={(key) => run(String(key))} disabledKeys={actions.filter(([, , disabled]) => disabled === true).map(([id]) => id)}>
          {actions.map(([id, text]) =>
            id.startsWith('---') ? (
              <Separator key={id} className="menu-separator" />
            ) : (
              <MenuItem key={id} id={id} className={id === 'delete' ? 'menu-item danger' : 'menu-item'}>
                {text}
              </MenuItem>
            ),
          )}
        </Menu>
      </Popover>
    </MenuTrigger>
  );
}

type Opened =
  | { kind: 'new-category'; parent: number | null }
  | { kind: 'rename-category'; category: Category }
  | { kind: 'move-category'; id: number }
  | { kind: 'delete-category'; category: Category }
  | { kind: 'tag'; tag: Tag | null }
  | { kind: 'delete-tag'; tag: Tag }
  | { kind: 'collection'; collection: { id: number; name: string; description: string } | null }
  | { kind: 'delete-collection'; id: number; name: string }
  | { kind: 'smart'; smart: SmartCollection | null }
  | { kind: 'delete-smart'; smart: SmartCollection }
  | { kind: 'fields' };

function Section({ title, onAdd, addLabel, children }: { title: string; onAdd?(): void; addLabel?: string; children: React.ReactNode }): React.JSX.Element {
  return (
    <section className="side-section" aria-label={title}>
      <div className="side-heading">
        <h2>{title}</h2>
        {onAdd !== undefined ? (
          <Button className="row-menu" aria-label={addLabel} onPress={onAdd}>
            +
          </Button>
        ) : null}
      </div>
      {children}
    </section>
  );
}

export interface SidebarProps {
  info: LibraryInfo;
  folders: FolderEntry[];
  taxonomy: Taxonomy;
  filter: Filter;
  onFilter(filter: Filter): void;
  /** Something changed in the taxonomy: read it and the books again. */
  onChanged(): void;
  notice(message: string): void;
  /** How many suggestions and possible duplicates wait for the person's word. */
  waiting: number;
}

export function Sidebar({ info, folders, taxonomy, filter, onFilter, onChanged, notice, waiting }: SidebarProps): React.JSX.Element {
  const tree = useMemo(() => buildTree(folders), [folders]);
  const categories = useMemo(() => categoryTree(taxonomy.categories), [taxonomy.categories]);
  const [opened, setOpened] = useState<Opened | null>(null);
  const close = (): void => setOpened(null);
  const fail = (reason: unknown): void => notice(String(reason).replace(/^Error:\s*/, ''));

  // The folders a root holds show from the start; deeper ones open on demand. Categories too.
  const [expanded, setExpanded] = useState<Set<Key>>(() => new Set());
  useEffect(() => {
    setExpanded((current) => {
      const wanted = [...tree.map((node) => `folder:${node.entry.id}`), ...taxonomy.categories.filter((category) => category.depth === 0).map((category) => `category:${category.id}`)];
      const missing = wanted.filter((key) => !current.has(key));
      if (missing.length === 0) {
        return current;
      }
      return new Set([...current, ...missing]);
    });
  }, [tree, taxonomy.categories]);

  const selectedIn = (kind: Filter['kind']): Set<Key> => (filter.kind === kind && 'id' in filter ? new Set([`${kind}:${filter.id}`]) : new Set());
  const choose = (keys: 'all' | Set<Key>): void => {
    const key = keys === 'all' ? undefined : [...keys][0];
    if (key === undefined) {
      return;
    }
    const text = String(key);
    if (text === 'all' || text === 'review') {
      onFilter({ kind: text });
      return;
    }
    const [kind, id] = text.split(':');
    onFilter({ kind: kind as 'folder', id: Number(id) });
  };

  const moveCategory = (category: Category, way: 'up' | 'down'): void => {
    const { before, after } = siblingsAround(taxonomy.categories, category.id);
    const place = way === 'up' ? (before === null ? null : { at: 'before' as const, id: before.id }) : after === null ? null : { at: 'after' as const, id: after.id };
    if (place !== null) {
      api.moveCategory(category.id, category.parentId, place).then(onChanged).catch(fail);
    }
  };

  const categoryItems = (nodes: CategoryNode[]): React.JSX.Element => (
    <>
      {nodes.map((node) => {
        const category = node.category;
        const around = siblingsAround(taxonomy.categories, category.id);
        return (
          <TreeItem key={category.id} id={`category:${category.id}`} textValue={category.name} className="folder-item" data-category={category.id}>
            <TreeItemContent>
              {({ hasChildItems, isExpanded }) => (
                <div className="folder-row">
                  <Chevron hasChildItems={hasChildItems} isExpanded={isExpanded} />
                  <span className="folder-name" title={category.name}>
                    {category.name}
                  </span>
                  <span className="folder-count">{category.total}</span>
                  <Actions
                    label={category.name}
                    actions={[
                      ['sub', 'Nova subcategoria…'],
                      ['rename', 'Renomear…'],
                      ['move', 'Mover para…'],
                      ['up', 'Subir', around.before === null],
                      ['down', 'Descer', around.after === null],
                      ['---1', ''],
                      ['delete', 'Excluir…'],
                    ]}
                    run={(action) => {
                      if (action === 'sub') setOpened({ kind: 'new-category', parent: category.id });
                      else if (action === 'rename') setOpened({ kind: 'rename-category', category });
                      else if (action === 'move') setOpened({ kind: 'move-category', id: category.id });
                      else if (action === 'up' || action === 'down') moveCategory(category, action);
                      else if (action === 'delete') setOpened({ kind: 'delete-category', category });
                    }}
                  />
                </div>
              )}
            </TreeItemContent>
            {node.children.length > 0 ? categoryItems(node.children) : null}
          </TreeItem>
        );
      })}
    </>
  );

  const moveCollection = (id: number, way: 'up' | 'down'): void => {
    const at = taxonomy.collections.findIndex((collection) => collection.id === id);
    const neighbour = taxonomy.collections[way === 'up' ? at - 1 : at + 1];
    if (neighbour !== undefined) {
      api
        .moveCollection(id, way === 'up' ? { at: 'before', id: neighbour.id } : { at: 'after', id: neighbour.id })
        .then(onChanged)
        .catch(fail);
    }
  };

  return (
    <nav className="sidebar" aria-label="Organização">
      <Tree aria-label="Todos os livros" className="folders" selectionMode="single" selectedKeys={filter.kind === 'all' || filter.kind === 'review' ? new Set([filter.kind]) : new Set()} onSelectionChange={choose}>
        <TreeItem id="all" textValue="Todos os livros" className="folder-item">
          <TreeItemContent>
            <div className="folder-row">
              <span className="folder-chevron" />
              <span className="folder-name">Todos os livros</span>
              <span className="folder-count">{info.books}</span>
            </div>
          </TreeItemContent>
        </TreeItem>
        <TreeItem id="review" textValue="Revisar" className="folder-item" data-waiting={waiting}>
          <TreeItemContent>
            <div className="folder-row">
              <span className="folder-chevron" />
              <span className="folder-name">Revisar</span>
              <span className={waiting > 0 ? 'folder-count waiting' : 'folder-count'}>{waiting > 0 ? waiting : ''}</span>
            </div>
          </TreeItemContent>
        </TreeItem>
      </Tree>

      <Section title="Pastas">
        <Tree aria-label="Filtrar por pasta" className="folders" selectionMode="single" selectedKeys={selectedIn('folder')} expandedKeys={expanded} onExpandedChange={setExpanded} onSelectionChange={choose}>
          <FolderItems nodes={tree} />
        </Tree>
      </Section>

      <Section title="Categorias" addLabel="Nova categoria" onAdd={() => setOpened({ kind: 'new-category', parent: null })}>
        {categories.length === 0 ? <p className="side-empty">Nenhuma categoria.</p> : null}
        <Tree aria-label="Filtrar por categoria" className="folders" selectionMode="single" selectedKeys={selectedIn('category')} expandedKeys={expanded} onExpandedChange={setExpanded} onSelectionChange={choose}>
          {categoryItems(categories)}
        </Tree>
      </Section>

      <Section title="Etiquetas" addLabel="Nova etiqueta" onAdd={() => setOpened({ kind: 'tag', tag: null })}>
        {taxonomy.tags.length === 0 ? <p className="side-empty">Nenhuma etiqueta.</p> : null}
        <GridList aria-label="Filtrar por etiqueta" className="side-list" selectionMode="single" selectedKeys={selectedIn('tag')} onSelectionChange={choose} items={taxonomy.tags}>
          {(tag) => (
            <GridListItem id={`tag:${tag.id}`} textValue={tag.name} className="side-item" data-tag={tag.id}>
              <span className={`tag-dot tag-${tag.color ?? 'none'}`} aria-hidden="true" />
              <span className="folder-name">{tag.name}</span>
              <span className="folder-count">{tag.works}</span>
              <Actions
                label={tag.name}
                actions={[
                  ['edit', 'Renomear ou mudar a cor…'],
                  ['---1', ''],
                  ['delete', 'Excluir…'],
                ]}
                run={(action) => setOpened(action === 'edit' ? { kind: 'tag', tag } : { kind: 'delete-tag', tag })}
              />
            </GridListItem>
          )}
        </GridList>
      </Section>

      <Section title="Coleções" addLabel="Nova coleção" onAdd={() => setOpened({ kind: 'collection', collection: null })}>
        {taxonomy.collections.length === 0 ? <p className="side-empty">Nenhuma coleção.</p> : null}
        <GridList aria-label="Filtrar por coleção" className="side-list" selectionMode="single" selectedKeys={selectedIn('collection')} onSelectionChange={choose} items={taxonomy.collections}>
          {(collection) => {
            const at = taxonomy.collections.indexOf(collection);
            return (
              <GridListItem id={`collection:${collection.id}`} textValue={collection.name} className="side-item" data-collection={collection.id}>
                <span className="folder-name" title={collection.description || collection.name}>
                  {collection.name}
                </span>
                <span className="folder-count">{collection.works}</span>
                <Actions
                  label={collection.name}
                  actions={[
                    ['edit', 'Renomear ou descrever…'],
                    ['up', 'Subir', at === 0],
                    ['down', 'Descer', at === taxonomy.collections.length - 1],
                    ['---1', ''],
                    ['delete', 'Excluir…'],
                  ]}
                  run={(action) => {
                    if (action === 'edit') setOpened({ kind: 'collection', collection });
                    else if (action === 'up' || action === 'down') moveCollection(collection.id, action);
                    else setOpened({ kind: 'delete-collection', id: collection.id, name: collection.name });
                  }}
                />
              </GridListItem>
            );
          }}
        </GridList>
      </Section>

      <Section title="Coleções inteligentes" addLabel="Nova coleção inteligente" onAdd={() => setOpened({ kind: 'smart', smart: null })}>
        {taxonomy.smart.length === 0 ? <p className="side-empty">Nenhuma coleção inteligente.</p> : null}
        <GridList aria-label="Filtrar por coleção inteligente" className="side-list" selectionMode="single" selectedKeys={selectedIn('smart')} onSelectionChange={choose} items={taxonomy.smart}>
          {(smart) => (
            <GridListItem id={`smart:${smart.id}`} textValue={smart.name} className="side-item" data-smart={smart.id}>
              <span className="smart-mark" aria-hidden="true">
                ⚙
              </span>
              <span className="folder-name" title={smart.broken ?? smart.name}>
                {smart.name}
              </span>
              {smart.broken !== null ? (
                <span className="broken" title={smart.broken} aria-label="A regra não funciona mais">
                  !
                </span>
              ) : (
                <span className="folder-count">{smart.works}</span>
              )}
              <Actions
                label={smart.name}
                actions={[
                  ['edit', 'Editar a regra…'],
                  ['---1', ''],
                  ['delete', 'Excluir…'],
                ]}
                run={(action) => setOpened(action === 'edit' ? { kind: 'smart', smart } : { kind: 'delete-smart', smart })}
              />
            </GridListItem>
          )}
        </GridList>
      </Section>

      <div className="side-footer">
        <Button className="button small" onPress={() => setOpened({ kind: 'fields' })}>
          Campos personalizados…
        </Button>
      </div>

      {opened?.kind === 'new-category' ? (
        <NameDialog
          title={opened.parent === null ? 'Nova categoria' : 'Nova subcategoria'}
          label="Nome"
          initial=""
          submitLabel="Criar"
          onSubmit={(name) => api.createCategory(opened.parent, name, { at: 'last' }).then(onChanged)}
          onClose={close}
        />
      ) : null}
      {opened?.kind === 'rename-category' ? (
        <NameDialog title="Renomear categoria" label="Nome" initial={opened.category.name} submitLabel="Renomear" onSubmit={(name) => api.renameCategory(opened.category.id, name).then(onChanged)} onClose={close} />
      ) : null}
      {opened?.kind === 'move-category' ? <MoveCategoryDialog categories={taxonomy.categories} id={opened.id} onClose={close} onMoved={onChanged} /> : null}
      {opened?.kind === 'delete-category' ? (
        <ConfirmDialog
          title={`Excluir “${opened.category.name}”?`}
          message="A categoria e as subcategorias dela saem da árvore. Os livros continuam na biblioteca e nas outras categorias."
          confirmLabel="Excluir"
          onConfirm={() =>
            api.deleteCategory(opened.category.id).then(() => {
              if (filter.kind === 'category') {
                onFilter({ kind: 'all' });
              }
              onChanged();
            })
          }
          onClose={close}
        />
      ) : null}
      {opened?.kind === 'tag' ? <TagDialog tag={opened.tag} onClose={close} onSaved={onChanged} /> : null}
      {opened?.kind === 'delete-tag' ? (
        <ConfirmDialog
          title={`Excluir a etiqueta “${opened.tag.name}”?`}
          message="Ela sai de todos os livros que a têm. Os livros continuam na biblioteca."
          confirmLabel="Excluir"
          onConfirm={() =>
            api.deleteTag(opened.tag.id).then(() => {
              if (sameFilter(filter, { kind: 'tag', id: opened.tag.id })) {
                onFilter({ kind: 'all' });
              }
              onChanged();
            })
          }
          onClose={close}
        />
      ) : null}
      {opened?.kind === 'collection' ? <CollectionDialog collection={opened.collection} onClose={close} onSaved={onChanged} /> : null}
      {opened?.kind === 'delete-collection' ? (
        <ConfirmDialog
          title={`Excluir a coleção “${opened.name}”?`}
          message="Só a coleção some. Os livros dela continuam na biblioteca."
          confirmLabel="Excluir"
          onConfirm={() =>
            api.deleteCollection(opened.id).then(() => {
              if (sameFilter(filter, { kind: 'collection', id: opened.id })) {
                onFilter({ kind: 'all' });
              }
              onChanged();
            })
          }
          onClose={close}
        />
      ) : null}
      {opened?.kind === 'smart' ? <SmartDialog smart={opened.smart} taxonomy={taxonomy} folders={folders} onClose={close} onSaved={onChanged} /> : null}
      {opened?.kind === 'delete-smart' ? (
        <ConfirmDialog
          title={`Excluir “${opened.smart.name}”?`}
          message="Só a regra some. Nenhum livro muda."
          confirmLabel="Excluir"
          onConfirm={() =>
            api.deleteSmart(opened.smart.id).then(() => {
              if (sameFilter(filter, { kind: 'smart', id: opened.smart.id })) {
                onFilter({ kind: 'all' });
              }
              onChanged();
            })
          }
          onClose={close}
        />
      ) : null}
      {opened?.kind === 'fields' ? <FieldsDialog fields={taxonomy.fields} onClose={close} onChanged={onChanged} /> : null}
    </nav>
  );
}
