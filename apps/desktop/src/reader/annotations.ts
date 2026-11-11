/**
 * Annotations in a flowing book, inside the reader: what the reader selects, and the marks drawn
 * over the text.
 *
 * A selection is the DOM's own, in the section documents. In continuous reading the text of one
 * document can be cut into several parts, each in its own frame, and a drag that leaves one frame
 * carries on into the next: the frame it started in keeps the pointer, and this extends the
 * selection of the frames below or above by hand. What goes to the main page is where the
 * selection starts and ends, as the reader's CFIs; the host works out the rest.
 *
 * Marks are drawn in an SVG layer over each section's frame, from the client rectangles of the
 * marked ranges, in the frame's own coordinates: in paginated reading the layer is the one foliate-js
 * places over its frame (`create-overlayer`), and in continuous reading it lies over the part's
 * frame in its slot. Nothing is added to the book's documents.
 */

import { cfiOf, rangeOf } from './anchor';
import type { FromReader, ReaderRect } from './messages';

/** One annotation as the main page describes it: its look and the stretches it covers. */
export interface FlowMark {
  id: string;
  color: string;
  kind: 'highlight' | 'underline' | 'note';
  note: boolean;
  ranges: { start: string; end: string }[];
}

/** Highlighter colors, as for fixed layouts. */
const FILL: Record<string, string> = {
  yellow: '#ffe36b',
  green: '#a8e6a1',
  blue: '#99c9fc',
  pink: '#faadd6',
  purple: '#c7b5fc',
};
const INK: Record<string, string> = {
  yellow: '#d99a00',
  green: '#21993a',
  blue: '#1a66d9',
  pink: '#d93380',
  purple: '#7340d9',
};
const SVG = 'http://www.w3.org/2000/svg';
const MAX_TEXT = 65536;

interface Overlay {
  doc: Document;
  index: number;
  element: SVGSVGElement;
  /** Each mark's rectangles as last drawn, in the frame's coordinates, for clicks. */
  drawn: Map<string, DOMRect[]>;
  observer: ResizeObserver | null;
}

interface Hooks {
  /** The section a reader CFI is in. */
  sectionOf(cfi: string): number;
  /** A section's base CFI, which the reader's CFIs of its positions start with. */
  sectionCfi(index: number): string;
  send(message: FromReader): void;
  dark(): boolean;
  /** Scrolls continuous reading, while a drag is near the top or bottom of the screen. */
  scrollBy(pixels: number): void;
}

const rectOf = (rect: DOMRect | { left: number; top: number; width: number; height: number }, dx: number, dy: number): ReaderRect => ({
  x: rect.left + dx,
  y: rect.top + dy,
  width: rect.width,
  height: rect.height,
});

/** Rectangles of one line merged, so a mark is one shape per line and never darker where two meet. */
function merged(rects: DOMRect[]): DOMRect[] {
  const lines: DOMRect[] = [];
  const sorted = rects
    .filter((rect) => rect.width > 0.5 && rect.height > 0.5)
    .sort((a, b) => a.top - b.top || a.left - b.left);
  for (const rect of sorted) {
    const last = lines[lines.length - 1];
    const sameLine =
      last !== undefined &&
      Math.min(last.bottom, rect.bottom) - Math.max(last.top, rect.top) > Math.min(last.height, rect.height) * 0.5 &&
      rect.left <= last.right + 2 &&
      rect.right >= last.left - 2;
    if (sameLine) {
      const left = Math.min(last.left, rect.left);
      const top = Math.min(last.top, rect.top);
      lines[lines.length - 1] = new DOMRect(left, top, Math.max(last.right, rect.right) - left, Math.max(last.bottom, rect.bottom) - top);
    } else {
      lines.push(rect);
    }
  }
  return lines;
}

