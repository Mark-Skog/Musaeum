/**
 * The reader, the same for every book: a bar with the way of reading, the page turn, the contents
 * and where the reader is, over a fixed-layout document drawn by the compositor or a flowing book
 * laid out at its own origin. C switches between paginated and continuous reading, A turns the page
 * animation on and off, for both; the position is kept in the library as the reader goes, and
 * kept once more on the way out.
 */

import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import {
  Button,
  Dialog,
  DialogTrigger,
  Heading,
  Label,
  Modal,
  ModalOverlay,
  Popover,
  Radio,
  RadioGroup,
  Switch,
  ToggleButton,
  ToggleButtonGroup,
} from 'react-aria-components';

import type { RegionSelection, Tool } from '../../compositor/annotator';
import type { ReaderStatus } from '../../compositor/messages';
import { FixedReader } from '../../fixed/fixed-reader';
import { type SavedReadingState, type StartupOptions } from '../../protocol';
import { enqueueReaderCloseTask, isAbortError } from '../../reader-session-lock';
import type { FlowMode, Shortcut } from '../../reader/messages';
import {
  DEFAULT_STYLE,
  isReadingStyle,
  LINE_HEIGHTS,
  MAX_SIZE,
  MIN_SIZE,
  THEMES,
  type ReadingStyle,
} from '../../reader/typography';
import { invoke } from '@tauri-apps/api/core';

import { listen } from '@tauri-apps/api/event';
import {
  api,
  isWebAddress,
  type Annotation,
  type AnnotationKind,
  type BookRow,
  type FixedAnnotationView,
  type FlowAnnotationView,
  type FlowPosition,
  type NavPoint,
  type PdfExportReport,
  type PdfImportReport,
  type ReaderTarget,
  type RegionText,
} from '../api';
import { formatName } from '../library/LibraryView';
import { FlowReader, type Relocation } from './flow-reader';
import {
  Inspector,
  type InspectorFilterState,
  NoteEditor,
  type PdfAction,
  type PdfActions,
  type ScreenRect,
  SelectionToolbar,
  UndoToast,
} from './annotations/AnnotationUi';
import { AnnotationStore, draft, type Entry } from './annotations/store';
import { type ExchangeControls, useExchange } from '../ExchangeSection';
import { getLocale, type MessageKey, t, tn, useLocale, useT } from '../i18n';
import { onEscape, useFocusReturn } from '../focus';
import { ScrollFrame } from '../OverlayScrollbar';
import { onMotionChange } from '../preferences/store';
import type { ExchangeAction, ExchangeOutcome } from '../exchange';
import { deriveFixedFitMode, type ReaderTabSnapshot, resolveFixedSnapshotZoom } from '../tabs-model';

/** What the reader has selected now, in either kind of book, with its box on the screen. */
export type ReaderSelection =
  | { kind: 'fixed'; spans: { page: number; start: number; end: number }[]; text: string; rect: ScreenRect | null }
  | {
      kind: 'region';
      region: RegionSelection;
      /** What a copy gives, once read: by the OCR on a scan, or by the characters of a page. */
      text: string;
      /** What read it, once read. */
      source: RegionText['source'] | null;
      /** Whether its text is still being read. */
      reading: boolean;
      rect: ScreenRect | null;
    }
  | { kind: 'flow'; start: string; end: string; text: string; rect: ScreenRect | null };

const fixedEntry = (view: FixedAnnotationView): Entry => ({ annotation: view.annotation, marks: view.marks, ranges: [], pending: false });
const flowEntry = (view: FlowAnnotationView): Entry => ({ annotation: view.annotation, marks: [], ranges: view.ranges, pending: false });

