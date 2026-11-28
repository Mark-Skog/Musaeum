/**
 * Pure, DOM-free state machine for the unified tab strip: a permanent Library home tab (`'home'`)
 * followed by deduplicated open book tabs, with neighbor focus on close, circular keyboard cycling,
 * direct index selection (`Ctrl+1..Ctrl+9`), and synchronization with library metadata edits/removals.
 */

import type { FlowMode } from '../reader/messages';
import type { BookRow, ReaderTarget } from './api';

export const HOME_TAB_ID = 'home' as const;
export type HomeTabId = typeof HOME_TAB_ID;
export type TabId = HomeTabId | string;

export interface HomeTab {
  readonly id: HomeTabId;
  readonly kind: 'home';
  readonly closable: false;
}

export const HOME_TAB: HomeTab = Object.freeze({
  id: HOME_TAB_ID,
  kind: 'home',
  closable: false,
});

/**
 * Extensible reader state snapshot reserved for Etapa 2B (WebGL2/compositor & flow suspension
 * and instant rehydration when switching between open book tabs).
 */
export interface ReaderTabSnapshot {
  readonly layout: 'fixed' | 'flow';
  readonly mode?: FlowMode;
  readonly page?: number;
  readonly zoom?: number;
  readonly spreads?: 'auto' | 'single' | 'double';
  readonly scrollOffset?: number;
  readonly readerCfi?: string | null;
  readonly progress?: number | null;
  readonly capturedAt?: number;
  readonly extra?: Readonly<Record<string, unknown>>;
}

export interface BookTab {
  /** Stable unique tab identifier (`doc:<documentId>`). */
  readonly id: string;
  /** Canonical open-book identifier (`BookRow.documentId`). */
  readonly documentId: number;
  /** Canonical library work identifier (`BookRow.workId`). */
  readonly workId: number;
  readonly title: string;
  readonly format?: string;
  /** Full `BookRow` descriptor when opened from the library. */
  readonly book?: BookRow;
  /** Optional deep-link destination (e.g. full-text search match) for the current open request. */
  readonly target: ReaderTarget | null;
  /** Monotonic counter incremented when a fresh `target` is requested on an open tab. */
  readonly targetSeq: number;
  /** Reserved for Etapa 2B tab suspension/rehydration. */
  readonly snapshot?: ReaderTabSnapshot;
}

export interface OpenBookTabInput {
  readonly documentId: number;
  readonly workId: number;
  readonly title: string;
  readonly format?: string;
  readonly book?: BookRow;
}

export interface TabsState {
  readonly home: HomeTab;
  readonly bookTabs: readonly BookTab[];
  readonly activeTabId: TabId;
}

/** Deterministic tab identifier for a library document. */
export function bookTabId(documentId: number): string {
  return `doc:${documentId}`;
}

/** Creates the initial tab state with only the immutable Home tab active. */
export function createInitialTabsState(): TabsState {
  return {
    home: HOME_TAB,
    bookTabs: [],
    activeTabId: HOME_TAB_ID,
  };
}

/** Ordered list of all open tab identifiers, always starting with `'home'`. */
export function orderedTabIds(state: TabsState): TabId[] {
  return [HOME_TAB_ID, ...state.bookTabs.map((tab) => tab.id)];
}

/** Returns the active `BookTab`, or `null` when the Home tab is active. */
export function activeBookTab(state: TabsState): BookTab | null {
  if (state.activeTabId === HOME_TAB_ID) {
    return null;
  }
  return state.bookTabs.find((tab) => tab.id === state.activeTabId) ?? null;
}

function asBookRow(input: OpenBookTabInput | BookRow): BookRow | undefined {
  if ('book' in input && input.book !== undefined) {
    return input.book;
  }
  if ('layout' in input && 'path' in input) {
    return input;
  }
  return undefined;
}

/**
 * Opens a book tab or activates its existing tab if already open (updating `title`, `workId`,
 * `format`, `book`, and `target` without duplicating or reordering existing tabs).
 */
