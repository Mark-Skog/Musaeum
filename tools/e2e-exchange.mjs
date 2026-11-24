#!/usr/bin/env node
// Phase 7, Part 2, in the real window, driven through the WebView2's debugging protocol: the
// annotations of several kinds of book out as W3C Web Annotations (JSON-LD) and back in, and the
// study reports, in Markdown and PDF.
//
//   node tools/e2e-exchange.mjs [--build target/release] [--out FILE.json] [--require-idle SECONDS]
//                               [--real BOOK.pdf+BOOK.pdf]
//
// 1. A library with a text PDF, its EPUB twin of the Phase 4 tests, a PDF with pages turned every
//    way (gen-fixtures --only annot) and a scanned book (gen-fixtures --only ocr, scanned by
//    visual-check). Annotated as a person does: on the PDF a highlight with a Markdown note in
//    Portuguese, a highlight and an underline; on the turned page a highlight; on the scan a box,
//    whose quote the OCR reads; on the EPUB three notes.
// 2. JSON-LD out of the library's menu (every book), of the reader's panel (one book) and of several
//    books chosen; the file is the W3C model with the library's selectors in it.
// 3. Study reports: from the reader, from one work's details and from several works chosen, in
//    Markdown (structure, reading order, chapters, the note's Markdown) and in PDF (PDFium reads its
//    text with every accent; the same report twice is the same bytes; opened in Musaeum, the band
//    beside an annotation is the color of its mark). Nothing is written inside a library folder.
// 4. The same file again: nothing new. Hostile files (a context to fetch, nesting, a key twice,
//    not JSON, a total that lies): refused, nothing changes.
// 5. A clean library with the same files: every annotation comes back, and the library writes back
//    the very same annotations; again, nothing new; drawn where they were.
// 6. A library with the second editions of the PDF and the EPUB: each annotation is taken by its
//    work and found again there, exact, approximate or an orphan, none lost; the works it does not
//    have are named.
// 7. Real books of the collection, copied: their own annotations in, then out as JSON-LD and as a
//    report; the books in the collection are as they were.
//
// Everything lives in a temporary folder, removed at the end. The run waits for the machine to be
// free and stops as soon as someone uses it (exit code 3).

import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

import { click, closeApp, idleSeconds, launch, press, typeInto, waitFor } from './lib/cdp.mjs';
import { decodePng } from './lib/png.mjs';
import { annotatedEpub, annotatedPdf } from './lib/test-books.mjs';

const root = resolve(import.meta.dirname, '..');
const SCALE_ONE = 4096;
const GENERATED_AT = 1_795_466_405_123;
const PDF_ID = '4d7573616575d6d0e8e8a1b2c3d4e5f6';
// The version the reports name, the app's own.
const VERSION = JSON.parse(readFileSync(join(root, 'apps', 'desktop', 'src-tauri', 'tauri.conf.json'), 'utf8')).version;

function parseArguments(argv) {
  const options = {
    build: join(root, 'target', 'release'),
    out: join(root, 'bench-results', 'e2e-exchange', 'report.json'),
    requireIdle: 60,
    real: [
      'C:\\Marte\\Archives\\DigitalLibrary\\Books\\Mathematics\\Applied Mathematics\\2003 - Information Theory, Inference, and Learning Algorithms - David JC MacKay.pdf',
      'C:\\Marte\\Archives\\DigitalLibrary\\Books\\Business & Economics\\2013 - Introdução à Economia - N Gregory Mankiw.pdf',
    ],
  };
  for (let at = 0; at < argv.length; at++) {
    const flag = argv[at];
    const value = () => argv[++at];
    if (flag === '--build') options.build = resolve(value());
    else if (flag === '--out') options.out = resolve(value());
    else if (flag === '--require-idle') options.requireIdle = Number(value());
    else if (flag === '--real') options.real = value().split('+').filter((path) => path.length > 0).map((path) => resolve(path));
    else throw new Error(`unknown argument: ${flag}`);
  }
  return options;
}

const options = parseArguments(process.argv.slice(2));
const app = join(options.build, 'musaeum-desktop.exe');

class Interrupted extends Error {}
let quietSince = 0;
function checkStillQuiet() {
  if (options.requireIdle <= 0) {
    return;
  }
  const idle = idleSeconds();
  if (idle < quietSince) {
    throw new Interrupted(`someone used the machine ${idle} s ago`);
  }
  quietSince = idle;
}

const report = { started: new Date().toISOString(), options, steps: [], failures: [] };
function step(label, ok, detail = {}) {
  report.steps.push({ label, ok, ...detail });
  if (!ok) {
    report.failures.push(label);
    console.error(`  FAILED: ${label} ${JSON.stringify(detail).slice(0, 3000)}`);
  } else {
    console.log(`  ok: ${label}`);
  }
  return ok;
}

const sha256 = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');

function listTree(directory) {
  return readdirSync(directory)
    .map((name) => {
      const stat = statSync(join(directory, name), { bigint: true });
      return { name, size: String(stat.size), modified: String(stat.mtimeNs), sha256: stat.isFile() ? sha256(join(directory, name)) : null };
    })
    .sort((a, b) => a.name.localeCompare(b.name));
}

// --- The app ---

const reader = (main) => main.evaluate('window.musaeumApp.reader()');

async function firstScreen(main) {
  await waitFor(main, "document.querySelector('.welcome') !== null || document.querySelector('.library') !== null", { what: 'the first screen' });
}

async function waitForLibrary(main) {
  await waitFor(main, "window.musaeumApp.view() === 'library' && document.querySelector('.library') !== null", { timeoutMs: 20_000, what: 'the library' });
}

