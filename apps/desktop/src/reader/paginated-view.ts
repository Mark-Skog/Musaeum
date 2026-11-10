/**
 * Paginated reading, through foliate-js's paginator: one section at a time, in columns, a page
 * turned at a time, with or without the turn animated.
 *
 * foliate-js keeps its own anchor, which it only moves when the reader turns a page or goes
 * somewhere; a relayout puts the page holding it back on the screen. This view keeps the same
 * anchor as a CFI, so that switching to continuous reading, closing the book or saving the
 * position all start from the very character the reader saw first, never from the start of
 * whatever page a new layout happens to put it on.
 */

import { type Paginator } from '../../../../vendor/foliate-js/paginator.js';
import { boxOf, cfiOf, lineHeightAt, rangeOf, skipSpaces, startOf } from './anchor';
import type { LoadableSection, ViewAnchor, ViewEvents } from './continuous-view';
import { MARGIN_FRACTION, MAX_LINE_PX, type ReadingStyle } from './typography';

interface RelocateDetail {
  reason: string | null;
  range: Range | null;
  index: number;
}

/** The space above and below the text, in CSS pixels. */
const VERTICAL_MARGIN_PX = 44;

export class PaginatedView {
  readonly element: Paginator;
  private anchor: ViewAnchor | null = null;
  private anchorRange: { doc: Document; range: Range } | null = null;
  /** Navigations this view started, whose anchor it chose itself. */
  private navigating = 0;

  constructor(
    book: { sections: LoadableSection[] },
    private readonly sections: LoadableSection[],
    style: ReadingStyle,
    private css: string,
    animated: boolean,
    private readonly events: ViewEvents,
  ) {
    this.element = document.createElement('foliate-paginator') as Paginator;
    this.element.open(book as never);
    this.element.setAttribute('flow', 'paginated');
    this.applyLayout(style);
    this.setAnimated(animated);
    this.element.addEventListener('load', (event) => {
      const { doc, index } = (event as CustomEvent<{ doc: Document; index: number }>).detail;
      this.events.document(doc, index);
    });
    this.element.addEventListener('relocate', (event) =>
      this.onRelocate((event as CustomEvent<RelocateDetail>).detail),
    );
  }

  mount(container: HTMLElement): void {
    container.append(this.element);
    this.element.setStyles(this.css);
  }

  destroy(): void {
    try {
      this.element.destroy();
    } catch {
      // Never opened a section: nothing to let go of.
    }
    this.element.remove();
  }

  private applyLayout(style: ReadingStyle): void {
    this.element.setAttribute('gap', `${Math.round(MARGIN_FRACTION[style.margin] * 100)}%`);
    this.element.setAttribute('margin', `${VERTICAL_MARGIN_PX}px`);
    this.element.setAttribute('max-inline-size', `${Math.round((MAX_LINE_PX * style.size) / 100)}px`);
    this.element.setAttribute('max-column-count', '2');
  }

  setAnimated(animated: boolean): void {
    this.element.toggleAttribute('animated', animated);
  }

  currentAnchor(): ViewAnchor | null {
    return this.anchor;
  }

  private onRelocate({ reason, range, index }: RelocateDetail): void {
    if (reason === 'selection') {
      return;
    }
    if (reason === 'anchor') {
      // A new layout put the anchor's page back: the anchor itself did not move.
      if (this.anchor !== null) {
        this.events.relocate(this.anchor, 'layout');
      }
      return;
    }
    if (reason === 'navigation' && this.navigating > 0) {
      if (this.anchor !== null) {
        this.events.relocate(this.anchor, 'navigation');
      }
      return;
    }
    // Any other navigation is foliate-js's own, as when turning past a section's last page: the
    // anchor is then the first character of the page it shows, as for any page turn.
    if (range === null) {
      return;
    }
    const visible = range.cloneRange();
    visible.collapse(true);
    const start = skipSpaces(visible);
    this.anchor = { index, cfi: cfiOf(this.sections[index].cfi, start) };
    this.anchorRange = { doc: start.startContainer.ownerDocument as Document, range: start };
    this.events.relocate(this.anchor, 'page');
  }

  async goTo(index: number, target: (doc: Document) => Range | null): Promise<void> {
    this.navigating++;
    try {
      await this.goToAnchor(index, target);
    } finally {
      this.navigating--;
    }
  }

  private async goToAnchor(index: number, target: (doc: Document) => Range | null): Promise<void> {
    await this.element.goTo({
      index,
      anchor: (doc: Document) => {
        const range = target(doc) ?? startOf(doc);
        if (range === null) {
          return 0;
        }
        this.anchor = { index, cfi: cfiOf(this.sections[index].cfi, range) };
        this.anchorRange = { doc, range };
        return range;
      },
    });
  }

  /** Lays the section out again with new typography; foliate-js puts the anchor's page back. */
  relayout(style: ReadingStyle, css: string): void {
    this.css = css;
    this.applyLayout(style);
    this.element.setStyles(css);
    this.element.render();
  }

  next(): Promise<void> {
    return this.element.next();
  }

  prev(): Promise<void> {
    return this.element.prev();
  }

  async toEdge(end: boolean): Promise<void> {
    if (end) {
      await this.element.lastSection();
    } else {
      await this.element.firstSection();
    }
  }

  documentOf(index: number): Document | null {
    const shown = this.element.getContents()[0];
    return shown !== undefined && shown.index === index ? shown.doc : null;
  }

  private range(): { doc: Document; range: Range } | null {
    const anchor = this.anchor;
    if (anchor === null) {
      return null;
    }
    const doc = this.documentOf(anchor.index);
    if (doc === null) {
      return null;
    }
    if (this.anchorRange?.doc === doc) {
      return this.anchorRange;
    }
    const range = rangeOf(doc, anchor.cfi);
    if (range === null) {
      return null;
    }
    this.anchorRange = { doc, range };
    return this.anchorRange;
  }

  anchorRangeNow(): Range | null {
    return this.range()?.range ?? null;
  }

  /** For tests: whether the anchor is on the page shown, and where. */
  inspect(): { anchor: ViewAnchor | null; onPage: boolean; left: number | null; lineHeight: number } {
    const found = this.range();
    if (found === null) {
      return { anchor: this.anchor, onPage: false, left: null, lineHeight: 0 };
    }
    const box = boxOf(found.range);
    const { start, size } = this.element;
    if (box === null) {
      return { anchor: this.anchor, onPage: false, left: null, lineHeight: 0 };
    }
    // The section's frame sits one screen into the scrolled element, after a blank page.
    const left = box.left - (start - size);
    return {
      anchor: this.anchor,
      onPage: left >= -1 && box.right - (start - size) <= size + 1,
      left,
      lineHeight: lineHeightAt(found.range),
    };
  }

  atStart(): boolean {
    return this.element.atStart;
  }

  atEnd(): boolean {
    return this.element.atEnd;
  }
}
