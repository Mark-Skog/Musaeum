import { describe, expect, it } from 'vitest';

import type { AddRootsResult, FolderEntry, Taxonomy } from '../api';
import { setLocale } from '../i18n';
import {
  cardHeightForCoverWidth,
  clampCoverWidth,
  clampSidebarWidth,
  COVER_DEFAULT_WIDTH,
  COVER_MAX_WIDTH,
  COVER_MIN_WIDTH,
  COVER_ZOOM_STEP,
  formatAddRootsNotice,
  isEditableTarget,
  resizeSidebarWidth,
  resolveVisibleFolders,
  SIDEBAR_DEFAULT_WIDTH,
  SIDEBAR_MAX_WIDTH,
  SIDEBAR_MIN_WIDTH,
  stepCoverWidth,
  visibleSidebarSections,
} from './sidebar-model';

const emptyTaxonomy = (): Taxonomy => ({
  categories: [],
  tags: [],
  collections: [],
  smart: [],
  fields: [],
});

const folder = (id: number, parentId: number | null, rootId: number, name: string, books: number): FolderEntry => ({
  id,
  parentId,
  rootId,
  name,
  books,
});

describe('sidebar width clamping and resizing', () => {
  it('clamps below min, above max, and falls back for invalid values', () => {
    expect(clampSidebarWidth(undefined)).toBe(SIDEBAR_DEFAULT_WIDTH);
    expect(clampSidebarWidth(null)).toBe(SIDEBAR_DEFAULT_WIDTH);
    expect(clampSidebarWidth('240')).toBe(SIDEBAR_DEFAULT_WIDTH);
    expect(clampSidebarWidth(Number.NaN)).toBe(SIDEBAR_DEFAULT_WIDTH);
    expect(clampSidebarWidth(Number.POSITIVE_INFINITY)).toBe(SIDEBAR_DEFAULT_WIDTH);
    expect(clampSidebarWidth(-10)).toBe(SIDEBAR_MIN_WIDTH);
    expect(clampSidebarWidth(100)).toBe(SIDEBAR_MIN_WIDTH);
    expect(clampSidebarWidth(240.4)).toBe(240);
    expect(clampSidebarWidth(600)).toBe(SIDEBAR_MAX_WIDTH);
  });

  it('computes resized width within bounds', () => {
    expect(resizeSidebarWidth(240, 40)).toBe(280);
    expect(resizeSidebarWidth(240, -100)).toBe(SIDEBAR_MIN_WIDTH);
    expect(resizeSidebarWidth(350, 200)).toBe(SIDEBAR_MAX_WIDTH);
    expect(resizeSidebarWidth(240, Number.NaN)).toBe(240);
  });

  it('detects editable targets so Ctrl+B does not hijack text editing', () => {
    expect(isEditableTarget(null)).toBe(false);
    expect(isEditableTarget({ tagName: 'INPUT' } as unknown as EventTarget)).toBe(true);
    expect(isEditableTarget({ tagName: 'TEXTAREA' } as unknown as EventTarget)).toBe(true);
    expect(isEditableTarget({ tagName: 'DIV', isContentEditable: true } as unknown as EventTarget)).toBe(true);
    expect(isEditableTarget({ tagName: 'DIV', getAttribute: (name: string) => (name === 'role' ? 'textbox' : null) } as unknown as EventTarget)).toBe(true);
    expect(isEditableTarget({ tagName: 'DIV', isContentEditable: false } as unknown as EventTarget)).toBe(false);
  });
});

describe('single-root folder promotion vs. multiple roots', () => {
  it('promotes child folders when there is 1 root with subfolders', () => {
    const folders: FolderEntry[] = [
      folder(1, null, 1, 'C:\\Users\\Mark\\Books', 2),
      folder(2, 1, 1, 'Fiction', 4),
      folder(3, 1, 1, 'Science', 3),
      folder(4, 3, 1, 'Physics', 1),
      folder(5, 1, 1, 'Empty', 0),
    ];

    const resolved = resolveVisibleFolders(folders, 1);
    expect(resolved.singleRootPromoted).toBe(true);
    expect(resolved.nodes.map((node) => node.entry.name)).toEqual(['Fiction', 'Science']);
    expect(resolved.nodes[1].total).toBe(4);
    expect(resolved.nodes[1].children.map((node) => node.entry.name)).toEqual(['Physics']);
  });

  it('keeps the root node when a single root has no visible subfolders', () => {
    const folders: FolderEntry[] = [
      folder(1, null, 1, 'C:\\Books', 5),
      folder(2, 1, 1, 'Empty', 0),
    ];

    const resolved = resolveVisibleFolders(folders, 1);
    expect(resolved.singleRootPromoted).toBe(false);
    expect(resolved.nodes.map((node) => node.entry.name)).toEqual(['C:\\Books']);
    expect(resolved.nodes[0].total).toBe(5);
  });

  it('keeps every root node when 2 or more roots are monitored', () => {
    const folders: FolderEntry[] = [
      folder(1, null, 1, 'C:\\Books', 0),
      folder(2, 1, 1, 'Fiction', 4),
      folder(3, null, 2, 'D:\\Comics', 3),
    ];

    const resolved = resolveVisibleFolders(folders, 2);
    expect(resolved.singleRootPromoted).toBe(false);
    expect(resolved.nodes.map((node) => node.entry.id)).toEqual([1, 3]);
    expect(resolved.nodes[0].children.map((node) => node.entry.name)).toEqual(['Fiction']);
  });

  it('does not promote when 2 roots exist in library info even if one root has zero books', () => {
    const folders: FolderEntry[] = [
      folder(1, null, 1, 'C:\\Books', 0),
      folder(2, 1, 1, 'Fiction', 4),
      folder(3, null, 2, 'D:\\EmptyRoot', 0),
    ];

    const resolved = resolveVisibleFolders(folders, 2);
    expect(resolved.singleRootPromoted).toBe(false);
    expect(resolved.nodes.map((node) => node.entry.id)).toEqual([1]);
  });
});

