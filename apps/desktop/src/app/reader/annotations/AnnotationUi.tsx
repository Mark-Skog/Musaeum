/**
 * What the reader sees of annotations: the bar that comes up over a selection or a mark, the note
 * editor, and the inspector, which lists the book's annotations by where they are, filters and
 * searches them, and takes the reader to one.
 */

import { useEffect, useMemo, useRef, useState } from 'react';
import {
  Button,
  Dialog,
  Heading,
  Input,
  Modal,
  ModalOverlay,
  SearchField,
  Switch,
  TextArea,
  TextField,
  ToggleButton,
  ToggleButtonGroup,
} from 'react-aria-components';

import { type AnchorState, type AnnotationKind, api } from '../../api';
import { type ExchangeControls, ExchangeSection } from '../../ExchangeSection';
import { type LinkHandler, Markdown } from './markdown';
import type { Entry } from './store';

/** The palette, in the order the bar shows it. */
export const COLORS: { id: string; label: string; swatch: string }[] = [
  { id: 'yellow', label: 'Amarelo', swatch: '#ffe36b' },
  { id: 'green', label: 'Verde', swatch: '#a8e6a1' },
  { id: 'blue', label: 'Azul', swatch: '#99c9fc' },
  { id: 'pink', label: 'Rosa', swatch: '#faadd6' },
  { id: 'purple', label: 'Roxo', swatch: '#c7b5fc' },
];

const KIND_LABEL: Record<AnnotationKind, string> = {
  highlight: 'Realce',
  underline: 'Sublinhado',
  note: 'Nota',
};

const STATE_LABEL: Record<AnchorState, string | null> = {
  exact: null,
  approximate: 'Aproximada: o texto mudou um pouco nesta revisão',
  orphan: 'Órfã: o trecho não foi encontrado nesta revisão',
};

export interface ScreenRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface ToolbarProps {
  rect: ScreenRect;
  /** The mark this bar is for, or null for a new selection. */
  current: { color: string; kind: AnnotationKind } | null;
  onColor(color: string): void;
  onUnderline(): void;
  onNote(): void;
  onCopy(): void;
  onRemove?(): void;
  onClose(): void;
}

const BAR_WIDTH = 320;
const BAR_HEIGHT = 44;

/** The bar over a selection or a mark: colors, underline, a note, copy, and remove for a mark. */
export function SelectionToolbar(props: ToolbarProps): React.JSX.Element {
  const { rect, current } = props;
  const above = rect.y - BAR_HEIGHT - 8;
  const top = above >= 56 ? above : rect.y + rect.height + 8;
  const left = Math.min(Math.max(8, rect.x + rect.width / 2 - BAR_WIDTH / 2), window.innerWidth - BAR_WIDTH - 8);
  const bar = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const onKey = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') {
        props.onClose();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [props]);
  return (
    <div
      ref={bar}
      className="selection-toolbar"
      role="toolbar"
      aria-label={current === null ? 'Anotar a seleção' : 'Editar a anotação'}
      style={{ top, left }}
    >
      {COLORS.map((color) => (
        <Button
          key={color.id}
          id={`mark-color-${color.id}`}
          className={`swatch${current?.color === color.id ? ' current' : ''}`}
          aria-label={current === null ? `Realçar em ${color.label.toLowerCase()}` : `Mudar para ${color.label.toLowerCase()}`}
          aria-pressed={current?.color === color.id}
          style={{ background: color.swatch }}
          onPress={() => props.onColor(color.id)}
        />
      ))}
      <span className="toolbar-gap" />
      <Button
        id="mark-underline"
        className={`button tool${current?.kind === 'underline' ? ' current' : ''}`}
        aria-label={current?.kind === 'underline' ? 'Realçar em vez de sublinhar' : 'Sublinhar'}
        onPress={props.onUnderline}
      >
        <span style={{ textDecoration: 'underline' }}>S</span>
      </Button>
      <Button id="mark-note" className="button tool" aria-label="Nota" onPress={props.onNote}>
        ✎
      </Button>
      <Button id="mark-copy" className="button tool" aria-label="Copiar o texto" onPress={props.onCopy}>
        ⧉
      </Button>
      {props.onRemove !== undefined ? (
        <Button id="mark-remove" className="button tool danger" aria-label="Remover a marcação" onPress={props.onRemove}>
          ✕
        </Button>
      ) : null}
    </div>
  );
}

