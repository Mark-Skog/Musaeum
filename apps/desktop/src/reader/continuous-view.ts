/**
 * Continuous reading: every section of the book one after another, in one scroll, with only the
 * ones near the screen loaded.
 *
 * Each section is an iframe as tall as its text, so the page itself scrolls and the browser's own
 * scrolling, smooth and native, carries the reader from one section to the next. Sections far
 * from the screen are unloaded and keep their height, measured or, before they were ever loaded,
 * estimated from their text; when one above the screen loads and its height turns out different,
 * the scroll moves by the difference in the same frame, so nothing on the screen moves.
 *
 * Where the normalizer cut a long document into parts, the parts follow each other with nothing
 * between them: the containers it repeated at the cut lose their margins, padding and borders on
 * the side of the cut, and the space after the last block of one part and the space before the
 * first block of the next become one, as two margins do in one document, so a chapter reads as
 * one even across parts.
 *
 * The anchor is a character, never a scroll offset. A new font, new margins or a new width lay
 * everything out again, and the anchor's line is put back at the top of the screen.
 */

import { boxOf, cfiOf, firstVisible, lineHeightAt, rangeOf, startOf } from './anchor';
import type { SectionPlan } from './messages';
import { MARGIN_FRACTION, MAX_LINE_PX, type ReadingStyle } from './typography';

/** A section, as the EPUB loader of foliate-js gives it. */
export interface LoadableSection {
  load(): Promise<string>;
  unload(): void;
  cfi: string;
  linear?: string;
}

export interface ViewAnchor {
  index: number;
  cfi: string;
}

export interface ViewEvents {
  /** The reader moved, by `reason`: `scroll`, `navigation` or `layout`. */
  relocate(anchor: ViewAnchor, reason: string): void;
  /** A section's document loaded, for the keys and links it must pass up. */
  document(doc: Document, index: number): void;
  /** In continuous reading, the slot a section's frame is in, where its marks are drawn. */
  overlay?(doc: Document, index: number, slot: HTMLElement): void;
  /** A section's document went away. */
  released?(doc: Document): void;
}

interface Slot {
  index: number;
  element: HTMLDivElement;
  frame: HTMLIFrameElement | null;
  doc: Document | null;
  height: number;
  measured: boolean;
  loading: Promise<void> | null;
  observer: ResizeObserver | null;
  /** Changes whenever the slot is unloaded, so a load that finishes late knows to give up. */
  generation: number;
  /**
   * At a cut above this part: the space before its first block, as last measured. It outlives an
   * unload, so the part above keeps its room until the typography changes.
   */
  lead: number | null;
}

/** Sections kept loaded around the screen, in screen heights above and below it. */
const LOAD_ABOVE = 1;
const LOAD_BELOW = 2;
const KEEP_ABOVE = 4;
const KEEP_BELOW = 5;
/** How long scrolling must stop before the anchor is taken from the screen again. */
const SETTLE_MS = 160;
/** Space between two documents, in ems of the base size. */
const CHAPTER_GAP_EM = 2.5;

const XHTML = 'http://www.w3.org/1999/xhtml';
/**
 * The element put at the end of a part the normalizer cut, after its last block. With a border on
 * top and no margin, it ends where the flow of blocks ends, floats aside, and the margin of the
 * block before it stays above it: the browser itself says where the part's text stops.
 */
const SEAM_TAG = 'musaeum-seam';
const SEAM_STYLE = [
  'display: block',
  'position: static',
  'float: none',
  'clear: none',
  'height: 0',
  'min-height: 0',
  'margin: 0',
  'padding: 0',
  'border: 0 none',
  'border-top: 1px solid transparent',
  'visibility: hidden',
]
  .map((declaration) => `${declaration} !important`)
  .join('; ');