describe('hiding empty sidebar sections', () => {
  it('hides all empty sections by default and reveals each once populated', () => {
    const tax = emptyTaxonomy();
    expect(visibleSidebarSections(tax, false)).toEqual({
      categories: false,
      tags: false,
      collections: false,
      smart: false,
      hasHidden: true,
    });

    tax.tags.push({ id: 1, name: 'Sci-Fi', color: null, works: 2 });
    expect(visibleSidebarSections(tax, false)).toEqual({
      categories: false,
      tags: true,
      collections: false,
      smart: false,
      hasHidden: true,
    });

    tax.categories.push({ id: 1, parentId: null, name: 'Science', depth: 0, works: 1, total: 1 });
    tax.collections.push({ id: 1, name: 'Reading list', description: '', works: 1 });
    tax.smart.push({ id: 1, name: 'Unread', rules: { match: 'all', rules: [] }, works: 3, broken: null });
    expect(visibleSidebarSections(tax, false)).toEqual({
      categories: true,
      tags: true,
      collections: true,
      smart: true,
      hasHidden: false,
    });
  });

  it('shows all sections when showEmpty is enabled', () => {
    expect(visibleSidebarSections(emptyTaxonomy(), true)).toEqual({
      categories: true,
      tags: true,
      collections: true,
      smart: true,
      hasHidden: false,
    });
  });
});

describe('multi-folder addition notices', () => {
  const emptyInfo = { open: true, location: null, roots: [], books: 0, indexing: false, error: null };

  it('returns null when cancelled or all folders succeed', () => {
    expect(formatAddRootsNotice(null)).toBeNull();
    expect(formatAddRootsNotice({ info: emptyInfo, added: ['C:\\A', 'C:\\B'], rejected: [] })).toBeNull();
  });

  it('formats partial additions with rejected reasons in pt-BR and en', () => {
    const partial: AddRootsResult = {
      info: emptyInfo,
      added: ['C:\\A'],
      rejected: [{ path: 'C:\\A\\Sub', reason: 'overlaps C:\\A' }],
    };
    setLocale('pt-BR');
    expect(formatAddRootsNotice(partial)).toBe('1 pasta adicionada. Recusadas: overlaps C:\\A');
    setLocale('en');
    expect(formatAddRootsNotice(partial)).toBe('1 folder added. Refused: overlaps C:\\A');

    const onlyRejected: AddRootsResult = {
      info: emptyInfo,
      added: [],
      rejected: [
        { path: 'C:\\Lib', reason: 'holds library' },
        { path: 'C:\\Sub', reason: 'overlaps root' },
      ],
    };
    expect(formatAddRootsNotice(onlyRejected)).toBe('holds library · overlaps root');
  });
});

describe('fixed cover sizing and zoom stepping', () => {
  it('clamps cover width and ignores invalid inputs', () => {
    expect(clampCoverWidth(Number.NaN)).toBe(COVER_DEFAULT_WIDTH);
    expect(clampCoverWidth(40)).toBe(COVER_MIN_WIDTH);
    expect(clampCoverWidth(999)).toBe(COVER_MAX_WIDTH);
    expect(clampCoverWidth(190.4)).toBe(190);
  });

  it('steps cover width cleanly in zoom increments within bounds', () => {
    expect(stepCoverWidth(COVER_DEFAULT_WIDTH, 1)).toBe(COVER_DEFAULT_WIDTH + COVER_ZOOM_STEP);
    expect(stepCoverWidth(COVER_DEFAULT_WIDTH, -1)).toBe(COVER_DEFAULT_WIDTH - COVER_ZOOM_STEP);
    expect(stepCoverWidth(COVER_MIN_WIDTH, -3)).toBe(COVER_MIN_WIDTH);
    expect(stepCoverWidth(COVER_MAX_WIDTH, 3)).toBe(COVER_MAX_WIDTH);
  });

  it('keeps 2:3 cover height plus metadata room proportional to cover width', () => {
    const small = cardHeightForCoverWidth(COVER_MIN_WIDTH);
    const medium = cardHeightForCoverWidth(COVER_DEFAULT_WIDTH);
    const large = cardHeightForCoverWidth(COVER_MAX_WIDTH);
    expect(small).toBeLessThan(medium);
    expect(medium).toBeLessThan(large);
    expect(medium).toBeGreaterThan(COVER_DEFAULT_WIDTH * 1.5);
  });
});
