/**
 * A critically damped spring: the fastest motion to a target that never overshoots it. It is solved
 * exactly rather than integrated, so a long frame cannot make it unstable and the same steps always
 * give the same positions, which is what lets the tests run it on a clock of their own.
 *
 * It can be interrupted at any moment: retargeting keeps the position and the velocity it had, so a
 * page that is sliding changes course without a jerk, and a finger that catches it takes over from
 * exactly where it was.
 */

/** Below these, the spring counts as at rest and snaps to its target. */
const REST_DISTANCE = 0.5;
const REST_SPEED = 5;

export class Spring {
  position: number;
  velocity = 0;
  target: number;

  /** `stiffness` is the natural frequency, in radians per second: higher settles faster. */
  constructor(
    position: number,
    readonly stiffness = 20,
  ) {
    this.position = position;
    this.target = position;
  }

  get atRest(): boolean {
    return this.position === this.target && this.velocity === 0;
  }

  /** Jumps straight there, as a page turn with no animation does. */
  snap(position: number): void {
    this.position = position;
    this.target = position;
    this.velocity = 0;
  }

  /** Advances by `dtMs` milliseconds. */
  step(dtMs: number): void {
    if (this.atRest || dtMs <= 0) {
      return;
    }
    const t = dtMs / 1000;
    const omega = this.stiffness;
    const c1 = this.position - this.target;
    const c2 = this.velocity + omega * c1;
    const decay = Math.exp(-omega * t);
    this.position = this.target + (c1 + c2 * t) * decay;
    this.velocity = (c2 - omega * (c1 + c2 * t)) * decay;
    const close = Math.abs(this.position - this.target) < REST_DISTANCE;
    if (close && Math.abs(this.velocity) < REST_SPEED) {
      this.snap(this.target);
    }
  }
}