/** Elements that are content even with no text in them. */
const EMBEDDED = 'img, svg, video, audio, canvas, iframe, object, embed, input, button, select, textarea, br, hr, math';
/** Text that is more than white space a browser collapses: a no-break space is content. */
const SOLID_TEXT = /[^ \t\n\r\f]/;

/** Whether the margins of `container`'s children meet, as they do in a flow of blocks. */
function collapsing(container: Element): boolean {
  const display = container.ownerDocument.defaultView?.getComputedStyle(container).display ?? '';
  return !/flex|grid/.test(display);
}

/** How a node takes part in a flow of blocks: as a block, as a line, or not at all. */
function flowRole(node: Node): 'block' | 'line' | 'none' {
  if (node.nodeType === Node.TEXT_NODE || node.nodeType === Node.CDATA_SECTION_NODE) {
    return SOLID_TEXT.test(node.textContent ?? '') ? 'line' : 'none';
  }
  if (node.nodeType !== Node.ELEMENT_NODE) {
    return 'none';
  }
  const element = node as Element;
  const style = element.ownerDocument.defaultView?.getComputedStyle(element);
  if (style === undefined || style.display === 'none' || style.position === 'absolute' || style.position === 'fixed' || style.float !== 'none') {
    return 'none';
  }
  if (style.display.startsWith('inline') || style.display.startsWith('ruby') || style.display === 'contents') {
    // An inline with nothing in it, like the empty anchors books put between sections, makes a
    // line the browser treats as not there.
    const empty =
      style.display === 'inline' &&
      !SOLID_TEXT.test(element.textContent ?? '') &&
      !element.matches(EMBEDDED) &&
      element.querySelector(EMBEDDED) === null;
    return empty ? 'none' : 'line';
  }
  // A block with nothing in it lets the margins around it meet through it.
  return element.getBoundingClientRect().height === 0 ? 'none' : 'block';
}

/** The first or last block of `container`'s own flow, or null if a line or nothing is there. */
function edgeBlock(container: Element, edge: 'start' | 'end', skip: Node | null): Element | null {
  let node = edge === 'end' ? container.lastChild : container.firstChild;
  while (node !== null) {
    if (node !== skip) {
      const role = flowRole(node);
      if (role === 'line') {
        return null;
      }
      if (role === 'block') {
        return node as Element;
      }
    }
    node = edge === 'end' ? node.previousSibling : node.nextSibling;
  }
  return null;
}

export class ContinuousView {
  readonly root: HTMLDivElement;
  private readonly column: HTMLDivElement;
  private readonly slots: Slot[];
  private tops: number[] = [];
  private topsValid = false;
  private anchor: ViewAnchor | null = null;
  private anchorRange: { doc: Document; range: Range } | null = null;
  private style: ReadingStyle;
  private css = '';
  private pxPerUnit = 0.6;
  private settleTimer = 0;
  private frameRequested = false;
  /** Where this view itself last put the scroll: a scroll event there is not the reader moving. */
  private ownTop: number | null = null;
  private destroyed = false;
  private readonly resizeObserver: ResizeObserver;

  constructor(
    private readonly sections: LoadableSection[],
    private readonly plans: SectionPlan[],
    private readonly sandbox: string,
    style: ReadingStyle,
    css: string,
    private readonly events: ViewEvents,
    /** A section's frame title, for the screen reader, in the person's language. */
    private readonly titleOf: (index: number) => string,
  ) {
    this.style = style;
    this.css = css;
    this.root = document.createElement('div');
    this.root.className = 'continuous';
    this.root.tabIndex = -1;
    this.column = document.createElement('div');
    this.column.className = 'continuous-column';
    this.root.append(this.column);
    this.slots = sections.map((_, index) => {
      const element = document.createElement('div');
      element.className = 'continuous-section';
      element.dataset.section = String(index);
      this.column.append(element);
      return {
        index,
        element,
        frame: null,
        doc: null,
        height: 0,
        measured: false,
        loading: null,
        observer: null,
        generation: 0,
        lead: null,
      };
    });
    this.root.addEventListener('scroll', () => this.onScroll(), { passive: true });
    this.resizeObserver = new ResizeObserver(() => this.relayout('window'));
  }

