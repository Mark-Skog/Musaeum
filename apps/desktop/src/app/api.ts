/** What the app asks the host, typed as the Rust side answers. */

import type { RegionSelection } from '../compositor/annotator';
import { invoke } from '@tauri-apps/api/core';

import type { FlowMode, SectionPlan } from '../reader/messages';

export interface RootSummary {
  id: number;
  path: string;
  status: string;
}

export interface RejectedFolder {
  path: string;
  reason: string;
}

export interface AddRootsResult {
  info: LibraryInfo;
  added: string[];
  rejected: RejectedFolder[];
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

/** Where a person put books beside the folders, as the side of the library lists it. */
export interface Category {
  id: number;
  parentId: number | null;
  name: string;
  depth: number;
  /** Works put in it directly, and in it or under it. */
  works: number;
  total: number;
}

export interface Tag {
  id: number;
  name: string;
  color: string | null;
  works: number;
}

export interface Collection {
  id: number;
  name: string;
  description: string;
  works: number;
}

export interface SmartRule {
  field: string;
  op: string;
  value: unknown;
}

export interface SmartRules {
  match: 'all' | 'any';
  rules: SmartRule[];
}

export interface SmartCollection {
  id: number;
  name: string;
  rules: SmartRules;
  works: number;
  broken: string | null;
}

export type FieldKind = 'text' | 'number' | 'date' | 'boolean' | 'choice' | 'url';

export interface CustomField {
  id: number;
  name: string;
  kind: FieldKind;
  choices: string[];
}

export interface Taxonomy {
  categories: Category[];
  tags: Tag[];
  collections: Collection[];
  smart: SmartCollection[];
  fields: CustomField[];
}

/** Where an item goes among the others of its list. */
export type Place = { at: 'first' } | { at: 'last' } | { at: 'before'; id: number } | { at: 'after'; id: number };

export interface Credit {
  name: string;
  role: string;
}

export interface Identifier {
  scheme: string;
  value: string;
}

export interface CustomValue {
  fieldId: number;
  value: string | number | boolean | null;
}

/** A work, as the editor shows it. */
export interface WorkMetadata {
  id: number;
  title: string;
  creators: Credit[];
  series: string | null;
  seriesPosition: number | null;
  year: number | null;
  language: string | null;
  identifiers: Identifier[];
  rating: number | null;
  status: 'unread' | 'reading' | 'read' | 'abandoned';
  /** What the book is about, as plain text. */
  description: string | null;
  categories: number[];
  tags: number[];
  /** The tags' names, in the same order. */
  tagNames: string[];
  collections: number[];
  custom: CustomValue[];
  /** Where each field came from: `filename`, `embedded` (the file), `calibre` or `user`. */
  provenance: Record<string, string>;
  documents: number[];
  formats: string[];
}

/** What changes; a field left out stays, `null` clears it. */
export interface WorkEdit {
  title?: string;
  creators?: Credit[];
  series?: string | null;
  seriesPosition?: number | null;
  year?: number | null;
  language?: string | null;
  identifiers?: Identifier[];
  rating?: number | null;
  status?: string;
  description?: string | null;
  categories?: number[];
  tags?: string[];
  collections?: number[];
  custom?: CustomValue[];
}

/** What changes on many works at once; a field left out stays as each has it, `null` clears it. */
export interface BatchEdit {
  creators?: Credit[];
  series?: string | null;
  year?: number | null;
  language?: string | null;
  rating?: number | null;
  status?: string;
  addCategories?: number[];
  removeCategories?: number[];
  addTags?: string[];
  removeTags?: number[];
  addCollections?: number[];
  removeCollections?: number[];
}

/** How many of the works have something. */
export interface Shared {
  id: number;
  works: number;
}

/** What a set of works has in common. */
export interface BatchSummary {
  works: number;
  /** The fields every work has the same value of (`null` when none has one); one they differ on is not here. */
  same: Partial<Record<'creators' | 'series' | 'year' | 'language' | 'rating' | 'status', unknown>>;
  categories: Shared[];
  tags: Shared[];
  collections: Shared[];
}

/** A book of a listing, as a selection holds it. */
export interface SelectedBook {
  documentId: number;
  workId: number;
}

export type DuplicateReason =
  | { kind: 'same_identifier'; scheme: string; value: string }
  | { kind: 'same_title_and_author'; title: string; author: string };

/** Two works that may be one book, the older first. */
export interface DuplicatePair {
  works: [number, number];
  reason: DuplicateReason;
}

/** Two documents the library noticed may be one. */
export interface SuggestionReview {
  id: number;
  kind: 'same_internal_id' | 'same_file_id' | 'same_path' | 'same_fingerprint';
  documentId: number;
  candidateDocumentId: number;
  evidence: string;
  state: string;
  workId: number;
  candidateWorkId: number;
}

export interface Review {
  suggestions: SuggestionReview[];
  duplicates: DuplicatePair[];
}

/** How far the full-text index is, and the book being read into it. */
export interface FullTextStatus {
  indexed: number;
  empty: number;
  failed: number;
  pending: number;
  units: number;
  chars: number;
  running: boolean;
  current: string | null;
  read: number;
}

/** A page or a section a search found. */
export interface UnitHit {
  kind: 'page' | 'document';
  index: number;
  label: string | null;
  source: 'text_layer' | 'ocr' | 'flow';
  score: number;
  snippet: string;
  /** What matched in the snippet, as UTF-16 ranges. */
  marks: [number, number][];
  /** Where the first match starts in the unit's text, in UTF-16 code units. */
  position: number;
  matched: string;
}

export interface BookHit {
  blake3: string;
  score: number;
  documents: number[];
  works: number[];
  units: UnitHit[];
}

export interface FullTextResults {
  terms: string[];
  books: BookHit[];
  units: number;
  elapsedMs: number;
}

/** Where to open a book: a page and a point on it, or a place in a flowing book. */
export type ReaderTarget =
  | { kind: 'page'; page: number; position: number }
  | { kind: 'flow'; document: number; unit: number; position: number; length: number };

export interface CalibreColumn {
  label: string;
  name: string;
  datatype: string;
  kind: FieldKind | null;
  choices: string[];
  isMultiple: boolean;
}

export interface CalibrePreview {
  source: string;
  readFrom: string;
  books: number;
  files: number;
  missing: number;
  columns: CalibreColumn[];
  sample: string[];
  followed: boolean;
}

export interface CalibreReport {
  books: number;
  imported: number;
  waiting: number;
  missing: number;
  merged: number;
  fieldsCreated: number;
}

/** Which books to list besides the search: one folder, category, tag or collection, or a rule. */
export interface BrowseFilter {
  folder: number | null;
  category: number | null;
  tag: number | null;
  collection: number | null;
  smart: number | null;
  /** Only these works' books. */
  works?: number[] | null;
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

/** The interface's colors. */
export type ThemeChoice = 'system' | 'light' | 'dark';
/** How much the interface moves. */
export type MotionChoice = 'system' | 'reduce';

/** Every preference (`preferences.db`). */
export interface Preferences {
  language: string;
  theme: ThemeChoice;
  motion: MotionChoice;
  /** The name annotations are signed with; empty for the system's user name. */
  annotationAuthor: string;
  /** The OCR's languages, as Tesseract names them, in order. */
  ocrLanguages: string[];
  /** Width of the library sidebar in CSS pixels. */
  sidebarWidth: number;
  /** Whether the library sidebar is collapsed. */
  sidebarCollapsed: boolean;
  /** Whether empty taxonomy sections appear in the sidebar. */
  showEmptySidebarSections: boolean;
  /** Width of book covers in the library grid, in CSS pixels. */
  coverWidth: number;
  /** Whether book counts appear next to sidebar items. */
  showSidebarCounts: boolean;
}

export interface PreferencesView {
  preferences: Preferences;
  systemAuthor: string | null;
  path: string | null;
  problem: string | null;
}

export type PreferencesPatch = Partial<Preferences>;

export interface OcrModel {
  code: string;
  bcp47: string | null;
  bytes: number;
  verified: boolean;
  shipped: boolean;
}

export interface OcrModels {
  folder: string | null;
  models: OcrModel[];
  fetchable: OcrModel[];
  reading: string | null;
}

/** What a library's cache holds, by part, and what it weighs on the disk. */
export interface CacheReport {
  folder: string;
  linePages: number;
  lineBytes: number;
  ocrLines: number;
  ocrBytes: number;
  fulltextBytes: number;
  flowBytes: number;
  thumbnailBytes: number;
  databaseBytes: number;
  totalBytes: number;
}

export const api = {
  libraryInfo: () => invoke<LibraryInfo>('library_info'),
  chooseRoot: () => invoke<AddRootsResult | null>('library_choose_root'),
  addRoot: (path: string) => invoke<LibraryInfo>('library_add_root', { path }),
  removeRoot: (id: number) => invoke<LibraryInfo>('library_remove_root', { id }),
  chooseExisting: () => invoke<LibraryInfo | null>('library_choose_existing'),
  browse: (query: BrowseFilter & { text: string; offset: number; limit: number }) => invoke<BrowsePage>('library_browse', { query }),
  taxonomy: () => invoke<Taxonomy>('taxonomy'),
  createCategory: (parent: number | null, name: string, place: Place) => invoke<number>('category_create', { parent, name, place }),
  renameCategory: (id: number, name: string) => invoke<void>('category_rename', { id, name }),
  moveCategory: (id: number, parent: number | null, place: Place) => invoke<void>('category_move', { id, parent, place }),
  deleteCategory: (id: number) => invoke<void>('category_delete', { id }),
  createTag: (name: string, color: string | null) => invoke<number>('tag_create', { name, color }),
  /** `color` left out keeps the tag's color; `null` takes it away. */
  updateTag: (id: number, name: string | null, color?: string | null) =>
    invoke<void>('tag_update', { id, name, color: color ?? null, clearColor: color === null }),
  deleteTag: (id: number) => invoke<void>('tag_delete', { id }),
  createCollection: (name: string, description: string, place: Place) => invoke<number>('collection_create', { name, description, place }),
  updateCollection: (id: number, name: string | null, description: string | null) => invoke<void>('collection_update', { id, name, description }),
  moveCollection: (id: number, place: Place) => invoke<void>('collection_move', { id, place }),
  deleteCollection: (id: number) => invoke<void>('collection_delete', { id }),
  addToCollection: (collection: number, works: number[], place: Place) => invoke<void>('collection_add', { collection, works, place }),
  removeFromCollection: (collection: number, works: number[]) => invoke<void>('collection_remove', { collection, works }),
  createSmart: (name: string, rules: SmartRules, place: Place) => invoke<number>('smart_create', { name, rules, place }),
  updateSmart: (id: number, name: string | null, rules: SmartRules | null) => invoke<void>('smart_update', { id, name, rules }),
  deleteSmart: (id: number) => invoke<void>('smart_delete', { id }),
  previewSmart: (rules: SmartRules) => invoke<number>('smart_preview', { rules }),
  createField: (name: string, kind: FieldKind, choices: string[]) => invoke<number>('field_create', { name, kind, choices }),
  updateField: (id: number, name: string | null, choices: string[] | null) => invoke<void>('field_update', { id, name, choices }),
  deleteField: (id: number) => invoke<void>('field_delete', { id }),
  workMetadata: (work: number) => invoke<WorkMetadata>('work_metadata', { work }),
  editWork: (work: number, edit: WorkEdit) => invoke<WorkMetadata>('work_edit', { work, edit }),
  editWorks: (works: number[], edit: BatchEdit) => invoke<number>('works_edit', { works, edit }),
  worksSummary: (works: number[]) => invoke<BatchSummary>('works_summary', { works }),
  /** Every book of a listing, not only those read so far. */
  selectAll: (query: BrowseFilter & { text: string }) => invoke<SelectedBook[]>('library_select_all', { query: { ...query, offset: 0, limit: 0 } }),
  review: () => invoke<Review>('library_review'),
  acceptSuggestion: (id: number) => invoke<number>('suggestion_accept', { id }),
  dismissSuggestion: (id: number) => invoke<void>('suggestion_dismiss', { id }),
  dismissDuplicate: (a: number, b: number) => invoke<void>('duplicate_dismiss', { a, b }),
  mergeWorks: (keep: number, others: number[]) => invoke<number>('works_merge', { keep, others }),
  fulltextStatus: () => invoke<FullTextStatus>('fulltext_status'),
  fulltextSearch: (query: string, books = 50) => invoke<FullTextResults>('fulltext_search', { query, books }),
  fulltextFlowPlace: (document: number, unit: number, position: number, length: number) =>
    invoke<{ start: string; end: string }>('fulltext_flow_place', { document, unit, position, length }),
  fulltextPagePoint: (session: number, page: number, position: number) =>
    invoke<[number, number] | null>('fulltext_page_point', { session, page, position }),
  fulltextWake: () => invoke<void>('fulltext_wake'),
  calibreChoose: () => invoke<string | null>('calibre_choose'),
  calibrePreview: (path: string) => invoke<CalibrePreview>('calibre_preview', { path }),
  calibreImport: (path: string) => invoke<CalibreReport>('calibre_import', { path }),
  folders: () => invoke<FolderEntry[]>('library_folders'),
  setting: <T>(key: string) => invoke<T | null>('reader_setting', { key }),
  setSetting: (key: string, value: unknown) => invoke<void>('set_reader_setting', { key, value }),
  openFlow: (document: number) =>
    invoke<FlowInfo>('open_flow', { hostOrigin: location.origin, args: { document } }),
  saveFlowPosition: (
    document: number,
    position: { readerCfi: string; mode: FlowMode; pageTurnAnimation: boolean },
  ) => invoke<FlowPosition>('save_flow_position', { document, position }),
  closeBook: (origin?: string) => invoke<void>('close_book', { origin: origin ?? null }),
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
  /**
   * Writes a copy of the open PDF with its annotations in it, where the person chooses in the
   * system's dialog (a test gives the path). The book itself is never written. `null`: cancelled.
   */
  pdfAnnotationsExport: (session: number, target?: string) =>
    invoke<PdfExportReport | null>('pdf_annotations_export', { session, target: target ?? null }),
  /** Brings the annotations of a PDF the person chooses into the open book. */
  pdfAnnotationsImport: (session: number, source?: string) =>
    invoke<PdfImportReport | null>('pdf_annotations_import', { session, source: source ?? null }),
  xfdfExport: (session: number, target?: string) =>
    invoke<PdfExportReport | null>('xfdf_export', { session, target: target ?? null }),
  xfdfImport: (session: number, source?: string) =>
    invoke<PdfImportReport | null>('xfdf_import', { session, source: source ?? null }),
  /** Under a test: what PDFium reads in a file. */
  e2ePdfAnnotations: (path: string) => invoke<unknown>('e2e_pdf_annotations', { path }),
  /**
   * Writes annotations as W3C Web Annotations (JSON-LD), where the person chooses (a test gives the
   * path): of a document, of works, or of the whole library. `null`: cancelled.
   */
  exportJsonLd: (scope: ExchangeScope, target?: string) =>
    invoke<JsonLdExported | null>('annotations_export_jsonld', { scope, target: target ?? null }),
  /** Brings the annotations of a JSON-LD file into a document, a work's documents, or the library. */
  importJsonLd: (target: ExchangeTarget, source?: string) =>
    invoke<JsonLdImported | null>('annotations_import_jsonld', { target, source: source ?? null }),
  /** Writes a study report of a document or of works, as Markdown or as a PDF. */
  studyReport: (format: 'markdown' | 'pdf', scope: ReportScope, target?: string, generatedAt?: number) =>
    invoke<ReportWritten | null>('study_report', {
      format,
      scope,
      utcOffsetMinutes: -new Date().getTimezoneOffset(),
      target: target ?? null,
      generatedAt: generatedAt ?? null,
    }),
  /** Under a test: each page's text of a PDF, as PDFium reads it. */
  e2ePdfText: (path: string) => invoke<string[]>('e2e_pdf_text', { path }),
  /** Every preference, the system's user name, and where they are kept. */
  preferences: () => invoke<PreferencesView>('preferences_get'),
  /** Changes some preferences, all or none; answers with all of them. */
  setPreferences: (patch: PreferencesPatch) => invoke<PreferencesView>('preferences_set', { patch }),
  /** The OCR's models in the program's folder, verified or not, and those it could fetch. */
  ocrModels: () => invoke<OcrModels>('ocr_models'),
  /** What the open library's cache holds; null without a library. */
  cacheReport: () => invoke<CacheReport | null>('cache_report'),
  /** Forgets the pages' lines and what the OCR read; answers with the cache as it is after. */
  cacheClearLines: () => invoke<CacheReport | null>('cache_clear_lines'),
};

/** Which annotations an export writes. */
export type ExchangeScope = { kind: 'document'; ids: number } | { kind: 'works'; ids: number[] } | { kind: 'library' };

/** Where an import may put what it brings. */
export type ExchangeTarget = { kind: 'document'; id: number } | { kind: 'work'; id: number } | { kind: 'library' };

/** Which books a report is about. */
export type ReportScope = { kind: 'document'; ids: number } | { kind: 'works'; ids: number[] };

/** What a JSON-LD export wrote. */
export interface JsonLdExported {
  path: string;
  annotations: number;
  documents: number;
  works: number;
  bytes: number;
}

/** How the annotations an import brought into one book were found on it. */
export interface BroughtInto {
  documentId: number;
  title: string;
  added: number;
  exact: number;
  approximate: number;
  orphan: number;
  /** Not found again yet: the book is not there now, or its pages have not been looked at. */
  pending: number;
}

/** A work of the file the library does not have. */
export interface UnmatchedWork {
  title: string;
  creators: string[];
  annotations: number;
}

/** What a JSON-LD import brought. */
export interface JsonLdImported {
  path: string;
  total: number;
  added: number;
  /** Already in the library, by their UUID. */
  already: number;
  /** Of other books than the one asked for. */
  elsewhere: number;
  /** Brought into the book chosen though the file names another work. */
  otherWork: number;
  unmatched: UnmatchedWork[];
  books: BroughtInto[];
}

/** What a study report wrote. */
export interface ReportWritten {
  path: string;
  works: number;
  annotations: number;
  pages: number | null;
  bytes: number;
}

/** What an export to PDF or XFDF wrote. */
export interface PdfExportReport {
  path: string;
  /** Musaeum's annotations written, and the PDF annotations they became. */
  annotations: number;
  written: number;
  /** Orphans and notes about the whole document, which have no place on a page. */
  leftOut: number;
  /** Annotations the PDF already holds, which came from it: left as they are in the copy. */
  alreadyInPdf: number;
  bytes: number;
}

/** What an import from PDF or XFDF brought. */
export interface PdfImportReport {
  path: string;
  added: number;
  /** Already in the book, by their name, the deleted ones too. */
  already: number;
  /** Left out, by why: a kind not kept, a reply, hidden, unreadable. */
  skipped: Record<string, number>;
  /** An XFDF file that says it is about another PDF. */
  otherDocument: boolean;
}

/** Whether the default browser can be given this address: `http` and `https` only. */
export function isWebAddress(href: string): boolean {
  return /^https?:\/\/[^\s@/?#]+/i.test(href);
}

/** Where a cover is served, by the file's hash; `version` changes once a new one is drawn. */
export function coverUrl(blake3: string, version: number): string {
  return `http://thumb.localhost/${blake3}.jpg${version > 0 ? `?v=${version}` : ''}`;
}
