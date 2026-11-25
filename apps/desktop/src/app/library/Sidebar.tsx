/**
 * The side of the library: every book, its folders on the disk, the person's categories (a tree),
 * tags, collections and smart collections. One of them is chosen at a time, and the books shown
 * are those it holds. Empty organization sections are hidden by default and can be started from
 * a compact "+ Organize" menu at the bottom of the sidebar.
 */

import { memo, type ReactNode, type RefObject, useEffect, useMemo, useState } from 'react';
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
import { useLocale, useT } from '../i18n';
import { OverlayScrollbar } from '../OverlayScrollbar';
import { CollectionDialog, ConfirmDialog, MoveCategoryDialog, NameDialog, SmartDialog, TagDialog } from './dialogs';
import {
  type FolderNode,
  resolveVisibleFolders,
  shortName,
  SIDEBAR_MAX_WIDTH,
  SIDEBAR_MIN_WIDTH,
  visibleSidebarSections,
} from './sidebar-model';
import { type CategoryNode, categoryTree, type Filter, sameFilter, siblingsAround } from './taxonomy-model';

function Chevron({ hasChildItems, isExpanded }: { hasChildItems: boolean; isExpanded: boolean }): React.JSX.Element {
  const t = useT();
  return hasChildItems ? (
    <Button slot="chevron" className="folder-chevron" aria-label={isExpanded ? t('sidebar.collapse') : t('sidebar.expand')}>
      {isExpanded ? '▾' : '▸'}
    </Button>
  ) : (
    <span className="folder-chevron" />
  );
}

function FolderItems({ nodes, showCounts }: { nodes: FolderNode[]; showCounts: boolean }): React.JSX.Element {
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
                {showCounts ? <span className="folder-count">{node.total}</span> : null}
              </div>
            )}
          </TreeItemContent>
          {node.children.length > 0 ? <FolderItems nodes={node.children} showCounts={showCounts} /> : null}
        </TreeItem>
      ))}
    </>
  );
}

