#!/usr/bin/env node
// Experiment C's measurement: how long the fluid engine takes to lay a section out again when
// the font size or the window changes, against the target of 100 ms.
//
//   node tools/run-epub-bench.mjs [--epub fixtures/large.epub] [--out layout] [--bin DIR]

import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const TARGET_MS = 100;

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const binAt = process.argv.indexOf('--bin');
const exe = join(binAt >= 0 ? resolve(process.argv[binAt + 1]) : join(root, 'target', 'release'), 'musaeum-desktop.exe');
const outDir = join(root, 'bench-results', 'epub');
const epubAt = process.argv.indexOf('--epub');
const epub = epubAt >= 0 ? resolve(process.argv[epubAt + 1]) : join(root, 'fixtures', 'large.epub');

for (const needed of [exe, epub]) {
  if (!existsSync(needed)) {
    console.error(`${needed} not found. See "Como rodar" in the README.`);
    process.exit(1);
  }
}
mkdirSync(outDir, { recursive: true });

const outAt = process.argv.indexOf('--out');
const out = join(outDir, `${outAt >= 0 ? process.argv[outAt + 1] : 'layout'}.json`);
const run = spawnSync(exe, ['--epub', epub, '--bench', out, '--scenario', 'layout'], {
  stdio: 'ignore',
  timeout: 900_000,
});
if (!existsSync(out)) {
  console.error(`no report: ${run.error ?? `exit code ${run.status}`}`);
  process.exit(1);
}
const report = JSON.parse(readFileSync(out, 'utf8'));
if (report.error) {
  console.error(report.error);
  process.exit(1);
}

const fixed = (value, digits = 1) => (value === undefined || value === null ? '-' : value.toFixed(digits));
const { measurements, reader, openToReadyMs } = report.frontend;
console.log(`Opened in ${fixed(reader.openMs)} ms in the reader, ${fixed(openToReadyMs)} ms from the main page.`);
console.table(
  measurements.map((measurement) => ({
    flow: measurement.flow,
    section: measurement.section,
    change: measurement.change,
    'layout p50': fixed(measurement.layoutMs?.p50),
    'layout p95': fixed(measurement.layoutMs?.p95),
    'visible p50': fixed(measurement.visibleMs.p50),
    'visible p95': fixed(measurement.visibleMs.p95),
    'visible max': fixed(measurement.visibleMs.max),
    'position kept': `${measurement.positionKept}/${measurement.iterations}`,
    target: measurement.visibleMs.p95 <= TARGET_MS ? 'ok' : 'over',
  })),
);
