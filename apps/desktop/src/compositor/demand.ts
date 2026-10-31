/**
 * Builds the wish list for the host, in priority order, and sends it without ever making
 * a frame wait. At most one request is in flight; if the list changes meanwhile, only the
 * newest version gets sent, since the host would discard the ones in between anyway.
 */

import {
  DEMAND_ENTRY_WORDS,
  DEMAND_HEADER_WORDS,
  DEMAND_MAGIC,
  MAX_DEMAND_TILES,
} from '../protocol';
import type { TileList } from './frame-planner';
import { type DocumentLayout, tileCount, tileKey, wholePageLod } from './layout';
import type { TileCache } from './tile-cache';

/**
 * Above this speed, in viewports per second, sharp tiles would be stale before they
 * arrived. Only whole-page placeholders are requested until the scroll slows down.
 */
export const FAST_SCROLL_VIEWPORTS_PER_SECOND = 2.5;
/** Seconds of travel to prepare placeholders for. */
const LOOKAHEAD_SECONDS = 0.5;
const MIN_LOOKAHEAD_PAGES = 2;
const MAX_LOOKAHEAD_PAGES = 48;
/** Pages next to the screen prepared in full while the reader is not moving fast. */
const NEARBY_SHARP_PAGES = 1;

/** Encodes a demand list directly into `words`, returning the number of tiles written. */
export class DemandEncoder {
  readonly words = new Uint32Array(DEMAND_HEADER_WORDS + MAX_DEMAND_TILES * DEMAND_ENTRY_WORDS);
  private tiles = 0;
  private readonly seen = new Set<number>();

  reset(): void {
    this.tiles = 0;
    this.seen.clear();
  }

  add(page: number, lod: number, x: number, y: number): void {
    if (this.tiles >= MAX_DEMAND_TILES) {
      return;
    }
    const key = tileKey(page, lod, x, y);
    if (this.seen.has(key)) {
      return;
    }
    this.seen.add(key);
    const at = DEMAND_HEADER_WORDS + this.tiles * DEMAND_ENTRY_WORDS;
    this.words[at] = page;
    // Stored as the two's complement word that Rust reads back as an i32.
    this.words[at + 1] = lod >>> 0;
    this.words[at + 2] = x;
    this.words[at + 3] = y;
    this.tiles++;
  }

  get tileCount(): number {
    return this.tiles;
  }

  /** The bytes to send, as a fresh copy, because `fetch` holds on to them. */
  finish(epoch: number): Uint8Array<ArrayBuffer> {
    this.words[0] = DEMAND_MAGIC;
    this.words[1] = epoch;
    this.words[2] = this.tiles;
    this.words[3] = 0;
    const words = DEMAND_HEADER_WORDS + this.tiles * DEMAND_ENTRY_WORDS;
    return new Uint8Array(this.words.slice(0, words).buffer);
  }

  /** A cheap fingerprint of the current list, to skip resending an unchanged one. */
  fingerprint(): number {
    let hash = 2166136261;
    const end = DEMAND_HEADER_WORDS + this.tiles * DEMAND_ENTRY_WORDS;
    for (let at = DEMAND_HEADER_WORDS; at < end; at++) {
      hash = Math.imul(hash ^ this.words[at], 16777619);
    }
    return hash >>> 0;
  }
}

export interface DemandInputs {
  layout: DocumentLayout;
  cache: TileCache;
  missingPlaceholders: TileList;
  missingSharp: TileList;
  firstVisiblePage: number;
  lastVisiblePage: number;
  targetLod: number;
  /** Signed scroll speed in points per second; positive is down the document. */
  velocityPt: number;
  /** Viewport height in points. */
  viewHeightPt: number;
}

/** Sorts a tile list's indices by priority, smallest first. */
function sortedIndices(list: TileList, scratch: number[]): number[] {
  scratch.length = 0;
  for (let at = 0; at < list.length; at++) {
    scratch.push(at);
  }
  scratch.sort((a, b) => list.priorities[a] - list.priorities[b]);
  return scratch;
}

const indexScratch: number[] = [];

/**
 * Fills `encoder` with the wish list, most important first:
 *
 *   1. placeholders for pages on screen: quick to render, and they cover the most area;
 *   2. sharp tiles for the screen, centre outwards, unless scrolling too fast to matter;
 *   3. placeholders along the direction of travel;
 *   4. sharp tiles for the neighbouring pages, when not moving fast.
 */
