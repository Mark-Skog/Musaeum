#!/usr/bin/env node
// What a hostile script in the app's own page could ask of the host, tried from that page, in the
// real app run as a person runs it: without --e2e, whose test commands take paths. Each request
// must be refused, and nothing on the disk may change.
//
//   node tools/e2e-ipc.mjs [--app target/release/musaeum-desktop.exe] [--cli target/release/musaeum.exe]
//                          [--out FILE.json] [--require-idle SECONDS]
//
// 1. Tauri's own commands that the page does not use (the app's version, the window, the webviews,
//    the paths): not allowed by the window's capability. Listening to events is, and the app works.
// 2. Following a folder named by path, reading a Calibre library the dialog did not give, opening
//    a document that is not a book of the library (by its path, and through `..`): refused. A book
//    of the library opens, and a page asking for 100 000 render workers gets at most 16.
// 3. Writing or reading a file the dialog did not give (JSON-LD, the PDF copy, XFDF, a report), the
//    test commands, the benchmark's commands, and a link that is not on the web: refused.
// 4. Afterwards: the same folders followed, no file written anywhere, the books as they were, and
//    no worker left once the app closes.
//
// The library is made before the app starts, with the command line (`musaeum root add`). Everything
// lives in a temporary folder, removed at the end. Exit code 3 when someone used the machine.

import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, relative, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

import { writeCalibreLibrary } from './lib/calibre-library.mjs';
import { closeApp, idleSeconds, launch, waitFor } from './lib/cdp.mjs';
import { annotatedPdf } from './lib/test-books.mjs';

const root = resolve(import.meta.dirname, '..');

function parseArguments(argv) {
  const options = {
    app: join(root, 'target', 'release', 'musaeum-desktop.exe'),
    cli: join(root, 'target', 'release', 'musaeum.exe'),
    out: join(root, 'bench-results', 'e2e-ipc', 'report.json'),
    requireIdle: 60,
  };
  for (let at = 0; at < argv.length; at++) {
    const flag = argv[at];
    const value = () => argv[++at];
    if (flag === '--app') options.app = resolve(value());
    else if (flag === '--cli') options.cli = resolve(value());
    else if (flag === '--out') options.out = resolve(value());
    else if (flag === '--require-idle') options.requireIdle = Number(value());
    else throw new Error(`unknown argument: ${flag}`);
  }
  return options;
}

const options = parseArguments(process.argv.slice(2));

class Interrupted extends Error {}

const report = { started: new Date().toISOString(), options, steps: [], failures: [] };
function step(label, ok, detail = {}) {
  report.steps.push({ label, ok, ...detail });
  if (!ok) {
    report.failures.push(label);
    console.error(`  FAILED: ${label} ${JSON.stringify(detail).slice(0, 2000)}`);
  } else {
    console.log(`  ok: ${label}`);
  }
  return ok;
}

const sha256 = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');

/** Every file and folder under `directory`, with its size, time and, for a file, its hash. */
function listTree(directory) {
  const entries = [];
  const walk = (folder) => {
    for (const name of readdirSync(folder).sort()) {
      const path = join(folder, name);
      const stat = statSync(path, { bigint: true });
      entries.push({ path: relative(directory, path), size: String(stat.size), modified: String(stat.mtimeNs), sha256: stat.isFile() ? sha256(path) : null });
      if (stat.isDirectory()) {
        walk(path);
      }
    }
  };
  walk(directory);
  return entries;
}

/** Asks the host from the page, as any script there could, and says how it answered. */
async function ask(main, command, args = {}) {
  return main.evaluate(
    `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)}).then((value) => ({ ok: true, value }), (error) => ({ ok: false, error: typeof error === 'string' ? error : JSON.stringify(error) }))`,
  );
}

function processesNamed(name) {
  const out = execFileSync('tasklist', ['/FI', `IMAGENAME eq ${name}`, '/FO', 'CSV', '/NH'], { encoding: 'utf8', windowsHide: true });
  return out
    .split(/\r?\n/)
    .filter((line) => line.toLowerCase().includes(name.toLowerCase()))
    .map((line) => Number(line.split('","')[1]));
}

