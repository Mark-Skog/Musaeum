/**
 * The open book's annotations, as the reader shows them. A change shows at once and is written
 * after; if the library refuses it, what was there before comes back and the reason is reported.
 * Only the library writes: this keeps no copy of its own anywhere.
 */

import type { Annotation, AnnotationEdit, AnnotationKind, PageMark } from '../../api';
import { t } from '../../i18n';

export interface Entry {
  annotation: Annotation;
  /** Where it is on the pages of a fixed layout. */
  marks: PageMark[];
  /** Where it is in a flowing book, as the reader's CFIs. */
  ranges: { start: string; end: string }[];
  /** Shown, and not written yet. */
  pending: boolean;
}

export interface Backend {
  edit(uuid: string, edit: AnnotationEdit): Promise<Annotation>;
  remove(uuid: string): Promise<Annotation>;
  restore(uuid: string): Promise<Annotation>;
}

let provisional = 0;

/** `annotation` with the quote `learned` has, if it had none. */
function withQuote(annotation: Annotation, learned: Annotation): Annotation {
  if (annotation.quote !== null && annotation.quote !== '') {
    return annotation;
  }
  return { ...annotation, quote: learned.quote, quoteSource: learned.quoteSource };
}

/** A UUID-shaped name for an annotation not written yet, which no written one can have. */
export function provisionalUuid(): string {
  provisional++;
  return `00000000-0000-7000-8000-${provisional.toString(16).padStart(12, '0')}`;
}

/** An annotation as it will be, for showing before the library has it. */
export function draft(documentId: number, kind: AnnotationKind, color: string, note: string, quote: string, progression: number | null): Annotation {
  const now = Date.now();
  return {
    id: -1,
    uuid: provisionalUuid(),
    documentId,
    revisionId: -1,
    kind,
    color,
    note,
    quote,
    quoteSource: 'none',
    progression,
    anchorState: 'exact',
    createdAt: now,
    updatedAt: now,
    deletedAt: null,
    segments: [],
  };
}

export class AnnotationStore {
  private entries = new Map<string, Entry>();
  /** Quotes learned of annotations whose writing has not come back yet. */
  private readonly early = new Map<string, Annotation>();
  private readonly listeners = new Set<() => void>();
  private snapshot: Entry[] = [];

  constructor(
    private readonly backend: Backend,
    private readonly onError: (message: string) => void,
  ) {}

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  /** Every annotation shown, by where it starts, orphans last. */
  list = (): Entry[] => this.snapshot;

  get(uuid: string): Entry | undefined {
    return this.entries.get(uuid);
  }

  private changed(): void {
    this.snapshot = [...this.entries.values()].sort(
      (a, b) =>
        (a.annotation.progression ?? 2) - (b.annotation.progression ?? 2) ||
        a.annotation.createdAt - b.annotation.createdAt,
    );
    for (const listener of this.listeners) {
      listener();
    }
  }

  /** What the library has, replacing everything shown. */
  load(entries: Entry[]): void {
    this.entries = new Map(entries.map((entry) => [entry.annotation.uuid, entry]));
    this.changed();
  }

  /** Shows a new annotation at once, then writes it; the written one takes its place. */
  async create(shown: Entry, write: () => Promise<Entry>): Promise<Entry | null> {
    this.entries.set(shown.annotation.uuid, { ...shown, pending: true });
    this.changed();
    try {
      const written = await write();
      this.entries.delete(shown.annotation.uuid);
      const learned = this.early.get(written.annotation.uuid);
      this.early.delete(written.annotation.uuid);
      const annotation = learned === undefined ? written.annotation : withQuote(written.annotation, learned);
      this.entries.set(written.annotation.uuid, { ...written, annotation, pending: false });
      this.changed();
      return written;
    } catch (error) {
      this.entries.delete(shown.annotation.uuid);
      this.changed();
      this.onError(t('annot.store.notSaved', { reason: String(error) }));
      return null;
    }
  }

  async edit(uuid: string, edit: AnnotationEdit): Promise<void> {
    const before = this.entries.get(uuid);
    if (before === undefined) {
      return;
    }
    this.entries.set(uuid, { ...before, annotation: { ...before.annotation, ...edit } });
    this.changed();
    try {
      const written = await this.backend.edit(uuid, edit);
      const now = this.entries.get(uuid);
      if (now !== undefined) {
        this.entries.set(uuid, { ...now, annotation: written });
        this.changed();
      }
    } catch (error) {
      this.entries.set(uuid, before);
      this.changed();
      this.onError(t('annot.store.changeNotSaved', { reason: String(error) }));
    }
  }

  /**
   * What the library learned of an annotation by itself: the quote the OCR read under a mark on a
   * scan, after it was made. Only the quote is taken, so an edit on its way is not undone.
   */
  learned(annotation: Annotation): void {
    const now = this.entries.get(annotation.uuid);
    if (now === undefined) {
      // Read before the writing came back: kept for when it does.
      if (this.early.size < 64) {
        this.early.set(annotation.uuid, annotation);
      }
      return;
    }
    this.entries.set(annotation.uuid, { ...now, annotation: withQuote(now.annotation, annotation) });
    this.changed();
  }

  /** Takes an annotation away from the reader; the library keeps it, deleted logically. */
  async remove(uuid: string): Promise<Entry | null> {
    const before = this.entries.get(uuid);
    if (before === undefined) {
      return null;
    }
    this.entries.delete(uuid);
    this.changed();
    try {
      await this.backend.remove(uuid);
      return before;
    } catch (error) {
      this.entries.set(uuid, before);
      this.changed();
      this.onError(t('annot.store.notRemoved', { reason: String(error) }));
      return null;
    }
  }

  /** Brings back one just removed. */
  async restore(entry: Entry): Promise<void> {
    const uuid = entry.annotation.uuid;
    this.entries.set(uuid, entry);
    this.changed();
    try {
      const written = await this.backend.restore(uuid);
      this.entries.set(uuid, { ...entry, annotation: written });
      this.changed();
    } catch (error) {
      this.entries.delete(uuid);
      this.changed();
      this.onError(t('annot.store.notBack', { reason: String(error) }));
    }
  }
}
