#!/usr/bin/env node
// Tests the automatic delivery selection in the real app: every failure the selection has to
// survive is simulated on a real WebView2, a real render pool runs on the path it picks, and
// tiles have to reach the screen. Each case is a fresh process with a short scenario.
//
//   node tools/test-cascade.mjs [--pdf fixtures/scan-2000.pdf]

import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, rmSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const exe = join(root, 'target', 'release', 'musaeum-desktop.exe');
const outDir = join(root, 'bench-results', 'cascade');

const pdfAt = process.argv.indexOf('--pdf');
const pdf = pdfAt >= 0 ? resolve(process.argv[pdfAt + 1]) : join(root, 'fixtures', 'scan-2000.pdf');

for (const needed of [exe, pdf]) {
  if (!existsSync(needed)) {
    console.error(`${needed} not found. See "Como rodar" in the README.`);
    process.exit(1);
  }
}
rmSync(outDir, { recursive: true, force: true });
mkdirSync(outDir, { recursive: true });

const cases = [
  { name: 'nothing wrong', delivery: 'auto', faults: [], path: 'c', rejected: [] },
  { name: 'worker gets a copy', delivery: 'auto', faults: ['copied-transfer'], path: 'b', rejected: ['c'] },
  { name: 'transfer refused', delivery: 'auto', faults: ['rejected-transfer'], path: 'b', rejected: ['c'] },
  { name: 'no shared buffers', delivery: 'auto', faults: ['no-shared-buffers'], path: 'a', rejected: ['c', 'b'] },
  {
    name: 'copy and no isolation',
    delivery: 'auto',
    faults: ['copied-transfer', 'not-isolated'],
    path: 'a',
    rejected: ['c', 'b'],
  },
  { name: 'no isolation, Path C fine', delivery: 'auto', faults: ['not-isolated'], path: 'c', rejected: ['b'] },
  { name: 'forced Path A', delivery: 'a', faults: [], path: 'a', rejected: [] },
  { name: 'forced Path B', delivery: 'b', faults: [], path: 'b', rejected: [] },
  { name: 'forced Path C', delivery: 'c', faults: [], path: 'c', rejected: [] },
  { name: 'forced Path C, unavailable', delivery: 'c', faults: ['copied-transfer'], fails: true },
  { name: 'forced Path B, unavailable', delivery: 'b', faults: ['not-isolated'], fails: true },
];

function check(problems, condition, message) {
  if (!condition) {
    problems.push(message);
  }
}

let failures = 0;
const rows = [];
for (const [index, testCase] of cases.entries()) {
  const out = join(outDir, `case-${index}.json`);
  const args = ['--pdf', pdf, '--delivery', testCase.delivery, '--bench', out, '--scenario', 'smoke'];
  if (testCase.faults.length > 0) {
    args.push('--delivery-fault', testCase.faults.join(','));
  }
  const run = spawnSync(exe, args, { stdio: 'ignore', timeout: 60_000 });
  const report = existsSync(out) ? JSON.parse(readFileSync(out, 'utf8')) : null;
  const problems = [];
  let chosen = '-';
  let tiles = '-';
  let elapsed = '-';

  if (testCase.fails) {
    check(problems, run.status !== 0, `expected a failure, got exit code ${run.status}`);
    check(problems, report?.error?.includes('no delivery path'), `unexpected report: ${report?.error}`);
  } else if (report === null || report.error) {
    problems.push(`no report: ${report?.error ?? `exit code ${run.status}`}`);
  } else {
    const { deliverySelection: selection, overall } = report.frontend;
    chosen = selection.path;
    tiles = overall.tilesUploaded;
    elapsed = selection.elapsedMs.toFixed(1);
    check(problems, selection.path === testCase.path, `chose ${selection.path}, expected ${testCase.path}`);
    check(problems, report.frontend.session.delivery === testCase.path, 'the session used another path');
    const rejected = selection.rejected.map((entry) => entry.path).join();
    check(problems, rejected === testCase.rejected.join(), `rejected [${rejected}]`);
    check(problems, overall.tilesUploaded > 0, 'no tile reached the compositor');
    check(problems, overall.blankFrames === 0, `${overall.blankFrames} blank frames`);
    check(problems, report.pool.tiles_failed === 0, `${report.pool.tiles_failed} tiles failed`);
    check(problems, overall.sharpFrames > 0, 'the screen never became sharp');
  }

  failures += problems.length > 0 ? 1 : 0;
  rows.push({
    case: testCase.name,
    delivery: testCase.delivery,
    faults: testCase.faults.join(',') || '-',
    chosen: testCase.fails ? 'refused' : chosen,
    tiles,
    'selection ms': elapsed,
    result: problems.length === 0 ? 'ok' : problems.join('; '),
  });
}

console.table(rows);
console.log(failures === 0 ? 'All cases passed.' : `${failures} of ${cases.length} cases failed.`);
process.exit(failures === 0 ? 0 : 1);