  mount(container: HTMLElement): void {
    container.append(this.root);
    this.applyColumn();
    this.estimateAll();
    this.resizeObserver.observe(this.root);
  }

  destroy(): void {
    this.destroyed = true;
    this.resizeObserver.disconnect();
    clearTimeout(this.settleTimer);
    for (const slot of this.slots) {
      this.unload(slot);
    }
    this.root.remove();
  }

  /** The frames shown take their titles again: the language changed. */
  retitle(): void {
    for (const frame of this.root.querySelectorAll<HTMLIFrameElement>('iframe.continuous-frame')) {
      const index = Number(frame.dataset.section);
      if (Number.isInteger(index)) {
        frame.title = this.titleOf(index);
      }
    }
  }

  /** The anchor, as of the last time the reader stopped. */
  currentAnchor(): ViewAnchor | null {
    return this.anchor;
  }

  private columnWidth(): number {
    const width = this.root.clientWidth || window.innerWidth;
    const margins = width * MARGIN_FRACTION[this.style.margin] * 2;
    return Math.max(200, Math.min(MAX_LINE_PX * (this.style.size / 100), width - margins));
  }

  private applyColumn(): void {
    this.column.style.width = `${Math.round(this.columnWidth())}px`;
  }

  /** The base estimate of a section's height, from its text and the current typography. */
  private estimate(plan: SectionPlan): number {
    const viewport = this.root.clientHeight || window.innerHeight;
    if (plan.textLength === 0) {
      return viewport * 0.9;
    }
    return Math.max(48, plan.textLength * this.pxPerUnit + this.gapAround(plan));
  }

  private gapAround(plan: SectionPlan): number {
    const em = (18 * this.style.size) / 100;
    let gap = 0;
    if (plan.part === 0) {
      gap += CHAPTER_GAP_EM * em;
    }
    if (plan.part === plan.parts - 1) {
      gap += CHAPTER_GAP_EM * em;
    }
    return gap;
  }

  /** Learns how tall text is from the sections measured, for the ones never loaded. */
  private learn(): void {
    let height = 0;
    let units = 0;
    for (const slot of this.slots) {
      const plan = this.plans[slot.index];
      if (slot.measured && slot.doc !== null && plan.textLength > 200) {
        height += slot.height - this.gapAround(plan);
        units += plan.textLength;
      }
    }
    if (units > 0) {
      this.pxPerUnit = height / units;
    } else {
      const em = (18 * this.style.size) / 100;
      const lineHeight = em * (this.style.lineHeight || 1.4);
      const perLine = this.columnWidth() / (em * 0.5);
      this.pxPerUnit = lineHeight / perLine;
    }
  }

  private estimateAll(): void {
    this.learn();
    for (const slot of this.slots) {
      if (!slot.measured || slot.doc === null) {
        slot.measured = false;
        this.setSlotHeight(slot, this.estimate(this.plans[slot.index]));
      }
    }
    this.topsValid = false;
  }

  private setSlotHeight(slot: Slot, height: number): void {
    slot.height = height;
    slot.element.style.height = `${height}px`;
    this.topsValid = false;
  }

  private top(index: number): number {
    if (!this.topsValid) {
      this.tops = new Array<number>(this.slots.length);
      let y = 0;
      for (let at = 0; at < this.slots.length; at++) {
        this.tops[at] = y;
        y += this.slots[at].height;
      }
      this.topsValid = true;
    }
    return this.tops[index] ?? 0;
  }