export class FlowAnnotations {
  private marks: FlowMark[] = [];
  private readonly overlays = new Map<Document, Overlay>();
  private readonly docs = new Map<Document, number>();
  private hadSelection = false;
  private drag: { doc: Document; index: number } | null = null;
  private reportTimer = 0;

  constructor(private readonly hooks: Hooks) {}

  /** An SVG layer over a section's frame, with what foliate-js's paginator calls on it. */
  overlay(doc: Document, index: number): { element: SVGSVGElement; redraw(): void } {
    const element = document.createElementNS(SVG, 'svg');
    element.setAttribute('aria-hidden', 'true');
    element.dataset.musaeumMarks = String(index);
    Object.assign(element.style, {
      position: 'absolute',
      left: '0',
      top: '0',
      width: '100%',
      height: '100%',
      overflow: 'visible',
      pointerEvents: 'none',
      zIndex: '1',
    });
    const overlay: Overlay = { doc, index, element, drawn: new Map(), observer: null };
    this.overlays.set(doc, overlay);
    return { element, redraw: () => this.draw(overlay) };
  }

  /** In continuous reading: a layer over the part's frame in its slot, redrawn as it reflows. */
  overlayIn(slot: HTMLElement, doc: Document, index: number): void {
    const { element } = this.overlay(doc, index);
    slot.append(element);
    const overlay = this.overlays.get(doc);
    if (overlay !== undefined) {
      overlay.observer = new ResizeObserver(() => this.draw(overlay));
      overlay.observer.observe(doc.body ?? doc.documentElement);
      this.draw(overlay);
    }
  }

  /** A section's document went away. */
  release(doc: Document): void {
    const overlay = this.overlays.get(doc);
    overlay?.observer?.disconnect();
    overlay?.element.remove();
    this.overlays.delete(doc);
    this.docs.delete(doc);
  }

  setMarks(marks: FlowMark[]): void {
    this.marks = marks;
    this.redrawAll();
  }

  redrawAll(): void {
    for (const [doc, overlay] of this.overlays) {
      if (doc.defaultView === null) {
        this.release(doc);
      } else {
        this.draw(overlay);
      }
    }
  }

  /** The DOM range of one stretch of a mark inside one section, if it reaches it. */
  private rangeIn(doc: Document, index: number, start: string, end: string): Range | null {
    let first: number;
    let last: number;
    try {
      first = this.hooks.sectionOf(start);
      last = this.hooks.sectionOf(end);
    } catch {
      return null;
    }
    if (index < first || index > last) {
      return null;
    }
    const body = doc.body ?? doc.documentElement;
    if (body === null) {
      return null;
    }
    const range = doc.createRange();
    try {
      if (index === first) {
        const at = rangeOf(doc, start);
        if (at === null) {
          return null;
        }
        range.setStart(at.startContainer, at.startOffset);
      } else {
        range.setStart(body, 0);
      }
      if (index === last) {
        const at = rangeOf(doc, end);
        if (at === null) {
          return null;
        }
        range.setEnd(at.startContainer, at.startOffset);
      } else {
        range.setEnd(body, body.childNodes.length);
      }
    } catch {
      return null;
    }
    return range.collapsed ? null : range;
  }