/** Writing a note, in Markdown, with a preview. */
export function NoteEditor({
  quote,
  note,
  onSave,
  onCancel,
  onLink,
}: {
  quote: string | null;
  note: string;
  onSave(note: string): void;
  onCancel(): void;
  onLink?: LinkHandler;
}): React.JSX.Element {
  const [text, setText] = useState(note);
  const [preview, setPreview] = useState(false);
  return (
    <ModalOverlay isOpen isDismissable onOpenChange={(open) => !open && onCancel()} className="modal-overlay">
      <Modal className="modal note-modal">
        <Dialog aria-label="Nota">
          <Heading slot="title">Nota</Heading>
          {quote !== null && quote !== '' ? <blockquote className="note-quote">{quote}</blockquote> : null}
          {preview ? (
            <div className="note-preview">{text.trim() === '' ? <p className="muted">Nada escrito.</p> : <Markdown source={text} onLink={onLink} />}</div>
          ) : (
            <TextField className="note-field" value={text} onChange={setText} autoFocus aria-label="Texto da nota, em Markdown">
              <TextArea id="note-text" rows={8} placeholder="Escreva em Markdown: **negrito**, *itálico*, listas com -" />
            </TextField>
          )}
          <div className="modal-actions">
            <Switch className="switch" isSelected={preview} onChange={setPreview}>
              <span className="switch-track" aria-hidden="true" />
              Visualizar
            </Switch>
            <span className="toolbar-gap" />
            <Button className="button" onPress={onCancel}>
              Cancelar
            </Button>
            <Button id="note-save" className="button primary" onPress={() => onSave(text)}>
              Guardar
            </Button>
          </div>
        </Dialog>
      </Modal>
    </ModalOverlay>
  );
}

/** Where an annotation is, as the inspector says it. */
function placeOf(entry: Entry, pageCount: number | null): string {
  const page = entry.marks[0]?.page;
  if (page !== undefined) {
    return pageCount !== null ? `p. ${page + 1}` : `p. ${page + 1}`;
  }
  const progression = entry.annotation.progression;
  return progression !== null ? `${Math.round(progression * 100)}%` : '';
}

/** What the panel offers for a PDF: its own annotations, in and out. */
export interface PdfActions {
  /** The action running, if one is. */
  busy: PdfAction | null;
  /** What the last one did. */
  report: { text: string; failed: boolean } | null;
  run(action: PdfAction): void;
}

export type PdfAction = 'export-pdf' | 'import-pdf' | 'export-xfdf' | 'import-xfdf';

const PDF_ACTIONS: { id: PdfAction; label: string; busy: string }[] = [
  { id: 'export-pdf', label: 'Exportar PDF com anotações (cópia)…', busy: 'Gravando e conferindo a cópia…' },
  { id: 'import-pdf', label: 'Importar anotações do PDF…', busy: 'Lendo as anotações do PDF…' },
  { id: 'export-xfdf', label: 'Exportar XFDF…', busy: 'Gravando o XFDF…' },
  { id: 'import-xfdf', label: 'Importar XFDF…', busy: 'Lendo o XFDF…' },
];

/** A color as the reader keeps it, as CSS: a palette name's swatch, or the color given. */
export function swatchOf(color: string): string {
  if (color.startsWith('#')) {
    return color;
  }
  return COLORS.find((known) => known.id === color)?.swatch ?? '#ffe36b';
}