export function openBookTab(
  state: TabsState,
  input: OpenBookTabInput | BookRow,
  target?: ReaderTarget | null,
): TabsState {
  const row = asBookRow(input);
  const nextBook =
    row !== undefined && (row.title !== input.title || row.workId !== input.workId)
      ? { ...row, title: input.title, workId: input.workId }
      : row;
  const existingIndex = state.bookTabs.findIndex((tab) => tab.documentId === input.documentId);

  if (existingIndex >= 0) {
    const current = state.bookTabs[existingIndex];
    const nextFormat = input.format ?? current.format;
    const mergedBook =
      nextBook ??
      (current.book !== undefined && (current.book.title !== input.title || current.book.workId !== input.workId)
        ? { ...current.book, title: input.title, workId: input.workId }
        : current.book);
    const nextTarget = target !== undefined ? target : current.target;
    const nextTargetSeq = target !== undefined && target !== null ? current.targetSeq + 1 : current.targetSeq;

    const tabChanged =
      current.title !== input.title ||
      current.workId !== input.workId ||
      current.format !== nextFormat ||
      current.book !== mergedBook ||
      current.target !== nextTarget ||
      current.targetSeq !== nextTargetSeq;

    if (!tabChanged && state.activeTabId === current.id) {
      return state;
    }

    const nextTabs = tabChanged
      ? state.bookTabs.map((tab, idx) =>
          idx === existingIndex
            ? {
                ...tab,
                workId: input.workId,
                title: input.title,
                format: nextFormat,
                book: mergedBook,
                target: nextTarget,
                targetSeq: nextTargetSeq,
              }
            : tab,
        )
      : state.bookTabs;

    return {
      ...state,
      bookTabs: nextTabs,
      activeTabId: current.id,
    };
  }

  const id = bookTabId(input.documentId);
  const created: BookTab = {
    id,
    documentId: input.documentId,
    workId: input.workId,
    title: input.title,
    format: input.format,
    book: nextBook,
    target: target ?? null,
    targetSeq: target != null ? 1 : 0,
  };

  return {
    ...state,
    bookTabs: [...state.bookTabs, created],
    activeTabId: id,
  };
}

/** Activates an existing tab (`'home'` or a `BookTab.id`). Unknown IDs are safe no-ops. */
export function activateTab(state: TabsState, tabId: TabId): TabsState {
  if (tabId === state.activeTabId) {
    return state;
  }
  if (tabId === HOME_TAB_ID) {
    return { ...state, activeTabId: HOME_TAB_ID };
  }
  if (!state.bookTabs.some((tab) => tab.id === tabId)) {
    return state;
  }
  return { ...state, activeTabId: tabId };
}

/**
 * Removes all book tabs matching `shouldClose`, preserving `'home'` and selecting the nearest
 * surviving neighbor (right first, then left, then `'home'`) when the active tab is closed.
 */
function closeTabsWhere(state: TabsState, shouldClose: (tab: BookTab) => boolean): TabsState {
  const removedIndices = new Set<number>();
  state.bookTabs.forEach((tab, idx) => {
    if (shouldClose(tab)) {
      removedIndices.add(idx);
    }
  });
  if (removedIndices.size === 0) {
    return state;
  }

  const nextBookTabs = state.bookTabs.filter((_, idx) => !removedIndices.has(idx));
  const activeIdx = state.bookTabs.findIndex((tab) => tab.id === state.activeTabId);

  if (activeIdx < 0 || !removedIndices.has(activeIdx)) {
    return {
      ...state,
      bookTabs: nextBookTabs,
    };
  }

  let nextActiveId: TabId = HOME_TAB_ID;
  for (let right = activeIdx + 1; right < state.bookTabs.length; right++) {
    if (!removedIndices.has(right)) {
      nextActiveId = state.bookTabs[right].id;
      break;
    }
  }
  if (nextActiveId === HOME_TAB_ID) {
    for (let left = activeIdx - 1; left >= 0; left--) {
      if (!removedIndices.has(left)) {
        nextActiveId = state.bookTabs[left].id;
        break;
      }
    }
  }

  return {
    ...state,
    bookTabs: nextBookTabs,
    activeTabId: nextActiveId,
  };
}

/**
 * Closes a book tab by `tabId`.
 * - Attempting to close `'home'` is always a safe no-op.
 * - Closing a background tab preserves `activeTabId`.
 * - Closing the active tab selects the tab immediately to the right, or else immediately to the
 *   left, or `'home'` when closing the last book tab.
 */
export function closeTab(state: TabsState, tabId: TabId): TabsState {
  if (tabId === HOME_TAB_ID) {
    return state;
  }
  return closeTabsWhere(state, (tab) => tab.id === tabId);
}

/** Closes whichever book tab is currently active (`Ctrl+W` / `Ctrl+F4`); no-op on `'home'`. */
export function closeActiveBookTab(state: TabsState): TabsState {
  return closeTab(state, state.activeTabId);
}

/** Cycles forward (`Ctrl+Tab`) circularly through `['home', ...bookTabs]`. */
export function nextTab(state: TabsState): TabsState {
  const ids = orderedTabIds(state);
  if (ids.length <= 1) {
    return state;
  }
  const current = ids.indexOf(state.activeTabId);
  const nextIndex = ((current >= 0 ? current : 0) + 1) % ids.length;
  return activateTab(state, ids[nextIndex]);
}

/** Cycles backward (`Ctrl+Shift+Tab`) circularly through `['home', ...bookTabs]`. */
export function prevTab(state: TabsState): TabsState {
  const ids = orderedTabIds(state);
  if (ids.length <= 1) {
    return state;
  }
  const current = ids.indexOf(state.activeTabId);
  const prevIndex = ((current >= 0 ? current : 0) - 1 + ids.length) % ids.length;
  return activateTab(state, ids[prevIndex]);
}