  private draw(overlay: Overlay): void {
    const { doc, index, element } = overlay;
    element.replaceChildren();
    overlay.drawn.clear();
    if (doc.defaultView === null) {
      return;
    }
    const dark = this.hooks.dark();
    for (const mark of this.marks) {
      const rects: DOMRect[] = [];
      for (const { start, end } of mark.ranges) {
        const range = this.rangeIn(doc, index, start, end);
        if (range !== null) {
          rects.push(...merged([...range.getClientRects()]));
        }
      }
      if (rects.length === 0) {
        continue;
      }
      overlay.drawn.set(mark.id, rects);
      const group = document.createElementNS(SVG, 'g');
      group.dataset.mark = mark.id;
      const fill = FILL[mark.color] ?? FILL.yellow;
      const ink = INK[mark.color] ?? INK.yellow;
      for (const rect of rects) {
        const shape = document.createElementNS(SVG, 'rect');
        if (mark.kind === 'underline') {
          const thickness = Math.max(1.5, rect.height * 0.08);
          shape.setAttribute('x', String(rect.left));
          shape.setAttribute('y', String(rect.bottom - thickness));
          shape.setAttribute('width', String(rect.width));
          shape.setAttribute('height', String(thickness));
          shape.setAttribute('fill', ink);
        } else {
          shape.setAttribute('x', String(rect.left));
          shape.setAttribute('y', String(rect.top));
          shape.setAttribute('width', String(rect.width));
          shape.setAttribute('height', String(rect.height));
          shape.setAttribute('fill', fill);
          // Over light paper, a highlighter: the paper takes the color and the ink stays dark.
          shape.style.mixBlendMode = dark ? 'normal' : 'multiply';
          shape.setAttribute('fill-opacity', dark ? '0.35' : '1');
        }
        group.append(shape);
      }
      if (mark.note) {
        const first = rects[0];
        const size = Math.max(5, first.height * 0.3);
        const tab = document.createElementNS(SVG, 'rect');
        tab.setAttribute('x', String(first.left - size * 0.6));
        tab.setAttribute('y', String(first.top - size * 0.6));
        tab.setAttribute('width', String(size));
        tab.setAttribute('height', String(size));
        tab.setAttribute('rx', String(size * 0.2));
        tab.setAttribute('fill', ink);
        group.append(tab);
      }
      element.append(group);
    }
  }

  /** The mark under a point of a section's document, in its own coordinates. */
  hit(doc: Document, x: number, y: number): { id: string; rect: ReaderRect } | null {
    const overlay = this.overlays.get(doc);
    if (overlay === undefined) {
      return null;
    }
    for (const [id, rects] of overlay.drawn) {
      if (rects.some((rect) => x >= rect.left && x <= rect.right && y >= rect.top - 2 && y <= rect.bottom + 2)) {
        const frame = doc.defaultView?.frameElement?.getBoundingClientRect();
        const first = rects[0];
        return { id, rect: rectOf(first, frame?.left ?? 0, frame?.top ?? 0) };
      }
    }
    return null;
  }

  /** Listens to what the reader does in a section's document. */
  wire(doc: Document, index: number): void {
    this.docs.set(doc, index);
    const schedule = (): void => {
      clearTimeout(this.reportTimer);
      this.reportTimer = window.setTimeout(() => this.report(), 0);
    };
    doc.addEventListener('mousedown', (event) => {
      if (event.button === 0) {
        // A new press starts a new selection: the other frames' parts of the old one go.
        for (const other of this.docs.keys()) {
          if (other !== doc) {
            other.getSelection()?.removeAllRanges();
          }
        }
        this.drag = { doc, index };
      }
    });
    doc.addEventListener('mousemove', (event) => {
      if (this.drag?.doc === doc && (event.buttons & 1) !== 0) {
        this.extend(doc, index, event.clientX, event.clientY);
      }
    });
    doc.addEventListener('mouseup', () => {
      this.drag = null;
      schedule();
    });
    doc.addEventListener('keyup', (event) => {
      if (event.shiftKey || event.key === 'Shift') {
        schedule();
      }
    });
  }

  /** The documents loaded, with their frames' boxes in the reader's page, top to bottom. */
  private frames(): { doc: Document; index: number; box: DOMRect }[] {
    const frames: { doc: Document; index: number; box: DOMRect }[] = [];
    for (const [doc, index] of this.docs) {
      const frame = doc.defaultView?.frameElement;
      if (frame === null || frame === undefined) {
        continue;
      }
      frames.push({ doc, index, box: frame.getBoundingClientRect() });
    }
    return frames.sort((a, b) => a.index - b.index);
  }