async function rows(main, count) {
  return (
    await waitFor(
      main,
      `(() => { const r = window.musaeumApp.rows(); const i = window.musaeumApp.library(); return r.total === ${count} && i && !i.indexing && r.rows.every((b) => b.blake3 !== null) ? r.rows : null; })()`,
      { timeoutMs: 180_000, what: `${count} books in the library` },
    )
  ).slice();
}

const rowNamed = (list, name) => list.find((row) => basename(row.path) === name);

async function openBook(main, documentId) {
  checkStillQuiet();
  await waitFor(main, `document.querySelector('[data-document="${documentId}"]') !== null`, { timeoutMs: 20_000, what: `the card of document ${documentId}` });
  const before = await main.evaluate('window.musaeumApp.opened()');
  await click(main, `[data-document="${documentId}"]`);
  const started = Date.now();
  while (Date.now() - started < 2000 && (await main.evaluate('window.musaeumApp.opened()')) === before) {
    await delay(50);
  }
  if ((await main.evaluate('window.musaeumApp.opened()')) === before) {
    await press(main, 'Enter');
  }
  const state = await waitFor(
    main,
    `(() => { const r = window.musaeumApp.reader(); return r !== null && r.documentId === ${documentId} && (r.ready || r.error !== null) ? r : null; })()`,
    { timeoutMs: 90_000, what: `document ${documentId} to open` },
  );
  if (state.error !== null) {
    throw new Error(`document ${documentId} did not open: ${state.error}`);
  }
  return state;
}

async function back(main) {
  await click(main, '#back-to-library');
  await waitForLibrary(main);
}

async function written(main, count, what) {
  return waitFor(
    main,
    `(() => { const a = window.musaeumApp.reader()?.annotations ?? []; return a.length === ${count} && a.every((e) => !e.pending) ? a : null; })()`,
    { timeoutMs: 30_000, what },
  );
}

async function mouse(main, type, at, extra = {}) {
  await main.send('Input.dispatchMouseEvent', { type, x: at.x, y: at.y, button: type === 'mouseMoved' ? 'none' : 'left', ...extra });
}

async function drag(main, from, to) {
  checkStillQuiet();
  await mouse(main, 'mouseMoved', from, { buttons: 0 });
  await mouse(main, 'mousePressed', from, { buttons: 1, clickCount: 1 });
  for (let n = 1; n <= 12; n++) {
    const t = n / 12;
    await main.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: from.x + (to.x - from.x) * t, y: from.y + (to.y - from.y) * t, button: 'left', buttons: 1 });
    await delay(16);
  }
  await mouse(main, 'mouseReleased', to, { buttons: 0, clickCount: 1 });
}

async function pixel(main, at) {
  const shot = await main.send('Page.captureScreenshot', { format: 'png', clip: { x: Math.round(at.x), y: Math.round(at.y), width: 1, height: 1, scale: 1 } });
  const { pixels } = decodePng(Buffer.from(shot.data, 'base64'));
  return [pixels[0], pixels[1], pixels[2]];
}

const near = (rgb, expected, tolerance) => rgb.every((value, at) => Math.abs(value - expected[at]) <= tolerance);
const YELLOW = [255, 227, 107];
const GREEN = [168, 230, 161];

function decodeChars(bytes) {
  const buffer = Buffer.from(bytes);
  const count = buffer.readUInt32LE(8);
  const rotation = buffer.readUInt32LE(12);
  const chars = [];
  for (let index = 0; index < count; index++) {
    const at = 16 + index * 24;
    const quad = [];
    for (let corner = 0; corner < 8; corner++) {
      quad.push(buffer.readUInt16LE(at + 4 + corner * 2) / 65535);
    }
    chars.push({ ch: String.fromCodePoint(buffer.readUInt32LE(at)), quad });
  }
  return { rotation, chars, text: chars.map((c) => c.ch).join('') };
}

async function waitSharp(main) {
  return waitFor(main, '(async () => { const i = await window.musaeumApp.inspectFixed(); return i && i.sharp && !i.framePending && !i.moving ? i : null; })()', {
    timeoutMs: 60_000,
    what: 'the pages to be drawn sharp',
  });
}

async function layerOf(main, page) {
  await waitFor(main, `(async () => (await window.musaeumApp.inspectFixed())?.charPages?.includes(${page}))()`, {
    timeoutMs: 20_000,
    what: `the compositor to have page ${page}'s characters`,
  });
  const bytes = await waitFor(main, `window.musaeumApp.pageChars(${page})`, { timeoutMs: 20_000, what: `page ${page}'s characters` });
  return decodeChars(bytes);
}

/** Where a point of a page, in its unrotated frame, is on the screen now, in CSS pixels. */
async function screenOf(main, page, u, v, rotation) {
  const inspection = await main.evaluate('window.musaeumApp.inspectFixed()');
  const placed = inspection.pages.find((candidate) => candidate.page === page);
  if (placed === undefined) {
    return null;
  }
  const [x, y] = rotation === 1 ? [1 - v, u] : rotation === 2 ? [1 - u, 1 - v] : rotation === 3 ? [v, 1 - u] : [u, v];
  const { canvas, dpr } = await main.evaluate('window.musaeumApp.rects()');
  const width = placed.widthPt * (placed.scale / SCALE_ONE);
  const height = placed.heightPt * (placed.scale / SCALE_ONE);
  return { x: canvas.x + (placed.x + x * width) / dpr, y: canvas.y + (placed.y + y * height) / dpr };
}

