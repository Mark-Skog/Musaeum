/**
 * Scripted scrolling for the benchmark. Deterministic, with its own seeded generator, so
 * two runs ask the pipeline for exactly the same thing and only the delivery path differs.
 *
 * Speeds are in viewport heights per second, so the scenario means the same on any window.
 */

export interface SegmentContext {
  /** Lowest scroll position, in points. */
  minScrollPt: number;
  /** Highest scroll position, in points. */
  maxScrollPt: number;
  /** Viewport height, in points. */
  viewHeightPt: number;
}

export interface Segment {
  name: string;
  durationMs: number;
  /** Scroll position at `elapsedMs` into the segment, given where the segment started. */
  position(elapsedMs: number, startPt: number, context: SegmentContext): number;
  /** Whether the scroll is standing still, so the screen ought to be getting sharp. */
  isResting(elapsedMs: number): boolean;
}

export type ScenarioName = 'full' | 'quick' | 'smoke';

function clamp(value: number, context: SegmentContext): number {
  return Math.min(context.maxScrollPt, Math.max(context.minScrollPt, value));
}

function hold(name: string, durationMs: number): Segment {
  return {
    name,
    durationMs,
    position: (_elapsed, start, context) => clamp(start, context),
    isResting: () => true,
  };
}

/** A hand flicking through a book: fast at first, slowing down, then pausing to read. */
function flicks(name: string, count: number, peakSpeed: number): Segment {
  const flickMs = 1200;
  const restMs = 500;
  const periodMs = flickMs + restMs;
  const decayPerMs = 3 / flickMs;
  // How far one whole flick travels, in viewports: the integral of the speed curve.
  const distance = (ms: number): number =>
    ((peakSpeed / 1000) * (1 - Math.exp(-decayPerMs * ms))) / decayPerMs;
  return {
    name,
    durationMs: count * periodMs,
    position(elapsed, start, context) {
      const whole = Math.floor(elapsed / periodMs);
      const within = elapsed - whole * periodMs;
      const viewports = whole * distance(flickMs) + distance(Math.min(within, flickMs));
      return clamp(start + viewports * context.viewHeightPt, context);
    },
    isResting: (elapsed) => elapsed % periodMs >= flickMs,
  };
}

/** Holding the scrollbar's arrow, or an absurdly hard fling: constant and relentless. */
function constantSpeed(name: string, durationMs: number, speed: number): Segment {
  return {
    name,
    durationMs,
    position: (elapsed, start, context) =>
      clamp(start + (speed * elapsed * context.viewHeightPt) / 1000, context),
    isResting: () => false,
  };
}

/** Changing direction at full speed, which invalidates every prediction. */
function reversals(name: string, durationMs: number, peakSpeed: number, periodMs: number): Segment {
  const amplitude = (peakSpeed * periodMs) / (2 * Math.PI * 1000);
  return {
    name,
    durationMs,
    position: (elapsed, start, context) =>
      clamp(
        start + amplitude * context.viewHeightPt * Math.sin((2 * Math.PI * elapsed) / periodMs),
        context,
      ),
    isResting: () => false,
  };
}

/** Dragging the scrollbar: jumps to places never seen before, pausing at each one. */
function teleports(name: string, count: number, periodMs: number, seed: number): Segment {
  const targets = new Float64Array(count);
  let state = seed | 1;
  for (let at = 0; at < count; at++) {
    state ^= state << 13;
    state ^= state >>> 17;
    state ^= state << 5;
    targets[at] = (state >>> 0) / 4294967296;
  }
  return {
    name,
    durationMs: count * periodMs,
    position(elapsed, _start, context) {
      const index = Math.min(count - 1, Math.floor(elapsed / periodMs));
      const range = context.maxScrollPt - context.minScrollPt;
      return context.minScrollPt + targets[index] * range;
    },
    // Every moment after the first frame of a jump counts as standing still.
    isResting: () => true,
  };
}

export function buildScenario(name: ScenarioName): Segment[] {
  if (name === 'smoke') {
    // Enough to show that tiles flow, for tests that check a configuration rather than speed.
    return [
      hold('open', 800),
      constantSpeed('scroll', 700, 6),
      hold('settle', 500),
    ];
  }
  const scale = name === 'quick' ? 0.4 : 1;
  const ms = (value: number): number => Math.round(value * scale);
  return [
    hold('open', 1500),
    flicks('natural-flicks', name === 'quick' ? 3 : 8, 6),
    hold('settle-1', ms(1000)),
    constantSpeed('extreme-30vh', ms(8000), 30),
    hold('settle-2', ms(1000)),
    reversals('reversals-20vh', ms(8000), 20, 2000),
    hold('settle-3', ms(1000)),
    teleports('teleports', name === 'quick' ? 8 : 20, 400, 0x5eed),
    hold('settle-4', 1000),
  ];
}

/** Runs through a scenario's segments as frames go by. */
export class ScenarioPlayer {
  private index = 0;
  private segmentStartMs = -1;
  private segmentStartPt = 0;

  constructor(private readonly segments: Segment[]) {}

  get finished(): boolean {
    return this.index >= this.segments.length;
  }

  get current(): Segment | undefined {
    return this.segments[this.index];
  }

  /** Index of the current segment. */
  get currentIndex(): number {
    return this.index;
  }

  /**
   * Advances to `nowMs` and returns the scroll position, moving on to the next segment
   * when the current one has run out. Returns null once the scenario has finished.
   */
  sample(nowMs: number, scrollPt: number, context: SegmentContext): number | null {
    for (;;) {
      const segment = this.segments[this.index];
      if (segment === undefined) {
        return null;
      }
      if (this.segmentStartMs < 0) {
        this.segmentStartMs = nowMs;
        this.segmentStartPt = scrollPt;
      }
      const elapsed = nowMs - this.segmentStartMs;
      if (elapsed < segment.durationMs) {
        return segment.position(elapsed, this.segmentStartPt, context);
      }
      this.index++;
      this.segmentStartMs = -1;
    }
  }

  /** Whether the scroll is at rest at this moment. */
  isResting(nowMs: number): boolean {
    const segment = this.segments[this.index];
    return segment !== undefined && this.segmentStartMs >= 0
      ? segment.isResting(nowMs - this.segmentStartMs)
      : true;
  }
}
