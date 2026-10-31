/**
 * Measurement storage that does not disturb what it measures: fixed-size buffers, no
 * allocation while recording, and all sorting left for after the run.
 */

export interface Summary {
  count: number;
  mean: number;
  p50: number;
  p95: number;
  p99: number;
  max: number;
}

export class Samples {
  private readonly values: Float32Array;
  private written = 0;

  constructor(capacity: number) {
    this.values = new Float32Array(capacity);
  }

  add(value: number): void {
    if (this.written < this.values.length) {
      this.values[this.written++] = value;
    }
  }

  get count(): number {
    return this.written;
  }

  countAbove(limit: number): number {
    let above = 0;
    for (let at = 0; at < this.written; at++) {
      if (this.values[at] > limit) {
        above++;
      }
    }
    return above;
  }

  summary(): Summary {
    return summarize(this.values.subarray(0, this.written));
  }

  reset(): void {
    this.written = 0;
  }
}

/** Summarizes with nearest-rank percentiles, the same definition the Rust side uses. */
export function summarize(values: Float32Array): Summary {
  if (values.length === 0) {
    return { count: 0, mean: 0, p50: 0, p95: 0, p99: 0, max: 0 };
  }
  const sorted = values.slice().sort();
  const rank = (percent: number): number =>
    sorted[Math.max(0, Math.ceil((percent * sorted.length) / 100) - 1)];
  let total = 0;
  for (let at = 0; at < sorted.length; at++) {
    total += sorted[at];
  }
  return {
    count: sorted.length,
    mean: total / sorted.length,
    p50: rank(50),
    p95: rank(95),
    p99: rank(99),
    max: sorted[sorted.length - 1],
  };
}

/** Everything measured while one part of a scroll scenario runs. */
export class SegmentRecorder {
  /** Time between animation frames, in milliseconds. This is what the reader sees. */
  readonly frameInterval = new Samples(1 << 16);
  /** Time the compositor spent working per frame. Must fit the budget to hold 120 Hz. */
  readonly frameWork = new Samples(1 << 16);
  /** Time spent uploading tiles per frame. */
  readonly uploadWork = new Samples(1 << 16);
  /** Copying one tile to the GPU. */
  readonly tileUpload = new Samples(1 << 16);
  /** From the worker finishing a tile to the compositor having it, in milliseconds. */
  readonly tileDelivery = new Samples(1 << 16);
  /** From the scroll stopping to a completely sharp screen, in milliseconds. */
  readonly timeToSharp = new Samples(1 << 10);

  frames = 0;
  /** Frames where a visible page had no plate. Must stay at zero. */
  blankFrames = 0;
  /** Frames where part of the screen showed only a plate. */
  plateFrames = 0;
  /** Frames where part of the screen used a coarser tile. */
  softFrames = 0;
  /** Frames that were completely sharp. */
  sharpFrames = 0;
  tilesUploaded = 0;
  /** Tiles that arrived when already resident, so the render was wasted. */
  duplicateTiles = 0;
  /** Tiles dropped because every GPU layer was in use by the current frame. */
  droppedTiles = 0;
  /** Rests that ended before the screen became sharp, so `timeToSharp` never saw them. */
  sharpAbandoned = 0;
  visibleTiles = 0;
  exactTiles = 0;

  constructor(readonly name: string) {}

  report(refreshIntervalMs: number): Record<string, unknown> {
    const interval = this.frameInterval.summary();
    return {
      name: this.name,
      frames: this.frames,
      frameIntervalMs: interval,
      achievedFps: interval.mean > 0 ? 1000 / interval.mean : 0,
      frameWorkMs: this.frameWork.summary(),
      uploadWorkMs: this.uploadWork.summary(),
      tileUploadMs: this.tileUpload.summary(),
      tileDeliveryMs: this.tileDelivery.summary(),
      timeToSharpMs: this.timeToSharp.summary(),
      sharpAbandoned: this.sharpAbandoned,
      /** Frames that took more than 1.5 × the display's own interval. */
      missedFrames: this.frameInterval.countAbove(refreshIntervalMs * 1.5),
      /** Frame work over the budget of a 120 Hz display. */
      workOver120HzBudget: this.frameWork.countAbove(1000 / 120),
      blankFrames: this.blankFrames,
      plateFrames: this.plateFrames,
      softFrames: this.softFrames,
      sharpFrames: this.sharpFrames,
      tilesUploaded: this.tilesUploaded,
      duplicateTiles: this.duplicateTiles,
      droppedTiles: this.droppedTiles,
      exactCoverage: this.visibleTiles > 0 ? this.exactTiles / this.visibleTiles : 0,
    };
  }
}