const centerOf = (quad) => [(quad[0] + quad[2] + quad[4] + quad[6]) / 4, (quad[1] + quad[3] + quad[5] + quad[7]) / 4];

/** The screen points just before the first character of `needle` and just after its last. */
async function aim(main, page, needle) {
  const layer = await layerOf(main, page);
  const start = layer.text.indexOf(needle);
  if (start < 0) {
    throw new Error(`"${needle}" is not on page ${page}: ${layer.text.slice(0, 200)}`);
  }
  const end = start + needle.length - 1;
  const a = await screenOf(main, page, ...centerOf(layer.chars[start].quad), layer.rotation);
  const b = await screenOf(main, page, ...centerOf(layer.chars[end].quad), layer.rotation);
  const step = { x: (b.x - a.x) / Math.max(1, needle.length - 1), y: (b.y - a.y) / Math.max(1, needle.length - 1) };
  return {
    layer,
    from: { x: a.x - step.x * 0.45, y: a.y - step.y * 0.45 },
    to: { x: b.x + step.x * 0.45, y: b.y + step.y * 0.45 },
  };
}

/** The screen point between two words of `needle` on a page: paper, under a highlight if any. */
async function gapIn(main, page, layer, needle) {
  const start = layer.text.indexOf(needle);
  const space = layer.text.indexOf(' ', start + 1);
  const [u0, v0] = centerOf(layer.chars[space - 1].quad);
  const [u1, v1] = centerOf(layer.chars[space + 1].quad);
  return screenOf(main, page, (u0 + u1) / 2, (v0 + v1) / 2, layer.rotation);
}

async function selectionNow(main, kind) {
  return waitFor(main, `(() => { const s = window.musaeumApp.reader()?.selection; return s && s.kind === '${kind}' && !s.reading ? s : null; })()`, {
    timeoutMs: 8000,
    what: `a ${kind} selection`,
  });
}

async function goTo(main, page) {
  await main.evaluate(`window.musaeumApp.goToPage(${page})`);
  await waitFor(main, `window.musaeumApp.reader()?.status?.page === ${page}`, { timeoutMs: 10_000, what: `page ${page + 1}` });
  await waitSharp(main);
}

/** One of the actions, as a button of `surface` runs it, with the path the dialog would give. */
async function exchange(main, surface, action, path, generatedAt) {
  return waitFor(main, `window.musaeumApp.exchange(${JSON.stringify(surface)}, ${JSON.stringify(action)}, ${JSON.stringify(path)}, ${generatedAt ?? 'undefined'})`, {
    timeoutMs: 300_000,
    what: `${surface} ${action}`,
  });
}

const statusOf = (main, id) => main.evaluate(`document.querySelector('#${id}-status')?.textContent ?? null`);

/** A drag selection on a fixed page, marked with a button of the bar. */
async function markText(main, page, needle, button, count, note) {
  const target = await aim(main, page, needle);
  await drag(main, target.from, target.to);
  const selection = await selectionNow(main, 'fixed');
  await click(main, button);
  if (note !== undefined) {
    await waitFor(main, "document.querySelector('#note-text') !== null", { timeoutMs: 8000, what: 'the note editor' });
    await written(main, count, 'the note, before its text');
    await typeInto(main, '#note-text', note);
    await click(main, '#note-save');
    await waitFor(main, `(() => { const a = window.musaeumApp.reader().annotations; return a.find((e) => e.note === ${JSON.stringify(note)} && !e.pending) ? a : null; })()`, { timeoutMs: 10_000, what: 'the note to be kept' });
  } else {
    await written(main, count, `the mark on ${needle.slice(0, 30)}`);
  }
  return selection;
}

/** The items of a JSON-LD file, by id, without what differs from one export to another. */
/** Where two values parsed from JSON differ, by path, each side cut short: at most 20 places. */
function differences(a, b, path = '', found = []) {
  if (found.length >= 20) {
    return found;
  }
  const bothObjects = a !== null && b !== null && typeof a === 'object' && typeof b === 'object' && Array.isArray(a) === Array.isArray(b);
  if (bothObjects) {
    for (const key of new Set([...Object.keys(a), ...Object.keys(b)])) {
      differences(a[key], b[key], `${path}/${key}`, found);
    }
  } else if (JSON.stringify(a) !== JSON.stringify(b)) {
    const cut = (value) => (value === undefined ? 'absent' : JSON.stringify(value).slice(0, 240));
    found.push({ path, before: cut(a), after: cut(b) });
  }
  return found;
}

function itemsOf(path) {
  const file = JSON.parse(readFileSync(path, 'utf8'));
  return file.first.items.slice().sort((a, b) => a.id.localeCompare(b.id));
}

const NOTE = '**Importante**: ação, coração e pão — “aspas” → ✓ e `código`';

// --- The run ---

