#!/usr/bin/env node
// Phase 7, Part 1, in the real window, driven through the WebView2's debugging protocol with the
// input a person gives: a PDF's own annotations, out to a copy, back in, and through XFDF.
//
//   node tools/e2e-pdf-annotations.mjs [--build target/release] [--out FILE.json] [--require-idle SECONDS]
//                                      [--real BOOK.pdf+BOOK.pdf]
//
// - A book (gen-fixtures' annot-streams.pdf: object and cross-reference streams, an update before
//   ours, pages turned and boxed every way, a highlight, a popup, a link and a sticky note of its
//   own) is annotated as a person does: a highlight, a note in Portuguese with characters outside
//   PDFDocEncoding, an underline, a box, a stroke; a highlight on the page turned by its /Rotate 90,
//   and a box on the page shown turned by /Rotate 270 whose text is drawn turned to read upright.
// - "Exportar PDF com anotações (cópia)" writes the copy: the book's bytes, every one, then the
//   update. The book is the same byte for byte; nothing is left beside the copy.
// - The copy's objects as PDFium reads them: every annotation by its /NM, of its kind, with its note
//   and color, and the book's own annotations still there.
// - The copy opened in Musaeum: its annotations are drawn by PDFium where they were made, the
//   colors read off the screen, on the upright page and on the turned one.
// - Importing the copy's annotations into the copy and into the book: the book's own come in, ours
//   are known; importing again brings nothing.
// - XFDF out of the book, into a clean library with the same file: every annotation comes back with
//   its UUID, at its place, with its kind, color and note; a hostile XFDF (an external entity) is
//   refused and the file it names is never read.
//
// - Books of the real collection that already hold annotations of their own, copied into the
//   temporary folder (the collection is only read): those annotations come into the library, as many
//   as the PDF has of the kinds kept, and a copy exported afterwards does not write them twice.
//
// Everything lives in a temporary folder, removed at the end. The run waits for the machine to be
// free and stops as soon as someone uses it (exit code 3).

import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

import { click, closeApp, idleSeconds, launch, press, typeInto, waitFor } from './lib/cdp.mjs';
import { decodePng } from './lib/png.mjs';

const root = resolve(import.meta.dirname, '..');
const SCALE_ONE = 4096;