const NAVIGATION = new Set(['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'PageUp', 'PageDown', 'Home', 'End', ' ']);
const STYLE_SETTING = 'typography';
const LOADING_DELAY_MS = 120;

const MISSING_FILE_RE =
  /arquivo deste livro não está mais na pasta|book's file is no longer in its folder|cannot find the file specified|não pode encontrar o arquivo especificado|no such file or directory|os error 2|\benoent\b/i;

export function isMissingBookFileError(message: string): boolean {
  return MISSING_FILE_RE.test(message);
}

function fixedStateFromSnapshot(snapshot: ReaderTabSnapshot | undefined): SavedReadingState | null {
  if (snapshot === undefined || snapshot.layout !== 'fixed' || snapshot.page === undefined) {
    return null;
  }
  const v = snapshot.v ?? snapshot.scrollOffset ?? 0;
  return {
    page: Math.max(0, Math.trunc(snapshot.page)),
    u: snapshot.u ?? 0,
    v,
    progress: snapshot.progress ?? 0,
    mode: snapshot.mode ?? 'paginated',
    pageTurnAnimation: snapshot.pageTurnAnimation ?? true,
    zoom: resolveFixedSnapshotZoom(snapshot),
    spreads: snapshot.spreads ?? 'auto',
  };
}

function statusFromSnapshot(snapshot: ReaderTabSnapshot | undefined): ReaderStatus | null {
  const state = fixedStateFromSnapshot(snapshot);
  if (state === null || snapshot?.pageCount === undefined || snapshot.pageCount <= 0) {
    return null;
  }
  return {
    type: 'status',
    mode: state.mode,
    animating: state.mode === 'paginated' && state.pageTurnAnimation,
    animationPreference: state.pageTurnAnimation,
    spreads: state.spreads,
    zoom: state.zoom,
    page: state.page,
    pageCount: snapshot.pageCount,
    u: state.u,
    v: state.v,
    progress: state.progress,
  };
}

/** An entry of a fixed-layout document's table of contents, as the host sends it. */
export interface OutlineItem {
  label: string;
  page: number | null;
  level: number;
}

/** What the tests driving the real window can read of the reader. */
export interface ReaderProbe {
  kind: 'fixed' | 'flow';
  documentId: number;
  ready: boolean;
  error: string | null;
  fixed: FixedReader | null;
  flow: FlowReader | null;
  /** A fixed-layout document's table of contents, once read. */
  outline: OutlineItem[] | null;
  /** The annotations shown, and what is selected. */
  annotations: Entry[];
  selection: ReaderSelection | null;
  /** For a PDF: its own annotations in and out, with a path a test gives instead of the dialog. */
  pdf: ((action: PdfAction, path?: string) => Promise<PdfOutcome>) | null;
  /** What the panel says about the last of them. */
  pdfReport: { text: string; failed: boolean } | null;
  /** The book's annotations as JSON-LD and its reports, with a path a test gives instead of the dialog. */
  exchange: (action: ExchangeAction, path?: string, generatedAt?: number) => Promise<ExchangeOutcome>;
  exchangeReport: ExchangeControls['report'];
}

/** What one of a PDF's actions did: its report, nothing when cancelled, or why it failed. */
export type PdfOutcome = { ok: true; report: PdfExportReport | PdfImportReport | null } | { ok: false; error: string };

/** The words for what an import left out, by the reason the host gives (a code, not words). */
const SKIPPED_LABEL: Record<string, MessageKey> = {
  resposta: 'reader.skipped.reply',
  oculta: 'reader.skipped.hidden',
  '(ilegível)': 'reader.skipped.illegible',
  '(broken)': 'reader.skipped.illegible',
  '(no such page)': 'reader.skipped.noPage',
  'página que o livro não tem': 'reader.skipped.noPage',
  StrikeOut: 'reader.skipped.StrikeOut',
  Circle: 'reader.skipped.Circle',
  Stamp: 'reader.skipped.Stamp',
  FileAttachment: 'reader.skipped.FileAttachment',
  Line: 'reader.skipped.Line',
  Polygon: 'reader.skipped.Polygon',
  PolyLine: 'reader.skipped.PolyLine',
  Caret: 'reader.skipped.Caret',
  Sound: 'reader.skipped.Sound',
  Redact: 'reader.skipped.Redact',
};

function importText(report: PdfImportReport): string {
  const parts = [tn('reader.import.added', report.added)];
  if (report.already > 0) {
    parts.push(tn('reader.import.already', report.already));
  }
  const skipped = Object.entries(report.skipped).filter(([, count]) => Number(count) > 0);
  if (skipped.length > 0) {
    const list = skipped.map(([why, count]) => {
      const label = SKIPPED_LABEL[why];
      return t('reader.import.skippedItem', { count: Number(count), what: label === undefined ? why : t(label) });
    });
    parts.push(t('reader.import.skipped', { list: list.join(', ') }));
  }
  let text = parts.join('; ') + '.';
  if (report.otherDocument) {
    text += ` ${t('reader.import.otherDocument')}`;
  }
  return text;
}

function exportText(report: PdfExportReport, what: string): string {
  const parts = [tn('reader.export.saved', report.annotations, { what, path: report.path })];
  if (report.leftOut > 0) {
    parts.push(t('reader.export.leftOut', { count: report.leftOut }));
  }
  if (report.alreadyInPdf > 0) {
    parts.push(tn('reader.export.alreadyInPdf', report.alreadyInPdf));
  }
  return parts.join(' ');
}

function lineHeightLabel(value: number): string {
  return value === 0 ? t('reader.lineHeight.publisher') : value.toLocaleString(getLocale(), { minimumFractionDigits: 1 });
}

/** The table of contents' entry the reader is in: the last one at or before its section. */
function currentEntry(toc: NavPoint[], sectionOf: (href: string) => number, section: number): NavPoint | null {
  let found: NavPoint | null = null;
  const walk = (points: NavPoint[]): void => {
    for (const point of points) {
      if (point.href !== null) {
        const index = sectionOf(point.href);
        if (index >= 0 && index <= section) {
          found = point;
        }
      }
      if (point.children !== undefined) {
        walk(point.children);
      }
    }
  };
  walk(toc);
  return found;
}

function TocList({
  points,
  current,
  onPick,
}: {
  points: NavPoint[];
  current: NavPoint | null;
  onPick(point: NavPoint): void;
}): React.JSX.Element {
  return (
    <ol className="toc-list">
      {points.map((point, index) => (
        <li key={`${point.href ?? ''}-${index}`}>
          {point.href !== null ? (
            <Button
              className={`toc-entry${point === current ? ' current' : ''}`}
              aria-current={point === current ? 'location' : undefined}
              onPress={() => onPick(point)}
            >
              {point.label}
            </Button>
          ) : (
            <span className="toc-entry">{point.label}</span>
          )}
          {point.children !== undefined && point.children.length > 0 ? (
            <TocList points={point.children} current={current} onPick={onPick} />
          ) : null}
        </li>
      ))}
    </ol>
  );
}

function TypographyPanel({
  style,
  onChange,
}: {
  style: ReadingStyle;
  onChange(style: ReadingStyle): void;
}): React.JSX.Element {
  const t = useT();
  const set = <K extends keyof ReadingStyle>(key: K, value: ReadingStyle[K]): void =>
    onChange({ ...style, [key]: value });
  return (
    <Dialog className="typography" aria-label={t('reader.typography')}>
      <RadioGroup className="field" value={style.family} onChange={(value) => set('family', value as ReadingStyle['family'])}>
        <Label>{t('reader.font')}</Label>
        <div className="choices">
          <Radio value="publisher" id="font-publisher" className="choice">{t('reader.font.publisher')}</Radio>
          <Radio value="serif" id="font-serif" className="choice">{t('reader.font.serif')}</Radio>
          <Radio value="sans" id="font-sans" className="choice">{t('reader.font.sans')}</Radio>
        </div>
      </RadioGroup>
      <div className="field">
        <span className="field-label">{t('reader.size')}</span>
        <div className="stepper">
          <Button id="font-smaller" className="button" aria-label={t('reader.size.smaller')} isDisabled={style.size <= MIN_SIZE} onPress={() => set('size', Math.max(MIN_SIZE, style.size - 10))}>
            A−
          </Button>
          <output aria-live="polite">{style.size}%</output>
          <Button id="font-larger" className="button" aria-label={t('reader.size.larger')} isDisabled={style.size >= MAX_SIZE} onPress={() => set('size', Math.min(MAX_SIZE, style.size + 10))}>
            A+
          </Button>
        </div>
      </div>
      <RadioGroup className="field" value={String(style.lineHeight)} onChange={(value) => set('lineHeight', Number(value))}>
        <Label>{t('reader.lineHeight')}</Label>
        <div className="choices">
          {LINE_HEIGHTS.map((value) => (
            <Radio key={value} value={String(value)} id={`line-${String(value).replace('.', '-')}`} className="choice">
              {lineHeightLabel(value)}
            </Radio>
          ))}
        </div>
      </RadioGroup>
      <RadioGroup className="field" value={style.margin} onChange={(value) => set('margin', value as ReadingStyle['margin'])}>
        <Label>{t('reader.margins')}</Label>
        <div className="choices">
          <Radio value="narrow" id="margin-narrow" className="choice">{t('reader.margins.narrow')}</Radio>
          <Radio value="normal" id="margin-normal" className="choice">{t('reader.margins.normal')}</Radio>
          <Radio value="wide" id="margin-wide" className="choice">{t('reader.margins.wide')}</Radio>
        </div>
      </RadioGroup>
      <RadioGroup className="field" value={style.theme} onChange={(value) => set('theme', value as ReadingStyle['theme'])}>
        <Label>{t('reader.theme')}</Label>
        <div className="choices themes">
          <Radio value="light" id="theme-light" className="choice theme-light">{t('reader.theme.light')}</Radio>
          <Radio value="sepia" id="theme-sepia" className="choice theme-sepia">{t('reader.theme.sepia')}</Radio>
          <Radio value="dark" id="theme-dark" className="choice theme-dark">{t('reader.theme.dark')}</Radio>
        </div>
      </RadioGroup>
    </Dialog>
  );
}

export interface ReaderViewProps {
  book: BookRow;
  options: StartupOptions;
  /** Switches focus to the Library (`'home'`) tab while keeping this book's tab open. */
  onBack(): void;
  /** Closes this book's tab completely (used e.g. from the in-tab error screen). */
  onCloseTab?(): void;
  /** Called once an unmounted reader finishes flushing its reading position and closing its session. */
  onSessionClosed?(): void;
  /** Saved per-tab reader snapshot to rehydrate immediately without visual jumps or default resets. */
  snapshot?: ReaderTabSnapshot;
  /** Exposes synchronous snapshot capture for outgoing tab switches before unmount. */
  snapshotGetterRef?: React.MutableRefObject<(() => ReaderTabSnapshot | undefined) | null>;
  /** Called on unmount/back with the latest captured snapshot when available. */
  onSnapshotCapture?(snapshot: ReaderTabSnapshot): void;
  /** Exposes an async flush-and-close trigger for graceful window shutdown. */
  flushRef?: React.MutableRefObject<(() => Promise<void>) | null>;
  probe?: React.MutableRefObject<ReaderProbe | null>;
  /** Where to show the book once it is open, instead of where the reading stopped: a match found in its text. */
  target?: ReaderTarget | null;
  /** Monotonic sequence for `target`, allowing repeated search hits on an already-open tab. */
  targetSeq?: number;
  /** Marks the current `targetSeq` as consumed so later tab switches restore reading state instead. */
  onTargetReached?(): void;
  /** Opens the Preferences. */
  onPreferences?(): void;
  /** Whether the window is currently in `F11` fullscreen. */
  isFullscreen?: boolean;
  /** Toggles `F11` fullscreen from reader shortcuts (including inside flowing books). */
  onToggleFullscreen?(): void;
  /** Forwards global tab/app shortcuts captured inside the flowing reader's isolated iframe. */
  onReaderShortcut?(key: Shortcut): void;
}

export function ReaderView({
  book,
  options,
  onBack,
  onCloseTab,
  onSessionClosed,
  snapshot,
  snapshotGetterRef,
  onSnapshotCapture,
  flushRef,
  probe,
  target = null,
  targetSeq = 0,
  onTargetReached,
  onPreferences,
  isFullscreen = false,
  onToggleFullscreen,
  onReaderShortcut,
}: ReaderViewProps): React.JSX.Element {
  const t = useT();
  const kind = book.layout;
  const initialSnapshotRef = useRef<ReaderTabSnapshot | undefined>(snapshot);
  const hasOpenedLiveRef = useRef(false);
  const surface = useRef<HTMLDivElement>(null);
  const fixed = useRef<FixedReader | null>(null);
  const flow = useRef<FlowReader | null>(null);
  // The person's motion preference, or the system's, applies to the book already open.
  useEffect(
    () =>
      onMotionChange((reduced) => {
        fixed.current?.setReducedMotion(reduced);
        flow.current?.setReducedMotion(reduced);
      }),
    [],
  );
  const locale = useLocale();
  useEffect(() => flow.current?.setLanguage(), [locale]);
  const [status, setStatus] = useState<ReaderStatus | null>(() =>
    kind === 'fixed' ? statusFromSnapshot(snapshot) : null,
  );
  const [flowMode, setFlowMode] = useState<FlowMode>(() =>
    kind === 'flow' && snapshot?.layout === 'flow' && snapshot.mode !== undefined ? snapshot.mode : 'paginated',
  );
  const [flowAnimated, setFlowAnimated] = useState<boolean>(() =>
    kind === 'flow' && snapshot?.layout === 'flow' && snapshot.pageTurnAnimation !== undefined
      ? snapshot.pageTurnAnimation
      : true,
  );
  const [relocation, setRelocation] = useState<Relocation | null>(() =>
    kind === 'flow' && snapshot?.layout === 'flow' && snapshot.readerCfi != null
      ? {
          cfi: snapshot.readerCfi,
          section: snapshot.section ?? 0,
          progress: snapshot.progress ?? 0,
          atStart: false,
          atEnd: false,
          reason: 'snapshot',
        }
      : null,
  );
  const [saved, setSaved] = useState<FlowPosition | null>(() =>
    kind === 'flow' && snapshot?.layout === 'flow' && snapshot.progress != null
      ? {
          locator: (snapshot.locator ?? {}) as FlowPosition['locator'],
          progress: snapshot.progress,
          saved: true,
        }
      : null,
  );
  const [style, setStyle] = useState<ReadingStyle | null>(() =>
    kind === 'flow' && snapshot?.layout === 'flow' && snapshot.style !== undefined ? snapshot.style : null,
  );
  const [toc, setToc] = useState<NavPoint[]>([]);
  /** A fixed-layout document's table of contents, from its outline, once it was read. */
  const [outline, setOutline] = useState<OutlineItem[] | null>(null);
  const [tocOpen, setTocOpen] = useState<boolean>(() => snapshot?.tocOpen ?? false);
  const [link, setLinkState] = useState<string | null>(null);
  const [linkError, setLinkError] = useState<string | null>(null);
  const setLink = useCallback((href: string | null) => {
    setLinkError(null);
    setLinkState(href);
  }, []);
  const [error, setError] = useState<string | null>(null);
  const [ready, setReady] = useState(false);
  const [showLoading, setShowLoading] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    if (ready || error !== null) {
      setShowLoading(false);
      return;
    }
    const timer = window.setTimeout(() => setShowLoading(true), LOADING_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [ready, error]);
  const store = useMemo(
    () =>
      new AnnotationStore(
        { edit: api.editAnnotation, remove: api.deleteAnnotation, restore: api.restoreAnnotation },
        (message) => setNotice(message),
      ),
    // One store per book opened.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [book.documentId],
  );
  const entries = useSyncExternalStore(store.subscribe, store.list);
  /** What the host reads under each region drawn: one reading, shared by the copy and the prefetch. */
  const regionReads = useRef(new WeakMap<RegionSelection, Promise<RegionText | null>>());
  const readRegion = useCallback((region: RegionSelection): Promise<RegionText | null> => {
    const known = regionReads.current.get(region);
    if (known !== undefined) {
      return known;
    }
    const session = fixed.current?.session;
    if (session === null || session === undefined) {
      return Promise.resolve(null);
    }
    const reading = api.regionText(session.id, region).then(
      (read) => {
        if (read.pending) {
          // Asked again, it may find the lines traced by then.
          regionReads.current.delete(region);
        }
        return read;
      },
      () => {
        regionReads.current.delete(region);
        return null;
      },
    );
    regionReads.current.set(region, reading);
    return reading;
  }, []);

  // The quote the OCR read under a mark on a scan, after it was made.
  useEffect(() => {
    const stop = listen<Annotation>('annotation-text', (event) => store.learned(event.payload));
    return () => {
      void stop.then((unlisten) => unlisten());
    };
  }, [store]);
  const [selection, setSelection] = useState<ReaderSelection | null>(null);
  const [activeMark, setActiveMark] = useState<{ uuid: string; rect: ScreenRect } | null>(null);
  const [editing, setEditing] = useState<Entry | null>(null);
  const [panelOpen, setPanelOpen] = useState<boolean>(() => snapshot?.annotationsOpen ?? false);
  // A side panel that closes with the focus in it gives the focus back to its button.
  useFocusReturn(tocOpen, () => document.getElementById('toc-toggle')?.focus());
  useFocusReturn(panelOpen, () => document.getElementById('annotations-toggle')?.focus());
  const [undo, setUndo] = useState<Entry | null>(null);
  const lastColor = useRef('yellow');
  const closing = useRef(false);
  /** What a press on a page of a fixed layout does, and whether lines snap. */
  const [tool, setTool] = useState<Tool>(() =>
    kind === 'fixed' && snapshot?.layout === 'fixed' && snapshot.tool !== undefined ? snapshot.tool : 'text',
  );
  const [snap, setSnap] = useState<boolean>(() =>
    kind === 'fixed' && snapshot?.layout === 'fixed' && snapshot.snap !== undefined ? snapshot.snap : true,
  );
  const initialInspectorState = useMemo<Partial<InspectorFilterState>>(
    () => ({
      query: snapshot?.annotationSearch ?? '',
      colors: snapshot?.annotationFilterColors ? [...snapshot.annotationFilterColors] : [],
      kind: snapshot?.annotationFilterKind ?? 'all',
      orphans: snapshot?.annotationFilterOrphans ?? false,
    }),
    // Initialized once per mounted tab.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );
  const annotFilterRef = useRef<InspectorFilterState>({
    query: initialInspectorState.query ?? '',
    colors: initialInspectorState.colors ?? [],
    kind: initialInspectorState.kind ?? 'all',
    orphans: initialInspectorState.orphans ?? false,
  });
  const handleInspectorStateChange = useCallback((next: InspectorFilterState) => {
    annotFilterRef.current = next;
  }, []);

  const liveUiRef = useRef({ tocOpen, panelOpen, tool, snap });
  liveUiRef.current = { tocOpen, panelOpen, tool, snap };
  const lastFixedReadingRef = useRef<SavedReadingState | null>(fixedStateFromSnapshot(snapshot));
  const lastPageCountRef = useRef<number | undefined>(
    snapshot?.layout === 'fixed' ? snapshot.pageCount : undefined,
  );
  const lastFlowStateRef = useRef<{
    mode: FlowMode;
    animated: boolean;
    readerCfi: string | null;
    section: number | undefined;
    progress: number | null;
    locator: Readonly<Record<string, unknown>> | null;
    style: ReadingStyle | undefined;
  }>({
    mode: flowMode,
    animated: flowAnimated,
    readerCfi: relocation?.cfi ?? (snapshot?.layout === 'flow' ? (snapshot.readerCfi ?? null) : null),
    section: relocation?.section ?? (snapshot?.layout === 'flow' ? snapshot.section : undefined),
    progress: saved?.progress ?? relocation?.progress ?? (snapshot?.layout === 'flow' ? (snapshot.progress ?? null) : null),
    locator: (saved?.locator as Readonly<Record<string, unknown>> | undefined) ?? (snapshot?.layout === 'flow' ? (snapshot.locator ?? null) : null),
    style: style ?? (snapshot?.layout === 'flow' ? snapshot.style : undefined),
  });
  lastFlowStateRef.current = {
    mode: flowMode,
    animated: flowAnimated,
    readerCfi: relocation?.cfi ?? lastFlowStateRef.current.readerCfi,
    section: relocation?.section ?? lastFlowStateRef.current.section,
    progress: saved?.progress ?? relocation?.progress ?? lastFlowStateRef.current.progress,
    locator: (saved?.locator as Readonly<Record<string, unknown>> | undefined) ?? lastFlowStateRef.current.locator,
    style: style ?? lastFlowStateRef.current.style,
  };

  const buildCurrentSnapshot = useCallback((): ReaderTabSnapshot | undefined => {
    const ui = liveUiRef.current;
    const filter = annotFilterRef.current;
    const panelFields = {
      tocOpen: ui.tocOpen,
      annotationsOpen: ui.panelOpen,
      annotationSearch: filter.query,
      annotationFilterColors: [...filter.colors],
      annotationFilterKind: filter.kind,
      annotationFilterOrphans: filter.orphans,
      capturedAt: Date.now(),
    };

    if (!hasOpenedLiveRef.current) {
      // Never overwrite an existing snapshot with default `page = 0` or `zoom = 1` if the user
      // switches away while the reader is still opening asynchronously.
      const prior = initialSnapshotRef.current;
      if (prior !== undefined) {
        return {
          ...prior,
          ...(kind === 'fixed' ? { tool: ui.tool, snap: ui.snap } : {}),
          ...panelFields,
        };
      }
      const anyPanelChanged =
        ui.tocOpen ||
        ui.panelOpen ||
        filter.query !== '' ||
        filter.colors.length > 0 ||
        filter.kind !== 'all' ||
        filter.orphans ||
        (kind === 'fixed' && (ui.tool !== 'text' || !ui.snap));
      if (!anyPanelChanged) {
        return undefined;
      }
      return {
        layout: kind,
        ...(kind === 'fixed' ? { tool: ui.tool, snap: ui.snap } : {}),
        ...panelFields,
      };
    }

    if (kind === 'fixed') {
      const liveFixed = fixed.current?.currentReadingState() ?? lastFixedReadingRef.current;
      if (liveFixed === null) {
        return initialSnapshotRef.current !== undefined
          ? { ...initialSnapshotRef.current, tool: ui.tool, snap: ui.snap, ...panelFields }
          : undefined;
      }
      return {
        layout: 'fixed',
        mode: liveFixed.mode,
        pageTurnAnimation: liveFixed.pageTurnAnimation,
        page: liveFixed.page,
        pageCount: lastPageCountRef.current,
        u: liveFixed.u,
        v: liveFixed.v,
        scrollOffset: liveFixed.v,
        zoom: liveFixed.zoom,
        fitMode: deriveFixedFitMode(liveFixed.mode, liveFixed.zoom),
        spreads: liveFixed.spreads,
        tool: ui.tool,
        snap: ui.snap,
        progress: liveFixed.progress,
        ...panelFields,
      };
    }

    const liveFlow = lastFlowStateRef.current;
    const liveReader = flow.current;
    const readerCfi = liveReader?.relocation?.cfi ?? liveFlow.readerCfi;
    const section = liveReader?.relocation?.section ?? liveFlow.section;
    const progress =
      liveReader?.savedPosition?.progress ??
      liveReader?.relocation?.progress ??
      liveFlow.progress;
    const locator =
      (liveReader?.savedPosition?.locator as Readonly<Record<string, unknown>> | undefined) ??
      liveFlow.locator;
    return {
      layout: 'flow',
      mode: liveReader?.currentMode ?? liveFlow.mode,
      pageTurnAnimation: liveReader?.isAnimated ?? liveFlow.animated,
      readerCfi,
      section,
      progress,
      locator,
      style: liveReader?.currentStyle ?? liveFlow.style,
      ...panelFields,
    };
  }, [kind]);

  useEffect(() => {
    if (snapshotGetterRef === undefined) {
      return;
    }
    snapshotGetterRef.current = buildCurrentSnapshot;
    return () => {
      if (snapshotGetterRef.current === buildCurrentSnapshot) {
        snapshotGetterRef.current = null;
      }
    };
  }, [snapshotGetterRef, buildCurrentSnapshot]);

  const [pdfBusy, setPdfBusy] = useState<PdfAction | null>(null);
  const [pdfReport, setPdfReport] = useState<{ text: string; failed: boolean } | null>(null);
  const isPdf = kind === 'fixed' && book.format.toLowerCase() === 'pdf';

  /** One of a PDF's actions: the dialog asks where, unless a test gave the path. */
  const pdfAction = useCallback(
    async (action: PdfAction, path?: string): Promise<PdfOutcome> => {
      const session = fixed.current?.session;
      if (session === null || session === undefined) {
        return { ok: false, error: t('reader.notOpen') };
      }
      setPdfBusy(action);
      setPdfReport(null);
      try {
        let report: PdfExportReport | PdfImportReport | null;
        switch (action) {
          case 'export-pdf': {
            const written = await api.pdfAnnotationsExport(session.id, path);
            report = written;
            if (written !== null) {
              setPdfReport({ text: exportText(written, 'PDF'), failed: false });
            }
            break;
          }
          case 'export-xfdf': {
            const written = await api.xfdfExport(session.id, path);
            report = written;
            if (written !== null) {
              setPdfReport({ text: exportText(written, 'XFDF'), failed: false });
            }
            break;
          }
          case 'import-pdf':
          case 'import-xfdf': {
            const brought = action === 'import-pdf' ? await api.pdfAnnotationsImport(session.id, path) : await api.xfdfImport(session.id, path);
            report = brought;
            if (brought !== null) {
              store.load((await api.fixedAnnotations(session.id)).map(fixedEntry));
              setPdfReport({ text: importText(brought), failed: false });
            }
            break;
          }
        }
        return { ok: true, report };
      } catch (reason: unknown) {
        const message = String(reason instanceof Error ? reason.message : reason);
        setPdfReport({ text: t('common.couldNot', { reason: message }), failed: true });
        return { ok: false, error: message };
      } finally {
        setPdfBusy(null);
      }
    },
    [store],
  );
  // JSON-LD and the reports, for every kind of book. After an import, the book's annotations are
  // read again: what came from other bytes has been found on these by then.
  const reloadAnnotations = useCallback(async (): Promise<void> => {
    if (kind === 'fixed') {
      const session = fixed.current?.session;
      if (session !== null && session !== undefined) {
        store.load((await api.fixedAnnotations(session.id)).map(fixedEntry));
      }
    } else {
      store.load((await api.flowAnnotations(book.documentId)).map(flowEntry));
    }
  }, [kind, store, book.documentId]);
  const exchange = useExchange(
    { kind: 'document', documentId: book.documentId },
    (action) => {
      if (action === 'import-jsonld') {
        void reloadAnnotations();
      }
    },
    'reader',
  );
  const pdfActions = useMemo<PdfActions | undefined>(
    () => (isPdf ? { busy: pdfBusy, report: pdfReport, run: (action) => void pdfAction(action) } : undefined),
    [isPdf, pdfBusy, pdfReport, pdfAction],
  );

  const updateProbe = useCallback(() => {
    if (probe !== undefined) {
      probe.current = {
        kind,
        documentId: book.documentId,
        ready,
        error,
        fixed: fixed.current,
        flow: flow.current,
        outline,
        annotations: entries,
        selection,
        pdf: isPdf ? pdfAction : null,
        pdfReport,
        exchange: exchange.run,
        exchangeReport: exchange.report,
      };
    }
  }, [probe, kind, book.documentId, ready, error, outline, entries, selection, isPdf, pdfAction, pdfReport, exchange.run, exchange.report]);
  useEffect(updateProbe, [updateProbe]);

  // The typography chosen before, for every flowing book (unless already restored from the tab snapshot).
  useEffect(() => {
    if (kind !== 'flow' || style !== null) {
      return;
    }
    let active = true;
    void api
      .setting<unknown>(STYLE_SETTING)
      .then((value) => {
        if (active) {
          setStyle(isReadingStyle(value) ? value : DEFAULT_STYLE);
        }
      })
      .catch(() => {
        if (active) {
          setStyle(DEFAULT_STYLE);
        }
      });
    return () => {
      active = false;
    };
  }, [kind, style]);

  const onShortcut = useRef<(key: Shortcut) => void>(() => {});
  const onSessionClosedRef = useRef(onSessionClosed);
  onSessionClosedRef.current = onSessionClosed;
  const onSnapshotCaptureRef = useRef(onSnapshotCapture);
  onSnapshotCaptureRef.current = onSnapshotCapture;
  const buildSnapshotRef = useRef(buildCurrentSnapshot);
  buildSnapshotRef.current = buildCurrentSnapshot;
  const onTargetReachedRef = useRef(onTargetReached);
  onTargetReachedRef.current = onTargetReached;
  const onReaderShortcutRef = useRef(onReaderShortcut);
  onReaderShortcutRef.current = onReaderShortcut;

  const flushAndClose = useCallback(async (): Promise<void> => {
    if (closing.current) {
      return;
    }
    const captured = buildSnapshotRef.current();
    if (captured !== undefined) {
      onSnapshotCaptureRef.current?.(captured);
    }
    closing.current = true;
    const reader = fixed.current ?? flow.current;
    fixed.current = null;
    flow.current = null;
    if (reader !== null) {
      await enqueueReaderCloseTask(() => reader.close());
    }
  }, []);

  useEffect(() => {
    if (flushRef === undefined) {
      return;
    }
    flushRef.current = flushAndClose;
    return () => {
      if (flushRef.current === flushAndClose) {
        flushRef.current = null;
      }
    };
  }, [flushRef, flushAndClose]);

  // Opening and closing the document.
  useEffect(() => {
    const container = surface.current;
    if (container === null || (kind === 'flow' && style === null)) {
      return;
    }
    let cancelled = false;
    const abort = new AbortController();
    closing.current = false;
    const snapAtOpen = initialSnapshotRef.current;

    if (kind === 'fixed') {
      const canvas = document.createElement('canvas');
      canvas.className = 'reader-canvas';
      container.append(canvas);
      const initialFixed = fixedStateFromSnapshot(snapAtOpen);
      FixedReader.open({
        options,
        canvas,
        document: book.path,
        documentId: book.documentId,
        ...(initialFixed !== null ? { initialState: initialFixed } : {}),
        shortcuts: false,
        bench: null,
        signal: abort.signal,
        onStatus: (nextStatus) => {
          if (cancelled) {
            return;
          }
          hasOpenedLiveRef.current = true;
          lastPageCountRef.current = nextStatus.pageCount;
          lastFixedReadingRef.current = {
            page: nextStatus.page,
            u: nextStatus.u,
            v: nextStatus.v,
            progress: nextStatus.progress,
            mode: nextStatus.mode,
            pageTurnAnimation: nextStatus.animationPreference,
            zoom: nextStatus.zoom,
            spreads: nextStatus.spreads,
          };
          setStatus(nextStatus);
        },
        onFatal: (message) => {
          if (!cancelled) {
            setError(message);
          }
        },
        onSelection: (chosen) => {
          if (cancelled) {
            return;
          }
          const box = canvas.getBoundingClientRect();
          const rect = chosen.rect === null ? null : { ...chosen.rect, x: chosen.rect.x + box.left, y: chosen.rect.y + box.top };
          setActiveMark(null);
          setSelection(chosen.spans.length === 0 ? null : { kind: 'fixed', spans: chosen.spans, text: chosen.text, rect });
        },
        onMarkClick: (id, rect) => {
          if (cancelled) {
            return;
          }
          const box = canvas.getBoundingClientRect();
          setSelection(null);
          setActiveMark({ uuid: id, rect: { ...rect, x: rect.x + box.left, y: rect.y + box.top } });
        },
        onRegion: (region, rect) => {
          if (cancelled) {
            return;
          }
          const box = canvas.getBoundingClientRect();
          setActiveMark(null);
          setSelection(
            region === null
              ? null
              : { kind: 'region', region, text: '', source: null, reading: true, rect: rect === null ? null : { ...rect, x: rect.x + box.left, y: rect.y + box.top } },
          );
          if (region !== null) {
            // Read at once, so a copy has it by the time it is asked for.
            void readRegion(region).then((read) => {
              if (cancelled) {
                return;
              }
              setSelection((now) =>
                now?.kind === 'region' && now.region === region ? { ...now, text: read?.text ?? '', source: read?.source ?? null, reading: false } : now,
              );
            });
          }
        },
      })
        .then(async (reader) => {
          if (cancelled) {
            await enqueueReaderCloseTask(() => reader.close());
            return;
          }
          fixed.current = reader;
          await reader.ready();
          if (cancelled) {
            return;
          }
          hasOpenedLiveRef.current = true;
          const session = reader.session;
          if (session !== null) {
            lastPageCountRef.current = session.pageCount;
            const restoredReading = reader.currentReadingState();
            if (restoredReading !== null) {
              lastFixedReadingRef.current = restoredReading;
            }
            const wantedTool = liveUiRef.current.tool;
            const wantedSnap = liveUiRef.current.snap;
            if (wantedTool !== 'text') {
              reader.setTool(wantedTool);
            }
            if (!wantedSnap) {
              reader.setSnap(wantedSnap);
            }
            api
              .fixedAnnotations(session.id)
              .then((views) => {
                if (!cancelled) {
                  store.load(views.map(fixedEntry));
                }
              })
              .catch((reason: unknown) => {
                if (!cancelled) {
                  setNotice(t('reader.annotationsUnread', { reason: String(reason) }));
                }
              });
            invoke<OutlineItem[]>('document_outline', { session: session.id })
              .then((items) => {
                if (!cancelled) {
                  setOutline(items);
                }
              })
              .catch(() => {
                if (!cancelled) {
                  setOutline([]);
                }
              });
          }
          setReady(true);
        })
        .catch((reason: unknown) => {
          if (!cancelled && !isAbortError(reason)) {
            setError(String(reason instanceof Error ? reason.message : reason));
          }
        });

      return () => {
        const captured = buildSnapshotRef.current();
        if (captured !== undefined) {
          onSnapshotCaptureRef.current?.(captured);
        }
        cancelled = true;
        abort.abort();
        const reader = fixed.current;
        fixed.current = null;
        canvas.remove();
        if (reader !== null) {
          void enqueueReaderCloseTask(() => reader.close()).then(() => onSessionClosedRef.current?.());
        }
      };
    }

    const flowRestore =
      snapAtOpen?.layout === 'flow'
        ? {
            readerCfi: snapAtOpen.readerCfi,
            progress: snapAtOpen.progress,
            mode: snapAtOpen.mode,
            animated: snapAtOpen.pageTurnAnimation,
            style: snapAtOpen.style ?? style ?? DEFAULT_STYLE,
          }
        : undefined;

    FlowReader.open(
      container,
      book.documentId,
      { mode: 'paginated', animated: true, style: style ?? DEFAULT_STYLE },
      {
        relocate: (nextReloc) => {
          if (cancelled) {
            return;
          }
          hasOpenedLiveRef.current = true;
          lastFlowStateRef.current = {
            ...lastFlowStateRef.current,
            readerCfi: nextReloc.cfi,
            section: nextReloc.section,
            progress: nextReloc.progress,
          };
          setRelocation(nextReloc);
        },
        saved: (nextSaved) => {
          if (cancelled) {
            return;
          }
          lastFlowStateRef.current = {
            ...lastFlowStateRef.current,
            progress: nextSaved.progress,
            locator: nextSaved.locator,
          };
          setSaved(nextSaved);
        },
        shortcut: (key) => {
          if (!cancelled) {
            onShortcut.current(key);
          }
        },
        link: (href) => {
          if (!cancelled) {
            setLink(href);
          }
        },
        layout: () => {},
        error: (message) => {
          if (!cancelled) {
            setError(message);
          }
        },
        selection: (chosen) => {
          if (cancelled) {
            return;
          }
          setActiveMark(null);
          setSelection(
            chosen.start === null || chosen.end === null
              ? null
              : { kind: 'flow', start: chosen.start, end: chosen.end, text: chosen.text, rect: chosen.rect },
          );
        },
        markClick: (id, rect) => {
          if (cancelled) {
            return;
          }
          setSelection(null);
          setActiveMark({ uuid: id, rect });
        },
      },
      { restore: flowRestore, signal: abort.signal },
    )
      .then(async (reader) => {
        if (cancelled) {
          await enqueueReaderCloseTask(() => reader.close());
          return;
        }
        flow.current = reader;
        hasOpenedLiveRef.current = true;
        lastFlowStateRef.current = {
          ...lastFlowStateRef.current,
          mode: reader.currentMode,
          animated: reader.isAnimated,
          style: reader.currentStyle,
          readerCfi: reader.relocation?.cfi ?? lastFlowStateRef.current.readerCfi ?? reader.info.saved?.readerCfi ?? null,
        };
        setFlowMode(reader.currentMode);
        setFlowAnimated(reader.isAnimated);
        setToc(reader.info.toc);
        setReady(true);
        reader.focus();
        api
          .flowAnnotations(book.documentId)
          .then((views) => {
            if (!cancelled) {
              store.load(views.map(flowEntry));
            }
          })
          .catch((reason: unknown) => {
            if (!cancelled) {
              setNotice(t('reader.annotationsUnread', { reason: String(reason) }));
            }
          });
      })
      .catch((reason: unknown) => {
        if (!cancelled && !isAbortError(reason)) {
          setError(String(reason instanceof Error ? reason.message : reason));
        }
      });

    return () => {
      const captured = buildSnapshotRef.current();
      if (captured !== undefined) {
        onSnapshotCaptureRef.current?.(captured);
      }
      cancelled = true;
      abort.abort();
      const reader = flow.current;
      flow.current = null;
      if (reader !== null) {
        void enqueueReaderCloseTask(() => reader.close()).then(() => onSessionClosedRef.current?.());
      }
    };
    // The style only matters for opening: later changes go to the open reader.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [book.documentId, book.path, kind, options, style === null]);

  useEffect(updateProbe, [ready, error, updateProbe]);

  // A match found in the book's text, shown once the book is open: on a page, at its word; in a
  // flowing book, at the very place. Gone to once per requested target, so the person can read on.
  const reachedSeq = useRef(-1);
  useEffect(() => {
    if (!ready || target === null || reachedSeq.current === targetSeq) {
      return;
    }
    reachedSeq.current = targetSeq;
    onTargetReachedRef.current?.();
    if (target.kind === 'page') {
      const session = fixed.current?.session;
      const page = target.page;
      if (session === null || session === undefined) {
        return;
      }
      api
        .fulltextPagePoint(session.id, page, target.position)
        .then((point) => fixed.current?.send(point === null ? { type: 'go-to-page', page } : { type: 'go-to-point', page, u: point[0], v: point[1] }))
        .catch(() => fixed.current?.send({ type: 'go-to-page', page }));
      return;
    }
    api
      .fulltextFlowPlace(target.document, target.unit, target.position, target.length)
      .then((place) => {
        flow.current?.goTo({ cfi: place.start });
        flow.current?.focus();
      })
      .catch((reason: unknown) => setNotice(t('reader.matchUnshown', { reason: String(reason) })));
  }, [ready, target, targetSeq]);

  // The reader draws what the store has, every time it changes.
  useEffect(() => {
    if (!ready) {
      return;
    }
    fixed.current?.setMarks(
      entries.flatMap((entry) =>
        entry.marks.map((mark) => ({
          id: entry.annotation.uuid,
          color: entry.annotation.color,
          kind: entry.annotation.kind,
          note: entry.annotation.note.trim() !== '',
          page: mark.page,
          quads: mark.quads,
        })),
      ),
    );
    flow.current?.setMarks(
      entries
        .filter((entry) => entry.ranges.length > 0)
        .map((entry) => ({
          id: entry.annotation.uuid,
          color: entry.annotation.color,
          kind: entry.annotation.kind,
          note: entry.annotation.note.trim() !== '',
          ranges: entry.ranges,
        })),
    );
  }, [entries, ready]);

  const clearSelection = useCallback(() => {
    setSelection(null);
    setActiveMark(null);
    fixed.current?.clearSelection();
    flow.current?.clearSelection();
  }, []);

  /** Makes an annotation of the selection: shown at once, written by the library. */
  const annotate = useCallback(
    async (chosen: ReaderSelection, kind: AnnotationKind, color: string, note = ''): Promise<Entry | null> => {
      lastColor.current = color;
      const quote = chosen.text.replace(/\s+/g, ' ').trim();
      const mark = { kind, color, note };
      if (chosen.kind === 'region') {
        const session = fixed.current?.session;
        if (session === null || session === undefined) {
          return null;
        }
        const shown: Entry = { annotation: draft(book.documentId, kind, color, note, '', null), marks: [], ranges: [], pending: true };
        const written = await store.create(shown, async () => fixedEntry(await api.createRegionAnnotation(session.id, chosen.region, mark)));
        fixed.current?.clearSelection();
        return written;
      }
      if (chosen.kind === 'fixed') {
        const session = fixed.current?.session;
        if (session === null || session === undefined) {
          return null;
        }
        const shown: Entry = { annotation: draft(book.documentId, kind, color, note, quote, null), marks: [], ranges: [], pending: true };
        const written = await store.create(shown, async () => fixedEntry(await api.createFixedAnnotation(session.id, chosen.spans, mark)));
        fixed.current?.clearSelection();
        return written;
      }
      const shown: Entry = {
        annotation: draft(book.documentId, kind, color, note, quote, relocation?.progress ?? null),
        marks: [],
        ranges: [{ start: chosen.start, end: chosen.end }],
        pending: true,
      };
      flow.current?.clearSelection();
      return store.create(shown, async () => flowEntry(await api.createFlowAnnotation(book.documentId, chosen.start, chosen.end, mark)));
    },
    [book.documentId, relocation, store],
  );

  const copyText = useCallback((text: string) => {
    void navigator.clipboard.writeText(text).catch(() => setNotice(t('reader.copyFailed')));
  }, []);

  /** Copies a selection: its characters, or what the text layer or the OCR has under a region. */
  const copySelection = useCallback(
    (chosen: ReaderSelection) => {
      if (chosen.kind !== 'region') {
        copyText(chosen.text);
        return;
      }
      if (!chosen.reading) {
        if (chosen.text !== '') {
          copyText(chosen.text);
        } else {
          setNotice(t('reader.region.noText'));
        }
        return;
      }
      void readRegion(chosen.region).then((read) => {
        if (read === null) {
          setNotice(t('reader.region.unread'));
        } else if (read.text !== '') {
          copyText(read.text);
        } else if (read.pending) {
          setNotice(t('reader.region.pending'));
        } else {
          setNotice(t('reader.region.noText'));
        }
      });
    },
    [copyText, readRegion],
  );

  const removeEntry = useCallback(
    async (uuid: string) => {
      const removed = await store.remove(uuid);
      if (removed !== null) {
        setUndo(removed);
      }
    },
    [store],
  );

  const jump = useCallback((target: Entry) => {
    const mark = target.marks[0];
    if (mark !== undefined) {
      const quad = mark.quads[0];
      fixed.current?.send(
        quad === undefined
          ? { type: 'go-to-page', page: mark.page }
          : { type: 'go-to-point', page: mark.page, u: Math.min(quad[0], quad[6]), v: Math.min(quad[1], quad[3]) },
      );
    } else if (target.ranges.length > 0) {
      flow.current?.goTo({ cfi: target.ranges[0].start });
      flow.current?.focus();
    }
  }, []);

  const mode: FlowMode = kind === 'fixed' ? (status?.mode ?? 'paginated') : flowMode;
  const animated = kind === 'fixed' ? (status?.animationPreference ?? true) : flowAnimated;

  const changeMode = useCallback(
    (next: FlowMode) => {
      if (kind === 'fixed') {
        fixed.current?.send({ type: 'set-mode', mode: next });
      } else {
        flow.current?.setMode(next);
        setFlowMode(next);
      }
    },
    [kind],
  );

  const changeAnimation = useCallback(
    (next: boolean) => {
      if (kind === 'fixed') {
        fixed.current?.send({ type: 'set-animation', on: next });
      } else {
        flow.current?.setAnimated(next);
        setFlowAnimated(next);
      }
    },
    [kind],
  );

  const changeStyle = useCallback((next: ReadingStyle) => {
    setStyle(next);
    flow.current?.setStyle(next);
    void api.setSetting(STYLE_SETTING, next).catch(() => {});
  }, []);

  const back = useCallback(async () => {
    await flushAndClose();
    onBack();
  }, [flushAndClose, onBack]);

  const closeCurrentTab = useCallback(async () => {
    await flushAndClose();
    if (onCloseTab !== undefined) {
      onCloseTab();
    } else {
      onBack();
    }
  }, [flushAndClose, onCloseTab, onBack]);

  const turn = useCallback(
    (step: 1 | -1) => {
      if (kind === 'fixed') {
        fixed.current?.send({ type: 'turn', step });
      } else {
        flow.current?.turn(step);
      }
    },
    [kind],
  );

  const shortcut = useCallback(
    (key: Shortcut) => {
      switch (key) {
        case 'mode':
          changeMode(mode === 'paginated' ? 'continuous' : 'paginated');
          return;
        case 'animation':
          if (mode === 'paginated') {
            changeAnimation(!animated);
          }
          return;
        case 'escape':
          if (tocOpen) {
            setTocOpen(false);
          } else if (isFullscreen) {
            onToggleFullscreen?.();
          }
          return;
        case 'contents':
          setTocOpen((open) => !open);
          return;
        case 'fullscreen':
          onToggleFullscreen?.();
          return;
        default:
          onReaderShortcutRef.current?.(key);
          return;
      }
    },
    [mode, animated, tocOpen, isFullscreen, onToggleFullscreen, changeMode, changeAnimation],
  );
  onShortcut.current = shortcut;

  useEffect(() => {
    const onKey = (event: KeyboardEvent): void => {
      const target = event.target as HTMLElement | null;
      if (
        target !== null &&
        (['INPUT', 'SELECT', 'TEXTAREA'].includes(target.tagName) || target.isContentEditable)
      ) {
        return;
      }
      if (event.altKey && event.key === 'ArrowLeft') {
        event.preventDefault();
        void back();
        return;
      }
      if (event.ctrlKey && (event.key === 'c' || event.key === 'C') && (selection?.kind === 'fixed' || selection?.kind === 'region')) {
        event.preventDefault();
        copySelection(selection);
        return;
      }
      if (event.key === 'Escape' && (selection !== null || activeMark !== null)) {
        clearSelection();
        return;
      }
      if (event.ctrlKey || event.altKey || event.metaKey) {
        return;
      }
      if (event.key === 'c' || event.key === 'C') {
        event.preventDefault();
        shortcut('mode');
      } else if (event.key === 'a' || event.key === 'A') {
        event.preventDefault();
        shortcut('animation');
      } else if (event.key === 'Escape') {
        shortcut('escape');
      } else if (
        kind === 'flow' &&
        NAVIGATION.has(event.key) &&
        !event.defaultPrevented &&
        // Only a control that moves with the arrows keeps them: a group of options, a list. A
        // button just pressed, such as the one that closed the typography panel, does not.
        target?.closest('[role="radiogroup"], [role="listbox"], [role="tree"], [role="grid"], [role="slider"]') == null &&
        // The space bar presses a focused button; it does not also turn the page.
        !(event.key === ' ' && target?.closest('button, [role="button"], [role="switch"], label') != null)
      ) {
        event.preventDefault();
        flow.current?.key(event.key, event.shiftKey);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [kind, shortcut, back, selection, activeMark, clearSelection, copySelection]);

  const sectionOf = useMemo(() => {
    const info = flow.current?.info;
    const indexByHref = new Map((info?.sections ?? []).map((section, index) => [section.href, index]));
    return (href: string) => indexByHref.get(href.split('#')[0]) ?? -1;
    // The sections arrive with the TOC.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [toc]);
  const entry = kind === 'flow' && relocation !== null ? currentEntry(toc, sectionOf, relocation.section) : null;
  // In a fixed-layout document, the entry the reader is in is the last one at or before the page.
  const outlineEntry =
    kind === 'fixed' && outline !== null && status !== null
      ? outline.reduce<OutlineItem | null>(
          (found, item) => (item.page !== null && item.page <= status.page ? item : found),
          null,
        )
      : null;
  const progress = kind === 'flow' ? (saved?.progress ?? relocation?.progress ?? null) : null;

  const position =
    kind === 'fixed'
      ? status !== null
        ? t('reader.position.page', { page: status.page + 1, pages: status.pageCount })
        : ''
      : progress !== null
        ? `${Math.round(progress * 100)}%`
        : '';

  return (
    <div className={`reader reader-${kind}`}>
      <header className="reader-bar" aria-label={t('reader.bar')}>
        <Button id="back-to-library" className="button" onPress={() => void back()} aria-keyshortcuts="Alt+ArrowLeft">
          {t('reader.back')}
        </Button>
        <Button
          id="toc-toggle"
          className="button"
          aria-expanded={tocOpen}
          onPress={() => setTocOpen((open) => !open)}
        >
          {t('reader.toc')}
        </Button>
        <div className="reader-title" title={book.path}>
          <span className="reader-title-text">{book.title}</span>
          <span className="reader-title-sub">{entry?.label ?? outlineEntry?.label ?? formatName(book.format)}</span>
        </div>
        <ToggleButtonGroup
          className="segmented"
          aria-label={t('reader.mode.label')}
          selectionMode="single"
          disallowEmptySelection
          selectedKeys={[mode]}
          onSelectionChange={(keys) => changeMode([...keys][0] as FlowMode)}
        >
          <ToggleButton id="paginated" className="segment" aria-keyshortcuts="C">
            {t('reader.mode.paginated')}
          </ToggleButton>
          <ToggleButton id="continuous" className="segment" aria-keyshortcuts="C">
            {t('reader.mode.continuous')}
          </ToggleButton>
        </ToggleButtonGroup>
        <Switch
          id="page-turn"
          className="switch"
          isSelected={mode === 'paginated' && animated}
          isDisabled={mode !== 'paginated'}
          onChange={changeAnimation}
          aria-keyshortcuts="A"
        >
          <span className="switch-track" aria-hidden="true" />
          {t('reader.animation')}
        </Switch>
        {kind === 'flow' && style !== null ? (
          <DialogTrigger>
            <Button id="typography" className="button" aria-label={t('reader.typography')}>
              <span aria-hidden="true">Aa</span>
            </Button>
            <Popover className="popover" placement="bottom end">
              <TypographyPanel style={style} onChange={changeStyle} />
            </Popover>
          </DialogTrigger>
        ) : null}
        {kind === 'fixed' && status !== null ? (
          <>
            <Button className="button" aria-label={t('reader.zoom.out')} onPress={() => fixed.current?.send({ type: 'zoom', factor: 1 / 1.25 })}>
              −
            </Button>
            <output className="zoom">{Math.round(status.zoom * 100)}%</output>
            <Button className="button" aria-label={t('reader.zoom.in')} onPress={() => fixed.current?.send({ type: 'zoom', factor: 1.25 })}>
              +
            </Button>
            <ToggleButtonGroup
              className="segmented"
              aria-label={t('reader.spreads')}
              selectionMode="single"
              disallowEmptySelection
              selectedKeys={[status.spreads]}
              onSelectionChange={(keys) =>
                fixed.current?.send({ type: 'set-spreads', spreads: [...keys][0] as 'auto' | 'single' | 'double' })
              }
            >
              <ToggleButton id="auto" className="segment">{t('reader.spreads.auto')}</ToggleButton>
              <ToggleButton id="single" className="segment" aria-label={t('reader.spreads.single')}>
                1
              </ToggleButton>
              <ToggleButton id="double" className="segment" aria-label={t('reader.spreads.double')}>
                2
              </ToggleButton>
            </ToggleButtonGroup>
            <ToggleButtonGroup
              className="segmented"
              aria-label={t('reader.tools')}
              selectionMode="single"
              disallowEmptySelection
              selectedKeys={[tool]}
              onSelectionChange={(keys) => {
                const chosen = [...keys][0] as Tool;
                setTool(chosen);
                setSelection(null);
                fixed.current?.setTool(chosen);
              }}
            >
              <ToggleButton id="text" className="segment" aria-label={t('reader.tool.text.label')}>
                {t('reader.tool.text')}
              </ToggleButton>
              <ToggleButton id="lines" className="segment" aria-label={t('reader.tool.lines.label')}>
                {t('reader.tool.lines')}
              </ToggleButton>
              <ToggleButton id="rect" className="segment" aria-label={t('reader.tool.rect.label')}>
                {t('reader.tool.rect')}
              </ToggleButton>
              <ToggleButton id="free" className="segment" aria-label={t('reader.tool.free.label')}>
                {t('reader.tool.free')}
              </ToggleButton>
            </ToggleButtonGroup>
            {tool === 'lines' ? (
              <Switch
                id="line-snap"
                className="switch"
                isSelected={snap}
                onChange={(on) => {
                  setSnap(on);
                  fixed.current?.setSnap(on);
                }}
              >
                <span className="switch-track" aria-hidden="true" />
                {t('reader.snap')}
              </Switch>
            ) : null}
          </>
        ) : null}
        <Button
          id="annotations-toggle"
          className="button"
          aria-expanded={panelOpen}
          onPress={() => setPanelOpen((open) => !open)}
        >
          {entries.length > 0 ? t('reader.annotations.count', { count: entries.length }) : t('reader.annotations')}
        </Button>
        <Button id="open-preferences" className="button" aria-label={t('prefs.title')} aria-keyshortcuts="Control+Comma" onPress={onPreferences}>
          <span aria-hidden="true">⚙</span>
        </Button>
        <div className="reader-position" aria-live="polite">
          <Button className="button" aria-label={t('reader.previous')} onPress={() => turn(-1)}>
            ‹
          </Button>
          <output id="reading-position">{position}</output>
          <Button className="button" aria-label={t('reader.next')} onPress={() => turn(1)}>
            ›
          </Button>
        </div>
      </header>
      <div className="reader-body">
        {tocOpen ? (
          <ScrollFrame className="toc-wrap" target=".toc-panel">
            <nav className="toc-panel" aria-label={t('reader.toc')} onKeyDown={onEscape(() => setTocOpen(false))}>
            <Heading level={2} className="toc-heading">
              {t('reader.toc')}
            </Heading>
            {kind === 'flow' && toc.length > 0 ? (
              <TocList
                points={toc}
                current={entry}
                onPick={(point) => {
                  if (point.href !== null) {
                    flow.current?.goTo({ href: point.href });
                    setTocOpen(false);
                    flow.current?.focus();
                  }
                }}
              />
            ) : kind === 'fixed' && outline !== null && outline.length > 0 ? (
              <ol className="toc-list toc-flat">
                {outline.map((item, index) => (
                  <li key={index} style={{ paddingLeft: `${item.level * 14}px` }}>
                    {item.page !== null ? (
                      <Button
                        className={`toc-entry${item === outlineEntry ? ' current' : ''}`}
                        aria-current={item === outlineEntry ? 'location' : undefined}
                        onPress={() => {
                          if (item.page !== null) {
                            fixed.current?.send({ type: 'go-to-page', page: item.page });
                            setTocOpen(false);
                          }
                        }}
                      >
                        <span className="toc-label">{item.label}</span>
                        <span className="toc-page">{item.page + 1}</span>
                      </Button>
                    ) : (
                      <span className="toc-entry">{item.label}</span>
                    )}
                  </li>
                ))}
              </ol>
            ) : (
              <p className="toc-empty">
                {kind === 'flow'
                  ? t('reader.toc.noneBook')
                  : outline === null
                    ? t('reader.toc.reading')
                    : book.format === 'djvu' || book.format === 'djv'
                      ? t('reader.toc.djvu')
                      : t('reader.toc.noneDocument')}
              </p>
            )}
            </nav>
          </ScrollFrame>
        ) : null}
            <div ref={surface} className={ready ? 'reader-surface is-ready' : 'reader-surface'} style={kind === 'flow' && style !== null ? { background: THEMES[style.theme].background } : undefined} />
        {panelOpen ? (
          <Inspector
            documentId={book.documentId}
            entries={entries}
            pageCount={status?.pageCount ?? null}
            onJump={jump}
            onEdit={(target) => setEditing(target)}
            onRemove={(target) => void removeEntry(target.annotation.uuid)}
            onClose={() => setPanelOpen(false)}
            onLink={setLink}
            pdf={pdfActions}
            exchange={exchange}
            initialState={initialInspectorState}
            onStateChange={handleInspectorStateChange}
          />
        ) : null}
        {!ready && error === null ? (
          <div
            className="reader-loading"
            role="status"
            aria-busy="true"
            style={kind === 'flow' && style !== null ? { background: THEMES[style.theme].background } : undefined}
          >
            {showLoading ? t('reader.opening') : null}
          </div>
        ) : null}
        {error !== null ? (
          <div className="reader-error" role="alert">
            {isMissingBookFileError(error) ? (
              <>
                <p>{t('reader.missingFile.title')}</p>
                <p className="error-detail">{t('reader.missingFile.detail')}</p>
                <p className="error-detail">{book.path}</p>
              </>
            ) : (
              <>
                <p>{t('reader.failed')}</p>
                <p className="error-detail">{error}</p>
              </>
            )}
            <div className="reader-error-actions">
              {onCloseTab !== undefined ? (
                <Button id="close-error-tab" className="button primary" onPress={() => void closeCurrentTab()}>
                  {t('reader.closeTab')}
                </Button>
              ) : null}
              <Button className="button" onPress={() => void back()}>
                {t('reader.backToLibrary')}
              </Button>
            </div>
          </div>
        ) : null}
        {kind === 'flow' && relocation !== null ? (
          <div className="flow-progress" aria-hidden="true">
            <div className="flow-progress-fill" style={{ width: `${(progress ?? 0) * 100}%` }} />
          </div>
        ) : null}
      </div>
      <ModalOverlay isOpen={link !== null} onOpenChange={(open) => !open && setLink(null)} isDismissable className="modal-overlay">
        <Modal className="modal">
          <Dialog aria-label={t('reader.link.label')}>
            <Heading slot="title">{t('reader.link.title')}</Heading>
            <p>{t('reader.link.pointsTo')}</p>
            <p className="link-target">{link}</p>
            <p>
              {link !== null && isWebAddress(link)
                ? t('reader.link.web')
                : t('reader.link.notWeb')}
            </p>
            {linkError !== null ? (
              <p className="link-error" role="alert">
                {linkError}
              </p>
            ) : null}
            <div className="modal-actions">
              {link !== null && isWebAddress(link) ? (
                <Button
                  id="open-in-browser"
                  className="button primary"
                  onPress={() => {
                    const target = link;
                    api
                      .openInBrowser(target)
                      .then(() => setLink(null))
                      .catch((reason: unknown) => setLinkError(String(reason)));
                  }}
                >
                  {t('reader.link.open')}
                </Button>
              ) : null}
              <Button
                id="copy-link"
                className={`button${link !== null && isWebAddress(link) ? '' : ' primary'}`}
                onPress={() => {
                  if (link !== null) {
                    void navigator.clipboard.writeText(link).catch(() => {});
                  }
                  setLink(null);
                }}
              >
                {t('reader.link.copy')}
              </Button>
              <Button className="button" onPress={() => setLink(null)}>
                {t('common.close')}
              </Button>
            </div>
          </Dialog>
        </Modal>
      </ModalOverlay>
      {selection !== null && selection.rect !== null && activeMark === null ? (
        <SelectionToolbar
          rect={selection.rect}
          current={null}
          onColor={(color) => {
            const chosen = selection;
            setSelection(null);
            void annotate(chosen, 'highlight', color);
          }}
          onUnderline={() => {
            const chosen = selection;
            setSelection(null);
            void annotate(chosen, 'underline', lastColor.current);
          }}
          onNote={() => {
            const chosen = selection;
            setSelection(null);
            void annotate(chosen, 'note', lastColor.current).then((written) => {
              if (written !== null) {
                setEditing(written);
              }
            });
          }}
          onCopy={() => copySelection(selection)}
          onClose={clearSelection}
        />
      ) : null}
      {activeMark !== null && store.get(activeMark.uuid) !== undefined
        ? (() => {
            const target = store.get(activeMark.uuid) as Entry;
            const { uuid, color, kind: markKind } = target.annotation;
            return (
              <SelectionToolbar
                rect={activeMark.rect}
                current={{ color, kind: markKind }}
                onColor={(next) => {
                  lastColor.current = next;
                  void store.edit(uuid, { color: next });
                }}
                onUnderline={() => void store.edit(uuid, { kind: markKind === 'underline' ? 'highlight' : 'underline' })}
                onNote={() => {
                  setActiveMark(null);
                  setEditing(target);
                }}
                onCopy={() => {
                  const quote = target.annotation.quote ?? '';
                  if (quote !== '') {
                    copyText(quote);
                  } else {
                    setNotice(target.annotation.quoteSource === 'none' ? t('reader.mark.reading') : t('reader.mark.noText'));
                  }
                }}
                onRemove={() => {
                  setActiveMark(null);
                  void removeEntry(uuid);
                }}
                onClose={() => setActiveMark(null)}
              />
            );
          })()
        : null}
      {editing !== null ? (
        <NoteEditor
          quote={editing.annotation.quote}
          note={store.get(editing.annotation.uuid)?.annotation.note ?? editing.annotation.note}
          onCancel={() => setEditing(null)}
          onSave={(note) => {
            const uuid = editing.annotation.uuid;
            setEditing(null);
            void store.edit(uuid, { note });
          }}
          onLink={setLink}
        />
      ) : null}
      {undo !== null ? (
        <UndoToast
          text={t('reader.removed')}
          onUndo={() => {
            const removed = undo;
            setUndo(null);
            void store.restore(removed);
          }}
          onDone={() => setUndo(null)}
        />
      ) : null}
      {notice !== null ? (
        <div className="toast warning" role="alert">
          <span>{notice}</span>
          <Button className="button" onPress={() => setNotice(null)}>
            {t('common.close')}
          </Button>
        </div>
      ) : null}
    </div>
  );
}