  /** A drag that left its frame: the frames it crossed are selected by hand, up to the pointer. */
  private extend(origin: Document, originIndex: number, x: number, y: number): void {
    const frames = this.frames();
    const home = frames.find((frame) => frame.doc === origin);
    if (home === undefined || frames.length < 2) {
      return;
    }
    const pageX = home.box.left + x;
    const pageY = home.box.top + y;
    // Near the top or bottom of the screen, the reading scrolls along.
    if (pageY > window.innerHeight - 24) {
      this.hooks.scrollBy(24);
    } else if (pageY < 24) {
      this.hooks.scrollBy(-24);
    }
    const target = frames.find((frame) => pageY >= frame.box.top && pageY < frame.box.bottom);
    for (const frame of frames) {
      if (frame.doc !== origin) {
        frame.doc.getSelection()?.removeAllRanges();
      }
    }
    if (target === undefined || target.doc === origin) {
      return;
    }
    const downward = target.index > originIndex;
    const caret = caretAt(target.doc, pageX - target.box.left, pageY - target.box.top);
    for (const frame of frames) {
      if (frame.doc === origin) {
        continue;
      }
      const between = downward
        ? frame.index > originIndex && frame.index < target.index
        : frame.index < originIndex && frame.index > target.index;
      const body = frame.doc.body ?? frame.doc.documentElement;
      if (body === null) {
        continue;
      }
      const range = frame.doc.createRange();
      if (between) {
        range.selectNodeContents(body);
      } else if (frame.doc === target.doc && caret !== null) {
        if (downward) {
          range.setStart(body, 0);
          range.setEnd(caret.startContainer, caret.startOffset);
        } else {
          range.setStart(caret.startContainer, caret.startOffset);
          range.setEnd(body, body.childNodes.length);
        }
      } else {
        continue;
      }
      const selection = frame.doc.getSelection();
      selection?.removeAllRanges();
      selection?.addRange(range);
    }
  }

  /** Tells the main page what is selected now, or that nothing is. */
  report(): void {
    const parts: { doc: Document; index: number; range: Range; text: string }[] = [];
    for (const [doc, index] of this.docs) {
      if (doc.defaultView === null) {
        this.docs.delete(doc);
        continue;
      }
      const selection = doc.getSelection();
      if (selection === null || selection.isCollapsed || selection.rangeCount === 0) {
        continue;
      }
      parts.push({ doc, index, range: selection.getRangeAt(0), text: selection.toString() });
    }
    parts.sort((a, b) => a.index - b.index);
    if (parts.length === 0 || parts.every((part) => part.text.trim() === '')) {
      if (this.hadSelection) {
        this.hadSelection = false;
        this.hooks.send({ type: 'selection', start: null, end: null, text: '', rect: null });
      }
      return;
    }
    const first = parts[0];
    const last = parts[parts.length - 1];
    const end = last.range.cloneRange();
    end.collapse(false);
    const frame = first.doc.defaultView?.frameElement?.getBoundingClientRect();
    const line = [...first.range.getClientRects()].find((rect) => rect.width > 0 && rect.height > 0) ?? first.range.getBoundingClientRect();
    this.hadSelection = true;
    this.hooks.send({
      type: 'selection',
      start: cfiOf(this.hooks.sectionCfi(first.index), first.range),
      end: cfiOf(this.hooks.sectionCfi(last.index), end),
      text: parts
        .map((part) => part.text)
        .join('\n')
        .slice(0, MAX_TEXT),
      rect: rectOf(line, frame?.left ?? 0, frame?.top ?? 0),
    });
  }

  clearSelection(): void {
    for (const doc of this.docs.keys()) {
      doc.getSelection()?.removeAllRanges();
    }
    this.hadSelection = false;
  }

