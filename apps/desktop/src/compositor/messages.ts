/** Messages between the main thread and the compositor worker. Control only, never pixels. */

import type { BenchOptions, SessionInfo } from '../protocol';
import type { AnnotatorMessage, FixedMark, PageSpan, RegionSelection, Tool } from './annotator';
import type { SabRingBuffers } from '../sab-ring';
import type { ModeKind, ReadingSnapshot } from './reader';
import type { SpreadsSetting } from './spreads';

export interface InitMessage {
  type: 'init';
  canvas: OffscreenCanvas;
  width: number;
  height: number;
  session: SessionInfo;
  /** Path B's ring. Null on the other paths. */
  ring: SabRingBuffers | null;
  /** Path C's WebView2 buffers, transferred with this message. Null on the other paths. */
  direct: { slab: ArrayBuffer; ring: ArrayBuffer } | null;
  bench: BenchOptions | null;
  devicePixelRatio: number;
  /** Device pixels per point that continuous reading starts at, before the window limits it. */
  comfortableScale: number;
  prefersReducedMotion: boolean;
  /** The whole screen, in device pixels: the most the canvas can ever have to show. */
  screenWidth: number;
  screenHeight: number;
  /** The GPU tile cache's budget, in bytes. Null for one sized to the screen. */
  tileBudgetBytes: number | null;
}

export type ProbeRequest =
  | { type: 'probe'; step: 'take'; label: string; buffer: ArrayBuffer; simulateCopy: boolean }
  | { type: 'probe'; step: 'read'; label: string }
  | { type: 'probe'; step: 'write'; label: string; value: number }
  | { type: 'probe'; step: 'forget'; label: string };

export interface WheelMessage {
  type: 'wheel';
  deltaX: number;
  deltaY: number;
  deltaMode: number;
  wheelDeltaY?: number;
  ctrlKey: boolean;
  /** Pointer position over the canvas, in device pixels. */
  x: number;
  y: number;
  timeMs: number;
}

export type ToWorker =
  | InitMessage
  | ProbeRequest
  | { type: 'resize'; width: number; height: number }
  | WheelMessage
  | { type: 'key'; key: string; ctrl: boolean; shift: boolean }
  | { type: 'pointer'; phase: 'down' | 'move' | 'up'; x: number; y: number }
  | { type: 'set-mode'; mode: ModeKind }
  | { type: 'set-animation'; on: boolean }
  | { type: 'set-spreads'; spreads: SpreadsSetting }
  | { type: 'turn'; step: 1 | -1 }
  /** Shows a page, as an entry of the table of contents asks. */
  | { type: 'go-to-page'; page: number }
  /** Shows a point of a page as stored (before its rotation), as a mark in the inspector asks. */
  | { type: 'go-to-point'; page: number; u: number; v: number }
  | { type: 'zoom'; factor: number }
  /** Asks for an `Inspection`, for a test driving the real window. */
  | { type: 'inspect' }
  /** Sends the position now if it changed since the last one sent, then `flushed`. */
  | { type: 'flush' }
  /** A page's characters, as the host sent them, or null for a page without any. */
  | { type: 'chars'; page: number; buffer: ArrayBuffer | null }
  /** Every mark to draw, replacing the ones before. */
  | { type: 'marks'; marks: FixedMark[] }
  | { type: 'clear-selection' }
  /** Where the pointer is, with no button down. */
  | { type: 'hover'; x: number; y: number }
  /** What a press on a page does from now on. */
  | { type: 'set-tool'; tool: Tool }
  /** Whether a drag along lines snaps to words and follows the lines' curves. */
  | { type: 'set-snap'; on: boolean }
  /**
   * A page's line model as the host sent it (the page hash, then `MLIN`), or null when the host
   * has none to send. `retry` says the host has none yet and may later: asking again is allowed.
   */
  | { type: 'lines'; page: number; buffer: ArrayBuffer | null; retry: boolean };

export interface ProbeReply {
  type: 'probe-reply';
  label: string;
  byteLength: number;
  firstWord: number;
  hostWord: number;
  workerHasWebviewApi: boolean;
}

/** What the reading bar shows. */
export interface ReaderStatus {
  type: 'status';
  mode: ModeKind;
  /** Whether the page turn is on right now: never in continuous mode. */
  animating: boolean;
  animationPreference: boolean;
  spreads: SpreadsSetting;
  zoom: number;
  page: number;
  pageCount: number;
}

/** A page as it is on the screen this frame. */
export interface ShownPage {
  page: number;
  /** Its top-left corner, in whole device pixels of the canvas. */
  x: number;
  y: number;
  /** The scale its tiles are drawn at, in units of 1/SCALE_ONE pixel per point. */
  scale: number;
  widthPt: number;
  heightPt: number;
}

/** Everything a test of the real window checks, exactly as the reader has it. */
export interface Inspection {
  type: 'inspection';
  snapshot: ReadingSnapshot;
  animating: boolean;
  /** Whether a spring, a zoom or a scroll is still under way. */
  moving: boolean;
  /** Whether every visible page is drawn from tiles at the scale it is shown at. */
  sharp: boolean;
  /**
   * Whether the compositor still has a frame to draw. Until it has drawn it, `pages` and `sharp`
   * describe the frame before: a page turned without animation is not on the screen yet.
   */
  framePending: boolean;
  viewWidth: number;
  viewHeight: number;
  pages: ShownPage[];
  /** What is selected, and how many marks there are. */
  selection: PageSpan[];
  marks: number;
  /** The pages whose characters the compositor has. */
  charPages: number[];
  tool: Tool;
  snap: boolean;
  /** The pages whose line models the compositor has, with how many lines each. */
  linePages: { page: number; lines: number }[];
  /** The region drawn and not yet an annotation. */
  region: RegionSelection | null;
}

export type FromWorker =
  | AnnotatorMessage
  | ProbeReply
  | Inspection
  | { type: 'ready'; gpu: string; pageCount: number; clockRttMs: number }
  | { type: 'hud'; text: string }
  /** The first frame with nothing soft or missing, as wall-clock time in milliseconds. */
  | { type: 'first-sharp'; atEpochMs: number }
  /** Where the reader is, to be saved. Sent a moment after it stops changing. */
  | { type: 'reading-state'; state: ReadingSnapshot }
  | ReaderStatus
  | { type: 'bench-done'; report: Record<string, unknown> }
  /** Every position up to now has been sent. */
  | { type: 'flushed' }
  | { type: 'fatal'; message: string };
