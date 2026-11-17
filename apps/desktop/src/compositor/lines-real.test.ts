/**
 * The snap timed on line models of real scanned pages, as `corpus-check --lines` writes them
 * (the page hash, then `MLIN`, one file per page). Runs only when `MUSAEUM_LINE_MODELS` names that
 * folder; the report goes to `MUSAEUM_SNAP_REPORT` if set.
 */

import { describe, expect, it } from 'vitest';
import { decodeLines, pieceQuads, pointAt, snapRange } from './lines';

// Vitest runs in Node; the app's types know nothing of it, and need not.
declare const process: { env: Record<string, string | undefined> };
interface NodeFs {
  readdirSync(path: string): string[];
  readFileSync(path: string): Uint8Array;
  writeFileSync(path: string, data: string): void;
}
const FS_MODULE = 'node:fs';

const folder = process.env.MUSAEUM_LINE_MODELS;
const join = (directory: string, name: string): string => `${directory}/${name}`;

describe.skipIf(folder === undefined)('snapping on real scans', () => {
  it('takes under a millisecond on every page', async () => {
    const { readdirSync, readFileSync, writeFileSync } = (await import(/* @vite-ignore */ FS_MODULE)) as NodeFs;
    const files = readdirSync(folder ?? '.').filter((name) => name.endsWith('.mlin'));
    const times: number[] = [];
    let pages = 0;
    for (const name of files) {
      const bytes = readFileSync(join(folder ?? '.', name));
      // A copy: a Node Buffer is a view into a shared pool, and its `.buffer` is the whole pool.
      const page = decodeLines(new Uint8Array(bytes).buffer);
      if (page.lines.length === 0) {
        continue;
      }
      pages++;
      const snap = (step: number): void => {
        const a = page.lines[step % page.lines.length];
        const b = page.lines[(step * 7 + 3) % page.lines.length];
        // Half the ascent up across each line, from where along it the step says.
        const inside = (line: typeof a, t: number): { x: number; y: number } => {
          const at = pointAt(page, line, t);
          const rise = line.ascent / 2;
          return { x: at.x + (at.dy * rise) / page.aspect, y: at.y - at.dx * rise };
        };
        const from = inside(a, (step % 10) / 10);
        const to = inside(b, (step % 7) / 7);
        for (const piece of snapRange(page, from, to, true)) {
          pieceQuads(page, page.lines[piece.index], piece, true);
        }
      };
      for (let step = 0; step < 20; step++) {
        snap(step);
      }
      for (let step = 0; step < 50; step++) {
        const started = performance.now();
        snap(step);
        times.push(performance.now() - started);
      }
    }
    times.sort((a, b) => a - b);
    const at = (q: number): number => times[Math.round((times.length - 1) * q)] ?? 0;
    const report = { pages, snaps: times.length, p50Us: at(0.5) * 1000, p99Us: at(0.99) * 1000, maxUs: at(1) * 1000 };
    console.info(`snap on ${pages} real pages: p50 ${report.p50Us.toFixed(1)} µs, p99 ${report.p99Us.toFixed(1)} µs, max ${report.maxUs.toFixed(1)} µs`);
    if (process.env.MUSAEUM_SNAP_REPORT !== undefined) {
      writeFileSync(process.env.MUSAEUM_SNAP_REPORT, JSON.stringify(report, null, 2));
    }
    expect(pages).toBeGreaterThan(0);
    expect(at(0.99)).toBeLessThan(1);
  });
});
