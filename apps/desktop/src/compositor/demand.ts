/**
 * Builds the wish list for the host, in priority order, and sends it without ever making a frame
 * wait. At most one request is in flight; if the list changes meanwhile, only the newest version
 * gets sent, since the host would discard the ones in between anyway.
 */

import {
  DEMAND_ENTRY_WORDS,
  DEMAND_HEADER_WORDS,
  DEMAND_MAGIC,
  MAX_DEMAND_TILES,
} from '../protocol';
import type { TileList } from './frame-planner';
import type { TileCache } from './tile-cache';
import { pageScaleKey, placeholderScale, tileIndex } from './tiles';

/**
 * Above this speed, in pages per second, sharp tiles would be stale before they arrived. Only
 * whole-page placeholders are requested until the reader slows down.
 */
export const FAST_PAGES_PER_SECOND = 3;
/** Seconds of travel to prepare placeholders for. */
const LOOKAHEAD_SECONDS = 0.5;
const MIN_LOOKAHEAD_PAGES = 2;
const MAX_LOOKAHEAD_PAGES = 48;

/** Encodes a demand list directly into `words`, returning the number of tiles written. */
export class DemandEncoder {
  readonly words = new Uint32Array(DEMAND_HEADER_WORDS + MAX_DEMAND_TILES * DEMAND_ENTRY_WORDS);
  private tiles = 0;
  /** Each page and scale on the list gets a small number, so a tile's key stays an exact number. */
  private readonly groups = new Map<number, number>();
  private readonly seen = new Set<number>();

  reset(): void {
    this.tiles = 0;
    this.groups.clear();
    this.seen.clear();
  }

  add(page: number, scale: number, x: number, y: number): void {
    if (this.tiles >= MAX_DEMAND_TILES) {
      return;
    }
    const group = pageScaleKey(page, scale);
    let number = this.groups.get(group);
    if (number === undefined) {
      number = this.groups.size;
      this.groups.set(group, number);
    }
    const key = number * 1_048_576 + tileIndex(x, y);
    if (this.seen.has(key)) {
      return;
    }
    this.seen.add(key);
    const at = DEMAND_HEADER_WORDS + this.tiles * DEMAND_ENTRY_WORDS;
    this.words[at] = page;
    this.words[at + 1] = scale;
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
  widths: Float32Array;
  heights: Float32Array;
  cache: TileCache;
  /** Placeholders the current frame lacks. */
  missingPlaceholders: TileList;
  /**
   * Tiles at the scale where the reader is heading, nearest the centre first; empty while that is
   * not known yet, as in the middle of a zoom.
   */
  wanted: TileList;
  /** Whole screens on either side, at the scale they will be shown at. */
  neighbours: TileList;
  firstVisiblePage: number;
  lastVisiblePage: number;
  /** Signed speed through the document; positive is forward. */
  pagesPerSecond: number;
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

function addAll(encoder: DemandEncoder, list: TileList, sorted: boolean): void {
  const order = sorted ? sortedIndices(list, indexScratch) : null;
  for (let n = 0; n < list.length; n++) {
    const at = order === null ? n : order[n];
    encoder.add(list.pages[at], list.scales[at], list.columns[at], list.rows[at]);
  }
}

/**
 * Fills `encoder` with the wish list, most important first:
 *
 *   1. placeholders for pages on screen: quick to render, and they cover the most area;
 *   2. sharp tiles for where the reader is heading, centre outwards, unless moving too fast;
 *   3. placeholders along the direction of travel, and a couple behind;
 *   4. the screens on either side in full, when not moving fast.
 */
export function buildDemand(encoder: DemandEncoder, inputs: DemandInputs): void {
  const { widths, heights, cache, pagesPerSecond } = inputs;
  encoder.reset();
  addAll(encoder, inputs.missingPlaceholders, false);

  const fast = Math.abs(pagesPerSecond) > FAST_PAGES_PER_SECOND;
  if (!fast) {
    addAll(encoder, inputs.wanted, true);
  }

  const pageCount = widths.length;
  if (pageCount === 0 || inputs.lastVisiblePage < 0) {
    return;
  }
  const direction = pagesPerSecond >= 0 ? 1 : -1;
  const travelPages = Math.ceil(Math.abs(pagesPerSecond) * LOOKAHEAD_SECONDS);
  const ahead = Math.min(MAX_LOOKAHEAD_PAGES, MIN_LOOKAHEAD_PAGES + travelPages);
  const edge = direction > 0 ? inputs.lastVisiblePage : inputs.firstVisiblePage;
  const addPlaceholder = (page: number): void => {
    if (page >= 0 && page < pageCount) {
      const scale = placeholderScale(widths[page], heights[page]);
      if (cache.find(page, scale, 0, 0) < 0) {
        encoder.add(page, scale, 0, 0);
      }
    }
  };
  for (let step = 1; step <= ahead; step++) {
    addPlaceholder(edge + direction * step);
  }
  // A couple behind as well, because readers often turn back.
  const behind = direction > 0 ? inputs.firstVisiblePage : inputs.lastVisiblePage;
  for (let step = 1; step <= MIN_LOOKAHEAD_PAGES; step++) {
    addPlaceholder(behind - direction * step);
  }

  if (!fast) {
    addAll(encoder, inputs.neighbours, true);
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