async function run(work) {
  const generated = join(work, 'gerados');
  mkdirSync(generated, { recursive: true });
  execFileSync(join(options.build, 'gen-fixtures.exe'), ['--only', 'annot', '--out', generated], { windowsHide: true });
  execFileSync(join(options.build, 'gen-fixtures.exe'), ['--only', 'ocr', '--out', generated], { windowsHide: true });
  const truth = JSON.parse(readFileSync(join(generated, 'ocr-native.json'), 'utf8'));
  const pdf = annotatedPdf({ id: PDF_ID });
  const epub = annotatedEpub();
  const shelf = join(work, 'estante');
  const clean = join(work, 'estante-limpa');
  const second = join(work, 'estante-segunda');
  const out = join(work, 'saida');
  for (const folder of [shelf, clean, second, out]) {
    mkdirSync(folder, { recursive: true });
  }
  writeFileSync(join(shelf, 'anotar.pdf'), pdf.v1);
  writeFileSync(join(shelf, 'anotar.epub'), epub.v1);
  copyFileSync(join(generated, 'annot-streams.pdf'), join(shelf, 'girado.pdf'));
  execFileSync(join(options.build, 'visual-check.exe'), ['scan', '--document', join(generated, 'ocr-native.pdf'), '--out', join(shelf, 'escaneado.pdf'), '--dpi', '300', '--noise', '1', '--seed', '1'], {
    stdio: 'ignore',
    windowsHide: true,
  });
  for (const name of readdirSync(shelf)) {
    copyFileSync(join(shelf, name), join(clean, name));
  }
  writeFileSync(join(second, 'anotar.pdf'), pdf.v2);
  writeFileSync(join(second, 'anotar.epub'), epub.v2);
  const shelfBefore = listTree(shelf);
  const files = {
    all: join(out, 'biblioteca.jsonld'),
    book: join(out, 'livro.jsonld'),
    chosen: join(out, 'escolhidos.jsonld'),
    readerMd: join(out, 'relatorio-leitor.md'),
    readerPdf: join(out, 'relatorio-leitor.pdf'),
    readerPdfAgain: join(out, 'relatorio-leitor-2.pdf'),
    workMd: join(out, 'relatorio-obra.md'),
    workPdf: join(out, 'relatorio-obra.pdf'),
    batchMd: join(out, 'relatorio-escolhidos.md'),
    cleanAll: join(out, 'limpa.jsonld'),
  };

  // --- 1. The first library, annotated in the window ---
  let session = await launch(app, ['--library', join(work, 'biblioteca'), '--e2e', '--width', '1400', '--height', '950'], 'window.musaeumApp !== undefined');
  let main = session.cdp;
  let originals = null;
  try {
    await firstScreen(main);
    await main.evaluate(`window.musaeumApp.addRoot(${JSON.stringify(shelf)})`);
    const books = await rows(main, 4);
    const ids = Object.fromEntries(['anotar.pdf', 'anotar.epub', 'girado.pdf', 'escaneado.pdf'].map((name) => [name, rowNamed(books, name)]));

    // The text PDF: a highlight with a note in Markdown, a highlight and an underline.
    await openBook(main, ids['anotar.pdf'].documentId);
    await waitSharp(main);
    await markText(main, 0, pdf.targets.kept, '#mark-note', 1, NOTE);
    await goTo(main, 1);
    await markText(main, 1, pdf.targets.edited, '#mark-color-green', 2);
    await goTo(main, 2);
    await markText(main, 2, pdf.targets.removed, '#mark-underline', 3);
    const pdfMade = await reader(main);
    step('three annotations on the text PDF, one with a note in Markdown and accents', pdfMade.annotations.length === 3 && pdfMade.annotations.some((entry) => entry.note === NOTE), {
      made: pdfMade.annotations.map((entry) => ({ kind: entry.kind, color: entry.color, quote: entry.quote })),
    });

    // The reader's panel offers the four actions, for every kind of book.
    await click(main, '#annotations-toggle');
    const offered = await waitFor(main, "['#exchange-export-jsonld', '#exchange-import-jsonld', '#exchange-report-md', '#exchange-report-pdf'].every((id) => document.querySelector(id) !== null)", { timeoutMs: 8000, what: 'the actions' }).catch(() => false);
    step('the reader’s annotations panel offers JSON-LD both ways and the two reports', offered === true);

    // One book's JSON-LD, and its reports, from the reader.
    const bookOut = await exchange(main, 'reader', 'export-jsonld', files.book);
    step('the reader writes the book’s JSON-LD', bookOut.ok && bookOut.done?.result.annotations === 3 && existsSync(files.book), { bookOut, status: await statusOf(main, 'exchange') });
    const readerMd = await exchange(main, 'reader', 'report-md', files.readerMd, GENERATED_AT);
    const markdown = existsSync(files.readerMd) ? readFileSync(files.readerMd, 'utf8') : '';
    const order = [pdf.targets.kept, pdf.targets.edited, pdf.targets.removed].map((line) => markdown.indexOf(line.replace(/[\\`*_[\]<>#|~$]/g, (c) => `\\${c}`)));
    step(
      'the Markdown report: front matter, the work, the annotations in reading order, the note’s Markdown as written',
      readerMd.ok &&
        markdown.startsWith('---\ntitle: "anotar"\n') &&
        markdown.includes('\n# anotar\n') &&
        order.every((at) => at > 0) &&
        order[0] < order[1] &&
        order[1] < order[2] &&
        markdown.includes(NOTE.replace('“aspas” → ✓', '“aspas” → ✓')) &&
        markdown.includes('**Sublinhado** · ') &&
        markdown.includes(' · p. 3 · ') &&
        markdown.endsWith(`pelo Musaeum ${VERSION}.*\n`),
      { order, head: markdown.slice(0, 600) },
    );
    const readerPdf = await exchange(main, 'reader', 'report-pdf', files.readerPdf, GENERATED_AT);
    const again = await exchange(main, 'reader', 'report-pdf', files.readerPdfAgain, GENERATED_AT);
    const pages = readerPdf.ok ? await main.evaluate(`window.musaeumApp.pdfText(${JSON.stringify(files.readerPdf)})`) : [];
    const text = pages.join('\n').replace(/\s+/g, ' ');
    step(
      'the PDF report: PDFium reads its text, every accent, the note, the page numbers',
      readerPdf.ok &&
        pages.length === readerPdf.done.result.pages &&
        [pdf.targets.kept, pdf.targets.edited, pdf.targets.removed].every((line) => text.includes(line)) &&
        text.includes('Importante: ação, coração e pão — “aspas” → ✓ e código') &&
        text.includes(`página 1 de ${pages.length}`),
      { result: readerPdf.done?.result ?? readerPdf.error, text: text.slice(0, 800) },
    );
    step('the same report twice is the same PDF, byte for byte', again.ok && Buffer.compare(readFileSync(files.readerPdf), readFileSync(files.readerPdfAgain)) === 0);
    const inside = await exchange(main, 'reader', 'report-md', join(shelf, 'dentro.md'));
    step('nothing is written inside a folder of the library', !inside.ok && /pasta da biblioteca/.test(inside.error) && !existsSync(join(shelf, 'dentro.md')), { inside });
    await back(main);

    // The turned page: a highlight on the page shown turned by its /Rotate 90.
    await openBook(main, ids['girado.pdf'].documentId);
    await goTo(main, 1);
    await markText(main, 1, 'a ação do coração', '#mark-color-blue', 1);
    await back(main);

    // The scan: a box over three lines of prose, its quote read by the OCR.
    await openBook(main, ids['escaneado.pdf'].documentId);
    await waitSharp(main);
    const boxed = truth.lines.filter((line) => line.page === 0 && line.kind === 'body' && line.block === 2 && line.text !== '').slice(0, 3);
    const size = truth.page;
    const corner = async ([x, y]) => screenOf(main, 0, x / size[0], 1 - y / size[1], 0);
    await click(main, 'button[aria-label="Caixa retangular"]');
    await waitSharp(main);
    await drag(main, await corner([64, boxed[0].start[1] + boxed[0].size * 0.9]), await corner([552, boxed[2].start[1] - boxed[2].size * 0.4]));
    // The text under a box on a scan is read by the OCR, in the background.
    await waitFor(main, "(() => { const s = window.musaeumApp.reader()?.selection; return s && s.kind === 'region' && !s.reading ? s : null; })()", {
      timeoutMs: 120_000,
      what: 'the text under the box to be read',
    });
    await click(main, '#mark-color-purple');
    const ocr = await waitFor(main, "(() => { const a = window.musaeumApp.reader()?.annotations ?? []; return a.length === 1 && a[0].quote !== null && a[0].quoteSource === 'ocr' ? a[0] : null; })()", {
      timeoutMs: 180_000,
      what: 'the quote read by the OCR',
    }).catch(() => null);
    step('a box on the scan, its quote read by the OCR', ocr !== null && boxed.every((line) => ocr.quote.includes(line.text.split(' ')[0])), { quote: ocr?.quote, lines: boxed.map((line) => line.text) });
    await click(main, 'button[aria-label="Selecionar texto"]');
    await back(main);

    // What the OCR read for the box reaches the full-text index without waiting for a change in
    // the folders: the scan is read again, and a word of its quote finds it.
    const indexedAfterOcr = await waitFor(main, '(async () => { const s = await window.musaeumApp.fulltextStatus(); return s.pending === 0 && s.indexed >= 3 ? s : null; })()', {
      timeoutMs: 180_000,
      what: 'the text index, with what the OCR read',
    }).catch(() => null);
    const ocrWord = boxed[1].text
      .split(/\s+/)
      .map((word) => word.replace(/[^\p{L}\p{N}]/gu, ''))
      .reduce((longest, word) => (word.length > longest.length ? word : longest), '');
    const ocrFound = indexedAfterOcr !== null ? await main.evaluate(`window.musaeumApp.fulltextSearch(${JSON.stringify(ocrWord)})`) : null;
    step(
      'what the OCR read on the scan reaches the full-text index, and a word of it finds the scan',
      ocrFound !== null && ocrFound.books.some((book) => book.documents.includes(ids['escaneado.pdf'].documentId)),
      { word: ocrWord, status: indexedAfterOcr ?? (await main.evaluate('window.musaeumApp.fulltextStatus()')), books: ocrFound?.books.map((book) => book.documents) },
    );

    // The EPUB: three notes, where the full-text index finds their paragraphs.
    await openBook(main, ids['anotar.epub'].documentId);
    await waitFor(main, '(async () => { const s = await window.musaeumApp.fulltextStatus(); return s.pending === 0 && s.indexed >= 3 ? s : null; })()', { timeoutMs: 300_000, what: 'the text index' });
    const flowNotes = [];
    for (const [target, length, words] of [
      [epub.targets.kept, 60, 'Capítulo 1, parágrafo 3'],
      [epub.targets.edited, 70, 'Um parágrafo que a segunda edição muda'],
      [epub.targets.removed, 60, 'Este parágrafo some na segunda edição'],
    ]) {
      const found = await main.evaluate(`window.musaeumApp.fulltextSearch(${JSON.stringify(`"${words}"`)})`);
      const unit = found.books.find((book) => book.documents.includes(ids['anotar.epub'].documentId))?.units[0];
      const place = await main.evaluate(`window.musaeumApp.flowPlace(${ids['anotar.epub'].documentId}, ${unit.index}, ${unit.position}, ${length})`);
      const note = await main.evaluate(`window.musaeumApp.annotateFlow(${JSON.stringify(place.start)}, ${JSON.stringify(place.end)}, ${JSON.stringify(`Nota em *Markdown*: ${words.slice(0, 12)}`)})`);
      flowNotes.push({ quote: note.annotation.quote, expected: target.slice(0, length).replace(/\s+/g, ' ').trim() });
    }
    step('three notes on the EPUB, each quoting its paragraph', flowNotes.every((entry) => entry.quote === entry.expected), { flowNotes });
    await back(main);

    // The library's menu: every book's annotations, in one file.
    await click(main, '#library-annotations');
    const menu = await waitFor(main, "(() => { const items = [...document.querySelectorAll('[role=menuitem]')].map((n) => n.textContent); return items.length === 2 ? items : null; })()", { timeoutMs: 8000, what: 'the menu' }).catch(() => null);
    await press(main, 'Escape');
    step('the library’s toolbar has a menu for every book’s annotations', menu !== null && menu.some((item) => item.includes('Exportar')) && menu.some((item) => item.includes('Importar')), { menu });
    const all = await exchange(main, 'library', 'export-jsonld', files.all);
    const file = existsSync(files.all) ? JSON.parse(readFileSync(files.all, 'utf8')) : null;
    const items = file?.first?.items ?? [];
    const types = new Set(items.flatMap((item) => item.target.flatMap((target) => (target.selector ?? []).map((selector) => selector.type))));
    step(
      'the whole library as W3C Web Annotations: every annotation, its UUID, its note in Markdown, its selectors',
      all.ok &&
        all.done.result.annotations === 8 &&
        all.done.result.documents === 4 &&
        file['@context'][0] === 'http://www.w3.org/ns/anno.jsonld' &&
        file.type === 'AnnotationCollection' &&
        items.length === 8 &&
        items.every((item) => /^urn:uuid:[0-9a-f-]{36}$/.test(item.id) && item.target.every((target) => /^urn:blake3:[0-9a-f]{64}$/.test(target.source.id))) &&
        items.some((item) => item.body?.value === NOTE && item.body.format === 'text/markdown') &&
        items.some((item) => item['musaeum:quoteSource'] === 'ocr') &&
        ['TextQuoteSelector', 'TextPositionSelector', 'RangeSelector', 'FragmentSelector', 'musaeum:PageCharRangeSelector', 'musaeum:PageQuadsSelector', 'musaeum:PageRegionSelector'].every((type) => types.has(type)),
      { result: all.done?.result ?? all.error, types: [...types] },
    );
    originals = items;

    // One work's details: its report, with the EPUB's chapters.
    await click(main, `[data-document="${ids['anotar.epub'].documentId}"]`);
    await waitFor(main, "document.querySelector('#work-exchange-report-md') !== null", { timeoutMs: 15_000, what: 'the details of the work' });
    const workMd = await exchange(main, 'details', 'report-md', files.workMd, GENERATED_AT);
    const workText = existsSync(files.workMd) ? readFileSync(files.workMd, 'utf8') : '';
    const at = (needle) => workText.indexOf(needle);
    step(
      'one work’s report, from its details: its chapters, each annotation under the chapter it is in',
      workMd.ok && at('## Primeiro') > 0 && at('## Terceiro') > at('## Primeiro') && at('Capítulo 1, parágrafo 3') > at('## Primeiro') && at('Este parágrafo some') > at('## Primeiro') && at('Este parágrafo some') < at('## Terceiro') && at('Um parágrafo que a segunda edição muda') > at('## Terceiro'),
      { head: workText.slice(0, 900) },
    );
    const workPdf = await exchange(main, 'details', 'report-pdf', files.workPdf, GENERATED_AT);
    const workPages = workPdf.ok ? await main.evaluate(`window.musaeumApp.pdfText(${JSON.stringify(files.workPdf)})`) : [];
    step('and as a PDF', workPdf.ok && workPages.join(' ').includes('Livro para anotar') && workPages.join(' ').includes('Terceiro'), { result: workPdf.done?.result ?? workPdf.error });

    // Every book chosen: one report of them all, and their JSON-LD.
    await press(main, 'a', 2);
    await waitFor(main, "document.querySelector('#batch-exchange-report-md') !== null", { timeoutMs: 15_000, what: 'the details of the books chosen' });
    const batchMd = await exchange(main, 'batch', 'report-md', files.batchMd, GENERATED_AT);
    const batchText = existsSync(files.batchMd) ? readFileSync(files.batchMd, 'utf8') : '';
    step(
      'several works in one report: one section each',
      batchMd.ok && batchMd.done.result.works === 4 && batchText.includes('\n# Relatório de estudo\n') && ['## anotar', '## Livro para anotar', '## girado', '## escaneado'].every((heading) => batchText.includes(`\n${heading}\n`)),
      { result: batchMd.done?.result ?? batchMd.error },
    );
    const chosen = await exchange(main, 'batch', 'export-jsonld', files.chosen);
    step('several works in one JSON-LD', chosen.ok && chosen.done.result.annotations === 8 && chosen.done.result.works === 4, { result: chosen.done?.result ?? chosen.error });
    await press(main, 'Escape');

    // The same file again: nothing new.
    const twice = await exchange(main, 'library', 'import-jsonld', files.all);
    step('the same file brought in again adds nothing', twice.ok && twice.done.result.added === 0 && twice.done.result.already === 8, { result: twice.done?.result ?? twice.error });

    // Hostile and broken files: refused, nothing changes.
    const hostile = {
      'contexto-remoto.jsonld': JSON.stringify({ ...file, '@context': 'https://evil.example/context.jsonld' }),
      'aninhado.jsonld': `${'['.repeat(100)}${']'.repeat(100)}`,
      'chave-dupla.jsonld': readFileSync(files.all, 'utf8').replace('"label": ', '"label": "x", "label": '),
      'nao-json.jsonld': '<?xml version="1.0"?><!DOCTYPE x [<!ENTITY y SYSTEM "file:///c:/windows/win.ini">]><x>&y;</x>',
      'total-mentiroso.jsonld': JSON.stringify({ ...file, total: 99 }),
    };
    const refusals = {};
    for (const [name, content] of Object.entries(hostile)) {
      const path = join(work, name);
      writeFileSync(path, content);
      refusals[name] = await exchange(main, 'library', 'import-jsonld', path);
    }
    const after = await exchange(main, 'library', 'import-jsonld', files.all);
    step(
      'hostile and broken files are refused, and nothing changes',
      Object.values(refusals).every((outcome) => !outcome.ok) && after.ok && after.done.result.added === 0 && after.done.result.already === 8,
      { refusals: Object.fromEntries(Object.entries(refusals).map(([name, outcome]) => [name, outcome.error ?? 'accepted'])) },
    );
  } finally {
    await closeApp(session);
  }
  step('the books of the first library are the same, byte for byte', JSON.stringify(listTree(shelf)) === JSON.stringify(shelfBefore));
  const leftovers = readdirSync(out).filter((name) => name.endsWith('.tmp'));
  step('no temporary file is left beside what was written', leftovers.length === 0, { files: readdirSync(out) });

  // --- 5. A clean library with the same files ---
  session = await launch(app, ['--library', join(work, 'biblioteca-limpa'), '--e2e', '--width', '1400', '--height', '950'], 'window.musaeumApp !== undefined');
  main = session.cdp;
  try {
    await firstScreen(main);
    await main.evaluate(`window.musaeumApp.addRoot(${JSON.stringify(clean)})`);
    const books = await rows(main, 4);
    const brought = await exchange(main, 'library', 'import-jsonld', files.all);
    const result = brought.done?.result;
    step(
      'into a clean library, every annotation comes back, each found where it was',
      brought.ok && result.added === 8 && result.already === 0 && result.unmatched.length === 0 && result.books.length === 4 && result.books.every((book) => book.exact === book.added),
      { result: result ?? brought.error },
    );
    const cleanAll = await exchange(main, 'library', 'export-jsonld', files.cleanAll);
    const restored = cleanAll.ok ? itemsOf(files.cleanAll) : [];
    const before = originals.slice().sort((a, b) => a.id.localeCompare(b.id));
    const differing = before
      .map((item, index) => ({ id: item.id, differences: differences(item, restored[index]) }))
      .filter((entry) => entry.differences.length > 0);
    step('and the clean library writes back the very same annotations: nothing lost, nothing changed', restored.length === 8 && differing.length === 0, { differing });
    const again = await exchange(main, 'library', 'import-jsonld', files.all);
    step('brought in again, nothing new', again.ok && again.done.result.added === 0 && again.done.result.already === 8);

    // Drawn where they were: the green highlight on the second page.
    await openBook(main, rowNamed(books, 'anotar.pdf').documentId);
    await goTo(main, 1);
    const layer = await layerOf(main, 1);
    const drawn = await pixel(main, await gapIn(main, 1, layer, pdf.targets.edited));
    step('and drawn where they were', near(drawn, GREEN, 24), { drawn });
    await back(main);

    // The PDF report opened in Musaeum: its pages drawn, the band of the first annotation yellow.
    await main.evaluate(`window.musaeumApp.addRoot(${JSON.stringify(out)})`);
    // The report and its twin written to test the bytes are the same bytes: one book, either path.
    const listed = await waitFor(main, "(() => { const r = window.musaeumApp.rows(); const row = r.rows.find((b) => /\\\\relatorio-leitor(-2)?\\.pdf$/.test(b.path)); const i = window.musaeumApp.library(); return row && row.blake3 !== null && i && !i.indexing ? row : null; })()", {
      timeoutMs: 120_000,
      what: 'the report in the library',
    }).catch(async (error) => {
      const seen = await main.evaluate("(() => { const r = window.musaeumApp.rows(); return { total: r.total, paths: r.rows.map((b) => [b.path, b.blake3 !== null]), library: window.musaeumApp.library() }; })()");
      throw new Error(`${error.message}: ${JSON.stringify(seen)}`);
    });
    await openBook(main, listed.documentId);
    const sharp = await waitSharp(main);
    const placed = sharp.pages.find((page) => page.page === 0);
    const top = await screenOf(main, 0, (64 + 1.8) / 595.276, 0, 0);
    const height = (placed.heightPt * (placed.scale / SCALE_ONE)) / (await main.evaluate('window.musaeumApp.rects()')).dpr;
    const strip = await main.send('Page.captureScreenshot', { format: 'png', clip: { x: Math.round(top.x), y: Math.round(top.y), width: 1, height: Math.round(height), scale: 1 } });
    const column = decodePng(Buffer.from(strip.data, 'base64')).pixels;
    let yellow = 0;
    for (let index = 0; index + 2 < column.length; index += 4) {
      if (near([column[index], column[index + 1], column[index + 2]], YELLOW, 24)) {
        yellow++;
      }
    }
    step('opened in Musaeum, the report’s first page is drawn with the band of the highlight in its color', yellow >= 8, { yellow, pages: sharp.pages.length });
    await back(main);
  } finally {
    await closeApp(session);
  }

  // --- 6. The second editions ---
  session = await launch(app, ['--library', join(work, 'biblioteca-segunda'), '--e2e', '--width', '1400', '--height', '950'], 'window.musaeumApp !== undefined');
  main = session.cdp;
  try {
    await firstScreen(main);
    await main.evaluate(`window.musaeumApp.addRoot(${JSON.stringify(second)})`);
    const books = await rows(main, 2);
    const brought = await exchange(main, 'library', 'import-jsonld', files.all);
    const result = brought.done?.result;
    const byTitle = Object.fromEntries((result?.books ?? []).map((book) => [book.title, book]));
    const one = (book) => book !== undefined && book.added === 3 && book.exact === 1 && book.approximate === 1 && book.orphan === 1 && book.pending === 0;
    step(
      'into the second editions: each annotation taken by its work and found again, exact, approximate or an orphan, none lost',
      brought.ok && result.added === 6 && one(byTitle.anotar) && one(byTitle['Livro para anotar']),
      { result: result ?? brought.error },
    );
    step(
      'the works the library does not have are named, with how many annotations wait for them',
      brought.ok && result.unmatched.length === 2 && result.unmatched.every((work) => work.annotations === 1) && result.unmatched.some((work) => work.title === 'girado') && result.unmatched.some((work) => work.title === 'escaneado'),
      { unmatched: result?.unmatched },
    );
    await openBook(main, rowNamed(books, 'anotar.pdf').documentId);
    const shown = await waitFor(main, '(() => { const a = window.musaeumApp.reader()?.annotations ?? []; return a.length === 3 ? a : null; })()', { timeoutMs: 30_000, what: 'the annotations of the second edition' });
    const state = (quote) => shown.find((entry) => entry.quote === quote)?.anchorState;
    step(
      'in the second edition, the kept line is exact, the edited one approximate, the removed one an orphan with its quote',
      state(pdf.targets.kept) === 'exact' && state(pdf.targets.edited) === 'approximate' && state(pdf.targets.removed) === 'orphan',
      { shown: shown.map((entry) => ({ quote: entry.quote, state: entry.anchorState })) },
    );
    await back(main);
  } finally {
    await closeApp(session);
  }
}

/** Real books with annotations of their own, copied: in, then out as JSON-LD and as a report. */
async function runReal(work) {
  const books = options.real.filter((path) => existsSync(path));
  if (books.length === 0) {
    console.log('  (no real books to read)');
    return;
  }
  const shelf = join(work, 'reais');
  const out = join(work, 'reais-saida');
  mkdirSync(shelf, { recursive: true });
  mkdirSync(out, { recursive: true });
  const sources = books.map((source, index) => {
    const before = statSync(source, { bigint: true });
    const name = `real-${index}.pdf`;
    copyFileSync(source, join(shelf, name));
    return { source, name, before: { size: String(before.size), modified: String(before.mtimeNs) } };
  });
  const session = await launch(app, ['--library', join(work, 'biblioteca-real'), '--e2e', '--width', '1400', '--height', '950'], 'window.musaeumApp !== undefined');
  const main = session.cdp;
  try {
    await firstScreen(main);
    await main.evaluate(`window.musaeumApp.addRoot(${JSON.stringify(shelf)})`);
    const listed = await rows(main, sources.length);
    for (const book of sources) {
      const title = book.source.split('\\').pop();
      const row = rowNamed(listed, book.name);
      await openBook(main, row.documentId);
      const brought = await waitFor(main, `window.musaeumApp.pdf('import-pdf', ${JSON.stringify(join(shelf, book.name))})`, { timeoutMs: 180_000, what: 'the PDF annotations' });
      const quotes = (await reader(main)).annotations.map((entry) => entry.quote).filter((quote) => quote !== null && quote.length > 12);
      const jsonld = join(out, `${book.name}.jsonld`);
      const pdfReport = join(out, `${book.name}.relatorio.pdf`);
      const exported = await exchange(main, 'reader', 'export-jsonld', jsonld);
      const made = await exchange(main, 'reader', 'report-pdf', pdfReport, GENERATED_AT);
      const text = made.ok ? (await main.evaluate(`window.musaeumApp.pdfText(${JSON.stringify(pdfReport)})`)).join(' ').replace(/\s+/g, ' ') : '';
      const sample = quotes.slice(0, 3).map((quote) => quote.replace(/\s+/g, ' ').slice(0, 40));
      step(
        `${title}: its own annotations go out as JSON-LD and as a report whose text PDFium reads`,
        brought.ok && exported.ok && exported.done.result.annotations === brought.report.added && made.ok && sample.every((quote) => text.includes(quote)),
        { added: brought.report?.added, exported: exported.done?.result ?? exported.error, report: made.done?.result ?? made.error, sample },
      );
      const again = await exchange(main, 'reader', 'import-jsonld', jsonld);
      step(`${title}: its JSON-LD brought back adds nothing`, again.ok && again.done.result.added === 0 && again.done.result.already === brought.report.added, { again: again.done?.result ?? again.error });
      await back(main);
      const after = statSync(book.source, { bigint: true });
      step(`${title}: the book in the collection is as it was`, String(after.size) === book.before.size && String(after.mtimeNs) === book.before.modified);
    }
  } finally {
    await closeApp(session);
  }
}

async function main() {
  if (!existsSync(app)) {
    throw new Error(`${app} is missing: run npm run build:app first`);
  }
  if (options.requireIdle > 0) {
    const idle = idleSeconds();
    if (idle < options.requireIdle) {
      throw new Interrupted(`someone used the machine ${idle} s ago; this opens windows, so it waits for ${options.requireIdle} s of quiet`);
    }
    quietSince = idle;
  }
  const work = mkdtempSync(join(tmpdir(), 'musaeum-e2e-exchange-'));
  try {
    console.log('fixtures');
    await run(work);
    console.log('real books, copied');
    await runReal(work);
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