async function run(work) {
  const library = join(work, 'biblioteca');
  const shelf = join(work, 'estante');
  const outside = join(work, 'fora');
  const calibre = join(work, 'Calibre');
  const out = join(work, 'saida');
  for (const folder of [shelf, outside, out]) {
    mkdirSync(folder, { recursive: true });
  }
  const pdf = annotatedPdf();
  const book = join(shelf, 'livro.pdf');
  writeFileSync(book, pdf.v1);
  const secret = join(outside, 'secreto.pdf');
  writeFileSync(secret, pdf.v1);
  writeCalibreLibrary(calibre, []);
  execFileSync(options.cli, ['--library', library, 'root', 'add', shelf], { windowsHide: true });
  execFileSync(options.cli, ['--library', library, 'reconcile'], { windowsHide: true });
  const before = { shelf: listTree(shelf), outside: listTree(outside), calibre: listTree(calibre) };

  const session = await launch(options.app, ['--library', library, '--width', '1200', '--height', '800'], "document.querySelector('.library') !== null");
  const main = session.cdp;
  try {
    step('the page is not under a test: no test hooks', (await main.evaluate('window.musaeumApp === undefined')) === true);

    // 1. Tauri's own commands.
    for (const [command, args] of [
      ['plugin:app|version', {}],
      ['plugin:app|name', {}],
      ['plugin:window|title', { label: 'main' }],
      ['plugin:webview|get_all_webviews', {}],
      ['plugin:path|resolve_directory', { directory: 1 }],
      ['plugin:resources|close', { rid: 1 }],
    ]) {
      const answer = await ask(main, command, args);
      step(`Tauri's ${command} is not allowed to the page`, !answer.ok && /not allowed|denied|not found/i.test(answer.error), { answer });
    }
    const roots = await ask(main, 'library_info');
    step('the page still reaches the app’s own commands', roots.ok && roots.value.roots?.length === 1, { roots });

    // 2. Folders and books named by path.
    const added = await ask(main, 'library_add_root', { path: outside });
    step('a folder named by path is not followed', !added.ok && /under a test/.test(added.error), { added });
    const preview = await ask(main, 'calibre_preview', { path: calibre });
    step('a Calibre library the dialog did not give is not read', !preview.ok && /not chosen/.test(preview.error), { preview });
    const imported = await ask(main, 'calibre_import', { path: calibre });
    step('nor brought in', !imported.ok && /not chosen/.test(imported.error), { imported });
    // The second goes through the followed folder and back out with `..`, written as is: `join` would
    // resolve it.
    for (const document of [secret, `${shelf}\\..\\fora\\secreto.pdf`]) {
      const opened = await ask(main, 'open_session', { args: { document, delivery: 'a', workers: 2 } });
      step(`a document outside the library is not opened (${document.slice(work.length + 1)})`, !opened.ok && /neither a book of the library/.test(JSON.stringify(opened.error)), { opened });
    }
    const workersBefore = processesNamed('musaeum-render-pdf.exe').length;
    const own = await ask(main, 'open_session', { args: { document: book, delivery: 'a', workers: 100000 } });
    await delay(1500);
    const workers = processesNamed('musaeum-render-pdf.exe').length - workersBefore;
    step('a book of the library opens, with at most 16 render workers whatever the page asks', own.ok && workers >= 1 && workers <= 16, { workers, error: own.error });
    const sessionId = own.ok ? own.value.id : 0;
    const documentId = own.ok ? own.value.documentId : 0;

    // 3. Files the dialog did not give, and the commands of tests and benchmarks. With an annotation
    // in the library, so that every export gets as far as asking where to write.
    const mark = await ask(main, 'create_fixed_annotation', { session: sessionId, spans: [{ page: 0, start: 0, end: 12 }], mark: { kind: 'highlight', color: 'yellow' } });
    step('an annotation, made as the reader makes one', mark.ok, { mark });
    const writes = [
      ['annotations_export_jsonld', { scope: { kind: 'library' }, target: join(out, 'a.jsonld') }, join(out, 'a.jsonld')],
      ['annotations_import_jsonld', { target: { kind: 'library' }, source: join(out, 'b.jsonld') }, null],
      ['study_report', { format: 'markdown', scope: { kind: 'document', ids: documentId }, utcOffsetMinutes: 0, target: join(out, 'c.md'), generatedAt: null }, join(out, 'c.md')],
      ['study_report', { format: 'pdf', scope: { kind: 'document', ids: documentId }, utcOffsetMinutes: 0, target: join(out, 'c.pdf'), generatedAt: null }, join(out, 'c.pdf')],
      ['pdf_annotations_export', { session: sessionId, target: join(out, 'd.pdf') }, join(out, 'd.pdf')],
      ['pdf_annotations_import', { session: sessionId, source: secret }, null],
      ['xfdf_export', { session: sessionId, target: join(out, 'e.xfdf') }, join(out, 'e.xfdf')],
      ['xfdf_import', { session: sessionId, source: join(out, 'f.xfdf') }, null],
    ];
    for (const [command, args, written] of writes) {
      const answer = await ask(main, command, args);
      const refused = !answer.ok && /only given under a test/.test(answer.error);
      step(`${command} takes no path from the page`, refused && (written === null || !existsSync(written)), { answer });
    }
    for (const [command, args, pattern] of [
      ['e2e_pdf_text', { path: secret }, /test/],
      ['e2e_pdf_annotations', { path: secret }, /test/],
      ['e2e_opened_links', {}, /test/],
      ['e2e_resize_window', { width: 10, height: 10 }, /test/],
      ['bench_resize_window', { width: 10, height: 10 }, /benchmark/],
      ['bench_finish', { frontend: '{}' }, /benchmark/],
    ]) {
      const answer = await ask(main, command, args);
      step(`${command} is refused outside a test or a benchmark`, !answer.ok && pattern.test(answer.error), { answer });
    }
    for (const url of ['file:///C:/Windows/System32/notepad.exe', 'javascript:alert(1)', 'ms-settings:privacy', '\\\\server\\share\\x.exe']) {
      const answer = await ask(main, 'open_in_browser', { url });
      step(`a link that is not on the web is not opened (${url.split(':')[0]})`, !answer.ok, { answer });
    }
    if (sessionId > 0) {
      await ask(main, 'close_session');
    }
    const after = await ask(main, 'library_info');
    step('afterwards, the same single folder is followed', after.ok && after.value.roots?.length === 1 && after.value.roots[0].path.toLowerCase() === shelf.toLowerCase(), { roots: after.value?.roots });
  } finally {
    report.console = session.console;
    await closeApp(session).catch((error) => step('the app closes when asked', false, { error: String(error) }));
  }
  await delay(1000);
  const left = ['musaeum-render-pdf.exe', 'musaeum-ingest.exe', 'musaeum-ocr.exe', 'musaeum-pdf-annots.exe'].flatMap((name) => processesNamed(name).map((pid) => ({ name, pid })));
  step('no worker is left once the app has closed', left.length === 0, { left });
  step('nothing was written to the disk outside the library: the books, the other folder and the Calibre library are as they were, and nothing was exported', JSON.stringify({ shelf: listTree(shelf), outside: listTree(outside), calibre: listTree(calibre) }) === JSON.stringify(before) && readdirSync(out).length === 0, {
    out: readdirSync(out),
  });
}