  /** For tests: selects a stretch by the reader's CFIs, as a drag over it would. */
  select(start: string, end: string): boolean {
    let done = false;
    for (const [doc, index] of this.docs) {
      const range = this.rangeIn(doc, index, start, end);
      const selection = doc.getSelection();
      selection?.removeAllRanges();
      if (range !== null) {
        selection?.addRange(range);
        done = true;
      }
    }
    this.report();
    return done;
  }

  /** For tests: how many marks are drawn, in how many rectangles, and over how many frames each. */
  inspect(): { marks: number; drawn: number; rects: number; frames: Record<string, number>; drift: number } {
    const frames: Record<string, number> = {};
    let rects = 0;
    let drift = 0;
    for (const overlay of this.overlays.values()) {
      if (overlay.doc.defaultView === null) {
        continue;
      }
      for (const [id, drawn] of overlay.drawn) {
        frames[id] = (frames[id] ?? 0) + 1;
        rects += drawn.length;
        // What the text's rectangles are now, against what was drawn: a layer not redrawn after a
        // new layout is off by however far the text moved.
        const mark = this.marks.find((candidate) => candidate.id === id);
        const fresh: DOMRect[] = [];
        for (const { start, end } of mark?.ranges ?? []) {
          const range = this.rangeIn(overlay.doc, overlay.index, start, end);
          if (range !== null) {
            fresh.push(...merged([...range.getClientRects()]));
          }
        }
        if (fresh.length !== drawn.length) {
          drift = Math.max(drift, 9999);
          continue;
        }
        fresh.forEach((rect, at) => {
          const old = drawn[at];
          drift = Math.max(drift, Math.abs(rect.left - old.left), Math.abs(rect.top - old.top), Math.abs(rect.width - old.width));
        });
      }
    }
    return { marks: this.marks.length, drawn: Object.keys(frames).length, rects, frames, drift };
  }

  /** For tests: brings a text into view and says where a drag over it would start and end. */
  private probeNeedle(needle: string): { start: { x: number; y: number }; end: { x: number; y: number }; text: string; sections: number[] } | null {
    for (const frame of this.frames()) {
      const body = frame.doc.body ?? frame.doc.documentElement;
      if (body === null) {
        continue;
      }
      const walk = frame.doc.createTreeWalker(body, NodeFilter.SHOW_TEXT);
      for (let node = walk.nextNode(); node !== null; node = walk.nextNode()) {
        const text = node as Text;
        const at = text.data.indexOf(needle);
        if (at < 0) {
          continue;
        }
        text.parentElement?.scrollIntoView({ block: 'center' });
        const range = frame.doc.createRange();
        const box = (offset: number): DOMRect => {
          range.setStart(text, offset);
          range.setEnd(text, offset + 1);
          return [...range.getClientRects()][0] ?? range.getBoundingClientRect();
        };
        const frameBox = frame.doc.defaultView?.frameElement?.getBoundingClientRect();
        const first = box(at);
        const last = box(at + needle.length - 1);
        return {
          start: { x: (frameBox?.left ?? 0) + first.left + 1, y: (frameBox?.top ?? 0) + first.top + first.height / 2 },
          end: { x: (frameBox?.left ?? 0) + last.right - 1, y: (frameBox?.top ?? 0) + last.top + last.height / 2 },
          text: needle,
          sections: [frame.index],
        };
      }
    }
    return null;
  }