function parseArguments(argv) {
  const options = {
    build: join(root, 'target', 'release'),
    out: join(root, 'bench-results', 'e2e-pdf-annotations', 'report.json'),
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

async function waitForLibrary(main) {
  await waitFor(main, "window.musaeumApp.view() === 'library' && document.querySelector('.library') !== null", { timeoutMs: 20_000, what: 'the library' });
}

async function rowFor(main, name) {
  return waitFor(
    main,
    `(() => { const r = window.musaeumApp.rows(); const i = window.musaeumApp.library(); const row = r.rows.find((b) => b.path.endsWith(${JSON.stringify('\\' + name)})); return row && row.blake3 !== null && i && !i.indexing ? row : null; })()`,
    { timeoutMs: 120_000, what: `the book ${name}` },
  );
}

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

async function clicks(main, at, count) {
  checkStillQuiet();
  await mouse(main, 'mouseMoved', at, { buttons: 0 });
  for (let n = 1; n <= count; n++) {
    await mouse(main, 'mousePressed', at, { buttons: 1, clickCount: n });
    await mouse(main, 'mouseReleased', at, { buttons: 0, clickCount: n });
    await delay(60);
  }
}

async function pixel(main, at) {
  const shot = await main.send('Page.captureScreenshot', { format: 'png', clip: { x: Math.round(at.x), y: Math.round(at.y), width: 1, height: 1, scale: 1 } });
  const { pixels } = decodePng(Buffer.from(shot.data, 'base64'));
  return [pixels[0], pixels[1], pixels[2]];
}

const near = (rgb, expected, tolerance) => rgb.every((value, at) => Math.abs(value - expected[at]) <= tolerance);
const YELLOW = [255, 227, 107];
const YELLOW_INK = [217, 153, 0];
const GREEN = [168, 230, 161];
const BLUE = [153, 201, 252];
const PURPLE = [199, 181, 252];

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

/**
 * The screen points just before the first character of `needle` and just after its last, along
 * the line as shown, whichever way the page and its text are turned.
 */
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
    start,
    end: end + 1,
    from: { x: a.x - step.x * 0.45, y: a.y - step.y * 0.45 },
    to: { x: b.x + step.x * 0.45, y: b.y + step.y * 0.45 },
    middle: { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 },
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

/** The screen point a little under the middle of `needle`: where an underline is drawn. */
async function underOf(main, page, layer, needle) {
  const start = layer.text.indexOf(needle);
  const middle = layer.chars[start + Math.floor(needle.length / 2)].quad;
  // Bottom edge of the glyph, then 7 % of its side further out, in the glyph's own frame.
  const t = 0.07;
  const u = (middle[6] + middle[4]) / 2 + ((middle[6] - middle[0]) + (middle[4] - middle[2])) / 2 * t;
  const v = (middle[7] + middle[5]) / 2 + ((middle[7] - middle[1]) + (middle[5] - middle[3])) / 2 * t;
  return screenOf(main, page, u, v, layer.rotation);
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

async function pdf(main, action, path) {
  return waitFor(main, `window.musaeumApp.pdf(${JSON.stringify(action)}, ${JSON.stringify(path)})`, { timeoutMs: 180_000, what: action });
}

const fullAnnotations = async (main) => (await main.evaluate('window.musaeumApp.fixedAnnotations()')).map((view) => view.annotation);

/** What of an annotation must come back the same: its kind, color, note, and where it is. */
function essence(annotation) {
  const places = annotation.segments.map((segment) => {
    const region = segment.selectors.find((selector) => selector.kind === 'page_region')?.data;
    const quads = segment.selectors.find((selector) => selector.kind === 'page_quads')?.data;
    return region !== undefined
      ? { page: region.page, tool: region.tool, quads: region.quads, clip: region.clip, path: region.path }
      : { page: quads?.page, tool: 'text', quads: quads?.quads };
  });
  return { uuid: annotation.uuid, pdfNm: annotation.pdfNm, kind: annotation.kind, color: annotation.color, note: annotation.note, quote: annotation.quote, places };
}

/** The name an annotation goes out with: the /NM another program gave it, or its UUID. */
function exportName(annotation) {
  return annotation.pdfNm !== null && !annotation.pdfNm.startsWith('musaeum:') ? annotation.pdfNm : annotation.uuid;
}

function samePlaces(a, b) {
  const numbers = (place) => [...(place.quads ?? []).flat(), ...(place.clip ?? []), ...(place.path ?? [])];
  return (
    a.length === b.length &&
    a.every((place, at) => {
      const other = b[at];
      const x = numbers(place);
      const y = numbers(other);
      const sameTool = place.tool === other.tool || (place.tool === 'lines' && other.tool === 'quads');
      return place.page === other.page && sameTool && x.length === y.length && x.every((value, index) => Math.abs(value - y[index]) <= 1e-4);
    })
  );
}

// --- The run ---

async function run(work) {
  const generated = join(work, 'gerados');
  mkdirSync(generated, { recursive: true });
  execFileSync(join(options.build, 'gen-fixtures.exe'), ['--only', 'annot', '--out', generated], { windowsHide: true });
  const shelf = join(work, 'estante');
  const cleanShelf = join(work, 'estante-limpa');
  const exported = join(work, 'exportados');
  for (const folder of [shelf, cleanShelf, exported]) {
    mkdirSync(folder, { recursive: true });
  }
  const bookPath = join(shelf, 'livro.pdf');
  copyFileSync(join(generated, 'annot-streams.pdf'), bookPath);
  copyFileSync(join(generated, 'annot-streams.pdf'), join(cleanShelf, 'livro.pdf'));
  const bookBefore = readFileSync(bookPath);
  const shelfBefore = listTree(shelf);
  const copyPath = join(exported, 'livro (anotado).pdf');
  const xfdfPath = join(work, 'livro.xfdf');

  let session = await launch(app, ['--library', join(work, 'biblioteca'), '--e2e', '--width', '1400', '--height', '950'], 'window.musaeumApp !== undefined');
  let main = session.cdp;
  let originals = [];
  try {
    await waitFor(main, "document.querySelector('.welcome') !== null || document.querySelector('.library') !== null", { what: 'the first screen' });
    await main.evaluate(`window.musaeumApp.addRoot(${JSON.stringify(shelf)})`);
    const row = await rowFor(main, 'livro.pdf');
    await openBook(main, row.documentId);
    await waitSharp(main);

    // 1. Annotated as a person does, on the first page.
    let target = await aim(main, 0, 'a ação do coração');
    await drag(main, target.from, target.to);
    await selectionNow(main, 'fixed');
    await click(main, '#mark-color-yellow');
    await written(main, 1, 'the highlight');

    target = await aim(main, 0, 'maçã, avó, pêssego');
    await drag(main, target.from, target.to);
    await selectionNow(main, 'fixed');
    await click(main, '#mark-note');
    await waitFor(main, "document.querySelector('#note-text') !== null", { timeoutMs: 8000, what: 'the note editor' });
    await written(main, 2, 'the note, before its text');
    const noteText = 'Ação, coração e pão — “aspas” → ✓ fora do PDFDocEncoding';
    await typeInto(main, '#note-text', noteText);
    await click(main, '#note-save');
    await waitFor(main, `(() => { const a = window.musaeumApp.reader().annotations; return a.find((e) => e.note === ${JSON.stringify(noteText)}) ? a : null; })()`, { timeoutMs: 10_000, what: 'the note to be kept' });

    target = await aim(main, 0, 'sublinhar');
    await clicks(main, target.middle, 2);
    await selectionNow(main, 'fixed');
    await click(main, '#mark-underline');
    await written(main, 3, 'the underline');

    const boxWord = await aim(main, 0, 'caixa');
    await click(main, 'button[aria-label="Caixa retangular"]');
    await waitSharp(main);
    await drag(main, { x: boxWord.from.x - 4, y: boxWord.from.y - 14 }, { x: boxWord.to.x + 4, y: boxWord.to.y + 12 });
    await selectionNow(main, 'region');
    await click(main, '#mark-color-green');
    await written(main, 4, 'the box');

    const strokeLine = await aim(main, 0, 'riscar à mão');
    await click(main, 'button[aria-label="Traço livre"]');
    await drag(main, strokeLine.from, strokeLine.to);
    await selectionNow(main, 'region');
    await click(main, '#mark-color-pink');
    await written(main, 5, 'the stroke');
    await click(main, 'button[aria-label="Selecionar texto"]');

    // On the page turned by its /Rotate 90: its text runs across the page, so it is shown running
    // down the screen. A highlight on it.
    await goTo(main, 1);
    const sideways = await aim(main, 1, 'a ação do coração');
    await drag(main, sideways.from, sideways.to);
    const sidewaysSelection = await selectionNow(main, 'fixed');
    await click(main, '#mark-color-blue');
    await written(main, 6, 'the highlight on the page turned a quarter');
    // On the page shown turned by /Rotate 270 from its page tree node, its text drawn turned so it
    // reads upright: a box around a word. (Selecting text that runs down the page in the file's own
    // space is not something the text tool does yet: ADR 0012, section 5.)
    await goTo(main, 3);
    const upright = await aim(main, 3, 'coração');
    await click(main, 'button[aria-label="Caixa retangular"]');
    await waitSharp(main);
    await drag(main, { x: upright.from.x - 4, y: upright.from.y - 14 }, { x: upright.to.x + 4, y: upright.to.y + 12 });
    await selectionNow(main, 'region');
    await click(main, '#mark-color-purple');
    const made = await written(main, 7, 'the box on the page shown turned');
    await click(main, 'button[aria-label="Selecionar texto"]');
    step(
      'seven annotations made in the window, two of them on pages shown turned',
      made.length === 7 && sidewaysSelection.spans[0]?.page === 1 && sidewaysSelection.text === 'a ação do coração' && made.some((entry) => entry.kind === 'note' && entry.note === noteText),
      { made: made.map((entry) => ({ kind: entry.kind, color: entry.color, quote: entry.quote, marks: entry.marks })), selected: sidewaysSelection.text },
    );
    originals = (await fullAnnotations(main)).map(essence);

    // 2. The panel offers the four actions; the export writes the copy, checked by PDFium.
    await click(main, '#annotations-toggle');
    const offered = await waitFor(main, "['#pdf-export-pdf', '#pdf-import-pdf', '#pdf-export-xfdf', '#pdf-import-xfdf'].every((id) => document.querySelector(id) !== null)", { timeoutMs: 8000, what: 'the PDF actions' }).catch(() => false);
    step('the annotations panel offers to export and import PDF and XFDF', offered === true);
    const out = await pdf(main, 'export-pdf', copyPath);
    const status = await main.evaluate("document.querySelector('.pdf-exchange-status')?.textContent ?? ''");
    step('the copy is written with the seven annotations, and the panel says where', out.ok && out.report?.annotations === 7 && out.report?.written === 7 && status.includes('salvo em'), { out, status });
    const copyBytes = readFileSync(copyPath);
    step(
      'the book is the same, byte for byte; the copy is its bytes and then the update',
      Buffer.compare(readFileSync(bookPath), bookBefore) === 0 &&
        JSON.stringify(listTree(shelf)) === JSON.stringify(shelfBefore) &&
        Buffer.compare(copyBytes.subarray(0, bookBefore.length), bookBefore) === 0 &&
        copyBytes.length > bookBefore.length &&
        copyBytes.subarray(-6).toString('latin1').includes('%%EOF'),
      { book: bookBefore.length, copy: copyBytes.length },
    );
    step('nothing but the copy is left in its folder', readdirSync(exported).length === 1, { files: readdirSync(exported) });

    // 2b. Nothing goes into a folder of the library: not the copy, not the XFDF (ADR 0013, point 9).
    const intoShelf = await pdf(main, 'export-pdf', join(shelf, 'dentro (anotado).pdf'));
    const xfdfIntoShelf = await pdf(main, 'export-xfdf', join(shelf, 'dentro.xfdf'));
    step(
      'a copy or an XFDF inside a folder of the library is refused, and nothing appears there',
      !intoShelf.ok && /pasta da biblioteca/.test(intoShelf.error) && !xfdfIntoShelf.ok && /pasta da biblioteca/.test(xfdfIntoShelf.error) && JSON.stringify(listTree(shelf)) === JSON.stringify(shelfBefore),
      { intoShelf, xfdfIntoShelf, shelf: readdirSync(shelf) },
    );

    // 3. The copy's objects, as PDFium reads them.
    const found = await main.evaluate(`window.musaeumApp.pdfAnnotations(${JSON.stringify(copyPath)})`);
    const native = found.reading.annots;
    const subtypeFor = (essence) => {
      const tool = essence.places[0].tool;
      if (tool === 'rect') return 'Square';
      if (tool === 'free') return 'Ink';
      return essence.kind === 'underline' ? 'Underline' : 'Highlight';
    };
    const objects = originals.map((original) => {
      const written = native.find((annot) => annot.name === original.uuid);
      return {
        uuid: original.uuid,
        found: written !== undefined,
        subtype: written?.subtype,
        expected: subtypeFor(original),
        page: written?.page,
        contents: written?.contents,
        subject: written?.subject,
        color: written?.color,
      };
    });
    step(
      'PDFium finds every annotation in the copy by its /NM, of its kind, on its page, with its note',
      objects.every((object, at) => object.found && object.subtype === object.expected && object.page === originals[at].places[0].page && object.contents === originals[at].note),
      { objects },
    );
    step(
      'the book’s own annotations are all still in the copy',
      native.some((annot) => annot.name === 'existing-highlight' && annot.contents === 'Nota de outro programa') &&
        native.some((annot) => annot.subtype === 'Text' && annot.contents === 'Uma nota antiga') &&
        found.counts[6]?.Popup === 1 &&
        found.counts[6]?.Link === 1,
      { counts: found.counts[6] },
    );

    // 4. The copy in Musaeum: drawn by PDFium, where the annotations were made.
    await back(main);
    await main.evaluate(`window.musaeumApp.addRoot(${JSON.stringify(exported)})`);
    const copyRow = await rowFor(main, 'livro (anotado).pdf');
    await openBook(main, copyRow.documentId);
    await waitSharp(main);
    const none = await reader(main);
    const layer0 = await layerOf(main, 0);
    const highlightAt = await gapIn(main, 0, layer0, 'a ação do coração');
    const underlineAt = await underOf(main, 0, layer0, 'sublinhar');
    const boxWordCopy = await aim(main, 0, 'caixa');
    const boxAt = { x: boxWordCopy.from.x + (boxWordCopy.to.x - boxWordCopy.from.x) * 0.15, y: boxWordCopy.from.y - 9 };
    const colors = {
      highlight: await pixel(main, highlightAt),
      underline: await pixel(main, underlineAt),
      box: await pixel(main, boxAt),
    };
    await goTo(main, 1);
    const layer1 = await layerOf(main, 1);
    colors.sideways = await pixel(main, await gapIn(main, 1, layer1, 'a ação do coração'));
    await goTo(main, 3);
    const uprightCopy = await aim(main, 3, 'coração');
    // Up the page as shown, in the box above the letters: paper under the box.
    const across = { x: uprightCopy.to.x - uprightCopy.from.x, y: uprightCopy.to.y - uprightCopy.from.y };
    const length = Math.hypot(across.x, across.y) || 1;
    const up = { x: across.y / length, y: -across.x / length };
    colors.upright = await pixel(main, { x: uprightCopy.middle.x + up.x * 9 - across.x * 0.35, y: uprightCopy.middle.y + up.y * 9 - across.y * 0.35 });
    step(
      'opened in Musaeum, the copy shows its annotations drawn by PDFium where they were made, on the turned pages too',
      none.annotations.length === 0 &&
        near(colors.highlight, YELLOW, 24) &&
        near(colors.underline, YELLOW_INK, 40) &&
        near(colors.box, GREEN, 24) &&
        near(colors.sideways, BLUE, 24) &&
        near(colors.upright, PURPLE, 24),
      { colors },
    );

    // 5. Its annotations brought into the copy, then again: nothing more.
    const intoCopy = await pdf(main, 'import-pdf', copyPath);
    const intoCopyAgain = await pdf(main, 'import-pdf', copyPath);
    step(
      'the copy’s annotations come into it: ours and the two of the book; again, none',
      intoCopy.ok && intoCopy.report.added === 9 && intoCopy.report.already === 0 && intoCopyAgain.ok && intoCopyAgain.report.added === 0 && intoCopyAgain.report.already === 9,
      { first: intoCopy, again: intoCopyAgain },
    );
    await back(main);

    // 6. Into the book: its own two come in, ours are known by their /NM. Again, nothing.
    await openBook(main, row.documentId);
    await waitSharp(main);
    const intoBook = await pdf(main, 'import-pdf', copyPath);
    const intoBookAgain = await pdf(main, 'import-pdf', copyPath);
    const after = await fullAnnotations(main);
    step(
      'importing the copy into the book brings only what it did not have, once',
      intoBook.ok && intoBook.report.added === 2 && intoBook.report.already === 7 && intoBookAgain.ok && intoBookAgain.report.added === 0 && intoBookAgain.report.already === 9 && after.length === 9,
      { first: intoBook, again: intoBookAgain, count: after.length },
    );
    const sameOriginals = originals.every((original) => {
      const now = after.find((annotation) => annotation.uuid === original.uuid);
      return now !== undefined && JSON.stringify(essence(now)) === JSON.stringify(original);
    });
    step('our annotations in the book are untouched by the import', sameOriginals);
    originals = after.map(essence);

    // A second copy: the book's own two are in its bytes already, and are not written again.
    // Not into `exportados`: the library follows it since the first copy was opened there, so
    // nothing is written in it any more (ADR 0014, D11).
    const secondFolder = join(work, 'exportados-2');
    mkdirSync(secondFolder, { recursive: true });
    const secondPath = join(secondFolder, 'livro (segunda).pdf');
    const second = await pdf(main, 'export-pdf', secondPath);
    const secondFound = second.ok ? await main.evaluate(`window.musaeumApp.pdfAnnotations(${JSON.stringify(secondPath)})`) : null;
    const secondNative = secondFound?.reading.annots ?? [];
    step(
      'a copy made after importing the book’s own annotations does not write them twice',
      second.ok &&
        second.report.annotations === 7 &&
        second.report.alreadyInPdf === 2 &&
        secondNative.filter((annot) => annot.contents === 'Nota de outro programa').length === 1 &&
        secondNative.filter((annot) => annot.contents === 'Uma nota antiga').length === 1 &&
        secondNative.length === 9,
      { report: second.report ?? second.error, count: secondNative.length },
    );
    step('the book is still the same, byte for byte', Buffer.compare(readFileSync(bookPath), bookBefore) === 0);

    // 7. XFDF out of the book.
    const xfdf = await pdf(main, 'export-xfdf', xfdfPath);
    const text = existsSync(xfdfPath) ? readFileSync(xfdfPath, 'utf8') : '';
    step(
      'the XFDF is written with every annotation, the file and its identifiers',
      xfdf.ok && xfdf.report.annotations === 9 && text.startsWith('<?xml') && text.includes('<ids original="') && originals.every((original) => text.includes(`name="${exportName(original)}"`)),
      { report: xfdf.report ?? xfdf.error, missing: originals.filter((original) => !text.includes(`name="${exportName(original)}"`)).map(exportName) },
    );
  } finally {
    await closeApp(session);
  }

  // 8. A clean library with the same file: the XFDF brings everything back.
  session = await launch(app, ['--library', join(work, 'biblioteca-limpa'), '--e2e', '--width', '1400', '--height', '950'], 'window.musaeumApp !== undefined');
  main = session.cdp;
  try {
    await waitFor(main, "document.querySelector('.welcome') !== null || document.querySelector('.library') !== null", { what: 'the first screen' });
    await main.evaluate(`window.musaeumApp.addRoot(${JSON.stringify(cleanShelf)})`);
    const row = await rowFor(main, 'livro.pdf');
    await openBook(main, row.documentId);
    await waitSharp(main);
    const brought = await pdf(main, 'import-xfdf', xfdfPath);
    const there = (await fullAnnotations(main)).map(essence);
    const comparison = originals.map((original) => {
      // Ours come back with their UUIDs; one another program made, by the /NM it came with.
      const now = there.find((annotation) => annotation.uuid === original.uuid || (original.pdfNm !== null && !original.pdfNm.startsWith('musaeum:') && annotation.pdfNm === original.pdfNm));
      return {
        uuid: original.uuid,
        found: now !== undefined,
        kind: now?.kind === original.kind,
        color: now?.color === original.color,
        note: now?.note === original.note,
        place: now !== undefined && samePlaces(now.places, original.places),
      };
    });
    step(
      'into a clean library, the XFDF brings every annotation back as itself: UUID, kind, color, note and place',
      brought.ok && brought.report.added === originals.length && comparison.every((entry) => entry.found && entry.kind && entry.color && entry.note && entry.place),
      { brought: brought.report ?? brought.error, comparison },
    );
    await waitSharp(main);
    const layer0 = await layerOf(main, 0);
    const drawn = await pixel(main, await gapIn(main, 0, layer0, 'a ação do coração'));
    step('and draws them where they were', near(drawn, YELLOW, 24), { color: drawn });

    // 9. A hostile XFDF: an external entity naming a file of this run, which must never be read.
    const canary = join(work, 'segredo.txt');
    writeFileSync(canary, 'NÃO DEVE SER LIDO');
    const hostile = join(work, 'hostil.xfdf');
    writeFileSync(
      hostile,
      `<?xml version="1.0"?>\n<!DOCTYPE xfdf [<!ENTITY xxe SYSTEM "file:///${canary.replace(/\\/g, '/')}">]>\n<xfdf xmlns="http://ns.adobe.com/xfdf/"><annots><text page="0" rect="10,10,30,30"><contents>&xxe;</contents></text></annots></xfdf>\n`,
    );
    const refused = await pdf(main, 'import-xfdf', hostile);
    const count = (await fullAnnotations(main)).length;
    const shown = await main.evaluate("document.querySelector('.pdf-exchange-status.failed')?.textContent ?? null");
    step(
      'a hostile XFDF is refused, nothing comes in, and the panel says why',
      !refused.ok && /document type|DOCTYPE/i.test(refused.error) && count === originals.length && (shown === null || shown.includes('Não foi possível')),
      { refused, count },
    );
  } finally {
    await closeApp(session);
  }
}

/** Real books with annotations of their own, copied: what they hold comes in, and goes out once. */
async function runReal(work) {
  const books = options.real.filter((path) => existsSync(path));
  if (books.length === 0) {
    console.log('  (no real books to read)');
    return;
  }
  const shelf = join(work, 'reais');
  // Copies go outside the library's folders, which are never written.
  const copies = join(work, 'reais-copias');
  mkdirSync(shelf, { recursive: true });
  mkdirSync(copies, { recursive: true });
  const sources = books.map((source, index) => {
    const before = statSync(source, { bigint: true });
    const name = `real-${index}.pdf`;
    copyFileSync(source, join(shelf, name));
    return { source, name, before: { size: String(before.size), modified: String(before.mtimeNs) } };
  });
  const session = await launch(app, ['--library', join(work, 'biblioteca-real'), '--e2e', '--width', '1400', '--height', '950'], 'window.musaeumApp !== undefined');
  const main = session.cdp;
  try {
    await waitFor(main, "document.querySelector('.welcome') !== null || document.querySelector('.library') !== null", { what: 'the first screen' });
    await main.evaluate(`window.musaeumApp.addRoot(${JSON.stringify(shelf)})`);
    for (const book of sources) {
      const path = join(shelf, book.name);
      const row = await rowFor(main, book.name);
      const held = await main.evaluate(`window.musaeumApp.pdfAnnotations(${JSON.stringify(path)})`);
      const kept = held.reading.annots.filter((annot) => !annot.reply && (annot.flags & 34) === 0 && ['Highlight', 'Underline', 'Squiggly', 'Square', 'Ink', 'Text', 'FreeText'].includes(annot.subtype)).length;
      await openBook(main, row.documentId);
      const brought = await pdf(main, 'import-pdf', path);
      const again = await pdf(main, 'import-pdf', path);
      const shown = (await reader(main)).annotations;
      step(
        `${book.source.split('\\').pop()}: its ${kept} annotations come in, once`,
        brought.ok && brought.report.added === kept && again.ok && again.report.added === 0 && again.report.already === kept && shown.length === kept,
        { brought: brought.report ?? brought.error, again: again.report ?? again.error, shown: shown.length, kinds: held.reading.annots.map((annot) => annot.subtype) },
      );
      const copyPath = join(copies, `${book.name}.copia.pdf`);
      const out = await pdf(main, 'export-pdf', copyPath);
      step(
        `${book.source.split('\\').pop()}: a copy exported after does not write them again`,
        !out.ok && /já estão todas no PDF/.test(out.error),
        { out },
      );
      await back(main);
      const after = statSync(book.source, { bigint: true });
      step(`${book.source.split('\\').pop()}: the book in the collection is as it was`, String(after.size) === book.before.size && String(after.mtimeNs) === book.before.modified);
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
  const work = mkdtempSync(join(tmpdir(), 'musaeum-e2e-pdf-'));
  try {
    console.log('fixture');
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
