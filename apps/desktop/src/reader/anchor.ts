/**
 * Positions inside one section's document: the first character a reader sees, where a position is
 * on the screen, and the CFI that names it.
 *
 * The anchor of a view is a collapsed range before one character. It is what stays put when the
 * text is laid out again: the view puts that character's line back where it was, and the Locator
 * kept in the library is the anchor's, so nothing drifts however many times the text reflows.
 */

import * as CFI from '../../../../vendor/foliate-js/epubcfi.js';

/** Elements that take space but have no text, and can be where a reader is. */
const VISUAL = new Set(['img', 'svg', 'video', 'image', 'math', 'hr']);

function isHidden(element: Element): boolean {
  if (element.hasAttribute('data-musaeum-removed') || element.hasAttribute('hidden')) {
    return true;
  }
  const view = element.ownerDocument.defaultView;
  return view !== null && view.getComputedStyle(element).display === 'none';
}

/** Text and pictures under `root`, in order, skipping what is hidden and blank text. */
function walker(doc: Document, root: Node): TreeWalker {
  return doc.createTreeWalker(root, NodeFilter.SHOW_TEXT | NodeFilter.SHOW_ELEMENT, {
    acceptNode(node) {
      if (node.nodeType === Node.ELEMENT_NODE) {
        const element = node as Element;
        if (isHidden(element)) {
          return NodeFilter.FILTER_REJECT;
        }
        return VISUAL.has(element.localName) ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_SKIP;
      }
      return /\S/.test((node as Text).data) ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_SKIP;
    },
  });
}

/** The box of one character, or of an element, in the document's own coordinates. */
export function boxOf(range: Range): DOMRect | null {
  const node = range.startContainer;
  if (node.nodeType === Node.TEXT_NODE) {
    const text = node as Text;
    const doc = text.ownerDocument;
    const single = doc.createRange();
    // The character after the anchor; at the very end of a text node, the one before.
    const at = Math.min(range.startOffset, Math.max(0, text.length - 1));
    single.setStart(text, at);
    single.setEnd(text, Math.min(text.length, at + 1));
    const rects = Array.from(single.getClientRects()).filter((rect) => rect.height > 0);
    if (rects.length > 0) {
      return rects[0];
    }
    const parent = text.parentElement;
    return parent?.getBoundingClientRect() ?? null;
  }
  const element =
    node.nodeType === Node.ELEMENT_NODE
      ? ((node as Element).childNodes[range.startOffset] as Element | undefined) ?? (node as Element)
      : node.parentElement;
  if (element === null || element === undefined) {
    return null;
  }
  const target = element.nodeType === Node.ELEMENT_NODE ? element : element.parentElement;
  return target?.getBoundingClientRect() ?? null;
}

/** The height of a line of text at a position, from the styles that apply there. */
export function lineHeightAt(range: Range): number {
  const node = range.startContainer;
  const element = node.nodeType === Node.ELEMENT_NODE ? (node as Element) : node.parentElement;
  const view = element?.ownerDocument.defaultView;
  if (element === null || element === undefined || view === null || view === undefined) {
    return 0;
  }
  const style = view.getComputedStyle(element);
  const fontSize = parseFloat(style.fontSize) || 16;
  const lineHeight = parseFloat(style.lineHeight);
  return Number.isFinite(lineHeight) ? lineHeight : fontSize * 1.2;
}

/**
 * The first character whose line reaches below `y`, in the document's coordinates: the first one
 * a reader sees when the view's top edge is at `y`. A line cut in half by the edge counts, since
 * part of it shows.
 */
