/** Messages between the main thread and the compositor worker. Control only, never pixels. */

import type { BenchOptions, SessionInfo } from '../protocol';
import type { SabRingBuffers } from '../sab-ring';

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
  zoom: number;
  /** The whole screen, in device pixels: the most the canvas can ever have to show. */
  screenWidth: number;
  screenHeight: number;
  /** Overrides the tile cache's size, in tiles, to measure what it costs. */
  gpuLayers: number | null;
}

export type ProbeRequest =
  | { type: 'probe'; step: 'take'; label: string; buffer: ArrayBuffer; simulateCopy: boolean }
  | { type: 'probe'; step: 'read'; label: string }
  | { type: 'probe'; step: 'write'; label: string; value: number }
  | { type: 'probe'; step: 'forget'; label: string };

export type ToWorker =
  | InitMessage
  | ProbeRequest
  | { type: 'resize'; width: number; height: number }
  | { type: 'wheel'; deltaPx: number };

export interface ProbeReply {
  type: 'probe-reply';
  label: string;
  byteLength: number;
  firstWord: number;
  hostWord: number;
  workerHasWebviewApi: boolean;
}

export type FromWorker =
  | ProbeReply
  | { type: 'ready'; gpu: string; pageCount: number; clockRttMs: number }
  | { type: 'hud'; text: string }
  /** The first frame with nothing soft or missing, as wall-clock time in milliseconds. */
  | { type: 'first-sharp'; atEpochMs: number }
  | { type: 'bench-done'; report: Record<string, unknown> }
  | { type: 'fatal'; message: string };
