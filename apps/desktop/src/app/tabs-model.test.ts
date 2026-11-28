import { describe, expect, it } from 'vitest';

import type { BookRow, ReaderTarget } from './api';
import {
  activateTab,
  activeBookTab,
  bookTabId,
  clearTabTarget,
  closeActiveBookTab,
  closeAllBookTabs,
  closeTab,
  closeTabForDocument,
  closeTabsForFolders,
  closeTabsForWork,
  closeTabsForWorks,
  createInitialTabsState,
  deriveFixedFitMode,
  HOME_TAB_ID,
  mergeWorksInTabs,
  nextTab,
  openBookTab,
  orderedTabIds,
  prevTab,
  type ReaderTabSnapshot,
  resolveFixedSnapshotZoom,
  selectTabByIndex,
  setTabSnapshot,
  updateWorkTitleInTabs,
} from './tabs-model';

function sampleBook(id: number, title: string, format = 'pdf', workId = id * 10, folderId = 1): BookRow {
  return {
    documentId: id,
    workId,
    title,
    creators: 'Author',
    layout: format === 'epub' ? 'flow' : 'fixed',
    format,
    instanceId: id,
    revisionId: id,
    folderId,
    path: `C:\\Books\\${title}.${format}`,
    state: 'ok',
    blake3: null,
    size: 1024,
    progress: null,
    lastReadAt: null,
  };
}

