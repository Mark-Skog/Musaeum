/**
 * The main page's side of a flowing book. It creates the reader's iframe at the book's own origin
 * and talks to it only through `reader/messages.ts`. Everything that comes back is untrusted
 * input, checked by `isFromReader`: the most it can do is move the reader's position, which the
 * host parses again before keeping, ask for the mode or the animation to switch, or name a link
 * that the person then decides about.
 */

import { type FlowInfo, type FlowPosition, api } from '../api';
import {
  type FlowMode,
  type FromReader,
  isFromReader,
  type ReaderTarget,
  type Shortcut,
  type ToReader,
} from '../../reader/messages';
import type { FlowMark } from '../../reader/annotations';
import type { ReaderRect } from '../../reader/messages';
import type { ReadingStyle } from '../../reader/typography';
import { template } from '../i18n';
import { isMotionReduced } from '../preferences/store';
import { createAbortError, withReaderSessionLock } from '../../reader-session-lock';

const READER_SANDBOX = 'allow-scripts allow-same-origin';
/** How long the position has to stay still before it is kept. */
const SAVE_AFTER_MS = 500;

export interface Relocation {
  cfi: string;
  section: number;
  progress: number;
  atStart: boolean;
  atEnd: boolean;
  reason: string;
}

export interface FlowReaderEvents {
  relocate(relocation: Relocation): void;
  saved(position: FlowPosition): void;
  shortcut(key: Shortcut): void;
  link(href: string): void;
  layout(ms: number, change: string): void;
  error(message: string): void;
  /** What the reader selected, with its box in CSS pixels of this page; `start` null when nothing. */
  selection?(selection: { start: string | null; end: string | null; text: string; rect: ReaderRect | null }): void;
  markClick?(id: string, rect: ReaderRect): void;
}

export interface FlowReaderRestore {
  readerCfi?: string | null;
  progress?: number | null;
  mode?: FlowMode;
  animated?: boolean;
  style?: ReadingStyle;
}

export class FlowReader {
  private readonly frame: HTMLIFrameElement;
  private readonly listener: (event: MessageEvent<unknown>) => void;
  private lastRelocation: Relocation | null = null;
  private lastSaved: FlowPosition | null = null;
  private readonly initialPosition: string | null;
  private readonly initialFraction: number | null;
  private saveTimer = 0;
  private dirty = false;
  private saving: Promise<unknown> = Promise.resolve();
  private opened: Promise<void>;
  private closed = false;
  private closePromise: Promise<void> | null = null;
  readonly layouts: { ms: number; change: string }[] = [];

  private constructor(
    readonly info: FlowInfo,
    container: HTMLElement,
    private mode: FlowMode,
    private animated: boolean,
    private style: ReadingStyle,
    initialPosition: string | null,
    initialFraction: number | null,
    private readonly events: FlowReaderEvents,
    signal?: AbortSignal,
  ) {
    this.initialPosition = initialPosition;
    this.initialFraction = initialFraction;
    this.frame = document.createElement('iframe');
    this.frame.setAttribute('sandbox', READER_SANDBOX);
    this.frame.className = 'reader-frame';
    this.frame.title = info.title;
    this.frame.src = info.readerUrl;
    let opened: () => void = () => {};
    let failed: (error: Error) => void = () => {};
    this.opened = new Promise((resolve, reject) => {
      if (signal?.aborted) {
        reject(createAbortError());
        return;
      }
      const onAbort = (): void => {
        reject(createAbortError());
      };
      signal?.addEventListener('abort', onAbort, { once: true });
      opened = () => {
        signal?.removeEventListener('abort', onAbort);
        resolve();
      };
      failed = (error) => {
        signal?.removeEventListener('abort', onAbort);
        reject(error);
      };
    });
    this.listener = (event) => {
      // A section of the book shares the reader's origin, so the origin alone proves nothing:
      // only the reader's own window counts, and only for messages of the known shapes.
      if (event.source !== this.frame.contentWindow || event.origin !== info.origin) {
        return;
      }
      if (!isFromReader(event.data)) {
        return;
      }
      this.handle(event.data, opened, failed);
    };
    window.addEventListener('message', this.listener);
    container.append(this.frame);
  }

  /** Opens a book of the library: normalized in the worker the first time, from the cache after. */
  static async open(
    container: HTMLElement,
    document: number,
    defaults: { mode: FlowMode; animated: boolean; style: ReadingStyle },
    events: FlowReaderEvents,
    options?: { restore?: FlowReaderRestore; signal?: AbortSignal },
  ): Promise<FlowReader> {
    const signal = options?.signal;
    const restore = options?.restore;
    if (signal?.aborted) {
      throw createAbortError();
    }
    const info = await withReaderSessionLock(async () => {
      if (signal?.aborted) {
        throw createAbortError();
      }
      const openedInfo = await api.openFlow(document);
      if (signal?.aborted) {
        await api.closeBook(openedInfo.origin).catch(() => {});
        throw createAbortError();
      }
      return openedInfo;
    });
    const mode = restore?.mode ?? info.saved?.mode ?? defaults.mode;
    const animated = restore?.animated ?? info.saved?.pageTurnAnimation ?? defaults.animated;
    const style = restore?.style ?? defaults.style;
    const initialPosition =
      restore?.readerCfi !== undefined && restore.readerCfi !== null
        ? restore.readerCfi
        : (info.saved?.readerCfi ?? null);
    const initialFraction =
      initialPosition === null && typeof restore?.progress === 'number' && Number.isFinite(restore.progress)
        ? restore.progress
        : null;
    const reader = new FlowReader(
      info,
      container,
      mode,
      animated,
      style,
      initialPosition,
      initialFraction,
      events,
      signal,
    );
    try {
      await reader.opened;
    } catch (error) {
      await reader.close();
      throw error;
    }
    if (signal?.aborted) {
      await reader.close();
      throw createAbortError();
    }
    return reader;
  }