export function firstVisible(doc: Document, y: number): Range | null {
  const body = doc.body ?? doc.documentElement;
  if (body === null) {
    return null;
  }
  const walk = walker(doc, body);
  const probe = doc.createRange();
  let last: Node | null = null;
  for (let node = walk.nextNode(); node !== null; node = walk.nextNode()) {
    last = node;
    if (node.nodeType === Node.ELEMENT_NODE) {
      const box = (node as Element).getBoundingClientRect();
      if (box.height > 0 && box.bottom > y) {
        const range = doc.createRange();
        range.setStartBefore(node);
        range.collapse(true);
        return range;
      }
      continue;
    }
    const text = node as Text;
    probe.selectNodeContents(text);
    const rects = probe.getClientRects();
    if (rects.length === 0 || rects[rects.length - 1].bottom <= y) {
      continue;
    }
    // The first character on a line that reaches below y. Lines go down as the text goes on.
    let low = 0;
    let high = text.length - 1;
    while (low < high) {
      const middle = (low + high) >> 1;
      probe.setStart(text, middle);
      probe.setEnd(text, middle + 1);
      const boxes = probe.getClientRects();
      const bottom = boxes.length > 0 ? boxes[boxes.length - 1].bottom : -Infinity;
      if (bottom > y) {
        high = middle;
      } else {
        low = middle + 1;
      }
    }
    // Skip the spaces a line starts with, which belong to no line the reader sees.
    while (low < text.length - 1 && /\s/.test(text.data[low])) {
      low++;
    }
    const range = doc.createRange();
    range.setStart(text, low);
    range.collapse(true);
    return range;
  }
  if (last !== null) {
    const range = doc.createRange();
    range.setStartAfter(last);
    range.collapse(true);
    return range;
  }
  return null;
}

/**
 * The first character at or after a position that is not white space: where the eye lands. The
 * space that ends a line belongs to that line, so a page that starts right after one would
 * otherwise be anchored on the page before.
 */
export function skipSpaces(range: Range): Range {
  const doc = range.startContainer.ownerDocument;
  const body = doc?.body ?? doc?.documentElement ?? null;
  if (doc === null || body === null || range.startContainer.nodeType !== Node.TEXT_NODE) {
    return range;
  }
  const walk = walker(doc, body);
  walk.currentNode = range.startContainer;
  let node: Node | null = range.startContainer;
  let offset = range.startOffset;
  // A few nodes at most: past that, the position was not at a line's end.
  for (let step = 0; node !== null && step < 16; step++) {
    if (node.nodeType === Node.TEXT_NODE) {
      const text = node as Text;
      while (offset < text.length && /\s/.test(text.data[offset])) {
        offset++;
      }
      if (offset < text.length) {
        const moved = doc.createRange();
        moved.setStart(text, offset);
        moved.collapse(true);
        return moved;
      }
    } else {
      const moved = doc.createRange();
      moved.setStartBefore(node);
      moved.collapse(true);
      return moved;
    }
    node = walk.nextNode();
    offset = 0;
  }
  return range;
}

/** The start of the document's text: where a section is entered from the top. */
export function startOf(doc: Document): Range | null {
  const body = doc.body ?? doc.documentElement;
  if (body === null) {
    return null;
  }
  const first = walker(doc, body).nextNode();
  const range = doc.createRange();
  if (first === null) {
    range.setStart(body, 0);
  } else if (first.nodeType === Node.TEXT_NODE) {
    const text = first as Text;
    const at = text.data.search(/\S/);
    range.setStart(text, Math.max(0, at));
  } else {
    range.setStartBefore(first);
  }
  range.collapse(true);
  return range;
}

/** The reader's CFI of a position in the section whose own CFI is `base`. */
export function cfiOf(base: string, range: Range): string {
  const collapsed = range.cloneRange();
  collapsed.collapse(true);
  return CFI.joinIndir(base, CFI.fromRange(collapsed));
}

/** The position a reader's CFI names in a section's document. */
export function rangeOf(doc: Document, cfi: string): Range | null {
  try {
    const parts = CFI.parse(cfi) as unknown as { parent?: unknown[] } & unknown[];
    const path = parts.parent ?? parts;
    // The first part is the package step to the section, which this document is already.
    const inner = (Array.isArray(path) ? path.slice(1) : []) as unknown[];
    if (inner.length === 0) {
      return startOf(doc);
    }
    const range = CFI.toRange(doc, inner);
    range.collapse(true);
    return range;
  } catch {
    return null;
  }
}

/** How much text comes before a position in the document's body, in UTF-16 code units. */
export function textOffset(doc: Document, range: Range): number {
  const body = doc.body ?? doc.documentElement;
  if (body === null) {
    return 0;
  }
  const before = doc.createRange();
  before.setStart(body, 0);
  try {
    before.setEnd(range.startContainer, range.startOffset);
  } catch {
    return 0;
  }
  return before.toString().length;
}