  /** The slot holding the line at `y`, in the scroll's coordinates. */
  private slotAt(y: number): Slot {
    this.top(0);
    let low = 0;
    let high = this.slots.length - 1;
    while (low < high) {
      const middle = (low + high + 1) >> 1;
      if (this.tops[middle] <= y) {
        low = middle;
      } else {
        high = middle - 1;
      }
    }
    return this.slots[low];
  }

  private setScrollTop(y: number): void {
    const target = Math.max(0, y);
    if (Math.abs(this.root.scrollTop - target) >= 0.5) {
      this.root.scrollTop = target;
      this.ownTop = this.root.scrollTop;
    }
  }

  // --- Loading ---

  private load(slot: Slot): Promise<void> {
    if (slot.doc !== null) {
      return Promise.resolve();
    }
    if (slot.loading !== null) {
      return slot.loading;
    }
    const generation = slot.generation;
    const section = this.sections[slot.index];
    slot.loading = (async () => {
      let src: string;
      try {
        src = await section.load();
      } catch (error) {
        console.warn(`section ${slot.index} could not be loaded`, error);
        return;
      }
      if (slot.generation !== generation || this.destroyed) {
        section.unload();
        return;
      }
      const frame = document.createElement('iframe');
      frame.setAttribute('sandbox', this.sandbox);
      frame.setAttribute('scrolling', 'no');
      frame.className = 'continuous-frame';
      frame.dataset.section = String(slot.index);
      frame.title = this.titleOf(slot.index);
      await new Promise<void>((resolve) => {
        frame.addEventListener('load', () => resolve(), { once: true });
        frame.src = src;
        slot.element.append(frame);
      });
      if (slot.generation !== generation || this.destroyed) {
        frame.remove();
        section.unload();
        return;
      }
      const doc = frame.contentDocument;
      if (doc === null) {
        return;
      }
      slot.frame = frame;
      slot.doc = doc;
      this.prepare(slot);
      this.events.document(doc, slot.index);
      this.events.overlay?.(doc, slot.index, slot.element);
      this.measure(slot, true);
      const observed = doc.body ?? doc.documentElement;
      slot.observer = new ResizeObserver(() => this.measure(slot, false));
      slot.observer.observe(observed);
    })().finally(() => {
      slot.loading = null;
    });
    return slot.loading;
  }

  private unload(slot: Slot): void {
    slot.generation++;
    if (slot.doc !== null) {
      this.events.released?.(slot.doc);
    }
    slot.observer?.disconnect();
    slot.observer = null;
    if (slot.frame !== null) {
      slot.frame.remove();
      this.sections[slot.index].unload();
    }
    if (this.anchorRange?.doc === slot.doc) {
      this.anchorRange = null;
    }
    slot.frame = null;
    slot.doc = null;
  }

  /** The reader's styles, the part's seams, and the space before and after a document. */
  private prepare(slot: Slot): void {
    const doc = slot.doc;
    if (doc === null) {
      return;
    }
    let style = doc.getElementById('musaeum-continuous') as HTMLStyleElement | null;
    if (style === null) {
      style = doc.createElementNS('http://www.w3.org/1999/xhtml', 'style') as HTMLStyleElement;
      style.id = 'musaeum-continuous';
      (doc.head ?? doc.documentElement).append(style);
    }
    const plan = this.plans[slot.index];
    const em = (18 * this.style.size) / 100;
    const viewport = this.root.clientHeight || window.innerHeight;
    style.textContent = [
      this.css,
      'html { margin: 0 !important; padding: 0 !important; overflow: hidden !important; height: auto !important; }',
      `body { margin: 0 !important; padding: 0 !important; display: flow-root !important; }`,
      `body { padding-top: ${plan.part === 0 ? CHAPTER_GAP_EM * em : 0}px !important; padding-bottom: ${plan.part === plan.parts - 1 ? CHAPTER_GAP_EM * em : 0}px !important; }`,
      `img, svg, video { max-width: 100% !important; height: auto; max-height: ${Math.round(viewport * 0.92)}px !important; object-fit: contain; }`,
    ].join('\n');
    this.openSeams(slot);
  }