export interface InspectorProps {
  documentId: number;
  entries: Entry[];
  pageCount: number | null;
  onJump(entry: Entry): void;
  onEdit(entry: Entry): void;
  onRemove(entry: Entry): void;
  onClose(): void;
  /** A click on a web link in a note. */
  onLink?: LinkHandler;
  /** For a PDF: its own annotations, in and out. */
  pdf?: PdfActions;
  /** The book's annotations as JSON-LD, both ways, and its study reports. */
  exchange?: ExchangeControls;
}

/** The book's annotations, by where they are: filtered, searched, and each a way back to its place. */
export function Inspector({ documentId, entries, pageCount, onJump, onEdit, onRemove, onClose, onLink, pdf, exchange }: InspectorProps): React.JSX.Element {
  const [query, setQuery] = useState('');
  const [found, setFound] = useState<Set<string> | null>(null);
  const [colors, setColors] = useState<Set<string>>(new Set());
  const [kind, setKind] = useState<'all' | AnnotationKind>('all');
  const [orphans, setOrphans] = useState(false);

  // The search is the library's, through FTS5: quotes and notes, any accent, inside words.
  useEffect(() => {
    const text = query.trim();
    if (text === '') {
      setFound(null);
      return;
    }
    let cancelled = false;
    const timer = window.setTimeout(() => {
      api
        .searchAnnotations(documentId, text)
        .then((hits) => {
          if (!cancelled) {
            setFound(new Set(hits.map((hit) => hit.annotation.uuid)));
          }
        })
        .catch(() => {
          if (!cancelled) {
            setFound(new Set());
          }
        });
    }, 120);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [documentId, query, entries]);

  const shown = useMemo(
    () =>
      entries.filter(
        (entry) =>
          (found === null || found.has(entry.annotation.uuid)) &&
          (colors.size === 0 || colors.has(entry.annotation.color)) &&
          (kind === 'all' || entry.annotation.kind === kind) &&
          (!orphans || entry.annotation.anchorState === 'orphan'),
      ),
    [entries, found, colors, kind, orphans],
  );
  const orphanCount = entries.filter((entry) => entry.annotation.anchorState === 'orphan').length;

  return (
    <aside className="annotation-panel" aria-label="Anotações">
      <div className="panel-head">
        <Heading level={2} className="toc-heading">
          Anotações <span className="count">{entries.length}</span>
        </Heading>
        <Button className="button" aria-label="Fechar as anotações" onPress={onClose}>
          ✕
        </Button>
      </div>
      <SearchField className="search panel-search" aria-label="Buscar nas anotações" value={query} onChange={setQuery}>
        <Input id="annotation-search" placeholder="Buscar em citações e notas" />
      </SearchField>
      <div className="panel-filters">
        <ToggleButtonGroup
          aria-label="Cores"
          selectionMode="multiple"
          selectedKeys={colors}
          onSelectionChange={(keys) => setColors(new Set([...keys].map(String)))}
          className="filter-colors"
        >
          {COLORS.map((color) => (
            <ToggleButton key={color.id} id={color.id} className="swatch small" aria-label={color.label} style={{ background: color.swatch }} />
          ))}
        </ToggleButtonGroup>
        <ToggleButtonGroup
          aria-label="Tipo"
          selectionMode="single"
          disallowEmptySelection
          selectedKeys={[kind]}
          onSelectionChange={(keys) => setKind([...keys][0] as 'all' | AnnotationKind)}
          className="segmented small"
        >
          <ToggleButton id="all" className="segment">
            Todas
          </ToggleButton>
          <ToggleButton id="highlight" className="segment">
            Realces
          </ToggleButton>
          <ToggleButton id="underline" className="segment">
            Sublinhados
          </ToggleButton>
          <ToggleButton id="note" className="segment">
            Notas
          </ToggleButton>
        </ToggleButtonGroup>
        <Switch id="filter-orphans" className="switch" isSelected={orphans} onChange={setOrphans}>
          <span className="switch-track" aria-hidden="true" />
          Só órfãs ({orphanCount})
        </Switch>
      </div>
      {shown.length === 0 ? (
        <p className="toc-empty">
          {entries.length === 0
            ? 'Selecione um trecho do texto para realçar, sublinhar ou anotar.'
            : 'Nenhuma anotação com esses filtros.'}
        </p>
      ) : (
        <ol className="annotation-list">
          {shown.map((entry) => {
            const { annotation } = entry;
            const state = STATE_LABEL[annotation.anchorState];
            const swatch = swatchOf(annotation.color);
            const canJump = entry.marks.length > 0 || entry.ranges.length > 0;
            return (
              <li key={annotation.uuid} className={`annotation-item ${annotation.anchorState}`} data-annotation={annotation.uuid}>
                <Button
                  className="annotation-jump"
                  isDisabled={!canJump}
                  aria-label={canJump ? `Ir para: ${annotation.quote ?? ''}` : `Órfã: ${annotation.quote ?? ''}`}
                  onPress={() => onJump(entry)}
                >
                  <span className={`annotation-swatch ${annotation.kind}`} style={{ background: annotation.kind === 'underline' ? 'transparent' : swatch, borderColor: swatch }} />
                  <span className="annotation-body">
                    <span className="annotation-meta">
                      {KIND_LABEL[annotation.kind]}
                      {placeOf(entry, pageCount) !== '' ? ` · ${placeOf(entry, pageCount)}` : ''}
                      {entry.pending ? ' · guardando…' : ''}
                    </span>
                    {annotation.quote !== null ? <span className="annotation-quote">{annotation.quote}</span> : null}
                    {state !== null ? <span className="annotation-state">{state}</span> : null}
                  </span>
                </Button>
                {annotation.note.trim() !== '' ? (
                  <div className="annotation-note">
                    <Markdown source={annotation.note} onLink={onLink} />
                  </div>
                ) : null}
                <div className="annotation-actions">
                  <Button className="button small" onPress={() => onEdit(entry)}>
                    Nota
                  </Button>
                  <Button className="button small" onPress={() => onRemove(entry)}>
                    Remover
                  </Button>
                </div>
              </li>
            );
          })}
        </ol>
      )}
      <div className="panel-foot">
      {exchange !== undefined ? (
        <ExchangeSection
          heading="Exportar e importar"
          controls={exchange}
          id="exchange"
          idle="JSON-LD do W3C com tudo das anotações; relatórios de estudo na ordem de leitura."
        />
      ) : null}
      {pdf !== undefined ? (
        <section className="pdf-exchange" aria-label="Anotações do PDF">
          <Heading level={3} className="pdf-exchange-heading">
            No arquivo PDF
          </Heading>
          <div className="pdf-exchange-actions">
            {PDF_ACTIONS.map((action) => (
              <Button
                key={action.id}
                id={`pdf-${action.id}`}
                className="button small"
                isDisabled={pdf.busy !== null}
                onPress={() => pdf.run(action.id)}
              >
                {action.label}
              </Button>
            ))}
          </div>
          <p
            className={`pdf-exchange-status${pdf.report?.failed === true ? ' failed' : ''}`}
            role="status"
            data-busy={pdf.busy ?? ''}
          >
            {pdf.busy !== null
              ? PDF_ACTIONS.find((action) => action.id === pdf.busy)?.busy
              : (pdf.report?.text ?? 'O livro na biblioteca nunca é alterado: a exportação grava uma cópia nova.')}
          </p>
        </section>
      ) : null}
      </div>
    </aside>
  );
}

/** A note that something was removed, with the way back. */
export function UndoToast({ text, onUndo, onDone }: { text: string; onUndo(): void; onDone(): void }): React.JSX.Element {
  useEffect(() => {
    const timer = window.setTimeout(onDone, 7000);
    return () => clearTimeout(timer);
  }, [onDone]);
  return (
    <div className="toast" role="status">
      <span>{text}</span>
      <Button id="undo-remove" className="button" onPress={onUndo}>
        Desfazer
      </Button>
    </div>
  );
}

