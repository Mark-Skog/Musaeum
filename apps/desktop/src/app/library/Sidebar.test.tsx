import { useRef } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';

import type { FolderEntry, LibraryInfo, Taxonomy } from '../api';
import { setLocale } from '../i18n';
import { InspectorShell } from './LibraryView';
import { Sidebar, SidebarShell } from './Sidebar';

const baseInfo = (rootCount: number): LibraryInfo => ({
  open: true,
  location: 'C:\\Library',
  roots: Array.from({ length: rootCount }, (_, idx) => ({
    id: idx + 1,
    path: `C:\\Root${idx + 1}`,
    status: 'available',
  })),
  books: 12,
  indexing: false,
  error: null,
});

const emptyTaxonomy = (): Taxonomy => ({
  categories: [],
  tags: [],
  collections: [],
  smart: [],
  fields: [],
});

const renderSidebar = ({
  info = baseInfo(1),
  folders = [],
  taxonomy = emptyTaxonomy(),
  showEmptySections = false,
  showCounts = false,
  waiting = 0,
}: {
  info?: LibraryInfo;
  folders?: FolderEntry[];
  taxonomy?: Taxonomy;
  showEmptySections?: boolean;
  showCounts?: boolean;
  waiting?: number;
}): string =>
  renderToStaticMarkup(
    <Sidebar
      info={info}
      folders={folders}
      taxonomy={taxonomy}
      filter={{ kind: 'all' }}
      showEmptySections={showEmptySections}
      showCounts={showCounts}
      onFilter={() => {}}
      onChanged={() => {}}
      notice={() => {}}
      waiting={waiting}
      onManageFolders={() => {}}
      onOpenFields={() => {}}
    />,
  );

