/**
 * Binary layouts shared with the Rust side. These mirror `crates/musaeum-shm` and
 * `crates/musaeum-core`; golden tests on both sides pin them to the same bytes.
 */

export const TILE_SIZE = 512;
export const TILE_BYTES = TILE_SIZE * TILE_SIZE * 4;
/** One device pixel per point, in the units of a tile's scale. */
export const SCALE_ONE = 4096;
/** Coarsest scale: 1/64 pixel per point. */
export const MIN_SCALE = SCALE_ONE / 64;
/** Finest scale: 32 pixels per point. */
export const MAX_SCALE = SCALE_ONE * 32;
/** Longest page edge a tile grid may have, in pixels: 1024 tiles. */
export const MAX_PAGE_PIXELS = 1 << 19;

/** Reads a four-character tag as the little-endian word Rust stores. */
export function fourCc(tag: string): number {
  return (
    (tag.charCodeAt(0) |
      (tag.charCodeAt(1) << 8) |
      (tag.charCodeAt(2) << 16) |
      (tag.charCodeAt(3) << 24)) >>>
    0
  );
}

export const SLAB_MAGIC = fourCc('MSLB');
export const SLAB_SLOT_COUNT_WORD = 2;
export const SLAB_PIXELS_OFFSET = 4096;

export const RING_MAGIC = fourCc('MDRG');
export const RING_CAPACITY_WORD = 2;
export const RING_HEAD_WORD = 0x40 / 4;
export const RING_CONSUMED_WORD = 0x80 / 4;
export const RING_ENTRIES_WORD = 0xc0 / 4;

export const PROBE_MAGIC = fourCc('MPRB');
export const BATCH_MAGIC = fourCc('MTLB');
export const BATCH_HEADER_BYTES = 8;
export const GEOMETRY_MAGIC = fourCc('MGEO');
export const DEMAND_MAGIC = fourCc('MDMD');
export const DEMAND_HEADER_WORDS = 4;
export const DEMAND_ENTRY_WORDS = 4;
/** Most tiles the host accepts in one demand list. */
export const MAX_DEMAND_TILES = 4096;

/** A finished tile, as eight words in the ring, in Path A batches and in the SAB ring. */
export const ANNOUNCEMENT_WORDS = 8;
export const A_SLOT = 0;
export const A_PAGE = 1;
export const A_SCALE = 2;
export const A_X = 3;
export const A_Y = 4;
export const A_EPOCH = 5;
export const A_RENDER_US = 6;
export const A_DONE_US = 7;

/**
 * How finished tiles reach the compositor.
 *
 * - `a`: the compositor worker pulls tiles from a custom protocol with `fetch`.
 * - `b`: the main thread copies tiles from a WebView2 shared buffer into a ring of
 *   `SharedArrayBuffer`s and wakes the worker through `Atomics`.
 * - `c`: the WebView2 buffers are transferred to the worker, which reads tiles in place.
 */
export type DeliveryPath = 'a' | 'b' | 'c';

/** What the command line asked for. `auto` tests the runtime and takes the best path. */
export type DeliveryRequest = 'auto' | DeliveryPath;

/** A failure to simulate at startup, to test the automatic selection end to end. */
export type DeliveryFault =
  | 'copied-transfer'
  | 'rejected-transfer'
  | 'no-shared-buffers'
  | 'not-isolated';

/** Word of a probe buffer that the host writes and a script reads. */
export const PROBE_HOST_WORD = 1;
/** Word of a probe buffer that a script writes and the host reads. */
export const PROBE_SCRIPT_WORD = 2;

/** On Windows, Tauri serves a custom scheme from this origin. */
export function tileBaseUrl(sessionId: number): string {
  return `http://tile.localhost/${sessionId}`;
}

/** Where the reader was, as `reading_state` keeps it. */
export interface SavedReadingState {
  page: number;
  u: number;
  v: number;
  progress: number;
  mode: 'paginated' | 'continuous';
  pageTurnAnimation: boolean;
  zoom: number;
  spreads: 'auto' | 'single' | 'double';
}

export interface SessionInfo {
  id: number;
  /** Which worker reads the file, told from its bytes. */
  format: 'pdf' | 'comic' | 'djvu';
  delivery: DeliveryPath;
  pageCount: number;
  orientation: string;
  slotCount: number;
  sandboxed: boolean;
  openUs: number;
  geometryUs: number;
  /** The catalog's `/PageLayout`, if it has one. */
  pageLayout: string | null;
  /** `L2R` or `R2L`, if the file says. */
  direction: string | null;
  /** Whether the page sizes came from the cache instead of a worker. */
  geometryCached: boolean;
  /** The library's document, if the file is in the library. */
  documentId: number | null;
  readingState: SavedReadingState | null;
  /** Pages left to classify and to draw thumbnails of, in the background, when it opened. */
  background: { toClassify: number; toDraw: number; toTrace: number };
}

export interface BenchOptions {
  out: string;
  scenario: 'full' | 'quick' | 'smoke' | 'zoom' | 'memory' | 'layout' | 'security';
  mainStressMs: number;
  uncapped: boolean;
  zoom: number;
}

export interface StartupOptions {
  /** A fixed-layout document: PDF, CBZ, CBR or DjVu. */
  document: string | null;
  /** Whether a test drives this window from outside, and the reader's state is exposed for it. */
  e2e: boolean;
  epub: string | null;
  bookProtection: 'full' | 'csp-only' | 'sandbox-only' | 'none';
  delivery: DeliveryRequest;
  deliveryFaults: DeliveryFault[];
  workers: number;
  pullBatch: number;
  gpuLayers: number | null;
  /** The GPU tile cache's budget, in mebibytes. */
  tileBudgetMb: number | null;
  noSandbox: boolean;
  width: number;
  height: number;
  bench: BenchOptions | null;
}