export function buildDemand(encoder: DemandEncoder, inputs: DemandInputs): void {
  const { layout, cache, velocityPt, viewHeightPt, targetLod } = inputs;
  encoder.reset();

  const { missingPlaceholders } = inputs;
  for (let at = 0; at < missingPlaceholders.length; at++) {
    encoder.add(
      missingPlaceholders.pages[at],
      missingPlaceholders.lods[at],
      missingPlaceholders.columns[at],
      missingPlaceholders.rows[at],
    );
  }

  const speedViewports = Math.abs(velocityPt) / Math.max(viewHeightPt, 1);
  const fast = speedViewports > FAST_SCROLL_VIEWPORTS_PER_SECOND;
  if (!fast) {
    const { missingSharp } = inputs;
    for (const at of sortedIndices(missingSharp, indexScratch)) {
      encoder.add(
        missingSharp.pages[at],
        missingSharp.lods[at],
        missingSharp.columns[at],
        missingSharp.rows[at],
      );
    }
  }

  if (layout.pageCount === 0 || inputs.lastVisiblePage < 0) {
    return;
  }

  const direction = velocityPt >= 0 ? 1 : -1;
  const averageHeight = layout.totalHeight / layout.pageCount;
  const travelPages = Math.ceil((Math.abs(velocityPt) * LOOKAHEAD_SECONDS) / averageHeight);
  const ahead = Math.min(MAX_LOOKAHEAD_PAGES, MIN_LOOKAHEAD_PAGES + travelPages);
  const edge = direction > 0 ? inputs.lastVisiblePage : inputs.firstVisiblePage;
  for (let step = 1; step <= ahead; step++) {
    addPlaceholder(encoder, layout, cache, edge + direction * step, targetLod);
  }
  // A couple behind as well, because readers often turn back.
  const behind = direction > 0 ? inputs.firstVisiblePage : inputs.lastVisiblePage;
  for (let step = 1; step <= MIN_LOOKAHEAD_PAGES; step++) {
    addPlaceholder(encoder, layout, cache, behind - direction * step, targetLod);
  }

  if (!fast) {
    for (let step = 1; step <= NEARBY_SHARP_PAGES; step++) {
      addSharpPage(encoder, layout, cache, inputs.lastVisiblePage + step, targetLod);
      addSharpPage(encoder, layout, cache, inputs.firstVisiblePage - step, targetLod);
    }
  }
}

function addPlaceholder(
  encoder: DemandEncoder,
  layout: DocumentLayout,
  cache: TileCache,
  page: number,
  targetLod: number,
): void {
  if (page < 0 || page >= layout.pageCount) {
    return;
  }
  const lod = Math.min(wholePageLod(layout.widths[page], layout.heights[page]), targetLod);
  if (cache.find(tileKey(page, lod, 0, 0)) < 0) {
    encoder.add(page, lod, 0, 0);
  }
}

function addSharpPage(
  encoder: DemandEncoder,
  layout: DocumentLayout,
  cache: TileCache,
  page: number,
  lod: number,
): void {
  if (page < 0 || page >= layout.pageCount) {
    return;
  }
  const columns = tileCount(layout.widths[page], lod);
  const rows = tileCount(layout.heights[page], lod);
  for (let y = 0; y < rows; y++) {
    for (let x = 0; x < columns; x++) {
      if (cache.find(tileKey(page, lod, x, y)) < 0) {
        encoder.add(page, lod, x, y);
      }
    }
  }
}

/** Sends demand lists, one at a time, keeping only the newest while one is in flight. */
export class DemandSender {
  private inFlight = false;
  private pending: Uint8Array<ArrayBuffer> | null = null;
  sent = 0;
  failures = 0;

  constructor(private readonly url: string) {}

  submit(body: Uint8Array<ArrayBuffer>): void {
    if (this.inFlight) {
      this.pending = body;
    } else {
      void this.send(body);
    }
  }

  private async send(body: Uint8Array<ArrayBuffer>): Promise<void> {
    this.inFlight = true;
    this.sent++;
    try {
      await fetch(this.url, {
        method: 'POST',
        body,
        cache: 'no-store',
        credentials: 'omit',
      });
    } catch (error) {
      // The next list replaces this one anyway, so a lost list only delays a tile.
      this.failures++;
      console.warn('sending a demand list failed', error);
    }
    this.inFlight = false;
    const next = this.pending;
    if (next !== null) {
      this.pending = null;
      void this.send(next);
    }
  }
}
