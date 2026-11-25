/**
 * The flowing book in the reader: one anchor, two ways of reading it.
 *
 * Whatever changes, the anchor is the first character the reader saw. Turning a page or scrolling
 * moves it; nothing else does. Changing the font, the margins or the width lays the text out
 * again around it, behind a crossfade; switching between paginated and continuous reading opens
 * the other view at it. Every move is reported up, as the reader's CFI, for the host to keep.
 */

import { type ReaderTarget, type SectionPlan, type Shortcut, type FlowMode, type FromReader } from './messages';
import { ContinuousView, type LoadableSection, type ViewAnchor } from './continuous-view';
import { PaginatedView } from './paginated-view';
import { guardSectionLoads } from './section-loads';
import { rangeOf, startOf, textOffset } from './anchor';
import { FlowAnnotations, type FlowMark } from './annotations';
import { changesLayout, contentCss, THEMES, type ReadingStyle } from './typography';

/** The part of foliate-js's EPUB this uses. */
export interface FlowBook {
  sections: (LoadableSection & { id: string; resolveHref(href: string): string })[];
  resolveHref(href: string): { index: number; anchor: (doc: Document) => Element | number | null } | null;
  resolveCFI(cfi: string): { index: number; anchor: (doc: Document) => Range | null };
}

type View = PaginatedView | ContinuousView;

export interface EngineOptions {
  mode: FlowMode;
  animated: boolean;
  style: ReadingStyle;
  sandbox: string;
  reducedMotion: boolean;
  /** A section's frame title, with `{number}` where its number goes. */
  sectionTitle: string;
}

/** Turns what a fragment link finds into a position. */
function asRange(doc: Document, found: Element | number | null): Range | null {
  if (found === null || typeof found === 'number') {
    return found === 1 ? null : startOf(doc);
  }
  const range = doc.createRange();
  range.setStartBefore(found);
  range.collapse(true);
  return range;
}

export class FlowEngine {
  private view: View;
  private mode: FlowMode;
  private animated: boolean;
  /** Whether the person wants as little motion as possible: no page moves while it holds. */
  private reducedMotion: boolean;
  private sectionTitle: string;
  private style: ReadingStyle;
  private anchor: ViewAnchor | null = null;
  /** Keeps changes in order: a second one waits for the first to finish. */
  private queue: Promise<void> = Promise.resolve();
  private wired: WeakRef<Document>[] = [];
  readonly annotations: FlowAnnotations;

  constructor(
    private readonly book: FlowBook,
    private readonly container: HTMLElement,
    private readonly plans: SectionPlan[],
    private readonly totalText: number,
    private readonly options: EngineOptions,
    private readonly send: (message: FromReader) => void,
  ) {
    this.mode = options.mode;
    this.animated = options.animated;
    this.reducedMotion = options.reducedMotion;
    this.sectionTitle = options.sectionTitle;
    this.style = options.style;
    // Both views, and anything else, ask for sections through this.
    guardSectionLoads(book.sections);
    this.annotations = new FlowAnnotations({
      sectionOf: (cfi) => this.book.resolveCFI(cfi).index,
      sectionCfi: (index) => this.book.sections[index].cfi,
      send: (message) => this.send(message),
      dark: () => this.style.theme === 'dark',
      scrollBy: (pixels) => {
        if (this.view instanceof ContinuousView) {
          this.view.scrollByPixels(pixels);
        }
      },
    });
    this.applyTheme();
    this.view = this.createView(this.mode);
  }

  private createView(mode: FlowMode): View {
    const css = contentCss(this.style);
    const events = {
      relocate: (anchor: ViewAnchor, reason: string) => this.onRelocate(anchor, reason),
      document: (doc: Document, index: number) => this.wire(doc, index),
      overlay: (doc: Document, index: number, slot: HTMLElement) => this.annotations.overlayIn(slot, doc, index),
      released: (doc: Document) => this.annotations.release(doc),
    };
    if (mode === 'paginated') {
      const view = new PaginatedView(this.book, this.book.sections, this.style, css, this.animated && !this.reducedMotion, events);
      // foliate-js places a layer over each section's frame and redraws it after every layout.
      view.element.addEventListener('create-overlayer', (event) => {
        const { doc, index, attach } = (
          event as CustomEvent<{ doc: Document; index: number; attach(overlayer: unknown): void }>
        ).detail;
        attach(this.annotations.overlay(doc, index));
      });
      view.mount(this.container);
      return view;
    }
    const view = new ContinuousView(this.book.sections, this.plans, this.options.sandbox, this.style, css, events, (index) => this.titleOf(index));
    view.mount(this.container);
    return view;
  }