  /** Takes the repeated containers' edges off at a cut between two parts of one document. */
  private openSeams(slot: Slot): void {
    const doc = slot.doc;
    const body = doc?.body;
    if (doc === null || doc === undefined || body === null || body === undefined) {
      return;
    }
    const plan = this.plans[slot.index];
    const next = this.plans[slot.index + 1];
    const flatten = (element: Element, side: 'start' | 'end'): void => {
      const style = (element as HTMLElement).style;
      if (style === undefined) {
        return;
      }
      style.setProperty(`margin-block-${side}`, '0', 'important');
      style.setProperty(`padding-block-${side}`, '0', 'important');
      style.setProperty(`border-block-${side}-width`, '0', 'important');
    };
    // `start` leads to the part's first element: the steps before the last are the containers the
    // normalizer repeated, and the last is the book's own element, which keeps its edges.
    if (plan.part > 0) {
      let element: Element | null = body;
      for (let depth = 0; depth < plan.start.length - 1 && element !== null; depth++) {
        element = element.firstElementChild;
        if (element !== null) {
          flatten(element, 'start');
        }
      }
    }
    if (next !== undefined && next.document === plan.document && next.part > 0) {
      let element: Element | null = body;
      for (let depth = 0; depth < next.start.length - 1 && element !== null; depth++) {
        element = element.lastElementChild;
        if (element !== null) {
          flatten(element, 'end');
        }
      }
      if (element !== null && doc.getElementsByTagNameNS(XHTML, SEAM_TAG).length === 0) {
        const probe = doc.createElementNS(XHTML, SEAM_TAG);
        probe.setAttribute('aria-hidden', 'true');
        probe.setAttribute('style', SEAM_STYLE);
        element.append(probe);
      }
    }
  }

  /**
   * At a cut below a loaded part: where its flow of blocks ends, and the space after its last
   * block, which in the uncut document meets the space before the next block as two margins do.
   */
  private trailing(slot: Slot): { end: number; space: number } | null {
    const doc = slot.doc;
    const plan = this.plans[slot.index];
    const next = this.plans[slot.index + 1];
    if (doc === null || doc.body === null || next === undefined || next.document !== plan.document || next.part === 0) {
      return null;
    }
    const probe = doc.getElementsByTagNameNS(XHTML, SEAM_TAG)[0];
    if (probe === undefined) {
      return null;
    }
    const top = doc.body.getBoundingClientRect().top;
    const end = probe.getBoundingClientRect().top - top;
    const container = probe.parentElement;
    const last = container !== null && collapsing(container) ? edgeBlock(container, 'end', probe) : null;
    const space = last === null ? 0 : Math.max(0, end - (last.getBoundingClientRect().bottom - top));
    return { end, space };
  }

  /** At a cut above a loaded part: the space before its first block. */
  private leading(slot: Slot): number | null {
    const doc = slot.doc;
    const plan = this.plans[slot.index];
    if (doc === null || doc.body === null || plan.part === 0) {
      return null;
    }
    let container: Element | null = doc.body;
    for (let depth = 0; depth < plan.start.length - 1 && container !== null; depth++) {
      container = container.firstElementChild;
    }
    const first = container !== null && collapsing(container) ? edgeBlock(container, 'start', null) : null;
    return first === null ? 0 : Math.max(0, first.getBoundingClientRect().top - doc.body.getBoundingClientRect().top);
  }