describe('Sidebar component presentation', () => {
  it('hides empty taxonomy sections by default and renders + Organize', () => {
    setLocale('en');
    const html = renderSidebar({});
    expect(html).toContain('Folders');
    expect(html).toContain('+ Organize');
    expect(html).not.toContain('Categories');
    expect(html).not.toContain('Tags');
    expect(html).not.toContain('Collections');
    expect(html).not.toContain('Smart collections');
  });

  it('shows a section automatically once it has at least one item', () => {
    setLocale('pt-BR');
    const tax = emptyTaxonomy();
    tax.categories.push({ id: 1, parentId: null, name: 'Ficção', depth: 0, works: 2, total: 2 });
    const html = renderSidebar({ taxonomy: tax });
    expect(html).toContain('Categorias');
    expect(html).toContain('Ficção');
    expect(html).toContain('+ Organizar');
    expect(html).not.toContain('Etiquetas');
    expect(html).not.toContain('Coleções');
  });

  it('renders all empty sections when showEmptySections is true', () => {
    setLocale('en');
    const html = renderSidebar({ showEmptySections: true });
    expect(html).toContain('Categories');
    expect(html).toContain('Tags');
    expect(html).toContain('Collections');
    expect(html).toContain('Smart collections');
    expect(html).not.toContain('+ Organize');
    expect(html).toContain('Custom fields…');
  });

  it('omits single root folder visually when it has subfolders, and keeps multiple roots', () => {
    setLocale('en');
    const singleRootFolders: FolderEntry[] = [
      { id: 1, parentId: null, rootId: 1, name: 'C:\\SingleBooksRoot', books: 1 },
      { id: 2, parentId: 1, rootId: 1, name: 'PromotedChild', books: 5 },
    ];
    const singleHtml = renderSidebar({ info: baseInfo(1), folders: singleRootFolders });
    expect(singleHtml).not.toContain('SingleBooksRoot');
    expect(singleHtml).toContain('PromotedChild');
    expect(singleHtml).toContain('Manage folders…');

    const multiRootFolders: FolderEntry[] = [
      { id: 1, parentId: null, rootId: 1, name: 'C:\\RootAlpha', books: 1 },
      { id: 2, parentId: 1, rootId: 1, name: 'ChildA', books: 2 },
      { id: 3, parentId: null, rootId: 2, name: 'D:\\RootBeta', books: 4 },
    ];
    const multiHtml = renderSidebar({ info: baseInfo(2), folders: multiRootFolders });
    expect(multiHtml).toContain('RootAlpha');
    expect(multiHtml).toContain('RootBeta');
  });

  it('marks SidebarShell inert and aria-hidden when collapsed, keeping it mounted for smooth animation', () => {
    function Harness({ collapsed }: { collapsed: boolean }): React.JSX.Element {
      const ref = useRef<HTMLElement>(null);
      return (
        <SidebarShell collapsed={collapsed} width={240} resizeLabel="Resize sidebar" scrollRef={ref}>
          <Sidebar
            info={baseInfo(1)}
            folders={[]}
            taxonomy={emptyTaxonomy()}
            filter={{ kind: 'all' }}
            showEmptySections={false}
            showCounts={false}
            onFilter={() => {}}
            onChanged={() => {}}
            notice={() => {}}
            waiting={0}
            onManageFolders={() => {}}
            onOpenFields={() => {}}
            scrollRef={ref}
          />
        </SidebarShell>
      );
    }

    const expandedHtml = renderToStaticMarkup(<Harness collapsed={false} />);
    expect(expandedHtml).toContain('class="sidebar-shell"');
    expect(expandedHtml).toContain('tabindex="0"');
    expect(expandedHtml).not.toContain('inert=""');
    expect(expandedHtml).not.toContain('aria-hidden="true" role="separator"');

    const collapsedHtml = renderToStaticMarkup(<Harness collapsed={true} />);
    expect(collapsedHtml).toContain('class="sidebar-shell"');
    expect(collapsedHtml).toContain('inert=""');
    expect(collapsedHtml).toContain('aria-hidden="true"');
    expect(collapsedHtml).toContain('tabindex="-1"');
  });

  it('hides book counts when showCounts is false while keeping review waiting and broken smart badges', () => {
    setLocale('en');
    const taxonomy: Taxonomy = {
      categories: [{ id: 1, parentId: null, name: 'Physics', depth: 0, works: 5, total: 9 }],
      tags: [{ id: 2, name: 'Unread', color: null, works: 7 }],
      collections: [{ id: 3, name: 'Favorites', description: '', works: 4 }],
      smart: [
        { id: 4, name: 'Recent', rules: { match: 'all', rules: [] }, works: 6, broken: null },
        { id: 5, name: 'BrokenQuery', rules: { match: 'all', rules: [] }, works: 0, broken: 'syntax error' },
      ],
      fields: [],
    };
    const hiddenCountsHtml = renderSidebar({ taxonomy, showCounts: false, waiting: 3 });
    expect(hiddenCountsHtml).not.toContain('<span class="folder-count">12</span>');
    expect(hiddenCountsHtml).not.toContain('<span class="folder-count">9</span>');
    expect(hiddenCountsHtml).not.toContain('<span class="folder-count">7</span>');
    expect(hiddenCountsHtml).not.toContain('<span class="folder-count">4</span>');
    expect(hiddenCountsHtml).not.toContain('<span class="folder-count">6</span>');
    expect(hiddenCountsHtml).toContain('folder-count waiting">3</span>');
    expect(hiddenCountsHtml).toContain('class="broken"');

    const shownCountsHtml = renderSidebar({ taxonomy, showCounts: true, waiting: 3 });
    expect(shownCountsHtml).toContain('<span class="folder-count">12</span>');
    expect(shownCountsHtml).toContain('<span class="folder-count">9</span>');
    expect(shownCountsHtml).toContain('<span class="folder-count">7</span>');
    expect(shownCountsHtml).toContain('<span class="folder-count">4</span>');
    expect(shownCountsHtml).toContain('<span class="folder-count">6</span>');
  });

  it('marks InspectorShell closed, inert and aria-hidden when not open', () => {
    const openHtml = renderToStaticMarkup(
      <InspectorShell open>
        <div className="inspector">Details</div>
      </InspectorShell>,
    );
    expect(openHtml).toContain('class="inspector-shell"');
    expect(openHtml).not.toContain('inert=""');
    expect(openHtml).not.toContain('aria-hidden="true"');
    expect(openHtml).toContain('Details');

    const closedHtml = renderToStaticMarkup(
      <InspectorShell open={false}>{null}</InspectorShell>,
    );
    expect(closedHtml).toContain('class="inspector-shell closed"');
    expect(closedHtml).toContain('inert=""');
    expect(closedHtml).toContain('aria-hidden="true"');
  });
});
