/** Messages between the main thread and the compositor worker. Control only, never pixels. */

import type { BenchOptions, SessionInfo } from '../protocol';
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
  | { type: 'zoom'; factor: number };

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

export type FromWorker =
  | ProbeReply
  | { type: 'ready'; gpu: string; pageCount: number; clockRttMs: number }
  | { type: 'hud'; text: string }
  /** The first frame with nothing soft or missing, as wall-clock time in milliseconds. */
  | { type: 'first-sharp'; atEpochMs: number }
  /** Where the reader is, to be saved. Sent a moment after it stops changing. */
  | { type: 'reading-state'; state: ReadingSnapshot }
  | ReaderStatus
  | { type: 'bench-done'; report: Record<string, unknown> }
  | { type: 'fatal'; message: string };
