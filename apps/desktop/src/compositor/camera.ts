/** Weight of the newest sample in the smoothed scroll speed. */
const VELOCITY_BLEND = 0.25;

/**
 * Updates the smoothed scroll speed, in points per second, which decides what to ask the
 * host for. Smoothing keeps one uneven frame from flipping that decision, but two events
 * must take effect at once:
 *
 * - **Standing still.** The reader wants sharp tiles now, not once the average decays,
 *   which would take over 100 ms after stopping from a fast scroll.
 * - **A jump.** Landing somewhere new from the scrollbar is not a speed. Treating it as
 *   one would make the planner think the reader is still flying through the document.
 */
export function nextVelocity(
  velocityPt: number,
  movedPt: number,
  frameIntervalMs: number,
  viewHeightPt: number,
): number {
  if (frameIntervalMs <= 0) {
    return velocityPt;
  }
  if (movedPt === 0 || Math.abs(movedPt) > viewHeightPt) {
    return 0;
  }
  const instant = (movedPt / frameIntervalMs) * 1000;
  return velocityPt * (1 - VELOCITY_BLEND) + instant * VELOCITY_BLEND;
}