  /**
   * How tall a loaded part's frame is, and how much room it takes in the column. At a cut, the
   * part takes room down to where its blocks end, less the smaller of the two spaces around the
   * cut: they become one, the larger, as two margins next to each other do. The frame goes on
   * below its room by that much, and by any float hanging past the end, under the next part.
   */
  private heights(slot: Slot): { frame: number; room: number } {
    const doc = slot.doc;
    const body = doc === null ? null : (doc.body ?? doc.documentElement);
    const frame = body === null ? 1 : Math.max(1, Math.ceil(body.getBoundingClientRect().height));
    const trailing = this.trailing(slot);
    if (trailing === null) {
      return { frame, room: frame };
    }
    const lead = this.slots[slot.index + 1]?.lead ?? null;
    const overlap = lead === null ? 0 : Math.min(trailing.space, lead);
    return { frame, room: Math.max(1, trailing.end - overlap) };
  }

  /** Reads a loaded section's height and keeps everything on the screen where it was. */
  private measure(slot: Slot, first: boolean): void {
    if (slot.doc === null || slot.frame === null) {
      return;
    }
    const lead = this.leading(slot);
    const leadMoved = lead !== null && (slot.lead === null || Math.abs(lead - slot.lead) >= 0.25);
    slot.lead = lead;
    const { frame, room } = this.heights(slot);
    const before = slot.height;
    const wasAbove = this.top(slot.index) + before <= this.root.scrollTop;
    slot.frame.style.height = `${frame}px`;
    slot.measured = true;
    if (Math.abs(room - before) >= 0.5) {
      this.setSlotHeight(slot, room);
      if (wasAbove) {
        // A section above the screen grew or shrank: the screen keeps showing the same thing.
        this.setScrollTop(this.root.scrollTop + (room - before));
      } else if (!first && this.anchor !== null && this.anchor.index === slot.index && !this.scrolling()) {
        this.restore();
      }
    }
    // The part above, cut from the same document, ends where this one's first block says.
    const above = this.slots[slot.index - 1];
    if (leadMoved && above !== undefined && above.doc !== null) {
      this.measure(above, false);
    }
  }

  private scrolling(): boolean {
    return this.settleTimer !== 0;
  }

  // --- Moving ---

  private onScroll(): void {
    const own = this.ownTop !== null && Math.abs(this.root.scrollTop - this.ownTop) < 1;
    this.ownTop = null;
    if (own) {
      this.schedule();
      return;
    }
    this.schedule();
    clearTimeout(this.settleTimer);
    this.settleTimer = window.setTimeout(() => {
      this.settleTimer = 0;
      this.settle();
    }, SETTLE_MS);
  }

  private schedule(): void {
    if (this.frameRequested) {
      return;
    }
    this.frameRequested = true;
    requestAnimationFrame(() => {
      this.frameRequested = false;
      this.fill();
    });
  }

  /** Loads what is near the screen and lets go of what is far from it. */
  private fill(): Promise<void>[] {
    if (this.destroyed) {
      return [];
    }
    const viewport = this.root.clientHeight;
    const top = this.root.scrollTop;
    const loads: Promise<void>[] = [];
    const first = this.slotAt(Math.max(0, top - viewport * LOAD_ABOVE)).index;
    const last = this.slotAt(top + viewport * (1 + LOAD_BELOW)).index;
    for (let index = first; index <= last; index++) {
      loads.push(this.load(this.slots[index]));
    }
    for (const slot of this.slots) {
      if (slot.doc === null) {
        continue;
      }
      const y = this.top(slot.index);
      const far =
        y + slot.height < top - viewport * KEEP_ABOVE || y > top + viewport * (1 + KEEP_BELOW);
      if (far && (this.anchor === null || this.anchor.index !== slot.index)) {
        this.unload(slot);
      }
    }
    return loads;
  }

  /** Takes the anchor from the screen, once the reader stopped scrolling. */
  private settle(): void {
    const anchor = this.anchorFromScreen();
    if (anchor !== null) {
      this.anchor = anchor;
      this.events.relocate(anchor, 'scroll');
    }
  }

