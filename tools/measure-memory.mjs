#!/usr/bin/env node
// Measures what the whole app costs in RAM against the target of 250 MB: the private working
// sets of the core, the WebView2 processes and the render workers, added up. Each configuration
// is a fresh process that goes through a person's use of it: open, read, step away.
//
//   node tools/measure-memory.mjs [--workers 2,6] [--gpu-layers N]

import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const TARGET_MB = 250;
const MB = 1_000_000;

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const exe = join(root, 'target', 'release', 'musaeum-desktop.exe');
const outDir = join(root, 'bench-results', 'memory');
const fixtures = join(root, 'fixtures');

function option(name, fallback) {
  const at = process.argv.indexOf(`--${name}`);
  return at >= 0 ? process.argv[at + 1] : fallback;
}
const workerCounts = option('workers', '4').split(',');
const gpuLayers = option('gpu-layers', null);

if (!existsSync(exe)) {
  console.error(`${exe} not found. See "Como rodar" in the README.`);
  process.exit(1);
}
mkdirSync(outDir, { recursive: true });

const runs = [
  { label: 'no document', args: [] },
  ...workerCounts.flatMap((workers) => [
    {
      label: `scan PDF, ${workers} workers`,
      args: ['--pdf', join(fixtures, 'scan-2000.pdf'), '--workers', workers],
    },
    {
      label: `vector PDF, ${workers} workers`,
      args: ['--pdf', join(fixtures, 'vector-500.pdf'), '--workers', workers],
    },
  ]),
  { label: 'EPUB', args: ['--epub', join(fixtures, 'large.epub')] },
];

const mb = (bytes) => (bytes / MB).toFixed(0);
const phaseRows = [];
const roleRows = [];
for (const [index, run] of runs.entries()) {
  const out = join(outDir, `run-${index}.json`);
  console.error(`measuring: ${run.label}...`);
  const layers = gpuLayers !== null && !run.args.includes('--epub') ? ['--gpu-layers', gpuLayers] : [];
  const result = spawnSync(exe, [...run.args, ...layers, '--bench', out, '--scenario', 'memory'], {
    stdio: 'ignore',
    timeout: 180_000,
  });
  const report = existsSync(out) ? JSON.parse(readFileSync(out, 'utf8')) : null;
  if (report === null || report.error) {
    console.error(`  failed: ${report?.error ?? `exit code ${result.status}`}`);
    continue;
  }
  for (const phase of report.memory.phases) {
    if (phase.phase === 'starting' || phase.phase === 'end') {
      continue;
    }
    phaseRows.push({
      configuration: run.label,
      phase: phase.phase,
      'mean MB': mb(phase.meanTotal),
      'peak MB': mb(phase.peakTotal),
      'last MB': mb(phase.lastTotal),
      '<= 250 MB': phase.lastTotal <= TARGET_MB * MB ? 'yes' : 'NO',
    });
    for (const [role, total] of Object.entries(phase.lastByRole)) {
      roleRows.push({
        configuration: run.label,
        phase: phase.phase,
        role,
        processes: total.processes,
        MB: Number(mb(total.privateWorkingSet)),
      });
    }
  }
}

console.log(`Private working set, added up over every process, against a target of ${TARGET_MB} MB:`);
console.table(phaseRows);
console.log('\nWho holds it, at the end of each phase:');
console.table(roleRows);
writeFileSync(join(outDir, 'summary.json'), JSON.stringify({ phaseRows, roleRows }, null, 2));
