#!/usr/bin/env node
// Experiment C's security test. Opens a deliberately hostile EPUB in the real app and checks,
// from outside the app, what its content managed to do.
//
// A canary HTTP server stands in for the rest of the world: anything a book gets to send out
// ends up here. Each barrier is tested alone as well, and the run with no barriers at all is
// the control: the book's scripts do run there, so their absence elsewhere means something.
// It also shows how far a script gets when everything else has failed, since the separate
// origin is all that is left to keep it away from the app.
//
//   node tools/test-epub-security.mjs [--port 48780]

import { spawn, spawnSync } from 'node:child_process';
import { createServer } from 'node:http';
import { existsSync, mkdirSync, readFileSync, rmSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const exe = join(root, 'target', 'release', 'musaeum-desktop.exe');
const generator = join(root, 'target', 'release', 'gen-fixtures.exe');
const outDir = join(root, 'bench-results', 'epub-security');
const portAt = process.argv.indexOf('--port');
const port = portAt >= 0 ? Number(process.argv[portAt + 1]) : 48780;

for (const needed of [exe, generator]) {
  if (!existsSync(needed)) {
    console.error(`${needed} not found. Run "npm run build:app" in apps/desktop first.`);
    process.exit(1);
  }
}
rmSync(outDir, { recursive: true, force: true });
mkdirSync(outDir, { recursive: true });

// The book has the canary's address baked in, so it is written for this run.
const book = join(outDir, 'hostile.epub');
{
  const fixtures = join(outDir, 'fixtures');
  const made = spawnSync(
    generator,
    ['--out', fixtures, '--scan-pages', '1', '--vector-pages', '1', '--canary-port', String(port)],
    { stdio: 'ignore' },
  );
  if (made.status !== 0) {
    console.error('could not generate the hostile book');
    process.exit(1);
  }
  const bytes = readFileSync(join(fixtures, 'hostile.epub'));
  rmSync(fixtures, { recursive: true, force: true });
  const { writeFileSync } = await import('node:fs');
  writeFileSync(book, bytes);
}

/** What reached the canary during the current run. */
let hits = [];
let reports = [];

const server = createServer((request, response) => {
  const chunks = [];
  request.on('data', (chunk) => chunks.push(chunk));
  request.on('end', () => {
    if (request.url === '/report') {
      try {
        reports.push(JSON.parse(Buffer.concat(chunks).toString('utf8')));
      } catch {
        reports.push({ unreadable: true });
      }
    } else {
      hits.push(request.url);
    }
    response.writeHead(200, {
      'Access-Control-Allow-Origin': '*',
      'Cross-Origin-Resource-Policy': 'cross-origin',
      'Content-Type': 'text/plain',
    });
    response.end('canary');
  });
});
await new Promise((ready) => server.listen(port, '127.0.0.1', ready));

function runApp(protection) {
  const out = join(outDir, `${protection}.json`);
  return new Promise((finished) => {
    const child = spawn(
      exe,
      ['--epub', book, '--epub-protection', protection, '--bench', out, '--scenario', 'security'],
      { stdio: 'ignore' },
    );
    const timer = setTimeout(() => child.kill(), 120_000);
    child.on('exit', (code) => {
      clearTimeout(timer);
      finished({ code, report: existsSync(out) ? JSON.parse(readFileSync(out, 'utf8')) : null });
    });
  });
}

const failures = [];
function check(protection, condition, message) {
  if (!condition) {
    failures.push(`${protection}: ${message}`);
  }
}

const configurations = [
  // What ships: both barriers up. Nothing may run and nothing may leave.
  { protection: 'full', scriptsRun: false, networkLeaks: false },
  // WebKit's situation: sections keep `allow-scripts`, and the policy alone has to hold.
  { protection: 'csp-only', scriptsRun: false, networkLeaks: false },
  // The sandbox stops scripts by itself, but has no say over images and the like.
  { protection: 'sandbox-only', scriptsRun: false, networkLeaks: true },
  // The control: with no barrier up, the attacks work, or this test would prove nothing.
  { protection: 'none', scriptsRun: true, networkLeaks: true },
];

const rows = [];
const reportsByRun = new Map();
for (const { protection, scriptsRun, networkLeaks } of configurations) {
  hits = [];
  reports = [];
  console.error(`running with protection "${protection}"...`);
  const { code, report } = await runApp(protection);
  // Requests and beacons may still be in the air when the app exits.
  await new Promise((settle) => setTimeout(settle, 300));

  if (report === null || report.error) {
    check(protection, false, `the run failed: exit ${code}, ${report?.error ?? 'no report'}`);
    rows.push({ protection, result: 'failed' });
    continue;
  }
  const { sections, strayMessages } = report.frontend;
  const marked = sections.filter((section) => section.scriptMark !== null);
  const scriptHits = hits.filter((url) => url.startsWith('/script/'));
  const netHits = hits.filter((url) => url.startsWith('/net/'));
  const stray = strayMessages.filter((message) => message.type !== 'unknown');
  reportsByRun.set(protection, [...reports]);

  check(protection, code === 0, `the app exited with ${code}, so a book may have reached it`);
  check(
    protection,
    report.options.bookProtection === protection,
    `ran with ${report.options.bookProtection}`,
  );
  if (scriptsRun) {
    check(protection, marked.length > 5, `only ${marked.length} scripts ran in the control`);
    check(protection, scriptHits.length > 5, `only ${scriptHits.length} reached the canary`);
    check(protection, reports.length > 5, `only ${reports.length} reports arrived`);
  } else {
    check(protection, marked.length === 0, `scripts ran: ${marked.map((s) => s.name)}`);
    check(protection, scriptHits.length === 0, `scripts reached the canary: ${scriptHits}`);
    check(protection, reports.length === 0, `${reports.length} script reports arrived`);
    check(protection, stray.length === 0, `messages reached the app: ${JSON.stringify(stray)}`);
    const loaded = sections.filter((section) => section.loaded).length;
    const expected = sections.length - (networkLeaks ? 2 : 0);
    check(protection, loaded >= expected, `only ${loaded} of ${sections.length} sections loaded`);
  }
  if (networkLeaks) {
    check(protection, netHits.length > 3, `only ${netHits.length} network attempts in the control`);
  } else {
    check(protection, netHits.length === 0, `the network was reached: ${netHits}`);
  }

  rows.push({
    protection,
    sections: sections.length,
    loaded: sections.filter((section) => section.loaded).length,
    'scripts ran': marked.length,
    'script hits': scriptHits.length,
    'network hits': netHits.length,
    'script reports': reports.length,
    'stray messages': stray.length,
    'exit code': code,
  });
}
await new Promise((closed) => server.close(closed));

// With everything else down, this is what a script can reach: the separate origin's job.
const control = reportsByRun.get('none') ?? [];
const allowed = (name, predicate) => {
  for (const report of control) {
    if (!predicate(String(report[name]))) {
      failures.push(`control, ${report.vector}: ${name} = ${report[name]}`);
    }
  }
};
allowed('topDocument', (value) => value.startsWith('blocked'));
allowed('topTauri', (value) => value.startsWith('blocked'));
allowed('tileFetch', (value) => !value.startsWith('status 2'));
allowed('ipcFetch', (value) => !value.startsWith('status 2'));
allowed('appFetch', (value) => !value.startsWith('status 2'));
// The real API, with the real key, from a script in the book: Tauri has to refuse it.
allowed('invokeRead', (value) => !value.startsWith('allowed'));
allowed('invokeOpen', (value) => !value.startsWith('allowed'));
allowed('invokeExit', (value) => !value.startsWith('allowed'));

console.table(rows);
if (control.length > 0) {
  const seen = (name) => [...new Set(control.map((report) => String(report[name])))].join(' | ');
  console.log('\nWith no barrier up, a script running in a section of the book saw:');
  console.table(
    [
      'origin',
      'topDocument',
      'topTauri',
      'ownTauri',
      'ownIpc',
      'webviewApi',
      'invokeRead',
      'invokeOpen',
      'invokeExit',
      'webviewPost',
      'ipcPost',
      'ipcFetch',
      'tileFetch',
      'appFetch',
    ].map((name) => ({ attempt: name, result: seen(name).slice(0, 110) })),
  );
  const ran = control.map((report) => report.vector).sort();
  console.log(`Vectors that ran with no barrier up (${ran.length}): ${ran.join(', ')}`);
}

if (failures.length > 0) {
  console.log('\nFAILED:');
  for (const failure of failures) {
    console.log(`  ${failure}`);
  }
  process.exit(1);
}
console.log('\nAll checks passed.');
