/**
 * Pure sidebar layout, folder-tree presentation, section visibility, and multi-folder notice helpers.
 */

import type { AddRootsResult, FolderEntry, Taxonomy } from '../api';
import { compareNames, tn } from '../i18n';

export const SIDEBAR_MIN_WIDTH = 180;
export const SIDEBAR_MAX_WIDTH = 420;
export const SIDEBAR_DEFAULT_WIDTH = 240;
export const SIDEBAR_STEP = 16;

export const COVER_MIN_WIDTH = 120;
export const COVER_MAX_WIDTH = 260;
export const COVER_DEFAULT_WIDTH = 168;
export const COVER_ZOOM_STEP = 12;

export function clampCoverWidth(width: number): number {
  if (!Number.isFinite(width)) return COVER_DEFAULT_WIDTH;
  return Math.min(COVER_MAX_WIDTH, Math.max(COVER_MIN_WIDTH, Math.round(width)));
}

export function stepCoverWidth(width: number, deltaSteps: number): number {
  const clamped = clampCoverWidth(width);
  return clampCoverWidth(clamped + deltaSteps * COVER_ZOOM_STEP);
}

/** Fixed card height (2:3 cover plus 2-line title, author and padding) for a given cover width. */
export function cardHeightForCoverWidth(coverWidth: number): number {
  const w = clampCoverWidth(coverWidth);
  const coverHeight = Math.round((w - 12) * 1.5);
  return coverHeight + 82;
}

/** Clamps a sidebar width preference into `[SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH]`. */
export function clampSidebarWidth(value: unknown): number {
  if (typeof value !== 'number' || !Number.isFinite(value)) {
    return SIDEBAR_DEFAULT_WIDTH;
  }
  return Math.min(SIDEBAR_MAX_WIDTH, Math.max(SIDEBAR_MIN_WIDTH, Math.round(value)));
}

/** Computes a clamped sidebar width from a starting width and horizontal pointer delta. */
export function resizeSidebarWidth(startWidth: number, deltaX: number): number {
  if (!Number.isFinite(deltaX)) {
    return clampSidebarWidth(startWidth);
  }
  return clampSidebarWidth(clampSidebarWidth(startWidth) + deltaX);
}

/** Returns true when keyboard focus is inside a text-editable control. */
export function isEditableTarget(target: EventTarget | null): boolean {
  if (target === null || typeof target !== 'object') {
    return false;
  }
  const element = target as {
    tagName?: string;
    isContentEditable?: boolean;
    getAttribute?: (name: string) => string | null;
    closest?: (selector: string) => unknown;
  };
  const tag = element.tagName?.toUpperCase();
  if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') {
    return true;
  }
  if (element.isContentEditable === true || element.getAttribute?.('contenteditable') === 'true' || element.getAttribute?.('role') === 'textbox') {
    return true;
  }
  return element.closest?.('input, textarea, select, [contenteditable="true"], [role="textbox"]') != null;
}

export interface FolderNode {
  entry: FolderEntry;
  children: FolderNode[];
  total: number;
}

/** A root's last folder name, which is what tells roots apart; the whole path is its tooltip. */
export function shortName(path: string): string {
  const parts = path.split(/[\\/]/).filter((part) => part.length > 0);
  return parts.at(-1) ?? path;
}

/** Builds a sorted folder tree and prunes subtrees that contain zero books. */
export function buildFolderTree(folders: FolderEntry[]): FolderNode[] {
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
    node.children.sort((a, b) => compareNames(a.entry.name, b.entry.name));
    node.total = node.entry.books + node.children.reduce((total, child) => total + sum(child), 0);
    return node.total;
  };
  roots.forEach(sum);
  const prune = (list: FolderNode[]): FolderNode[] => list.filter((node) => node.total > 0).map((node) => ({ ...node, children: prune(node.children) }));
  return prune(roots);
}

/**
 * Resolves the folder nodes displayed in the sidebar.
 * When the library tracks exactly 1 root and it has visible subfolders, those subfolders are
 * promoted to the first level so the redundant single root is omitted visually.
 */
export function resolveVisibleFolders(folders: FolderEntry[], activeRootCount: number): { nodes: FolderNode[]; singleRootPromoted: boolean } {
  const roots = buildFolderTree(folders);
  if (activeRootCount === 1 && roots.length === 1 && roots[0].children.length > 0) {
    return { nodes: roots[0].children, singleRootPromoted: true };
  }
  return { nodes: roots, singleRootPromoted: false };
}

export interface SidebarSectionVisibility {
  categories: boolean;
  tags: boolean;
  collections: boolean;
  smart: boolean;
  hasHidden: boolean;
}

/** Determines which taxonomy sections appear in the sidebar. */
export function visibleSidebarSections(taxonomy: Taxonomy, showEmpty: boolean): SidebarSectionVisibility {
  const categories = showEmpty || taxonomy.categories.length > 0;
  const tags = showEmpty || taxonomy.tags.length > 0;
  const collections = showEmpty || taxonomy.collections.length > 0;
  const smart = showEmpty || taxonomy.smart.length > 0;
  return {
    categories,
    tags,
    collections,
    smart,
    hasHidden: !(categories && tags && collections && smart),
  };
}

/** Formats a clear user-facing notice when one or more selected folders are rejected. */
export function formatAddRootsNotice(result: AddRootsResult | null): string | null {
  if (result === null || result.rejected.length === 0) {
    return null;
  }
  const reasons = result.rejected.map((item) => item.reason).join(' · ');
  if (result.added.length > 0) {
    return tn('library.foldersAddedWithRefused', result.added.length, { reasons });
  }
  return reasons;
}
