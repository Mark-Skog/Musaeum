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
import type { StartupOptions } from '../../protocol';
import type { FlowMode, Shortcut } from '../../reader/messages';
import {
  DEFAULT_STYLE,
  isReadingStyle,
  LINE_HEIGHTS,
  MAX_SIZE,
  MIN_SIZE,
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
  type RegionText,
} from '../api';
import { formatName } from '../library/LibraryView';
import { FlowReader, type Relocation } from './flow-reader';
import { Inspector, NoteEditor, type ScreenRect, SelectionToolbar, UndoToast } from './annotations/AnnotationUi';
import { AnnotationStore, draft, type Entry } from './annotations/store';

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
}

function lineHeightLabel(value: number): string {
  return value === 0 ? 'Do livro' : value.toLocaleString('pt-BR', { minimumFractionDigits: 1 });
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
  const set = <K extends keyof ReadingStyle>(key: K, value: ReadingStyle[K]): void =>
    onChange({ ...style, [key]: value });
  return (
    <Dialog className="typography" aria-label="Tipografia">
      <RadioGroup className="field" value={style.family} onChange={(value) => set('family', value as ReadingStyle['family'])}>
        <Label>Fonte</Label>
        <div className="choices">
          <Radio value="publisher" id="font-publisher" className="choice">Do livro</Radio>
          <Radio value="serif" id="font-serif" className="choice">Serifada</Radio>
          <Radio value="sans" id="font-sans" className="choice">Sem serifa</Radio>
        </div>
      </RadioGroup>
      <div className="field">
        <span className="field-label">Tamanho</span>
        <div className="stepper">
          <Button id="font-smaller" className="button" aria-label="Diminuir a letra" isDisabled={style.size <= MIN_SIZE} onPress={() => set('size', Math.max(MIN_SIZE, style.size - 10))}>
            A−
          </Button>
          <output aria-live="polite">{style.size}%</output>
          <Button id="font-larger" className="button" aria-label="Aumentar a letra" isDisabled={style.size >= MAX_SIZE} onPress={() => set('size', Math.min(MAX_SIZE, style.size + 10))}>
            A+
          </Button>
        </div>
      </div>
      <RadioGroup className="field" value={String(style.lineHeight)} onChange={(value) => set('lineHeight', Number(value))}>
        <Label>Entrelinha</Label>
        <div className="choices">
          {LINE_HEIGHTS.map((value) => (
            <Radio key={value} value={String(value)} id={`line-${String(value).replace('.', '-')}`} className="choice">
              {lineHeightLabel(value)}
            </Radio>
          ))}
        </div>
      </RadioGroup>
      <RadioGroup className="field" value={style.margin} onChange={(value) => set('margin', value as ReadingStyle['margin'])}>
        <Label>Margens</Label>
        <div className="choices">
          <Radio value="narrow" id="margin-narrow" className="choice">Estreitas</Radio>
          <Radio value="normal" id="margin-normal" className="choice">Médias</Radio>
          <Radio value="wide" id="margin-wide" className="choice">Largas</Radio>
        </div>
      </RadioGroup>
      <RadioGroup className="field" value={style.theme} onChange={(value) => set('theme', value as ReadingStyle['theme'])}>
        <Label>Tema</Label>
        <div className="choices themes">
          <Radio value="light" id="theme-light" className="choice theme-light">Claro</Radio>
          <Radio value="sepia" id="theme-sepia" className="choice theme-sepia">Sépia</Radio>
          <Radio value="dark" id="theme-dark" className="choice theme-dark">Escuro</Radio>
        </div>
      </RadioGroup>
    </Dialog>
  );
}

export interface ReaderViewProps {
  book: BookRow;
  options: StartupOptions;
  onBack(): void;
  probe?: React.MutableRefObject<ReaderProbe | null>;
}