/**
 * Selects a tab by 1-based shortcut digit (`Ctrl+1`..`Ctrl+9`):
 * - `1` -> `'home'`
 * - `2..8` -> `bookTabs[0..6]` when open (no-op if out of range)
 * - `9` -> last open tab (`bookTabs.at(-1)` when any book is open, otherwise `'home'`)
 */
export function selectTabByIndex(state: TabsState, shortcutDigit: number): TabsState {
  if (!Number.isInteger(shortcutDigit) || shortcutDigit < 1 || shortcutDigit > 9) {
    return state;
  }
  if (shortcutDigit === 1) {
    return activateTab(state, HOME_TAB_ID);
  }
  if (shortcutDigit === 9) {
    const last = state.bookTabs[state.bookTabs.length - 1];
    return activateTab(state, last !== undefined ? last.id : HOME_TAB_ID);
  }
  const target = state.bookTabs[shortcutDigit - 2];
  return target !== undefined ? activateTab(state, target.id) : state;
}

/** Closes any open tabs belonging to a deleted/removed `workId`. */
export function closeTabsForWork(state: TabsState, workId: number): TabsState {
  return closeTabsWhere(state, (tab) => tab.workId === workId);
}

/** Closes an open tab for a specific `documentId`. */
export function closeTabForDocument(state: TabsState, documentId: number): TabsState {
  return closeTabsWhere(state, (tab) => tab.documentId === documentId);
}

/** Closes any open tabs whose `book.folderId` belongs to a removed root folder tree. */
export function closeTabsForFolders(state: TabsState, folderIds: ReadonlySet<number>): TabsState {
  if (folderIds.size === 0) {
    return state;
  }
  return closeTabsWhere(state, (tab) => tab.book !== undefined && folderIds.has(tab.book.folderId));
}

/** Synchronizes open tab titles when a work's title is edited in the Library Inspector. */
export function updateWorkTitleInTabs(state: TabsState, workId: number, nextTitle: string): TabsState {
  const trimmed = nextTitle.trim();
  if (trimmed === '') {
    return state;
  }
  let changed = false;
  const nextBookTabs = state.bookTabs.map((tab) => {
    if (tab.workId !== workId) {
      return tab;
    }
    const nextBook = tab.book !== undefined && tab.book.title !== trimmed ? { ...tab.book, title: trimmed } : tab.book;
    if (tab.title === trimmed && nextBook === tab.book) {
      return tab;
    }
    changed = true;
    return {
      ...tab,
      title: trimmed,
      book: nextBook,
    };
  });
  return changed ? { ...state, bookTabs: nextBookTabs } : state;
}

/** Updates `workId` (and optionally `title`) on open tabs when works are merged in the library. */
export function mergeWorksInTabs(
  state: TabsState,
  keepWorkId: number,
  mergedWorkIds: readonly number[],
  keptTitle?: string,
): TabsState {
  const merged = new Set(mergedWorkIds.filter((id) => id !== keepWorkId));
  if (merged.size === 0) {
    return keptTitle !== undefined ? updateWorkTitleInTabs(state, keepWorkId, keptTitle) : state;
  }
  const title = keptTitle?.trim();
  let changed = false;
  const nextBookTabs = state.bookTabs.map((tab) => {
    const affected = tab.workId === keepWorkId || merged.has(tab.workId);
    if (!affected) {
      return tab;
    }
    const nextTitle = title !== undefined && title !== '' ? title : tab.title;
    const nextBook =
      tab.book !== undefined && (tab.book.workId !== keepWorkId || tab.book.title !== nextTitle)
        ? { ...tab.book, workId: keepWorkId, title: nextTitle }
        : tab.book;
    if (tab.workId === keepWorkId && tab.title === nextTitle && nextBook === tab.book) {
      return tab;
    }
    changed = true;
    return {
      ...tab,
      workId: keepWorkId,
      title: nextTitle,
      book: nextBook,
    };
  });
  return changed ? { ...state, bookTabs: nextBookTabs } : state;
}

/** Clears a one-shot navigation target once the reader has jumped to it. */
export function clearTabTarget(state: TabsState, tabId: string): TabsState {
  const idx = state.bookTabs.findIndex((tab) => tab.id === tabId);
  if (idx < 0 || state.bookTabs[idx].target === null) {
    return state;
  }
  return {
    ...state,
    bookTabs: state.bookTabs.map((tab, i) => (i === idx ? { ...tab, target: null } : tab)),
  };
}

/** Stores or clears an Etapa 2B reader snapshot on an open book tab. */
export function setTabSnapshot(
  state: TabsState,
  tabId: string,
  snapshot: ReaderTabSnapshot | undefined,
): TabsState {
  const idx = state.bookTabs.findIndex((tab) => tab.id === tabId);
  if (idx < 0 || state.bookTabs[idx].snapshot === snapshot) {
    return state;
  }
  return {
    ...state,
    bookTabs: state.bookTabs.map((tab, i) => (i === idx ? { ...tab, snapshot } : tab)),
  };
}