  private applyTheme(): void {
    const theme = THEMES[this.style.theme];
    document.documentElement.style.background = theme.background;
    document.documentElement.style.colorScheme = this.style.theme === 'dark' ? 'dark' : 'light';
    this.container.style.background = theme.background;
  }

  private serially(work: () => Promise<void>): Promise<void> {
    this.queue = this.queue.then(work, work);
    return this.queue;
  }

  /** Opens at a reader's CFI, or at the first section read in order. */
  open(position: string | null): Promise<void> {
    return this.serially(async () => {
      if (position !== null && (await this.tryCfi(position))) {
        return;
      }
      const first = this.book.sections.findIndex((section) => section.linear !== 'no');
      await this.view.goTo(Math.max(0, first), (doc) => startOf(doc));
    });
  }

  private async tryCfi(cfi: string): Promise<boolean> {
    try {
      const { index } = this.book.resolveCFI(cfi);
      if (index < 0 || index >= this.book.sections.length) {
        return false;
      }
      await this.view.goTo(index, (doc) => rangeOf(doc, cfi));
      return true;
    } catch {
      return false;
    }
  }

  goTo(target: ReaderTarget): Promise<void> {
    return this.serially(async () => {
      if ('cfi' in target) {
        await this.tryCfi(target.cfi);
      } else if ('href' in target) {
        const found = this.book.resolveHref(target.href);
        if (found !== null && found.index >= 0) {
          await this.view.goTo(found.index, (doc) => asRange(doc, found.anchor(doc)));
        }
      } else {
        await this.goToFraction(target.fraction);
      }
    });
  }

  /** A position this far through the book's text. */
  private async goToFraction(fraction: number): Promise<void> {
    const wanted = Math.max(0, Math.min(1, fraction)) * this.totalText;
    let index = 0;
    for (let at = 0; at < this.plans.length; at++) {
      if (this.plans[at].linear && this.plans[at].bookTextStart <= wanted) {
        index = at;
      }
    }
    const inside = wanted - this.plans[index].bookTextStart;
    await this.view.goTo(index, (doc) => {
      const body = doc.body ?? doc.documentElement;
      const walk = doc.createTreeWalker(body, NodeFilter.SHOW_TEXT);
      let seen = 0;
      for (let node = walk.nextNode(); node !== null; node = walk.nextNode()) {
        const text = node as Text;
        if (seen + text.length > inside && /\S/.test(text.data)) {
          const range = doc.createRange();
          let offset = Math.max(0, Math.floor(inside - seen));
          // Start at a word, so the anchor is a character the reader would point at.
          while (offset > 0 && /\S/.test(text.data[offset - 1] ?? '')) {
            offset--;
          }
          range.setStart(text, Math.min(offset, text.length));
          range.collapse(true);
          return range;
        }
        seen += text.length;
      }
      return startOf(doc);
    });
  }

  private onRelocate(anchor: ViewAnchor, reason: string): void {
    this.anchor = anchor;
    const plan = this.plans[anchor.index];
    let progress = 0;
    if (plan !== undefined && this.totalText > 0) {
      const range = this.view.anchorRangeNow();
      const doc = this.view.documentOf(anchor.index);
      const inside = range !== null && doc !== null ? Math.min(textOffset(doc, range), plan.textLength) : 0;
      progress = Math.min(1, (plan.bookTextStart + inside) / this.totalText);
    }
    this.send({
      type: 'relocate',
      cfi: anchor.cfi,
      section: anchor.index,
      progress,
      atStart: this.view.atStart(),
      atEnd: this.view.atEnd(),
      reason,
    });
  }