export function ReaderView({ book, options, onBack, probe }: ReaderViewProps): React.JSX.Element {
  const surface = useRef<HTMLDivElement>(null);
  const fixed = useRef<FixedReader | null>(null);
  const flow = useRef<FlowReader | null>(null);
  const [status, setStatus] = useState<ReaderStatus | null>(null);
  const [flowMode, setFlowMode] = useState<FlowMode>('paginated');
  const [flowAnimated, setFlowAnimated] = useState(true);
  const [relocation, setRelocation] = useState<Relocation | null>(null);
  const [saved, setSaved] = useState<FlowPosition | null>(null);
  const [style, setStyle] = useState<ReadingStyle | null>(null);
  const [toc, setToc] = useState<NavPoint[]>([]);
  /** A fixed-layout document's table of contents, from its outline, once it was read. */
  const [outline, setOutline] = useState<OutlineItem[] | null>(null);
  const [tocOpen, setTocOpen] = useState(false);
  const [link, setLinkState] = useState<string | null>(null);
  const [linkError, setLinkError] = useState<string | null>(null);
  const setLink = useCallback((href: string | null) => {
    setLinkError(null);
    setLinkState(href);
  }, []);
  const [error, setError] = useState<string | null>(null);
  const [ready, setReady] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
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
  const [panelOpen, setPanelOpen] = useState(false);
  const [undo, setUndo] = useState<Entry | null>(null);
  const lastColor = useRef('yellow');
  const closing = useRef(false);
  /** What a press on a page of a fixed layout does, and whether lines snap. */
  const [tool, setTool] = useState<Tool>('text');
  const [snap, setSnap] = useState(true);
  const kind = book.layout;

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
      };
    }
  }, [probe, kind, book.documentId, ready, error, outline, entries, selection]);
  useEffect(updateProbe, [updateProbe]);

  // The typography chosen before, for every flowing book.
  useEffect(() => {
    if (kind !== 'flow') {
      return;
    }
    void api
      .setting<unknown>(STYLE_SETTING)
      .then((value) => setStyle(isReadingStyle(value) ? value : DEFAULT_STYLE))
      .catch(() => setStyle(DEFAULT_STYLE));
  }, [kind]);

  const onShortcut = useRef<(key: Shortcut) => void>(() => {});

  // Opening and closing the document.
  useEffect(() => {
    const container = surface.current;
    if (container === null || (kind === 'flow' && style === null)) {
      return;
    }
    let cancelled = false;
    closing.current = false;
    if (kind === 'fixed') {
      const canvas = document.createElement('canvas');
      canvas.className = 'reader-canvas';
      container.append(canvas);
      FixedReader.open({
        options,
        canvas,
        document: book.path,
        shortcuts: false,
        bench: null,
        onStatus: setStatus,
        onFatal: (message) => setError(message),
        onSelection: (chosen) => {
          const box = canvas.getBoundingClientRect();
          const rect = chosen.rect === null ? null : { ...chosen.rect, x: chosen.rect.x + box.left, y: chosen.rect.y + box.top };
          setActiveMark(null);
          setSelection(chosen.spans.length === 0 ? null : { kind: 'fixed', spans: chosen.spans, text: chosen.text, rect });
        },
        onMarkClick: (id, rect) => {
          const box = canvas.getBoundingClientRect();
          setSelection(null);
          setActiveMark({ uuid: id, rect: { ...rect, x: rect.x + box.left, y: rect.y + box.top } });
        },
        onRegion: (region, rect) => {
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
              setSelection((now) =>
                now?.kind === 'region' && now.region === region ? { ...now, text: read?.text ?? '', source: read?.source ?? null, reading: false } : now,
              );
            });
          }
        },
      })
        .then(async (reader) => {
          if (cancelled) {
            await reader.close();
            return;
          }
          fixed.current = reader;
          await reader.ready();
          const session = reader.session;
          if (session !== null) {
            api
              .fixedAnnotations(session.id)
              .then((views) => {
                if (!cancelled) {
                  store.load(views.map(fixedEntry));
                }
              })
              .catch((reason: unknown) => setNotice(`As anotações não puderam ser lidas: ${String(reason)}`));
            invoke<OutlineItem[]>('document_outline', { session: session.id })
              .then((items) => {
                if (!cancelled) {
                  setOutline(items);
                }
              })
              .catch(() => setOutline([]));
          }
          setReady(true);
        })
        .catch((reason: unknown) => setError(String(reason instanceof Error ? reason.message : reason)));
      return () => {
        cancelled = true;
        const reader = fixed.current;
        fixed.current = null;
        canvas.remove();
        void reader?.close();
      };
    }
    FlowReader.open(container, book.documentId, { mode: 'paginated', animated: true, style: style ?? DEFAULT_STYLE }, {
      relocate: setRelocation,
      saved: setSaved,
      shortcut: (key) => onShortcut.current(key),
      link: setLink,
      layout: () => {},
      error: (message) => setError(message),
      selection: (chosen) => {
        setActiveMark(null);
        setSelection(
          chosen.start === null || chosen.end === null
            ? null
            : { kind: 'flow', start: chosen.start, end: chosen.end, text: chosen.text, rect: chosen.rect },
        );
      },
      markClick: (id, rect) => {
        setSelection(null);
        setActiveMark({ uuid: id, rect });
      },
    })
      .then(async (reader) => {
        if (cancelled) {
          await reader.close();
          return;
        }
        flow.current = reader;
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
          .catch((reason: unknown) => setNotice(`As anotações não puderam ser lidas: ${String(reason)}`));
      })
      .catch((reason: unknown) => setError(String(reason instanceof Error ? reason.message : reason)));
    return () => {
      cancelled = true;
      const reader = flow.current;
      flow.current = null;
      void reader?.close();
    };
    // The style only matters for opening: later changes go to the open reader.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [book.documentId, book.path, kind, options, style === null]);

  useEffect(updateProbe, [ready, error, updateProbe]);

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
    void navigator.clipboard.writeText(text).catch(() => setNotice('O texto não pôde ser copiado.'));
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
          setNotice('Não há texto legível nesta região.');
        }
        return;
      }
      void readRegion(chosen.region).then((read) => {
        if (read === null) {
          setNotice('O texto desta região não pôde ser lido.');
        } else if (read.text !== '') {
          copyText(read.text);
        } else if (read.pending) {
          setNotice('As linhas desta página ainda estão sendo analisadas. Tente de novo daqui a pouco.');
        } else {
          setNotice('Não há texto legível nesta região.');
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
    if (closing.current) {
      return;
    }
    closing.current = true;
    const reader = fixed.current ?? flow.current;
    fixed.current = null;
    flow.current = null;
    await reader?.close();
    onBack();
  }, [onBack]);

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
          setTocOpen(false);
          return;
        case 'contents':
          setTocOpen((open) => !open);
          return;
      }
    },
    [mode, animated, changeMode, changeAnimation],
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
        ? `Página ${status.page + 1} de ${status.pageCount}`
        : ''
      : progress !== null
        ? `${Math.round(progress * 100)}%`
        : '';

  return (
    <div className={`reader reader-${kind}`}>
      <header className="reader-bar" aria-label="Leitura">
        <Button id="back-to-library" className="button" onPress={() => void back()} aria-keyshortcuts="Alt+ArrowLeft">
          ← Biblioteca
        </Button>
        <Button
          id="toc-toggle"
          className="button"
          aria-expanded={tocOpen}
          onPress={() => setTocOpen((open) => !open)}
        >
          Sumário
        </Button>
        <div className="reader-title" title={book.path}>
          <span className="reader-title-text">{book.title}</span>
          <span className="reader-title-sub">{entry?.label ?? outlineEntry?.label ?? formatName(book.format)}</span>
        </div>
        <ToggleButtonGroup
          className="segmented"
          aria-label="Modo de leitura (C alterna)"
          selectionMode="single"
          disallowEmptySelection
          selectedKeys={[mode]}
          onSelectionChange={(keys) => changeMode([...keys][0] as FlowMode)}
        >
          <ToggleButton id="paginated" className="segment" aria-keyshortcuts="C">
            Paginado
          </ToggleButton>
          <ToggleButton id="continuous" className="segment" aria-keyshortcuts="C">
            Contínuo
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
          Animação
        </Switch>
        {kind === 'flow' && style !== null ? (
          <DialogTrigger>
            <Button id="typography" className="button">
              Aa
            </Button>
            <Popover className="popover" placement="bottom end">
              <TypographyPanel style={style} onChange={changeStyle} />
            </Popover>
          </DialogTrigger>
        ) : null}
        {kind === 'fixed' && status !== null ? (
          <>
            <Button className="button" aria-label="Diminuir" onPress={() => fixed.current?.send({ type: 'zoom', factor: 1 / 1.25 })}>
              −
            </Button>
            <output className="zoom">{Math.round(status.zoom * 100)}%</output>
            <Button className="button" aria-label="Aumentar" onPress={() => fixed.current?.send({ type: 'zoom', factor: 1.25 })}>
              +
            </Button>
            <ToggleButtonGroup
              className="segmented"
              aria-label="Páginas lado a lado"
              selectionMode="single"
              disallowEmptySelection
              selectedKeys={[status.spreads]}
              onSelectionChange={(keys) =>
                fixed.current?.send({ type: 'set-spreads', spreads: [...keys][0] as 'auto' | 'single' | 'double' })
              }
            >
              <ToggleButton id="auto" className="segment">Auto</ToggleButton>
              <ToggleButton id="single" className="segment">1</ToggleButton>
              <ToggleButton id="double" className="segment">2</ToggleButton>
            </ToggleButtonGroup>
            <ToggleButtonGroup
              className="segmented"
              aria-label="Marcar"
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
              <ToggleButton id="text" className="segment" aria-label="Selecionar texto">
                Texto
              </ToggleButton>
              <ToggleButton id="lines" className="segment" aria-label="Selecionar por linhas da imagem">
                Linhas
              </ToggleButton>
              <ToggleButton id="rect" className="segment" aria-label="Caixa retangular">
                Caixa
              </ToggleButton>
              <ToggleButton id="free" className="segment" aria-label="Traço livre">
                Traço
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
                Ajustar às palavras
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
          Anotações{entries.length > 0 ? ` (${entries.length})` : ''}
        </Button>
        <div className="reader-position" aria-live="polite">
          <Button className="button" aria-label="Anterior" onPress={() => turn(-1)}>
            ‹
          </Button>
          <output id="reading-position">{position}</output>
          <Button className="button" aria-label="Próxima" onPress={() => turn(1)}>
            ›
          </Button>
        </div>
      </header>
      <div className="reader-body">
        {tocOpen ? (
          <nav className="toc-panel" aria-label="Sumário">
            <Heading level={2} className="toc-heading">
              Sumário
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
                  ? 'Este livro não tem sumário.'
                  : outline === null
                    ? 'Lendo o sumário…'
                    : book.format === 'djvu' || book.format === 'djv'
                      ? 'O sumário de DjVu ainda não é lido.'
                      : 'Este documento não tem sumário.'}
              </p>
            )}
          </nav>
        ) : null}
        <div ref={surface} className="reader-surface" />
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
          />
        ) : null}
        {!ready && error === null ? <div className="reader-loading">Abrindo…</div> : null}
        {error !== null ? (
          <div className="reader-error" role="alert">
            <p>Este livro não pôde ser aberto.</p>
            <p className="error-detail">{error}</p>
            <Button className="button" onPress={() => void back()}>
              Voltar à biblioteca
            </Button>
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
          <Dialog aria-label="Link externo">
            <Heading slot="title">Link para fora do livro</Heading>
            <p>Este link aponta para:</p>
            <p className="link-target">{link}</p>
            <p>
              {link !== null && isWebAddress(link)
                ? 'O Musaeum só abre o endereço no navegador se você pedir.'
                : 'Este endereço não é da web. Copie-o para usá-lo em outro programa.'}
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
                  Abrir no navegador
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
                Copiar endereço
              </Button>
              <Button className="button" onPress={() => setLink(null)}>
                Fechar
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
                    setNotice(target.annotation.quoteSource === 'none' ? 'O texto desta marca ainda está sendo lido.' : 'Esta marca não tem texto.');
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
          text="Anotação removida."
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
            Fechar
          </Button>
        </div>
      ) : null}
    </div>
  );
}