describe('tabs-model pure state machine', () => {
  it('starts on the immutable non-closable Home tab', () => {
    const initial = createInitialTabsState();
    expect(initial.home).toEqual({ id: 'home', kind: 'home', closable: false });
    expect(Object.isFrozen(initial.home)).toBe(true);
    expect(initial.activeTabId).toBe(HOME_TAB_ID);
    expect(initial.bookTabs).toEqual([]);
    expect(activeBookTab(initial)).toBeNull();
    expect(orderedTabIds(initial)).toEqual(['home']);
  });

  it('opens books in order and deduplicates reopening an already-open book while refreshing its title and target', () => {
    const dune = sampleBook(1, 'Dune', 'epub', 100);
    const solaris = sampleBook(2, 'Solaris', 'pdf', 200);

    let state = createInitialTabsState();
    state = openBookTab(state, dune);
    expect(state.bookTabs).toHaveLength(1);
    expect(state.activeTabId).toBe(bookTabId(1));
    expect(activeBookTab(state)?.title).toBe('Dune');

    state = openBookTab(state, solaris);
    expect(state.bookTabs.map((t) => t.documentId)).toEqual([1, 2]);
    expect(state.activeTabId).toBe(bookTabId(2));

    // Reopening Dune activates existing first tab without duplicating or reordering
    const hit: ReaderTarget = { kind: 'flow', document: 1, unit: 2, position: 15, length: 4 };
    const renamedDune = { ...dune, title: 'Dune (Edição Definitiva)' };
    state = openBookTab(state, renamedDune, hit);
    expect(state.bookTabs).toHaveLength(2);
    expect(state.bookTabs.map((t) => t.documentId)).toEqual([1, 2]);
    expect(state.activeTabId).toBe(bookTabId(1));
    expect(state.bookTabs[0].title).toBe('Dune (Edição Definitiva)');
    expect(state.bookTabs[0].book?.title).toBe('Dune (Edição Definitiva)');
    expect(state.bookTabs[0].target).toEqual(hit);
    expect(state.bookTabs[0].targetSeq).toBe(1);

    // Clearing consumed target leaves tab open at current active position
    state = clearTabTarget(state, bookTabId(1));
    expect(state.bookTabs[0].target).toBeNull();
  });

  it('protects Home from closing and handles active vs background tab closing with neighbor focus', () => {
    let state = createInitialTabsState();
    expect(closeTab(state, HOME_TAB_ID)).toBe(state);
    expect(closeActiveBookTab(state)).toBe(state);

    state = openBookTab(state, sampleBook(1, 'Book A'));
    state = openBookTab(state, sampleBook(2, 'Book B'));
    state = openBookTab(state, sampleBook(3, 'Book C'));
    state = openBookTab(state, sampleBook(4, 'Book D'));

    // Activate Book B (`doc:2`), then close background tab Book D (`doc:4`) -> active stays `doc:2`
    state = activateTab(state, bookTabId(2));
    const afterBgClose = closeTab(state, bookTabId(4));
    expect(afterBgClose.activeTabId).toBe(bookTabId(2));
    expect(afterBgClose.bookTabs.map((t) => t.documentId)).toEqual([1, 2, 3]);

    // Close active middle tab Book B (`doc:2`) -> selects immediate right neighbor Book C (`doc:3`)
    const afterMiddleClose = closeActiveBookTab(afterBgClose);
    expect(afterMiddleClose.activeTabId).toBe(bookTabId(3));
    expect(afterMiddleClose.bookTabs.map((t) => t.documentId)).toEqual([1, 3]);

    // Close active rightmost tab Book C (`doc:3`) -> no right neighbor, selects immediate left Book A (`doc:1`)
    const afterRightmostClose = closeActiveBookTab(afterMiddleClose);
    expect(afterRightmostClose.activeTabId).toBe(bookTabId(1));
    expect(afterRightmostClose.bookTabs.map((t) => t.documentId)).toEqual([1]);

    // Attempt to close Home while a book is open -> safe no-op
    expect(closeTab(afterRightmostClose, HOME_TAB_ID)).toBe(afterRightmostClose);

    // Close the last open book tab -> returns to `'home'`
    const afterLastClose = closeActiveBookTab(afterRightmostClose);
    expect(afterLastClose.activeTabId).toBe(HOME_TAB_ID);
    expect(afterLastClose.bookTabs).toEqual([]);
  });

  it('cycles circularly with nextTab (Ctrl+Tab) and prevTab (Ctrl+Shift+Tab)', () => {
    const empty = createInitialTabsState();
    expect(nextTab(empty)).toBe(empty);
    expect(prevTab(empty)).toBe(empty);

    let state = empty;
    state = openBookTab(state, sampleBook(10, 'Alpha'));
    state = openBookTab(state, sampleBook(20, 'Beta'));
    state = activateTab(state, HOME_TAB_ID);

    // Forward: home -> doc:10 -> doc:20 -> home
    state = nextTab(state);
    expect(state.activeTabId).toBe(bookTabId(10));
    state = nextTab(state);
    expect(state.activeTabId).toBe(bookTabId(20));
    state = nextTab(state);
    expect(state.activeTabId).toBe(HOME_TAB_ID);

    // Backward: home -> doc:20 -> doc:10 -> home
    state = prevTab(state);
    expect(state.activeTabId).toBe(bookTabId(20));
    state = prevTab(state);
    expect(state.activeTabId).toBe(bookTabId(10));
    state = prevTab(state);
    expect(state.activeTabId).toBe(HOME_TAB_ID);
  });

  it('selects tabs directly via Ctrl+1..Ctrl+9 rules', () => {
    let state = createInitialTabsState();
    expect(selectTabByIndex(state, 9).activeTabId).toBe(HOME_TAB_ID);
    expect(selectTabByIndex(state, 2)).toBe(state);

    for (let i = 1; i <= 10; i++) {
      state = openBookTab(state, sampleBook(i, `Volume ${i}`));
    }
    expect(state.activeTabId).toBe(bookTabId(10));

    // Ctrl+1 -> Home
    state = selectTabByIndex(state, 1);
    expect(state.activeTabId).toBe(HOME_TAB_ID);

    // Ctrl+2..Ctrl+8 -> bookTabs[0..6]
    for (let digit = 2; digit <= 8; digit++) {
      state = selectTabByIndex(state, digit);
      expect(state.activeTabId).toBe(bookTabId(digit - 1));
    }

    // Ctrl+9 -> always the last open tab (Volume 10, even with 10 books open)
    state = selectTabByIndex(state, 9);
    expect(state.activeTabId).toBe(bookTabId(10));

    // Invalid digits are safe no-ops
    expect(selectTabByIndex(state, 0)).toBe(state);
    expect(selectTabByIndex(state, 10)).toBe(state);
  });

  it('synchronizes open tabs with work title edits, work deletions/merges, folder removals, and Etapa 2B snapshots', () => {
    let state = createInitialTabsState();
    // Work 500 has two open formats (PDF + EPUB), Work 600 has one
    state = openBookTab(state, sampleBook(1, 'Ficciones', 'pdf', 500, 10));
    state = openBookTab(state, sampleBook(2, 'Ficciones', 'epub', 500, 10));
    state = openBookTab(state, sampleBook(3, 'El Aleph', 'pdf', 600, 20));

    // Title edit in Library Inspector updates all tabs of work 500
    state = updateWorkTitleInTabs(state, 500, 'Ficções');
    expect(state.bookTabs[0].title).toBe('Ficções');
    expect(state.bookTabs[1].title).toBe('Ficções');
    expect(state.bookTabs[1].book?.title).toBe('Ficções');
    expect(state.bookTabs[2].title).toBe('El Aleph');

    // Snapshot field reserved for Etapa 2B
    const snap: ReaderTabSnapshot = {
      layout: 'fixed',
      mode: 'continuous',
      page: 12,
      pageCount: 240,
      u: 0.25,
      v: 0.6,
      scrollOffset: 0.6,
      zoom: 1.25,
      fitMode: deriveFixedFitMode('continuous', 1.25),
      spreads: 'double',
      progress: 0.05,
      tocOpen: true,
      annotationsOpen: true,
      annotationSearch: 'labirinto',
    };
    expect(snap.fitMode).toBe('manual');
    expect(deriveFixedFitMode('paginated', 1)).toBe('page');
    expect(deriveFixedFitMode('continuous', 1)).toBe('width');
    expect(resolveFixedSnapshotZoom(snap)).toBe(1.25);
    expect(resolveFixedSnapshotZoom({ layout: 'fixed', fitMode: 'width' })).toBe(1);

    state = setTabSnapshot(state, bookTabId(1), snap);
    expect(state.bookTabs[0].snapshot).toEqual(snap);
    expect(state.bookTabs[0].book?.progress).toBe(0.05);

    // Multi-work batch deletion closes matching background/active tabs cleanly
    const afterBatchDelete = closeTabsForWorks(state, new Set([500]));
    expect(afterBatchDelete.bookTabs.map((t) => t.documentId)).toEqual([3]);

    // Switching library databases closes all open book tabs and returns to Home
    const resetAll = closeAllBookTabs(state);
    expect(resetAll.bookTabs).toEqual([]);
    expect(resetAll.activeTabId).toBe(HOME_TAB_ID);

    // Activate `doc:2` (middle tab, work 500) and delete work 500 -> both `doc:1` and `doc:2` close, active moves right to `doc:3`
    state = activateTab(state, bookTabId(2));
    const afterWorkDelete = closeTabsForWork(state, 500);
    expect(afterWorkDelete.bookTabs.map((t) => t.documentId)).toEqual([3]);
    expect(afterWorkDelete.activeTabId).toBe(bookTabId(3));

    // Removing folder 20 closes `doc:3` and returns to `'home'`
    const afterFolderRemove = closeTabsForFolders(afterWorkDelete, new Set([20]));
    expect(afterFolderRemove.bookTabs).toEqual([]);
    expect(afterFolderRemove.activeTabId).toBe(HOME_TAB_ID);

    // Merging works updates workId and title on open tabs
    let merged = mergeWorksInTabs(state, 500, [500, 600], 'Obras Completas');
    expect(merged.bookTabs.every((t) => t.workId === 500 && t.title === 'Obras Completas')).toBe(true);
    merged = closeTabForDocument(merged, 2);
    expect(merged.bookTabs.map((t) => t.documentId)).toEqual([1, 3]);
  });
});