  /** Runs `change` behind a crossfade when the page can draw one, and reports how long it took. */
  private async transition(change: string, apply: () => Promise<void> | void): Promise<void> {
    const started = performance.now();
    const start = (document as Document & {
      startViewTransition?: (update: () => Promise<void>) => {
        updateCallbackDone: Promise<void>;
        ready: Promise<void>;
        finished: Promise<void>;
      };
    }).startViewTransition;
    if (this.reducedMotion || typeof start !== 'function') {
      await apply();
    } else {
      const transition = start.call(document, async () => {
        await apply();
      });
      // A change that comes while the last crossfade still plays cuts it short, which rejects
      // these two; that is expected, and the new layout is shown either way.
      transition.ready.catch(() => {});
      transition.finished.catch(() => {});
      await transition.updateCallbackDone.catch((error: unknown) => {
        console.warn('the crossfade was skipped', error);
      });
    }
    this.send({ type: 'layout', ms: performance.now() - started, change });
  }

  setMarks(marks: FlowMark[]): void {
    this.annotations.setMarks(marks);
  }

  clearSelection(): void {
    this.annotations.clearSelection();
  }

  setStyle(style: ReadingStyle): Promise<void> {
    return this.serially(async () => {
      const relayout = changesLayout(this.style, style);
      this.style = style;
      const css = contentCss(style);
      await this.transition(relayout ? 'typography' : 'theme', () => {
        this.applyTheme();
        if (this.view instanceof PaginatedView) {
          this.view.relayout(style, css);
        } else {
          this.view.relayout(relayout ? 'typography' : 'theme', style, css);
        }
      });
      // A new theme draws the marks in its own way; a new layout moves them.
      this.annotations.redrawAll();
    });
  }

  setMode(mode: FlowMode): Promise<void> {
    return this.serially(async () => {
      if (mode === this.mode) {
        return;
      }
      const anchor = this.view.currentAnchor() ?? this.anchor;
      await this.transition('mode', async () => {
        this.view.destroy();
        this.mode = mode;
        this.view = this.createView(mode);
        if (anchor !== null) {
          await this.view.goTo(anchor.index, (doc) => rangeOf(doc, anchor.cfi));
        } else {
          await this.view.goTo(0, (doc) => startOf(doc));
        }
      });
    });
  }

  setAnimated(animated: boolean): void {
    this.animated = animated;
    if (this.view instanceof PaginatedView) {
      this.view.setAnimated(animated && !this.reducedMotion);
    }
  }

  /** Holds every page still, or lets them move again as the page turn preference says. */
  setReducedMotion(on: boolean): void {
    this.reducedMotion = on;
    if (this.view instanceof PaginatedView) {
      this.view.setAnimated(this.animated && !on);
    }
  }

  /** The language changed: the frames shown take the new titles, and the next ones too. */
  setSectionTitle(template: string): void {
    this.sectionTitle = template;
    if (this.view instanceof ContinuousView) {
      this.view.retitle();
    }
  }

  private titleOf(index: number): string {
    return this.sectionTitle.split('{number}').join(String(index + 1));
  }

  turn(step: 1 | -1): Promise<void> {
    if (this.view instanceof PaginatedView) {
      return step > 0 ? this.view.next() : this.view.prev();
    }
    this.view.page(step, this.animated && !this.reducedMotion);
    return Promise.resolve();
  }

  /** What a key does in the book. Returns whether it did anything. */
  key(key: string, shift: boolean): boolean {
    const view = this.view;
    switch (key) {
      case 'ArrowRight':
      case 'PageDown':
        void this.turn(1);
        return true;
      case 'ArrowLeft':
      case 'PageUp':
        void this.turn(-1);
        return true;
      case ' ':
        void this.turn(shift ? -1 : 1);
        return true;
      case 'ArrowDown':
        if (view instanceof ContinuousView) {
          view.scrollByLines(3);
        } else {
          void this.turn(1);
        }
        return true;
      case 'ArrowUp':
        if (view instanceof ContinuousView) {
          view.scrollByLines(-3);
        } else {
          void this.turn(-1);
        }
        return true;
      case 'Home':
        void this.serially(() => view.toEdge(false));
        return true;
      case 'End':
        void this.serially(() => view.toEdge(true));
        return true;
      default:
        return false;
    }
  }