async function main() {
  for (const path of [options.app, options.cli]) {
    if (!existsSync(path)) {
      throw new Error(`${path} is missing: build it first`);
    }
  }
  if (options.requireIdle > 0) {
    const idle = idleSeconds();
    if (idle < options.requireIdle) {
      throw new Interrupted(`someone used the machine ${idle} s ago; this opens a window, so it waits for ${options.requireIdle} s of quiet`);
    }
  }
  const work = mkdtempSync(join(tmpdir(), 'musaeum-e2e-ipc-'));
  try {
    await run(work);
  } finally {
    for (let attempt = 0; attempt < 20 && existsSync(work); attempt++) {
      try {
        rmSync(work, { recursive: true, force: true, maxRetries: 10, retryDelay: 300 });
      } catch {
        await delay(500);
      }
    }
    report.leftInTemp = existsSync(work) ? work : null;
    report.finished = new Date().toISOString();
    mkdirSync(dirname(options.out), { recursive: true });
    writeFileSync(options.out, JSON.stringify(report, null, 2));
  }
  console.log(report.failures.length === 0 ? 'all passed' : `${report.failures.length} failed`);
  process.exitCode = report.failures.length === 0 && report.leftInTemp === null ? 0 : 1;
}

main().catch((error) => {
  console.error(error);
  process.exitCode = error instanceof Interrupted ? 3 : 1;
  try {
    mkdirSync(dirname(options.out), { recursive: true });
    writeFileSync(options.out, JSON.stringify({ ...report, error: String(error) }, null, 2));
  } catch {
    // The report is not what failed.
  }
});
