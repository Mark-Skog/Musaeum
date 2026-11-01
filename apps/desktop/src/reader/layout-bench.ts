/**
 * Measures how long the fluid engine takes to lay a section out again after a change. The
 * target is 100 ms: this cannot fit in one frame, so the product will crossfade over it.
 *
 * Two numbers come out of each change. `layoutMs` is the browser computing the new layout,
 * forced synchronously, which is the part that depends only on the engine and the machine. It
 * only exists for a font change: a new width reaches the section through the engine's own
 * resize handling, which cannot be forced. `visibleMs` is what the reader waits for: until a
 * frame has been shown with the final layout, after foliate-js recounts its columns and
 * restores the reading position. It is measured in whole frames, so it is exact to about one
 * interval of the display.
 */

import { Samples, type Summary } from '../bench/metrics';
import type { FluidEngine, FluidFlow, Typography } from './foliate-adapter';

export type LayoutChange = 'font' | 'box' | 'window';

export interface LayoutMeasurement {
  section: string;
  sectionBytes: number;
  flow: FluidFlow;
  change: LayoutChange;
  /** Only for a font change. */
  layoutMs: Summary | null;
  visibleMs: Summary;
  /** Changes after which the text the reader started from was still on screen. */
  positionKept: number;
  iterations: number;
}

export interface LayoutSection {
  index: number;
  name: string;
  bytes: number;
}

/** Sizes worth seeing, from an ordinary chapter to a whole novel in one file. */
const WANTED_SIZES_BYTES = [20 << 10, 200 << 10, 400 << 10, 1 << 20, 3 << 20];
/** How far from a wanted size a section may be and still stand in for it. */
const SIZE_TOLERANCE = 0.3;

function describeSize(bytes: number): string {
  return bytes >= 1 << 20
    ? `${(bytes / (1 << 20)).toFixed(1)} MiB`
    : `${Math.round(bytes / 1024)} KiB`;
}

/**
 * Picks which sections of a book to measure. foliate-js lays out one section at a time, so what
 * matters is a section's size: this covers the wanted sizes the book has, plus its median and
 * its largest section, which are what a reader of that particular book would meet.
 */
export function pickSections(sizes: readonly number[]): LayoutSection[] {
  const indexed = sizes
    .map((bytes, index) => ({ index, bytes }))
    .filter((section) => section.bytes > 0)
    .sort((a, b) => a.bytes - b.bytes);
  if (indexed.length === 0) {
    return [];
  }
  const picked = new Map<number, string>();
  const pick = (section: { index: number; bytes: number }, label: string): void => {
    if (!picked.has(section.index)) {
      picked.set(section.index, label);
    }
  };
  for (const wanted of WANTED_SIZES_BYTES) {
    const nearest = indexed.reduce((best, section) =>
      Math.abs(section.bytes - wanted) < Math.abs(best.bytes - wanted) ? section : best,
    );
    if (Math.abs(nearest.bytes - wanted) <= wanted * SIZE_TOLERANCE) {
      pick(nearest, describeSize(nearest.bytes));
    }
  }
  const median = indexed[Math.floor(indexed.length / 2)];
  pick(median, `${describeSize(median.bytes)} (median)`);
  const largest = indexed[indexed.length - 1];
  pick(largest, `${describeSize(largest.bytes)} (largest)`);

  return [...picked]
    .map(([index, name]) => ({ index, name, bytes: sizes[index] }))
    .sort((a, b) => a.bytes - b.bytes);
}

export interface BenchHost {
  engine: FluidEngine;
  typography: Typography;
  resizeWindow(width: number, height: number): Promise<void>;
}

/** Sizes to cycle through, so each change really changes something. */
const FONT_SIZES_PX = [16, 18, 20, 22, 24, 22, 20, 18];
/** All below the engine's 720 px text column, or a width change would not reflow any text. */
const BOX_WIDTHS_PX = [700, 620, 540, 660, 580];
const WINDOW_SIZES: [number, number][] = [
  [1400, 860],
  [1200, 800],
  [1000, 760],
  [1300, 840],
  [1100, 780],
];
const HOME_WINDOW_SIZE: [number, number] = [1600, 900];
/** Middle of the section, so that keeping the position means something. */
const READING_POSITION = 0.5;
/** Frames with an unchanged layout before it counts as finished. */
const STABLE_FRAMES = 3;
const ITERATIONS = 20;
/** A section this long takes seconds per iteration, and varies little. */
const LONG_SECTION_BYTES = 512 << 10;
const ITERATIONS_LONG_SECTION = 10;
/**
 * The pause between changes, longer than what foliate-js waits before it takes a scroll as the
 * reader's own and moves its anchor there. Anything faster moves the reading position in a way
 * nobody clicking a button would ever see.
 */
const BETWEEN_CHANGES_MS = 300;
const CHANGE_TIMEOUT_MS = 2000;
const SETTLE_TIMEOUT_MS = 8000;

function nextFrame(): Promise<number> {
  return new Promise((resolve) => requestAnimationFrame(resolve));
}

