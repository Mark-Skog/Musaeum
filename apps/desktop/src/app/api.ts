/** What the app asks the host, typed as the Rust side answers. */

import { invoke } from '@tauri-apps/api/core';

import type { FlowMode, SectionPlan } from '../reader/messages';

export interface RootSummary {
  id: number;
  path: string;
  status: string;
}

export interface LibraryInfo {
  open: boolean;
  location: string | null;
  roots: RootSummary[];
  books: number;
  indexing: boolean;
  error: string | null;
}

export interface BookRow {
  documentId: number;
  workId: number;
  title: string;
  creators: string | null;
  layout: 'fixed' | 'flow';
  format: string;
  instanceId: number;
  revisionId: number;
  folderId: number;
  path: string;
  state: string;
  blake3: string | null;
  size: number;
  progress: number | null;
  lastReadAt: number | null;
}

export interface BrowsePage {
  total: number;
  offset: number;
  rows: BookRow[];
}

export interface FolderEntry {
  id: number;
  parentId: number | null;
  rootId: number;
  name: string;
  books: number;
}

export interface NavPoint {
  label: string;
  href: string | null;
  cfi: string | null;
  kind?: string;
  children?: NavPoint[];
}

export interface SavedFlow {
  readerCfi: string;
  found: string;
  locator: unknown;
  mode: FlowMode;
  pageTurnAnimation: boolean;
}

export interface FlowInfo {
  origin: string;
  readerUrl: string;
  documentId: number;
  title: string;
  creators: string[];
  language: string | null;
  toc: NavPoint[];
  sections: SectionPlan[];
  totalText: number;
  saved: SavedFlow | null;
  cached: boolean;
  openMs: number;
}

export interface FlowPosition {
  locator: { locations?: { totalProgression?: number }; title?: string } & Record<string, unknown>;
  progress: number;
  saved: boolean;
}

export const api = {
  libraryInfo: () => invoke<LibraryInfo>('library_info'),
  chooseRoot: () => invoke<LibraryInfo | null>('library_choose_root'),
  addRoot: (path: string) => invoke<LibraryInfo>('library_add_root', { path }),
  chooseExisting: () => invoke<LibraryInfo | null>('library_choose_existing'),
  browse: (query: { text: string; folder: number | null; offset: number; limit: number }) =>
    invoke<BrowsePage>('library_browse', { query }),
  folders: () => invoke<FolderEntry[]>('library_folders'),
  setting: <T>(key: string) => invoke<T | null>('reader_setting', { key }),
  setSetting: (key: string, value: unknown) => invoke<void>('set_reader_setting', { key, value }),
  openFlow: (document: number) =>
    invoke<FlowInfo>('open_flow', { hostOrigin: location.origin, args: { document } }),
  saveFlowPosition: (
    document: number,
    position: { readerCfi: string; mode: FlowMode; pageTurnAnimation: boolean },
  ) => invoke<FlowPosition>('save_flow_position', { document, position }),
  closeBook: () => invoke<void>('close_book'),
  /** Opens a book's web link in the default browser; the host checks the address again. */
  openInBrowser: (url: string) => invoke<void>('open_in_browser', { url }),
};

/** Whether the default browser can be given this address: `http` and `https` only. */
export function isWebAddress(href: string): boolean {
  return /^https?:\/\/[^\s@/?#]+/i.test(href);
}

/** Where a cover is served, by the file's hash; `version` changes once a new one is drawn. */
export function coverUrl(blake3: string, version: number): string {
  return `http://thumb.localhost/${blake3}.jpg${version > 0 ? `?v=${version}` : ''}`;
}
