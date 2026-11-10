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
import type { ReadingStyle } from '../../reader/typography';

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
}

export class FlowReader {
  private readonly frame: HTMLIFrameElement;
  private readonly listener: (event: MessageEvent<unknown>) => void;
  private lastRelocation: Relocation | null = null;
  private lastSaved: FlowPosition | null = null;
  private saveTimer = 0;
  private saving: Promise<unknown> = Promise.resolve();
  private opened: Promise<void>;
  private closed = false;
  readonly layouts: { ms: number; change: string }[] = [];

  private constructor(
    readonly info: FlowInfo,
    container: HTMLElement,
    private mode: FlowMode,
    private animated: boolean,
    private style: ReadingStyle,
    private readonly events: FlowReaderEvents,
  ) {
    this.frame = document.createElement('iframe');
    this.frame.setAttribute('sandbox', READER_SANDBOX);
    this.frame.className = 'reader-frame';
    this.frame.title = info.title;
    this.frame.src = info.readerUrl;
    let opened: () => void = () => {};
    let failed: (error: Error) => void = () => {};
    this.opened = new Promise((resolve, reject) => {
      opened = resolve;
      failed = reject;
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
  ): Promise<FlowReader> {
    const info = await api.openFlow(document);
    const mode = info.saved?.mode ?? defaults.mode;
    const animated = info.saved?.pageTurnAnimation ?? defaults.animated;
    const reader = new FlowReader(info, container, mode, animated, defaults.style, events);
    try {
      await reader.opened;
    } catch (error) {
      await reader.close();
      throw error;
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
          position: this.info.saved?.readerCfi ?? null,
          sections: this.info.sections,
          totalText: this.info.totalText,
          reducedMotion: matchMedia('(prefers-reduced-motion: reduce)').matches,
        });
        return;
      case 'opened':
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
      case 'failed':
        failed(new Error(message.message));
        this.events.error(message.message);
        return;
      default:
        return;
    }
  }

  private scheduleSave(): void {
    clearTimeout(this.saveTimer);
    this.saveTimer = window.setTimeout(() => {
      this.saveTimer = 0;
      void this.save();
    }, SAVE_AFTER_MS);
  }

  /** Keeps the position now, as a Locator in the library. */
  save(): Promise<unknown> {
    const relocation = this.lastRelocation;
    if (relocation === null) {
      return this.saving;
    }
    this.saving = this.saving
      .then(() =>
        api.saveFlowPosition(this.info.documentId, {
          readerCfi: relocation.cfi,
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

  /** Closes the book, keeping the last position first. */
  async close(): Promise<void> {
    if (this.closed) {
      return;
    }
    if (this.saveTimer !== 0) {
      clearTimeout(this.saveTimer);
      this.saveTimer = 0;
      await this.save();
    } else {
      await this.saving;
    }
    this.closed = true;
    window.removeEventListener('message', this.listener);
    this.frame.remove();
    await api.closeBook().catch(() => {});
  }
}