  private post(message: ToReader): void {
    if (!this.closed) {
      this.frame.contentWindow?.postMessage(message, this.info.origin);
    }
  }

  private handle(message: FromReader, opened: () => void, failed: (error: Error) => void): void {
    switch (message.type) {
      case 'ready':
        this.post({
          type: 'open',
          mode: this.mode,
          animated: this.animated,
          style: this.style,
          position: this.initialPosition,
          sections: this.info.sections,
          totalText: this.info.totalText,
          reducedMotion: isMotionReduced(),
          sectionTitle: template('reader.section'),
        });
        return;
      case 'opened':
        if (this.initialPosition === null && this.initialFraction !== null && this.initialFraction > 0) {
          this.post({ type: 'go-to', target: { fraction: this.initialFraction } });
        }
        opened();
        return;
      case 'relocate':
        this.lastRelocation = message;
        this.events.relocate(message);
        if (message.reason !== 'layout') {
          this.scheduleSave();
        }
        return;
      case 'layout':
        this.layouts.push({ ms: message.ms, change: message.change });
        this.events.layout(message.ms, message.change);
        return;
      case 'shortcut':
        this.events.shortcut(message.key);
        return;
      case 'link':
        this.events.link(message.href);
        return;
      case 'selection':
        this.events.selection?.({
          start: message.start,
          end: message.end,
          text: message.text,
          rect: message.rect === null ? null : this.toPage(message.rect),
        });
        return;
      case 'mark-click':
        this.events.markClick?.(message.id, this.toPage(message.rect));
        return;
      case 'failed':
        failed(new Error(message.message));
        this.events.error(message.message);
        return;
      default:
        return;
    }
  }

  /** A box in the reader's page as a box in this one. */
  private toPage(rect: ReaderRect): ReaderRect {
    const frame = this.frame.getBoundingClientRect();
    return { x: rect.x + frame.left, y: rect.y + frame.top, width: rect.width, height: rect.height };
  }

  setMarks(marks: FlowMark[]): void {
    this.post({ type: 'marks', marks });
  }

  clearSelection(): void {
    this.post({ type: 'clear-selection' });
  }

  private scheduleSave(): void {
    this.dirty = true;
    clearTimeout(this.saveTimer);
    this.saveTimer = window.setTimeout(() => {
      this.saveTimer = 0;
      void this.save();
    }, SAVE_AFTER_MS);
  }

  /** Keeps the position now, as a Locator in the library. */
  save(): Promise<unknown> {
    const relocation = this.lastRelocation;
    const fallbackCfi = this.initialPosition;
    const cfi = relocation?.cfi ?? fallbackCfi;
    if (cfi === null) {
      this.dirty = false;
      return this.saving;
    }
    this.dirty = false;
    this.saving = this.saving
      .then(() =>
        api.saveFlowPosition(this.info.documentId, {
          readerCfi: cfi,
          mode: this.mode,
          pageTurnAnimation: this.animated,
        }),
      )
      .then((position) => {
        this.lastSaved = position;
        this.events.saved(position);
      })
      .catch((error: unknown) => console.warn('the position could not be kept', error));
    return this.saving;
  }

  get currentStyle(): ReadingStyle {
    return this.style;
  }

  get currentMode(): FlowMode {
    return this.mode;
  }

  get isAnimated(): boolean {
    return this.animated;
  }

  get relocation(): Relocation | null {
    return this.lastRelocation;
  }

  get savedPosition(): FlowPosition | null {
    return this.lastSaved;
  }

  setMode(mode: FlowMode): void {
    if (mode !== this.mode) {
      this.mode = mode;
      this.post({ type: 'set-mode', mode });
      this.scheduleSave();
    }
  }

  setAnimated(animated: boolean): void {
    this.animated = animated;
    this.post({ type: 'set-animation', animated });
    this.scheduleSave();
  }

  /** The person's motion preference changed: true holds every page still. */
  setReducedMotion(on: boolean): void {
    this.post({ type: 'set-reduced-motion', on });
  }

  /** The language changed: the sections' frames are named in the new one. */
  setLanguage(): void {
    this.post({ type: 'set-section-title', template: template('reader.section') });
  }

  setStyle(style: ReadingStyle): void {
    this.style = style;
    this.post({ type: 'set-style', style });
  }

  goTo(target: ReaderTarget): void {
    this.post({ type: 'go-to', target });
  }

  turn(step: 1 | -1): void {
    this.post({ type: 'turn', step });
  }

  key(key: string, shift: boolean): void {
    this.post({ type: 'key', key, shift });
  }

  focus(): void {
    this.frame.focus();
  }

  /** Closes the book, keeping the last position first and unmounting the layout iframe. */
  async close(): Promise<void> {
    if (this.closePromise !== null) {
      return this.closePromise;
    }
    this.closePromise = (async () => {
      if (this.closed) {
        return;
      }
      if (this.saveTimer !== 0) {
        clearTimeout(this.saveTimer);
        this.saveTimer = 0;
      }
      if (this.dirty) {
        await this.save();
      } else {
        await this.saving;
      }
      this.closed = true;
      window.removeEventListener('message', this.listener);
      try {
        this.frame.src = 'about:blank';
      } catch {
        // Ignore in minimal test DOM mocks.
      }
      this.frame.remove();
      await api.closeBook(this.info.origin).catch(() => {});
    })();
    return this.closePromise;
  }
}