/** A menu of actions behind a "⋯" button. */
function Actions({ label, actions, run }: { label: string; actions: [string, string, boolean?][]; run(id: string): void }): React.JSX.Element {
  const t = useT();
  return (
    <MenuTrigger>
      <Button className="row-menu" aria-label={t('sidebar.actionsOf', { name: label })}>
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
  | { kind: 'delete-smart'; smart: SmartCollection };

function Section({
  title,
  onAdd,
  addLabel,
  action,
  children,
}: {
  title: string;
  onAdd?(): void;
  addLabel?: string;
  action?: React.ReactNode;
  children: React.ReactNode;
}): React.JSX.Element {
  return (
    <section className="side-section" aria-label={title}>
      <div className="side-heading">
        <h2>{title}</h2>
        {action ??
          (onAdd !== undefined ? (
            <Button className="row-menu" aria-label={addLabel} onPress={onAdd}>
              +
            </Button>
          ) : null)}
      </div>
      {children}
    </section>
  );
}

export interface SidebarShellProps {
  collapsed: boolean;
  width: number;
  resizeLabel: string;
  scrollRef: RefObject<HTMLElement | null>;
  resizerRef?: RefObject<HTMLDivElement | null>;
  onPointerDown?: (event: React.PointerEvent<HTMLDivElement>) => void;
  onDoubleClick?: () => void;
  onKeyDown?: (event: React.KeyboardEvent<HTMLDivElement>) => void;
  children: ReactNode;
}

/**
 * Animated sidebar container kept mounted in the DOM so open/close glides smoothly without
 * unmounting or wrapping folder labels; uses `inert` + `aria-hidden` while collapsed.
 */
export function SidebarShell({
  collapsed,
  width,
  resizeLabel,
  scrollRef,
  resizerRef,
  onPointerDown,
  onDoubleClick,
  onKeyDown,
  children,
}: SidebarShellProps): React.JSX.Element {
  return (
    <div className="sidebar-shell" inert={collapsed || undefined} aria-hidden={collapsed || undefined}>
      <div className="sidebar-inner">
        {children}
        <OverlayScrollbar targetRef={scrollRef} />
      </div>
      <div
        ref={resizerRef}
        className="sidebar-resizer"
        role="separator"
        tabIndex={collapsed ? -1 : 0}
        aria-orientation="vertical"
        aria-label={resizeLabel}
        aria-valuenow={Math.round(width)}
        aria-valuemin={SIDEBAR_MIN_WIDTH}
        aria-valuemax={SIDEBAR_MAX_WIDTH}
        onPointerDown={onPointerDown}
        onDoubleClick={onDoubleClick}
        onKeyDown={onKeyDown}
      />
    </div>
  );
}

export interface SidebarProps {
  info: LibraryInfo;
  folders: FolderEntry[];
  taxonomy: Taxonomy;
  filter: Filter;
  showEmptySections: boolean;
  showCounts: boolean;
  onFilter(filter: Filter): void;
  /** Something changed in the taxonomy: read it and the books again. */
  onChanged(): void;
  notice(message: string): void;
  /** How many suggestions and possible duplicates wait for the person's word. */
  waiting: number;
  onManageFolders(): void;
  onOpenFields(): void;
  scrollRef?: RefObject<HTMLElement | null>;
}

export const Sidebar = memo(function Sidebar({
  info,
  folders,
  taxonomy,
  filter,
  showEmptySections,
  showCounts,
  onFilter,
  onChanged,
  notice,
  waiting,
  onManageFolders,
  onOpenFields,
  scrollRef,
}: SidebarProps): React.JSX.Element {
  const t = useT();
  const locale = useLocale();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const { nodes: tree, singleRootPromoted } = useMemo(() => resolveVisibleFolders(folders, info.roots.length), [folders, info.roots.length, locale]);
  const categories = useMemo(() => categoryTree(taxonomy.categories), [taxonomy.categories]);
  const visibility = useMemo(() => visibleSidebarSections(taxonomy, showEmptySections), [taxonomy, showEmptySections]);
  const [opened, setOpened] = useState<Opened | null>(null);
  const close = (): void => setOpened(null);
  const fail = (reason: unknown): void => notice(String(reason).replace(/^Error:\s*/, ''));

  // When multiple roots (or a single root without subfolders) are shown, expand the root level
  // automatically. When a single root's subfolders are promoted, keep them collapsed until opened.
  const [expanded, setExpanded] = useState<Set<Key>>(() => new Set());
  useEffect(() => {
    setExpanded((current) => {
      const wanted = [
        ...(singleRootPromoted ? [] : tree.map((node) => `folder:${node.entry.id}`)),
        ...taxonomy.categories.filter((category) => category.depth === 0).map((category) => `category:${category.id}`),
      ];
      const missing = wanted.filter((key) => !current.has(key));
      if (missing.length === 0) {
        return current;
      }
      return new Set([...current, ...missing]);
    });
  }, [tree, singleRootPromoted, taxonomy.categories]);

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
                  {showCounts ? <span className="folder-count">{category.total}</span> : null}
                  <Actions
                    label={category.name}
                    actions={[
                      ['sub', t('sidebar.newSubcategory')],
                      ['rename', t('sidebar.rename')],
                      ['move', t('sidebar.moveTo')],
                      ['up', t('sidebar.up'), around.before === null],
                      ['down', t('sidebar.down'), around.after === null],
                      ['---1', ''],
                      ['delete', t('sidebar.delete')],
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

  const runOrganize = (action: Key): void => {
    switch (String(action)) {
      case 'category':
        setOpened({ kind: 'new-category', parent: null });
        break;
      case 'tag':
        setOpened({ kind: 'tag', tag: null });
        break;
      case 'collection':
        setOpened({ kind: 'collection', collection: null });
        break;
      case 'smart':
        setOpened({ kind: 'smart', smart: null });
        break;
      case 'fields':
        onOpenFields();
        break;
    }
  };

  return (
    <nav ref={scrollRef} className="sidebar" aria-label={t('sidebar.label')}>
      <Tree aria-label={t('sidebar.all')} className="folders" selectionMode="single" selectedKeys={filter.kind === 'all' || filter.kind === 'review' ? new Set([filter.kind]) : new Set()} onSelectionChange={choose}>
        <TreeItem id="all" textValue={t('sidebar.all')} className="folder-item">
          <TreeItemContent>
            <div className="folder-row">
              <span className="folder-chevron" />
              <span className="folder-name">{t('sidebar.all')}</span>
              {showCounts ? <span className="folder-count">{info.books}</span> : null}
            </div>
          </TreeItemContent>
        </TreeItem>
        <TreeItem id="review" textValue={t('sidebar.review')} className="folder-item" data-waiting={waiting}>
          <TreeItemContent>
            <div className="folder-row">
              <span className="folder-chevron" />
              <span className="folder-name">{t('sidebar.review')}</span>
              <span className={waiting > 0 ? 'folder-count waiting' : 'folder-count'}>{waiting > 0 ? waiting : ''}</span>
            </div>
          </TreeItemContent>
        </TreeItem>
      </Tree>

      <Section
        title={t('sidebar.folders')}
        action={
          <Button className="row-menu side-manage-folders" aria-label={t('sidebar.folders.manage')} onPress={onManageFolders}>
            <span className="icon-box" title={t('sidebar.folders.manage')}>
              <svg className="icon" viewBox="0 0 16 16" width={14} height={14} fill="none" stroke="currentColor" aria-hidden="true">
                <path d="M2 4.5a1 1 0 0 1 1-1h3.2l1.3 1.5H13a1 1 0 0 1 1 1v5.5a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1z" />
              </svg>
            </span>
          </Button>
        }
      >
        <Tree aria-label={t('sidebar.folders.filter')} className="folders" selectionMode="single" selectedKeys={selectedIn('folder')} expandedKeys={expanded} onExpandedChange={setExpanded} onSelectionChange={choose}>
          <FolderItems nodes={tree} showCounts={showCounts} />
        </Tree>
      </Section>

      {visibility.categories ? (
        <Section title={t('sidebar.categories')} addLabel={t('sidebar.categories.new')} onAdd={() => setOpened({ kind: 'new-category', parent: null })}>
          {categories.length === 0 ? <p className="side-empty">{t('sidebar.categories.none')}</p> : null}
          <Tree aria-label={t('sidebar.categories.filter')} className="folders" selectionMode="single" selectedKeys={selectedIn('category')} expandedKeys={expanded} onExpandedChange={setExpanded} onSelectionChange={choose}>
            {categoryItems(categories)}
          </Tree>
        </Section>
      ) : null}

      {visibility.tags ? (
        <Section title={t('sidebar.tags')} addLabel={t('sidebar.tags.new')} onAdd={() => setOpened({ kind: 'tag', tag: null })}>
          {taxonomy.tags.length === 0 ? <p className="side-empty">{t('sidebar.tags.none')}</p> : null}
          <GridList aria-label={t('sidebar.tags.filter')} className="side-list" selectionMode="single" selectedKeys={selectedIn('tag')} onSelectionChange={choose} items={taxonomy.tags} dependencies={[locale]}>
            {(tag) => (
              <GridListItem id={`tag:${tag.id}`} textValue={tag.name} className="side-item" data-tag={tag.id}>
                <span className={`tag-dot tag-${tag.color ?? 'none'}`} aria-hidden="true" />
                <span className="folder-name">{tag.name}</span>
                {showCounts ? <span className="folder-count">{tag.works}</span> : null}
                <Actions
                  label={tag.name}
                  actions={[
                    ['edit', t('sidebar.tags.edit')],
                    ['---1', ''],
                    ['delete', t('sidebar.delete')],
                  ]}
                  run={(action) => setOpened(action === 'edit' ? { kind: 'tag', tag } : { kind: 'delete-tag', tag })}
                />
              </GridListItem>
            )}
          </GridList>
        </Section>
      ) : null}

      {visibility.collections ? (
        <Section title={t('sidebar.collections')} addLabel={t('sidebar.collections.new')} onAdd={() => setOpened({ kind: 'collection', collection: null })}>
          {taxonomy.collections.length === 0 ? <p className="side-empty">{t('sidebar.collections.none')}</p> : null}
          <GridList aria-label={t('sidebar.collections.filter')} className="side-list" selectionMode="single" selectedKeys={selectedIn('collection')} onSelectionChange={choose} items={taxonomy.collections} dependencies={[locale]}>
            {(collection) => {
              const at = taxonomy.collections.indexOf(collection);
              return (
                <GridListItem id={`collection:${collection.id}`} textValue={collection.name} className="side-item" data-collection={collection.id}>
                  <span className="folder-name" title={collection.description || collection.name}>
                    {collection.name}
                  </span>
                  {showCounts ? <span className="folder-count">{collection.works}</span> : null}
                  <Actions
                    label={collection.name}
                    actions={[
                      ['edit', t('sidebar.collections.edit')],
                      ['up', t('sidebar.up'), at === 0],
                      ['down', t('sidebar.down'), at === taxonomy.collections.length - 1],
                      ['---1', ''],
                      ['delete', t('sidebar.delete')],
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
      ) : null}

      {visibility.smart ? (
        <Section title={t('sidebar.smart')} addLabel={t('sidebar.smart.new')} onAdd={() => setOpened({ kind: 'smart', smart: null })}>
          {taxonomy.smart.length === 0 ? <p className="side-empty">{t('sidebar.smart.none')}</p> : null}
          <GridList aria-label={t('sidebar.smart.filter')} className="side-list" selectionMode="single" selectedKeys={selectedIn('smart')} onSelectionChange={choose} items={taxonomy.smart} dependencies={[locale]}>
            {(smart) => (
              <GridListItem id={`smart:${smart.id}`} textValue={smart.name} className="side-item" data-smart={smart.id}>
                <span className="smart-mark" aria-hidden="true">
                  ⚙
                </span>
                <span className="folder-name" title={smart.broken ?? smart.name}>
                  {smart.name}
                </span>
                {smart.broken !== null ? (
                  <span className="broken" title={smart.broken} aria-label={t('sidebar.smart.broken')}>
                    !
                  </span>
                ) : showCounts ? (
                  <span className="folder-count">{smart.works}</span>
                ) : null}
                <Actions
                  label={smart.name}
                  actions={[
                    ['edit', t('sidebar.smart.edit')],
                    ['---1', ''],
                    ['delete', t('sidebar.delete')],
                  ]}
                  run={(action) => setOpened(action === 'edit' ? { kind: 'smart', smart } : { kind: 'delete-smart', smart })}
                />
              </GridListItem>
            )}
          </GridList>
        </Section>
      ) : null}

      <div className="side-footer">
        {visibility.hasHidden ? (
          <MenuTrigger>
            <Button className="button small side-organize-button">{t('sidebar.organize')}</Button>
            <Popover className="popover menu-popover" placement="top start">
              <Menu className="menu" aria-label={t('sidebar.organize.label')} onAction={runOrganize}>
                {!visibility.categories ? (
                  <MenuItem id="category" className="menu-item">
                    {t('sidebar.categories.new')}
                  </MenuItem>
                ) : null}
                {!visibility.tags ? (
                  <MenuItem id="tag" className="menu-item">
                    {t('sidebar.tags.new')}
                  </MenuItem>
                ) : null}
                {!visibility.collections ? (
                  <MenuItem id="collection" className="menu-item">
                    {t('sidebar.collections.new')}
                  </MenuItem>
                ) : null}
                {!visibility.smart ? (
                  <MenuItem id="smart" className="menu-item">
                    {t('sidebar.smart.new')}
                  </MenuItem>
                ) : null}
                <Separator className="menu-separator" />
                <MenuItem id="fields" className="menu-item">
                  {t('sidebar.fields')}
                </MenuItem>
              </Menu>
            </Popover>
          </MenuTrigger>
        ) : (
          <Button className="button small" onPress={onOpenFields}>
            {t('sidebar.fields')}
          </Button>
        )}
      </div>

      {opened?.kind === 'new-category' ? (
        <NameDialog
          title={opened.parent === null ? t('sidebar.newCategory.title') : t('sidebar.newSubcategory.title')}
          label={t('sidebar.name')}
          initial=""
          submitLabel={t('sidebar.create')}
          onSubmit={(name) => api.createCategory(opened.parent, name, { at: 'last' }).then(onChanged)}
          onClose={close}
        />
      ) : null}
      {opened?.kind === 'rename-category' ? (
        <NameDialog title={t('sidebar.renameCategory.title')} label={t('sidebar.name')} initial={opened.category.name} submitLabel={t('sidebar.renameCategory.submit')} onSubmit={(name) => api.renameCategory(opened.category.id, name).then(onChanged)} onClose={close} />
      ) : null}
      {opened?.kind === 'move-category' ? <MoveCategoryDialog categories={taxonomy.categories} id={opened.id} onClose={close} onMoved={onChanged} /> : null}
      {opened?.kind === 'delete-category' ? (
        <ConfirmDialog
          title={t('sidebar.deleteNamed', { name: opened.category.name })}
          message={t('sidebar.deleteCategory.message')}
          confirmLabel={t('sidebar.confirmDelete')}
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
          title={t('sidebar.deleteTag.title', { name: opened.tag.name })}
          message={t('sidebar.deleteTag.message')}
          confirmLabel={t('sidebar.confirmDelete')}
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
          title={t('sidebar.deleteCollection.title', { name: opened.name })}
          message={t('sidebar.deleteCollection.message')}
          confirmLabel={t('sidebar.confirmDelete')}
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
          title={t('sidebar.deleteNamed', { name: opened.smart.name })}
          message={t('sidebar.deleteSmart.message')}
          confirmLabel={t('sidebar.confirmDelete')}
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
    </nav>
  );
});