  /** Keys and links in a section's document, which runs no script of its own. */
  private wire(doc: Document, index: number): void {
    doc.addEventListener('keydown', (event) => this.onKey(event));
    doc.addEventListener('click', (event) => this.onClick(event, index));
    this.annotations.wire(doc, index);
    // Weakly, and pruned here: only the test hook below reads these.
    this.wired = this.wired.filter((ref) => (ref.deref()?.defaultView ?? null) !== null);
    this.wired.push(new WeakRef(doc));
  }

  /** For tests: clicks the first link to `href` in a section on screen, as a reader would. */
  clickLink(href: string): boolean {
    for (const ref of this.wired) {
      const doc = ref.deref();
      if (doc === undefined || doc.defaultView === null) {
        continue;
      }
      const link = [...doc.querySelectorAll('a[href]')].find((element) => element.getAttribute('href') === href);
      if (link !== undefined) {
        (link as HTMLElement).click();
        return true;
      }
    }
    return false;
  }

  /** A key pressed in the reader: the book's own keys here, the app's passed up. */
  onKey(event: KeyboardEvent): void {
    if (event.ctrlKey || event.altKey || event.metaKey) {
      return;
    }
    const shortcut: Shortcut | null =
      event.key === 'c' || event.key === 'C'
        ? 'mode'
        : event.key === 'a' || event.key === 'A'
          ? 'animation'
          : event.key === 'Escape'
            ? 'escape'
            : event.key === 'F11'
              ? 'fullscreen'
              : null;
    if (shortcut !== null) {
      event.preventDefault();
      this.send({ type: 'shortcut', key: shortcut });
      return;
    }
    if (this.key(event.key, event.shiftKey)) {
      event.preventDefault();
    }
  }

  private onClick(event: MouseEvent, index: number): void {
    const target = event.target as Element | null;
    const link = target?.closest?.('a[href], area[href]');
    if (link === null || link === undefined) {
      // A click on a mark, with nothing being selected, opens it.
      const doc = target?.ownerDocument ?? null;
      if (doc !== null && (doc.getSelection()?.isCollapsed ?? true)) {
        const hit = this.annotations.hit(doc, event.clientX, event.clientY);
        if (hit !== null) {
          this.send({ type: 'mark-click', id: hit.id, rect: hit.rect });
        }
      }
      return;
    }
    const href = link.getAttribute('href') ?? '';
    event.preventDefault();
    if (/^[a-z][a-z0-9+.-]*:/i.test(href)) {
      // A link out of the book. Only the app may follow it, and only if the reader says so.
      if (/^(https?:|mailto:|tel:)/i.test(href)) {
        this.send({ type: 'link', href });
      }
      return;
    }
    const path = this.book.sections[index].resolveHref(href);
    void this.goTo({ href: path });
  }

  /** For tests driving the real window: where the anchor is and how it is drawn. */
  inspect(): Record<string, unknown> {
    const viewport = { width: this.container.clientWidth, height: this.container.clientHeight };
    if (this.view instanceof ContinuousView) {
      return { mode: this.mode, viewport, ...this.view.inspect() };
    }
    return { mode: this.mode, viewport, ...this.view.inspect() };
  }

  currentAnchor(): ViewAnchor | null {
    return this.view.currentAnchor() ?? this.anchor;
  }

  /** Opens a section at its start. */
  goToSection(index: number): Promise<void> {
    return this.serially(() => this.view.goTo(index, (doc) => startOf(doc)));
  }

  /** For tests: the sections that continue a document the normalizer cut. */
  cuts(): number[] {
    return this.plans.flatMap((plan, index) => (plan.part > 0 ? [index] : []));
  }

  /** For tests: what the host said about a section. */
  plan(index: number): SectionPlan | null {
    return this.plans[index] ?? null;
  }

  /** For tests: how the cuts between loaded parts look, in continuous reading. */
  seams(): { section: number; gap: number; typical: number | null }[] {
    return this.view instanceof ContinuousView ? this.view.seams() : [];
  }

  /** Resolves once every change asked for so far is done. */
  idle(): Promise<void> {
    return this.queue;
  }
}
