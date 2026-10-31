#!/usr/bin/env node
// Runs Experiments A and B: every delivery path against every fixture, with and without a
// busy main thread, each in a fresh process, and prints a comparison table.
//
//   node tools/run-bench.mjs [--scenario quick|full] [--uncapped] [--workers N]
//                            [--paths a,b] [--pdfs scan-2000,vector-500] [--stress 0,60]

import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const exe = join(root, 'target', 'release', 'musaeum-desktop.exe');
const outDir = join(root, 'bench-results');

function option(name, fallback) {
  const at = process.argv.indexOf(`--${name}`);
  return at >= 0 ? process.argv[at + 1] : fallback;
}

const scenario = option('scenario', 'full');
const uncapped = process.argv.includes('--uncapped');
const workers = option('workers', '6');
const paths = option('paths', 'a,b,c').split(',');
const pdfs = option('pdfs', 'scan-2000,vector-500').split(',');
const stresses = option('stress', '0,60').split(',');
const timeoutMs = scenario === 'full' ? 180_000 : 90_000;

if (!existsSync(exe)) {
  console.error(`${exe} not found. Run "npm run build:app" in apps/desktop first.`);
  process.exit(1);
}
mkdirSync(outDir, { recursive: true });

const results = [];
for (const pdf of pdfs) {
  const pdfPath = join(root, 'fixtures', `${pdf}.pdf`);
  if (!existsSync(pdfPath)) {
    console.error(`${pdfPath} not found. Run target/release/gen-fixtures first.`);
    process.exit(1);
  }
  for (const stress of stresses) {
    for (const path of paths) {
      const label = `${pdf}-path-${path}-stress-${stress}${uncapped ? '-uncapped' : ''}`;
      const out = join(outDir, `${label}.json`);
      const args = [
        '--pdf', pdfPath,
        '--delivery', path,
        '--workers', workers,
        '--bench', out,
        '--scenario', scenario,
        '--main-stress-ms', stress,
      ];
      if (uncapped) {
        args.push('--uncapped');
      }
      console.error(`running ${label}...`);
      const run = spawnSync(exe, args, { stdio: 'inherit', timeout: timeoutMs });
      if (run.error || run.status !== 0) {
        console.error(`  failed: ${run.error ?? `exit code ${run.status}`}`);
      }
      if (!existsSync(out)) {
        results.push({ label, error: 'no report written' });
        continue;
      }
      results.push({ label, report: JSON.parse(readFileSync(out, 'utf8')) });
    }
  }
}

const fixed = (value, digits = 2) => (value === undefined ? '-' : value.toFixed(digits));

function row(label, segment, frontend, pool) {
  return {
    run: label,
    segment: segment.name,
    frames: segment.frames,
    fps: fixed(segment.achievedFps, 0),
    'interval p50': fixed(segment.frameIntervalMs.p50),
    'interval p95': fixed(segment.frameIntervalMs.p95),
    'interval p99': fixed(segment.frameIntervalMs.p99),
    'work p50': fixed(segment.frameWorkMs.p50),
    'work p95': fixed(segment.frameWorkMs.p95),
    'work p99': fixed(segment.frameWorkMs.p99),
    'over 8.3ms': segment.workOver120HzBudget,
    blank: segment.blankFrames,
    plate: segment.plateFrames,
    soft: segment.softFrames,
    sharp: segment.sharpFrames,
    tiles: segment.tilesUploaded,
    dup: segment.duplicateTiles,
    'deliv p50': fixed(segment.tileDeliveryMs.p50),
    'deliv p95': fixed(segment.tileDeliveryMs.p95),
    'deliv p99': fixed(segment.tileDeliveryMs.p99),
    'upload p99': fixed(segment.tileUploadMs.p99),
    'sharp p95': fixed(segment.timeToSharpMs.p95, 0),
  };
}

const rows = [];
for (const { label, report, error } of results) {
  if (error || report?.error) {
    console.log(`${label}: ${error ?? report.error}`);
    continue;
  }
  const { frontend, pool } = report;
  rows.push(row(label, frontend.overall, frontend, pool));
  for (const segment of frontend.segments) {
    if (!segment.name.startsWith('settle')) {
      rows.push(row('', segment, frontend, pool));
    }
  }
}
console.table(rows);

const summary = results
  .filter((result) => result.report && !result.report.error)
  .map(({ label, report }) => ({
    run: label,
    gpu: report.frontend.gpu,
    refreshMs: report.frontend.refreshIntervalMs,
    isolated: report.frontend.crossOriginIsolated,
    probes: report.frontend.sharedBufferProbes,
    mainCopy: report.frontend.mainThreadCopyMs,
    mainFrames: report.frontend.mainThreadFrameIntervalMs,
    render: report.pool?.render_us,
    cancel: report.pool?.cancel_latency_us,
    cancelsRequested: report.pool?.cancels_requested,
    cancelled: report.pool?.tiles_cancelled,
    discarded: report.pool?.results_discarded,
    done: report.pool?.tiles_done,
    restarts: report.pool?.worker_restarts,
    violations: report.pool?.protocol_violations,
    evictions: report.frontend.gpuEvictions,
    demands: report.frontend.demandsSent,
  }));
const summaryPath = join(outDir, `summary${uncapped ? '-uncapped' : ''}.json`);
writeFileSync(summaryPath, JSON.stringify(summary, null, 2));
console.log(`\nDetails in ${outDir}; summary in ${summaryPath}`);
