/**
 * Tells a mouse wheel from a trackpad, so each gets the motion it needs and never both.
 *
 * A trackpad already moves smoothly, and Windows adds its own momentum when the fingers lift.
 * Adding more smoothing on top would be inertia twice over, so its deltas are applied as they come.
 * A mouse wheel moves in notches, which would jump, so the reader smooths those itself.
 *
 * The browser does not say which device sent a wheel event, so this goes by what the events look
 * like, as Chromium on Windows produces them: notches arrive as whole multiples of a line or of a
 * large step and only ever vertical; a precision touchpad sends small, often fractional deltas,
 * and diagonal ones. A gesture keeps the class its first events gave it, so one ambiguous event in
 * the middle cannot switch it, unless the trackpad's own signs show up.
 */

export type WheelSource = 'mouse' | 'trackpad';

export interface WheelInput {
  deltaX: number;
  deltaY: number;
  /** 0 for pixels, 1 for lines, 2 for pages, as in `WheelEvent`. */
  deltaMode: number;
  /** Milliseconds, on any clock that only moves forward. */
  timeMs: number;
  /** Chromium's legacy `wheelDeltaY`, which is a multiple of 120 for a notch, when there is one. */
  wheelDeltaY?: number;
}

/** A pause longer than this ends a gesture. */
const GESTURE_GAP_MS = 250;
/** The smallest notch Chromium reports, in CSS pixels. */
const NOTCH_MIN_PX = 50;

function looksLikeTrackpad(input: WheelInput): boolean {
  return input.deltaX !== 0 || !Number.isInteger(input.deltaY) || Math.abs(input.deltaY) < 4;
}

function looksLikeNotch(input: WheelInput): boolean {
  if (input.deltaMode !== 0) {
    return true;
  }
  if (input.wheelDeltaY !== undefined && input.wheelDeltaY !== 0 && input.wheelDeltaY % 120 === 0) {
    return input.deltaX === 0 && Math.abs(input.deltaY) >= NOTCH_MIN_PX;
  }
  const whole = Number.isInteger(input.deltaY);
  return input.deltaX === 0 && whole && Math.abs(input.deltaY) >= NOTCH_MIN_PX;
}

export class WheelClassifier {
  private source: WheelSource = 'mouse';
  private lastTimeMs = Number.NEGATIVE_INFINITY;

  classify(input: WheelInput): WheelSource {
    const continuing = input.timeMs - this.lastTimeMs < GESTURE_GAP_MS;
    this.lastTimeMs = input.timeMs;
    if (!continuing) {
      this.source = looksLikeNotch(input) ? 'mouse' : 'trackpad';
    } else if (this.source === 'mouse' && input.deltaMode === 0 && looksLikeTrackpad(input)) {
      this.source = 'trackpad';
    }
    return this.source;
  }
}
