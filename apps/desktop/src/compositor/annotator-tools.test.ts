import { describe, expect, it } from 'vitest';
import { Annotator, type AnnotatorMessage, type RegionSelection } from './annotator';
import { encodeLines } from './lines-fixture';
import { PlacedPages } from './view';

/** Five lines across a page, every 0.04 from 0.2, with a space after each fifth of the line. */
const LINES = encodeLines(
  [0, 1, 2, 3, 4].map((row) => ({
    points: [
      [0.1, 0.2 + row * 0.04],
      [0.9, 0.2 + row * 0.04],
    ] as [number, number][],
    gaps: [
      [0.18, 0.2],
      [0.38, 0.4],
      [0.58, 0.6],
      [0.78, 0.8],
    ] as [number, number][],
  })),
);

/** One 500 × 500 point page, drawn 1000 × 1000 pixels at the canvas's corner. */
function setup(frame: Uint8Array | null = null): { annotator: Annotator; placed: PlacedPages; posted: AnnotatorMessage[]; marks: number[][] } {
  const posted: AnnotatorMessage[] = [];
  const annotator = new Annotator(new Float32Array([500]), new Float32Array([500]), (message) => posted.push(message), (_x, _y, width, height) =>
    frame === null ? null : frame.subarray(0, width * height * 4),
  );
  const placed = new PlacedPages(2);
  placed.add(0, 0, 0, 2);
  return { annotator, placed, posted, marks: [] };
}

const px = (x: number, y: number): { x: number; y: number } => ({ x: x * 1000, y: y * 1000 });

function drag(annotator: Annotator, placed: PlacedPages, points: { x: number; y: number }[]): boolean {
  const [first, ...rest] = points;
  const taken = annotator.pointerDown(first.x, first.y, 0, placed);
  for (const point of rest) {
    annotator.pointerMove(point.x, point.y, placed);
  }
  annotator.pointerUp(placed);
  return taken;
}

const region = (posted: AnnotatorMessage[]): RegionSelection | null => {
  const last = posted.filter((message) => message.type === 'region').at(-1);
  return last?.type === 'region' ? last.region : null;
};

function drawn(annotator: Annotator, placed: PlacedPages): number {
  let count = 0;
  annotator.draw({ mark: () => count++ }, placed);
  return count;
}

describe('marking the picture of a page', () => {
  it('asks for the lines of the pages on the screen once the lines tool is on', () => {
    const { annotator, placed, posted } = setup();
    annotator.want(placed, 0);
    expect(posted.some((message) => message.type === 'need-lines')).toBe(false);
    annotator.setTool('lines');
    annotator.want(placed, 10);
    annotator.want(placed, 20);
    expect(posted.filter((message) => message.type === 'need-lines')).toEqual([{ type: 'need-lines', pages: [0] }]);
    // Told the host has none yet: asked again.
    annotator.forgetAskedLines(0);
    annotator.want(placed, 30);
    expect(posted.filter((message) => message.type === 'need-lines')).toHaveLength(2);
  });

  it('selects along lines, from a word on the first to a word on the last', () => {
    const { annotator, placed, posted } = setup();
    annotator.setTool('lines');
    annotator.setLines(0, LINES);
    expect(drag(annotator, placed, [px(0.1 + 0.8 * 0.3, 0.198), px(0.4, 0.26), px(0.1 + 0.8 * 0.5, 0.318)])).toBe(true);
    const made = region(posted);
    expect(made?.tool).toBe('lines');
    expect(made?.lines.map((line) => line.index)).toEqual([0, 1, 2, 3]);
    expect(made?.lines[0].from).toBeCloseTo(0.2);
    expect(made?.lines[3].to).toBeCloseTo(0.58);
    // One quad per line, the lines being straight.
    expect(made?.quads).toHaveLength(4);
    expect(made?.clip[0]).toBeCloseTo(0.1);
    // Drawn until it becomes an annotation or goes.
    expect(drawn(annotator, placed)).toBe(4);
    annotator.clear(true);
    expect(region(posted)).toBeNull();
    expect(drawn(annotator, placed)).toBe(0);
  });

  it('draws a box, with its edges, and a click is no box', () => {
    const { annotator, placed, posted } = setup();
    annotator.setTool('rect');
    drag(annotator, placed, [px(0.6, 0.5), px(0.2, 0.3)]);
    const made = region(posted);
    expect(made?.tool).toBe('rect');
    expect(made?.clip).toEqual([0.2, 0.3, 0.6, 0.5].map((value) => expect.closeTo(value, 5)));
    expect(drawn(annotator, placed)).toBe(5);
    posted.length = 0;
    drag(annotator, placed, [px(0.4, 0.4), px(0.401, 0.4)]);
    expect(posted.filter((message) => message.type === 'region' && message.region !== null)).toEqual([]);
  });

  it('draws a stroke, keeping its path, and pans when pressed off the page', () => {
    const { annotator, placed, posted } = setup();
    annotator.setTool('free');
    const path = [px(0.2, 0.2), px(0.25, 0.22), px(0.3, 0.26), px(0.35, 0.3)];
    drag(annotator, placed, path);
    const made = region(posted);
    expect(made?.tool).toBe('free');
    expect(made?.path).toHaveLength(8);
    expect(made?.quads).toHaveLength(3);
    expect(annotator.pointerDown(1500, 1500, 0, placed)).toBe(false);
  });

  it('keeps what it draws in the stored frame of a turned page', () => {
    const { annotator, placed, posted } = setup();
    annotator.setTool('rect');
    annotator.setLines(0, encodeLines([{ points: [[0.1, 0.2], [0.9, 0.2]] }], { rotation: 1 }));
    drag(annotator, placed, [px(0.2, 0.3), px(0.6, 0.4)]);
    // Shown 0.4 wide and 0.1 high; stored, turned back a quarter, 0.1 wide and 0.4 high.
    const clip = region(posted)?.clip ?? [0, 0, 0, 0];
    expect(clip[2] - clip[0]).toBeCloseTo(0.1);
    expect(clip[3] - clip[1]).toBeCloseTo(0.4);
  });

  it('hashes the frame under the region, and goes back to text', () => {
    const frame = new Uint8Array(4000 * 4000 * 4).fill(255);
    for (let at = 0; at < frame.length; at += 4 * 7) {
      frame[at] = 0;
    }
    const { annotator, placed, posted } = setup(frame);
    annotator.setTool('rect');
    drag(annotator, placed, [px(0.2, 0.3), px(0.6, 0.5)]);
    expect(region(posted)?.regionHash).toMatch(/^[0-9a-f]{16}$/);
    annotator.setTool('text');
    expect(region(posted)).toBeNull();
    expect(annotator.currentTool).toBe('text');
  });
});