  /**
   * For tests: where a person would press and let go to select some text on the screen, in this
   * page's coordinates. `seam` asks for a drag from the end of one frame into the next one.
   */
  probe(
    seam: boolean,
    needle: string | null = null,
  ): { start: { x: number; y: number }; end: { x: number; y: number }; text: string; sections: number[]; from?: string; boxes?: unknown[] } | null {
    if (needle !== null) {
      return this.probeNeedle(needle);
    }
    const frames = this.frames().filter((frame) => frame.box.bottom > 0 && frame.box.top < window.innerHeight);
    const visibleText = (doc: Document, fromEnd: boolean, least = seam ? 12 : 30): Text | null => {
      const body = doc.body ?? doc.documentElement;
      if (body === null) {
        return null;
      }
      const walk = doc.createTreeWalker(body, NodeFilter.SHOW_TEXT);
      const texts: Text[] = [];
      for (let node = walk.nextNode(); node !== null; node = walk.nextNode()) {
        const text = node as Text;
        if (text.data.trim().length >= least) {
          texts.push(text);
        }
      }
      const frame = doc.defaultView?.frameElement?.getBoundingClientRect();
      const onScreen = (text: Text): boolean => {
        // A press on a link drags the link, as in any browser, and text only for screen readers is
        // not on the screen: neither is where a person starts a selection.
        if (text.parentElement?.closest('a, button, [role="button"]') !== null) {
          return false;
        }
        const range = doc.createRange();
        range.selectNodeContents(text);
        const box = range.getBoundingClientRect();
        const top = box.top + (frame?.top ?? 0);
        const left = box.left + (frame?.left ?? 0);
        return (
          box.width >= 20 &&
          box.height >= 6 &&
          top > 40 &&
          top + box.height < window.innerHeight - 40 &&
          left >= 0 &&
          left + box.width <= window.innerWidth
        );
      };
      const ordered = fromEnd ? texts.reverse() : texts;
      return ordered.find(onScreen) ?? null;
    };
    const point = (text: Text, offset: number, after: boolean): { x: number; y: number } => {
      const doc = text.ownerDocument;
      const range = doc.createRange();
      range.setStart(text, offset);
      range.setEnd(text, offset + 1);
      const box = [...range.getClientRects()][0] ?? range.getBoundingClientRect();
      const frame = doc.defaultView?.frameElement?.getBoundingClientRect();
      return {
        x: (frame?.left ?? 0) + (after ? box.right - 1 : box.left + 1),
        y: (frame?.top ?? 0) + box.top + box.height / 2,
      };
    };
    const word = (text: Text, from: number): number => {
      let at = from;
      while (at > 0 && /\S/.test(text.data[at - 1])) {
        at--;
      }
      return at;
    };
    if (!seam) {
      for (const frame of frames) {
        const text = visibleText(frame.doc, false);
        if (text !== null) {
          const start = word(text, Math.min(4, Math.floor(text.length / 4)));
          const end = Math.min(text.length - 1, start + 40);
          return {
            start: point(text, start, false),
            end: point(text, end, true),
            text: text.data.slice(start, end + 1),
            sections: [frame.index],
          };
        }
      }
      return null;
    }
    for (let at = 0; at + 1 < frames.length; at++) {
      const above = visibleText(frames[at].doc, true);
      const below = visibleText(frames[at + 1].doc, false);
      if (above !== null && below !== null) {
        const start = word(above, Math.max(0, above.length - 30));
        const end = Math.min(below.length - 1, 20);
        return {
          start: point(above, start, false),
          end: point(below, end, true),
          text: `${above.data.slice(start)}…${below.data.slice(0, end + 1)}`,
          sections: [frames[at].index, frames[at + 1].index],
          from: `${above.parentElement?.localName ?? ''} ${above.parentElement?.className ?? ''}`.trim(),
          boxes: [frames[at].box.toJSON(), frames[at + 1].box.toJSON()],
        };
      }
    }
    return null;
  }
}

/** The caret at a point of a document, in its own coordinates. */
function caretAt(doc: Document, x: number, y: number): Range | null {
  const legacy = doc as Document & { caretRangeFromPoint?(x: number, y: number): Range | null };
  if (typeof legacy.caretRangeFromPoint === 'function') {
    return legacy.caretRangeFromPoint(x, y);
  }
  const position = doc.caretPositionFromPoint?.(x, y);
  if (position === null || position === undefined) {
    return null;
  }
  const range = doc.createRange();
  range.setStart(position.offsetNode, position.offset);
  return range;
}