function pause(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * Waits until the layout stops changing, and returns the moment of the first frame that showed
 * its final state. So the frames spent confirming that it stopped do not count.
 *
 * With `mustChange`, it first waits for the layout to move away from `before`: a window resize
 * reaches the page some time after it was asked for.
 */
async function settled(
  engine: FluidEngine,
  startedAt: number,
  before: string,
  mustChange: boolean,
): Promise<number> {
  if (mustChange) {
    while (engine.layoutFingerprint() === before) {
      const now = await nextFrame();
      if (now - startedAt > CHANGE_TIMEOUT_MS) {
        throw new Error(`the layout did not react within ${CHANGE_TIMEOUT_MS} ms: ${before}`);
      }
    }
  }
  // A frame later, whatever was changed before this frame has been drawn.
  await nextFrame();
  let shownAt = await nextFrame();
  let fingerprint = engine.layoutFingerprint();
  let unchanged = 0;
  while (unchanged < STABLE_FRAMES) {
    const now = await nextFrame();
    const current = engine.layoutFingerprint();
    if (current === fingerprint) {
      unchanged++;
    } else {
      fingerprint = current;
      shownAt = now;
      unchanged = 0;
    }
    if (now - startedAt > SETTLE_TIMEOUT_MS) {
      throw new Error(`the layout did not settle within ${SETTLE_TIMEOUT_MS} ms`);
    }
  }
  return shownAt;
}

async function measureChange(
  host: BenchHost,
  section: LayoutSection,
  flow: FluidFlow,
  change: LayoutChange,
  iterations: number,
): Promise<LayoutMeasurement> {
  const { engine } = host;
  const layout = new Samples(iterations);
  const visible = new Samples(iterations);
  let positionKept = 0;
  // What the engine anchors on stays the same through all of this. Comparing against whatever
  // the previous change left on screen would say nothing: a smaller font fits text from before
  // the anchor on the same page, and that text rightly leaves again when the font grows.
  const anchor = engine.readingPosition()?.startContainer ?? null;

  for (let iteration = 0; iteration < iterations; iteration++) {
    const doc = engine.currentDocument();
    if (doc === null) {
      throw new Error(`section ${section.name} has no document`);
    }
    const before = engine.layoutFingerprint();

    const started = performance.now();
    switch (change) {
      case 'font':
        engine.setTypography({
          ...host.typography,
          fontSizePx: FONT_SIZES_PX[iteration % FONT_SIZES_PX.length],
        });
        break;
      case 'box':
        engine.setBoxWidth(BOX_WIDTHS_PX[iteration % BOX_WIDTHS_PX.length]);
        break;
      case 'window': {
        const [width, height] = WINDOW_SIZES[iteration % WINDOW_SIZES.length];
        await host.resizeWindow(width, height);
        break;
      }
    }
    if (change === 'font') {
      // Forces the style and layout pass that the next frame would otherwise do.
      doc.documentElement.getBoundingClientRect();
      layout.add(performance.now() - started);
    }

    try {
      const shownAt = await settled(engine, started, before, change !== 'font');
      visible.add(shownAt - started);
    } catch (error) {
      throw new Error(`${flow}, ${section.name}, ${change}, iteration ${iteration}: ${error}`);
    }

    if (anchor !== null && engine.readingPosition()?.intersectsNode(anchor)) {
      positionKept++;
    }
    await pause(BETWEEN_CHANGES_MS);
  }

  return {
    section: section.name,
    sectionBytes: section.bytes,
    flow,
    change,
    layoutMs: change === 'font' ? layout.summary() : null,
    visibleMs: visible.summary(),
    positionKept,
    iterations,
  };
}

/** Runs every kind of change on every section, in both flows. */
export async function runLayoutBench(
  host: BenchHost,
  sections: LayoutSection[],
): Promise<LayoutMeasurement[]> {
  const { engine } = host;
  const reset = async (): Promise<void> => {
    engine.setBoxWidth(null);
    engine.setTypography(host.typography);
    const startedAt = performance.now();
    await settled(engine, startedAt, engine.layoutFingerprint(), false);
  };

  const results: LayoutMeasurement[] = [];
  for (const flow of ['paginated', 'scrolled'] as const) {
    engine.setFlow(flow);
    for (const section of sections) {
      await engine.goToSection(section.index, READING_POSITION);
      await reset();
      // Turning a page is what makes the engine anchor on the text instead of on a fraction of
      // the section, the way it does for someone actually reading.
      await engine.nextPage();
      await reset();
      const iterations =
        section.bytes > LONG_SECTION_BYTES ? ITERATIONS_LONG_SECTION : ITERATIONS;
      for (const change of ['font', 'box', 'window'] as const) {
        results.push(await measureChange(host, section, flow, change, iterations));
        if (change === 'window') {
          await host.resizeWindow(...HOME_WINDOW_SIZE);
          await pause(200);
        }
        await reset();
      }
    }
  }
  return results;
}
