/** What the app asks the host, typed as the Rust side answers. */

import type { RegionSelection } from '../compositor/annotator';
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

export type AnnotationKind = 'highlight' | 'underline' | 'note';
export type AnchorState = 'exact' | 'approximate' | 'orphan';

/** An annotation as the library keeps it (`musaeum_library::Annotation`). */
export interface Annotation {
  id: number;
  uuid: string;
  documentId: number;
  revisionId: number;
  kind: AnnotationKind;
  color: string;
  note: string;
  quote: string | null;
  quoteSource: 'text_layer' | 'flow' | 'ocr' | 'none';
  progression: number | null;
  anchorState: AnchorState;
  createdAt: number;
  updatedAt: number;
  deletedAt: number | null;
  segments: { ordinal: number; page: number | null; href: string | null; anchorState: AnchorState }[];
}

/** The text under a region drawn on the picture of a page. */
export interface RegionText {
  /** Lines in reading order, one `\n` between them. */
  text: string;
  source: Annotation['quoteSource'];
  /** The page's lines were not traced yet; asking again later may find text. */
  pending: boolean;
}

/** A region as the host's commands take it. */
function regionArgs(region: RegionSelection): Record<string, unknown> {
  return {
    page: region.page,
    tool: region.tool,
    quads: region.quads,
    clip: region.clip,
    lines: region.lines,
    path: region.path,
    regionHash: region.regionHash,
  };
}

export interface PageMark {
  page: number;
  start: number;
  end: number;
  quads: number[][];
}

export interface FixedAnnotationView {
  annotation: Annotation;
  marks: PageMark[];
}

export interface FlowAnnotationView {
  annotation: Annotation;
  ranges: { start: string; end: string }[];
}

export interface MarkArgs {
  kind: AnnotationKind;
  color: string;
  note?: string;
}

export interface AnnotationEdit {
  kind?: AnnotationKind;
  color?: string;
  note?: string;
}

export interface AnnotationHit {
  annotation: Annotation;
  matched: 'title_start' | 'word_start' | 'substring';
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
  fixedAnnotations: (session: number) => invoke<FixedAnnotationView[]>('fixed_annotations', { session }),
  createFixedAnnotation: (session: number, spans: { page: number; start: number; end: number }[], mark: MarkArgs) =>
    invoke<FixedAnnotationView>('create_fixed_annotation', { session, spans, mark }),
  /**
   * Makes an annotation of a region drawn on the picture of a page; the host adds the page's hash.
   * On a scan with no text layer it quotes nothing at first: the host reads the text under it in
   * the background and announces the annotation with its quote as `annotation-text`.
   */
  createRegionAnnotation: (session: number, region: RegionSelection, mark: MarkArgs) =>
    invoke<FixedAnnotationView>('create_region_annotation', { session, region: regionArgs(region), mark }),
  /** The text under a region drawn on the picture of a page, as a copy of it gives. */
  regionText: (session: number, region: RegionSelection) => invoke<RegionText>('region_text', { session, region: regionArgs(region) }),
  flowAnnotations: (document: number) => invoke<FlowAnnotationView[]>('flow_annotations', { document }),
  createFlowAnnotation: (document: number, start: string, end: string, mark: MarkArgs) =>
    invoke<FlowAnnotationView>('create_flow_annotation', { document, start, end, mark }),
  editAnnotation: (uuid: string, edit: AnnotationEdit) => invoke<Annotation>('edit_annotation', { uuid, edit }),
  deleteAnnotation: (uuid: string) => invoke<Annotation>('delete_annotation', { uuid }),
  restoreAnnotation: (uuid: string) => invoke<Annotation>('restore_annotation', { uuid }),
  searchAnnotations: (document: number | null, query: string) =>
    invoke<AnnotationHit[]>('search_annotations', { document, query, limit: 500 }),
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