  private anchorFromScreen(): ViewAnchor | null {
    const y = this.root.scrollTop + 1;
    const slot = this.slotAt(y);
    if (slot.doc === null) {
      return null;
    }
    const range = firstVisible(slot.doc, y - this.top(slot.index));
    if (range === null) {
      return null;
    }
    this.anchorRange = { doc: slot.doc, range };
    return { index: slot.index, cfi: cfiOf(this.sections[slot.index].cfi, range) };
  }

  /** The anchor's range in its document, if that document is loaded. */
  private range(): { doc: Document; range: Range } | null {
    const anchor = this.anchor;
    if (anchor === null) {
      return null;
    }
    const slot = this.slots[anchor.index];
    if (slot.doc === null) {
      return null;
    }
    if (this.anchorRange?.doc === slot.doc) {
      return this.anchorRange;
    }
    const range = rangeOf(slot.doc, anchor.cfi) ?? startOf(slot.doc);
    if (range === null) {
      return null;
    }
    this.anchorRange = { doc: slot.doc, range };
    return this.anchorRange;
  }

  /** Puts the anchor's line at the top of the screen. */
  private restore(): void {
    const found = this.range();
    if (found === null || this.anchor === null) {
      return;
    }
    const box = boxOf(found.range);
    if (box === null) {
      return;
    }
    this.setScrollTop(this.top(this.anchor.index) + box.top);
  }

  /** Opens the book at a section, at the position `target` finds in its document. */
  async goTo(index: number, target: (doc: Document) => Range | null): Promise<void> {
    const slot = this.slots[Math.max(0, Math.min(index, this.slots.length - 1))];
    await this.load(slot);
    if (slot.doc === null) {
      return;
    }
    const range = target(slot.doc) ?? startOf(slot.doc);
    if (range === null) {
      return;
    }
    this.anchor = { index: slot.index, cfi: cfiOf(this.sections[slot.index].cfi, range) };
    this.anchorRange = { doc: slot.doc, range };
    this.restore();
    // What is around the anchor now loads; anything above it that changes height moves the
    // scroll with it, and the anchor goes back to the top once all of it is in.
    await Promise.all(this.fill());
    this.restore();
    await Promise.all(this.fill());
    this.restore();
    this.events.relocate(this.anchor, 'navigation');
  }

  /** Changes the typography or the width, keeping the anchor's line at the top. */
  relayout(change: string, style?: ReadingStyle, css?: string): void {
    if (this.destroyed) {
      return;
    }
    if (style !== undefined && css !== undefined) {
      this.style = style;
      this.css = css;
    }
    // The anchor before anything moves, if the reader is in the middle of a scroll.
    if (this.scrolling()) {
      clearTimeout(this.settleTimer);
      this.settleTimer = 0;
      this.anchor = this.anchorFromScreen() ?? this.anchor;
    }
    this.applyColumn();
    for (const slot of this.slots) {
      if (slot.doc !== null) {
        this.prepare(slot);
      }
    }
    // Every loaded section, measured now, all in one go: everything read before anything is
    // written, so one layout, and nothing drawn between. A part not loaded will measure its lead
    // again when it is.
    for (const slot of this.slots) {
      slot.lead = slot.doc !== null ? this.leading(slot) : null;
    }
    const measured: [Slot, { frame: number; room: number }][] = [];
    for (const slot of this.slots) {
      if (slot.doc !== null && slot.frame !== null) {
        measured.push([slot, this.heights(slot)]);
      }
    }
    for (const [slot, { frame, room }] of measured) {
      if (slot.frame !== null) {
        slot.frame.style.height = `${frame}px`;
      }
      this.setSlotHeight(slot, room);
    }
    this.estimateAll();
    this.restore();
    if (change !== 'window' && this.anchor !== null) {
      this.events.relocate(this.anchor, 'layout');
    }
    this.schedule();
  }

  /** Scrolls by about a screen, keeping a line of what was seen. */
  page(step: 1 | -1, animated: boolean): void {
    const viewport = this.root.clientHeight;
    const anchor = this.range();
    const line = anchor !== null ? lineHeightAt(anchor.range) : 24;
    this.root.scrollBy({ top: step * Math.max(line, viewport - line * 1.5), behavior: animated ? 'smooth' : 'auto' });
  }

  /** Scrolls by a number of CSS pixels, as a drag near the screen's edge does. */
  scrollByPixels(pixels: number): void {
    this.root.scrollBy({ top: pixels });
  }

  scrollByLines(lines: number): void {
    const anchor = this.range();
    const line = anchor !== null ? lineHeightAt(anchor.range) : 24;
    this.root.scrollBy({ top: lines * line });
  }

  async toEdge(end: boolean): Promise<void> {
    const index = end ? this.slots.length - 1 : 0;
    await this.goTo(index, (doc) => (end ? null : startOf(doc)));
    if (end) {
      this.setScrollTop(this.root.scrollHeight);
      this.settle();
    }
  }

  /** For tests: where the anchor is drawn now, against the top of the screen. */
  inspect(): { anchor: ViewAnchor | null; top: number | null; lineHeight: number; viewportTop: number } {
    const found = this.range();
    const anchor = this.anchor;
    if (found === null || anchor === null) {
      return { anchor, top: null, lineHeight: 0, viewportTop: this.root.scrollTop };
    }
    const box = boxOf(found.range);
    return {
      anchor,
      top: box === null ? null : this.top(anchor.index) + box.top - this.root.scrollTop,
      lineHeight: lineHeightAt(found.range),
      viewportTop: this.root.scrollTop,
    };
  }

  /**
   * For tests: at each cut between two loaded parts of one document, the space from the last block
   * of the first part to the first block of the second, and the usual space between two blocks
   * side by side inside the first part, which is what the cut should look like.
   */
  seams(): { section: number; gap: number; typical: number | null; space: number | null; lead: number | null }[] {
    const blocks = 'p, li, h1, h2, h3, h4, h5, h6, pre, blockquote, figure, table, dl';
    const seams: { section: number; gap: number; typical: number | null; space: number | null; lead: number | null }[] = [];
    for (let at = 0; at + 1 < this.slots.length; at++) {
      const first = this.slots[at].doc;
      const second = this.slots[at + 1].doc;
      if (first === null || second === null || this.plans[at].document !== this.plans[at + 1].document) {
        continue;
      }
      const before = Array.from(first.querySelectorAll(blocks));
      const last = before.at(-1);
      const next = second.querySelector(blocks);
      if (last === undefined || next === null) {
        continue;
      }
      const gap =
        this.top(at + 1) + next.getBoundingClientRect().top - (this.top(at) + last.getBoundingClientRect().bottom);
      const gaps: number[] = [];
      for (let index = Math.max(1, before.length - 40); index < before.length; index++) {
        if (before[index - 1].parentElement !== before[index].parentElement) {
          continue;
        }
        const above = before[index - 1].getBoundingClientRect();
        const below = before[index].getBoundingClientRect();
        if (below.top >= above.bottom) {
          gaps.push(below.top - above.bottom);
        }
      }
      gaps.sort((a, b) => a - b);
      seams.push({
        section: at,
        gap,
        typical: gaps.length > 0 ? gaps[gaps.length >> 1] : null,
        space: this.trailing(this.slots[at])?.space ?? null,
        lead: this.slots[at + 1].lead,
      });
    }
    return seams;
  }

  /** The document of a loaded section, for tests and for text offsets. */
  documentOf(index: number): Document | null {
    return this.slots[index]?.doc ?? null;
  }

  /** The anchor's range, for the progress through its section. */
  anchorRangeNow(): Range | null {
    return this.range()?.range ?? null;
  }

  atStart(): boolean {
    return this.root.scrollTop <= 1;
  }

  atEnd(): boolean {
    return this.root.scrollTop + this.root.clientHeight >= this.root.scrollHeight - 1;
  }
}
