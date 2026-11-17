#!/usr/bin/env node
// Annotations in the real window, driven through the WebView2's debugging protocol with the input a
// person gives: drags, double and triple clicks, the bar's buttons, the note editor, the inspector.
//
//   node tools/e2e-annotations.mjs [--build target/release] [--out FILE.json] [--phase fixtures|corpus|lines|ocr|real|all]
//                                  [--corpus DIR] [--require-idle SECONDS] [--real INSPECT.json,INSPECT.json]
//
// 1. Fixtures: a PDF and an EPUB written for this (tools/lib/test-books.mjs) and the DjVu fixture.
//    In each it makes highlights, an underline and a note, one across a page break (PDF) or across a
//    cut of the normalizer (EPUB), and checks that they are drawn where their text is, on the screen
//    itself, after changing the zoom, the font, the margins and the mode, after closing and
//    reopening, after renaming the file and moving it to another folder, and after a safe save of a
//    second edition that adds a page or a paragraph before them, edits one sentence and removes
//    another: the kept ones are found exactly, the edited one approximately, the removed one stays
//    as an orphan, and none is lost.
// 2. Corpus: a fresh library over a real collection, only read: a few of its EPUBs and PDFs get a
//    highlight each, kept through typography, zoom and reopening; the collection is listed before and
//    after and must be the same.
//
// 3. Lines (Phase 5): a scanned book with no text layer. The lines the background finds on its
//    picture are counted against the ones drawn; a drag with the lines tool selects from a word of
//    one line to a word of another, the lines between whole; with snapping off, the ends stay where
//    the pointer is; a box and a stroke are drawn too. Each becomes an annotation, drawn on the
//    screen, found again exactly after closing and reopening.
//
// 4. OCR (Phase 5, Part 2): a book with a text layer, of prose, two columns over a photograph, a
//    chart, a table and text turned 90°, 45° and −30° (gen-fixtures --only ocr), and its scanned
//    twin, pictures only (visual-check scan). What a copy of three lines of the native book puts on
//    the system clipboard, the same lines of the twin copied with the lines tool put there too, word
//    for word and line for line; so does a box; turned text copies as it reads; a box over the
//    photograph copies nothing and a drag on it selects nothing. A highlight on the twin gets the
//    native highlight's quote from the OCR, in the background, the search finds it by a word
//    without its accent, and it keeps it after reopening; no OCR worker outlives the app. The
//    clipboard is put aside before and put back after.
//
// 5. Real pages (the audit of Phase 5): pages of two books of the real collection, only read, where
//    the lines tool failed by hand. The lines to drag along, and what they say, come from
//    `corpus-check --inspect --ocr` on the same pages (`--real`, its reports); the folders of the
//    two books are added to a fresh library, and the collection is listed before and after and
//    must be the same. A drag along lines of a table of contents takes the line with a leader of
//    dashes and the short line in italics between two others; one along a line underlined in
//    pencil, with scribbles in the margin, copies its words and not the next line's; and one up the
//    lines of a page printed the long way copies them whole, without the scanner's mark at 0°
//    under them. On a page with a text layer, the copy is the OCR's, as the audit decided.
//    Not part of `all`.
//
// Libraries, caches and the written books live in a temporary folder, removed at the end. The run
// waits for the machine to be free and stops as soon as someone uses it (exit code 3).

import { execFileSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, join, relative, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

import { click, closeApp, connectFrame, idleSeconds, launch, press, typeInto, waitFor } from './lib/cdp.mjs';
import { clipboardText, saveClipboard, setClipboardText } from './lib/clipboard.mjs';
import { decodePng } from './lib/png.mjs';
import { annotatedEpub, annotatedPdf, scanPdf } from './lib/test-books.mjs';

const root = resolve(import.meta.dirname, '..');
const SCALE_ONE = 4096;

function parseArguments(argv) {
  const options = {
    build: join(root, 'target', 'release'),
    out: join(root, 'bench-results', 'e2e-annotations', 'report.json'),
    phase: 'all',
    corpus: 'C:\\Marte\\Archives\\DigitalLibrary\\Books',
    requireIdle: 60,
    samples: 2,
    real: [],
  };
  for (let at = 0; at < argv.length; at++) {
    const flag = argv[at];
    const value = () => argv[++at];
    if (flag === '--build') options.build = resolve(value());
    else if (flag === '--out') options.out = resolve(value());
    else if (flag === '--phase') options.phase = value();
    else if (flag === '--corpus') options.corpus = resolve(value());
    else if (flag === '--require-idle') options.requireIdle = Number(value());
    else if (flag === '--samples') options.samples = Number(value());
    else if (flag === '--real') options.real = value().split(/[,+]/).filter((path) => path.length > 0).map((path) => resolve(path));
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

const report = { started: new Date().toISOString(), options, fixtures: null, corpus: null, lines: null, ocr: null, real: null, failures: [] };
function check(condition, message) {
  if (!condition) {
    report.failures.push(message);
    console.error(`  FAILED: ${message}`);
  }
  return condition;
}

// --- The app ---

const reader = (main) => main.evaluate('window.musaeumApp.reader()');
const annotations = async (main) => (await reader(main))?.annotations ?? [];

async function waitForLibrary(main) {
  await waitFor(main, "window.musaeumApp.view() === 'library' && document.querySelector('.library') !== null", { what: 'the library' });
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
  check(state.error === null, `document ${documentId} did not open: ${state.error}`);
  return state;
}

async function back(main) {
  await click(main, '#back-to-library');
  await waitForLibrary(main);
}

/** Waits for the annotations the reader shows to be `count`, all written. */
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

/** A press, a drag in small steps, and a release, as a hand does it. */
async function drag(main, from, to) {
  checkStillQuiet();
  await mouse(main, 'mouseMoved', from, { buttons: 0 });
  await mouse(main, 'mousePressed', from, { buttons: 1, clickCount: 1 });
  for (let step = 1; step <= 12; step++) {
    const t = step / 12;
    await main.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: from.x + (to.x - from.x) * t, y: from.y + (to.y - from.y) * t, button: 'left', buttons: 1 });
    await delay(16);
  }
  await mouse(main, 'mouseReleased', to, { buttons: 0, clickCount: 1 });
}

/** `count` clicks in quick succession at one point. */
async function clicks(main, at, count) {
  checkStillQuiet();
  await mouse(main, 'mouseMoved', at, { buttons: 0 });
  for (let n = 1; n <= count; n++) {
    await mouse(main, 'mousePressed', at, { buttons: 1, clickCount: n });
    await mouse(main, 'mouseReleased', at, { buttons: 0, clickCount: n });
    await delay(60);
  }
}

/** The color of one pixel of the window, at a point in CSS pixels. */
async function pixel(main, at) {
  const shot = await main.send('Page.captureScreenshot', { format: 'png', clip: { x: Math.round(at.x), y: Math.round(at.y), width: 1, height: 1, scale: 1 } });
  const { pixels } = decodePng(Buffer.from(shot.data, 'base64'));
  return [pixels[0], pixels[1], pixels[2]];
}

const near = (rgb, expected, tolerance) => rgb.every((value, at) => Math.abs(value - expected[at]) <= tolerance);
const GREEN = [168, 230, 161];
const BLUE = [153, 201, 252];
const PINK = [250, 173, 214];
/** What a highlight is over a black ground: its color dimmed to 55%, the type left white. */
const DARK_FILL = 0.55;
const onDark = (rgb) => rgb.map((value) => Math.round(value * DARK_FILL));

// --- Fixed layouts: characters and where they are on the screen ---

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
    chars.push({ ch: String.fromCodePoint(buffer.readUInt32LE(at)), quad, flags: buffer.readUInt16LE(at + 20) });
  }
  return { rotation, chars, text: chars.map((c) => c.ch).join('') };
}

async function pageChars(main, page) {
  const bytes = await waitFor(main, `window.musaeumApp.pageChars(${page})`, { timeoutMs: 20_000, what: `page ${page}'s characters` });
  return decodeChars(bytes);
}

/** Where a point of a page, in its stored frame, is on the screen now, in CSS pixels. */
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

/** Scrolls continuous reading until point `v` of `page`, as shown, is between `low` and `high` on the screen. */
async function reveal(main, page, v, low = 160, high = 740) {
  for (let n = 0; n < 30; n++) {
    const at = await screenOf(main, page, 0.5, v, 0);
    if (at !== null && at.y >= low && at.y <= high) {
      return true;
    }
    const delta = at === null ? (await main.evaluate('window.musaeumApp.reader().status?.page')) < page ? 500 : -500 : at.y < low ? -(low - at.y + 60) : at.y - high + 60;
    await main.send('Input.dispatchMouseEvent', { type: 'mouseWheel', x: 700, y: 450, deltaX: 0, deltaY: Math.max(-700, Math.min(700, delta)) });
    await delay(250);
  }
  return false;
}

/** The screen points of the first and last character of `needle` on a page. */
async function aimFixed(main, page, needle) {
  // The compositor asks for a page's characters once the page has settled on the screen.
  await waitFor(main, `(async () => (await window.musaeumApp.inspectFixed())?.charPages?.includes(${page}))()`, {
    timeoutMs: 15_000,
    what: `the compositor to have page ${page}'s characters`,
  });
  const layer = await pageChars(main, page);
  const start = layer.text.indexOf(needle);
  if (start < 0) {
    throw new Error(`"${needle}" is not on page ${page}: ${layer.text.slice(0, 200)}`);
  }
  const end = start + needle.length - 1;
  const [u0, v0] = centerOf(layer.chars[start].quad);
  const [u1, v1] = centerOf(layer.chars[end].quad);
  const left = layer.chars[start].quad;
  return {
    layer,
    start,
    end: end + 1,
    from: await screenOf(main, page, Math.max(left[0] + 0.0005, u0 - (left[2] - left[0]) * 0.35), v0, layer.rotation),
    to: await screenOf(main, page, u1 + (layer.chars[end].quad[2] - layer.chars[end].quad[0]) * 0.35, v1, layer.rotation),
  };
}

/** A point between two words of `needle`, inside the line's box: paper, under a highlight if any. */
async function gapIn(main, page, layer, needle) {
  const start = layer.text.indexOf(needle);
  const space = layer.text.indexOf(' ', start + 1);
  const before = layer.chars[space - 1].quad;
  const after = layer.chars[space + 1].quad;
  const u = (before[2] + after[0]) / 2;
  const v = (Math.min(before[1], after[1]) + Math.max(before[5], after[5])) / 2;
  return screenOf(main, page, u, v, layer.rotation);
}

async function waitSharp(main) {
  return waitFor(main, '(async () => { const i = await window.musaeumApp.inspectFixed(); return i && i.sharp && !i.framePending && !i.moving ? i : null; })()', {
    timeoutMs: 60_000,
    what: 'the pages to be drawn sharp',
  });
}

async function selectionNow(main, kind) {
  return waitFor(main, `(() => { const s = window.musaeumApp.reader()?.selection; return s && s.kind === '${kind}' ? s : null; })()`, {
    timeoutMs: 8000,
    what: `a ${kind} selection`,
  });
}

async function fixedBook(main, row, pdf) {
  const result = { title: row.title, steps: [] };
  await openBook(main, row.documentId);
  await waitSharp(main);
  const { targets } = pdf;
  const step = (label, ok, detail = {}) => {
    result.steps.push({ label, ok, ...detail });
    check(ok, `${row.title}: ${label} ${JSON.stringify(detail)}`);
  };

  // 1. A drag over a whole line, highlighted green.
  let aim = await aimFixed(main, 0, targets.kept);
  await drag(main, aim.from, aim.to);
  let selection = await selectionNow(main, 'fixed');
  step('a drag selects the line', selection.spans.length === 1 && selection.spans[0].start === aim.start && selection.spans[0].end === aim.end, { spans: selection.spans, text: selection.text });
  await click(main, '#mark-color-green');
  let list = await written(main, 1, 'the highlight');
  step('the highlight is kept with its quote', list[0].quote === targets.kept && list[0].color === 'green' && list[0].marks.length === 1, list[0]);
  await waitSharp(main);
  const gap = await gapIn(main, 0, aim.layer, targets.kept);
  const color = await pixel(main, gap);
  step('it is drawn on the screen, under the line', near(color, GREEN, 16), { color, at: gap });

  // 2. A double click on a word, underlined.
  aim = await aimFixed(main, 0, targets.word);
  const middle = { x: (aim.from.x + aim.to.x) / 2, y: aim.from.y };
  await clicks(main, middle, 2);
  selection = await selectionNow(main, 'fixed');
  step('a double click selects the word', selection.text === targets.word, { text: selection.text });
  await click(main, '#mark-underline');
  list = await written(main, 2, 'the underline');
  step('the underline is kept', list.some((entry) => entry.kind === 'underline' && entry.quote === targets.word));

  // 3. A triple click on a line of the second page, with a note in Markdown.
  await press(main, 'c');
  await waitFor(main, "window.musaeumApp.reader().status?.mode === 'continuous'", { timeoutMs: 5000, what: 'continuous reading' });
  await waitSharp(main);
  await reveal(main, 1, 0.15);
  await waitSharp(main);
  aim = await aimFixed(main, 1, targets.edited);
  await clicks(main, { x: (aim.from.x + aim.to.x) / 2, y: aim.from.y }, 3);
  selection = await selectionNow(main, 'fixed');
  step('a triple click selects the line', selection.text.trim() === targets.edited, { text: selection.text });
  await click(main, '#mark-note');
  await waitFor(main, "document.querySelector('#note-text') !== null", { timeoutMs: 8000, what: 'the note editor' });
  await written(main, 3, 'the note, before its text');
  await typeInto(main, '#note-text', 'Um **coração** que bate — releia isto.');
  await click(main, '#note-save');
  list = await waitFor(main, "(() => { const a = window.musaeumApp.reader().annotations; return a.find((e) => e.note.includes('coração')) ? a : null; })()", { timeoutMs: 10_000, what: 'the note to be kept' });
  step('the note is kept in Markdown', list.some((entry) => entry.kind === 'note' && entry.note === 'Um **coração** que bate — releia isto.'));

  // 4. Across the page break: from the last line of page 1 to the first of page 2.
  // The last line of page 1 sits near its bottom, the first of page 2 near its top: the break
  // between them in the middle of the screen.
  await reveal(main, 0, 0.86, 200, 420);
  await waitSharp(main);
  const first = await aimFixed(main, 0, targets.across[0]);
  const second = await aimFixed(main, 1, targets.across[1]);
  await drag(main, first.from, second.to);
  selection = await selectionNow(main, 'fixed');
  step('a drag across the page break selects on both pages', selection.spans.length === 2 && selection.spans[0].page === 0 && selection.spans[1].page === 1, { spans: selection.spans });
  await click(main, '#mark-color-blue');
  list = await written(main, 4, 'the mark across pages');
  const across = list.find((entry) => entry.marks.length === 2);
  step('one annotation, a segment on each page', across !== undefined && across.marks[0].page === 0 && across.marks[1].page === 1, across);

  // 5. The removed line of page 3, highlighted.
  await reveal(main, 2, 0.2);
  await waitSharp(main);
  aim = await aimFixed(main, 2, targets.removed);
  await drag(main, aim.from, aim.to);
  await selectionNow(main, 'fixed');
  await click(main, '#mark-color-yellow');
  list = await written(main, 5, 'the fifth annotation');

  // 5b. White type on a black ground: the highlight shows dimmed, instead of vanishing.
  await reveal(main, 3, 0.15);
  await waitSharp(main);
  aim = await aimFixed(main, 3, targets.dark);
  // A pixel of the first letter, before anything is over it: the mark must leave it as it is.
  const typeAt = await screenOf(main, 3, (aim.layer.chars[aim.start].quad[0] + aim.layer.chars[aim.start].quad[2]) / 2, (aim.layer.chars[aim.start].quad[1] + aim.layer.chars[aim.start].quad[5]) / 2, 0);
  const typeBefore = await pixel(main, typeAt);
  await drag(main, aim.from, aim.to);
  await selectionNow(main, 'fixed');
  await click(main, '#mark-color-pink');
  list = await written(main, 6, 'the highlight on the dark page');
  await waitSharp(main);
  const darkGap = await gapIn(main, 3, aim.layer, targets.dark);
  const darkColor = await pixel(main, darkGap);
  step('on a black ground the highlight is its color dimmed', near(darkColor, onDark(PINK), 16), { color: darkColor, expected: onDark(PINK) });
  const typeColor = await pixel(main, typeAt);
  // Light type over a dark page counts as the dark page (the renderer decides by the paper around
  // a pixel): the mark takes the maximum with a dim color, which leaves the type as it was, untinted.
  step('and the light type under it stays as it was', typeBefore.some((value) => value >= 150) && near(typeColor, typeBefore, 8), { before: typeBefore, color: typeColor });

  // 6. Zoom and mode: the marks follow the text, drawn in the same frames as the pages.
  const zoomCheck = async (label) => {
    await waitSharp(main);
    const inspection = await main.evaluate('window.musaeumApp.inspectFixed()');
    step(`${label}: every segment is a mark`, inspection.marks === 7, { marks: inspection.marks });
  };
  await click(main, 'button[aria-label="Aumentar"]');
  await click(main, 'button[aria-label="Aumentar"]');
  await zoomCheck('zoom 156%');
  await press(main, 'c');
  await zoomCheck('paginated');
  // Back to page 1, where the first highlight is: still drawn under its line, at this zoom.
  await main.send('Input.dispatchKeyEvent', { type: 'rawKeyDown', key: 'Home', code: 'Home', windowsVirtualKeyCode: 36 });
  await main.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Home', code: 'Home', windowsVirtualKeyCode: 36 });
  await waitSharp(main);
  const zoomedGap = await gapIn(main, 0, (await aimFixed(main, 0, targets.kept)).layer, targets.kept);
  const { canvas } = await main.evaluate('window.musaeumApp.rects()');
  const inside = (point) =>
    point !== null && point.x > canvas.x + 2 && point.x < canvas.x + canvas.width - 2 && point.y > canvas.y + 2 && point.y < canvas.y + canvas.height - 2;
  step('zoomed in, the first highlight is on the screen', inside(zoomedGap), { at: zoomedGap });
  if (inside(zoomedGap)) {
    const zoomed = await pixel(main, zoomedGap);
    step('zoomed in, the highlight is still under its line', near(zoomed, GREEN, 16), { color: zoomed });
  }

  // 7. A click on the highlight opens its bar: another color, kept.
  aim = await aimFixed(main, 0, targets.kept);
  await clicks(main, { x: (aim.from.x + aim.to.x) / 2, y: aim.from.y }, 1);
  await waitFor(main, "document.querySelector('#mark-remove') !== null", { timeoutMs: 8000, what: 'the bar of the highlight' });
  await click(main, '#mark-color-pink');
  await waitFor(main, "window.musaeumApp.reader().annotations.some((e) => e.color === 'pink' && !e.pending)", { timeoutMs: 8000, what: 'the new color' });
  await press(main, 'Escape');

  // 8. The inspector: a search without the accent finds the note, and a click goes to its page.
  await click(main, '#annotations-toggle');
  await waitFor(main, "document.querySelector('.annotation-panel') !== null", { timeoutMs: 5000, what: 'the inspector' });
  step('the inspector lists all six', (await main.evaluate("document.querySelectorAll('.annotation-item').length")) === 6);
  await typeInto(main, '#annotation-search', 'coracao');
  const found = await waitFor(main, "(() => { const n = document.querySelectorAll('.annotation-item'); return n.length === 1 ? n[0].dataset.annotation : null; })()", { timeoutMs: 5000, what: 'the search to find the note' }).catch(() => null);
  step('the search finds the note by a word without its accent', found !== null);
  if (found !== null) {
    await click(main, '.annotation-item .annotation-jump');
    const page = await waitFor(main, 'window.musaeumApp.reader().status?.page === 1', { timeoutMs: 8000, what: 'the jump to page 2' }).catch(() => false);
    step('a click in the inspector goes to its page', page === true);
  }
  await click(main, '#annotations-toggle');

  result.made = (await annotations(main)).map((entry) => ({ uuid: entry.uuid, kind: entry.kind, color: entry.color, quote: entry.quote, pages: entry.marks.map((mark) => mark.page) }));
  await back(main);
  return result;
}

/** After reopening: the same annotations, with the states expected. */
async function compare(main, made, expected, label) {
  // The reader asks for them once the book is open; they come a moment later.
  await waitFor(main, `(window.musaeumApp.reader()?.annotations ?? []).length >= ${made.length}`, {
    timeoutMs: 30_000,
    what: `${label}: the annotations to come back`,
  }).catch(() => null);
  const now = await annotations(main);
  const byUuid = new Map(now.map((entry) => [entry.uuid, entry]));
  const lost = made.filter((entry) => !byUuid.has(entry.uuid));
  check(lost.length === 0, `${label}: ${lost.length} annotations lost: ${JSON.stringify(lost)}`);
  const states = {};
  for (const entry of made) {
    const found = byUuid.get(entry.uuid);
    states[entry.quote?.slice(0, 40) ?? entry.uuid] = found?.anchorState ?? 'lost';
    const want = expected(entry);
    check(found?.anchorState === want, `${label}: "${entry.quote?.slice(0, 50)}" is ${found?.anchorState}, expected ${want}`);
    check(found?.quote === entry.quote, `${label}: the quote of "${entry.quote?.slice(0, 50)}" changed`);
  }
  return { count: now.length, states };
}

// --- Flowing books ---

async function flowFrame(main, state) {
  const frame = await connectFrame({ cdp: main }, state.flow.origin);
  await waitFor(frame, 'window.musaeumReaderE2E !== undefined', { what: 'the reader hooks' });
  await frame.evaluate('window.musaeumReaderE2E.idle()');
  return frame;
}

async function settle(frame) {
  await frame.evaluate('window.musaeumReaderE2E.idle()');
  await frame.evaluate('new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))');
  await delay(450);
  await frame.evaluate('window.musaeumReaderE2E.idle()');
}

/** Points of the reader's page as points of the window. */
async function toWindow(main, point) {
  const { frame } = await main.evaluate('window.musaeumApp.rects()');
  return { x: frame.x + point.x, y: frame.y + point.y };
}

async function aimFlow(main, frame, needle, section) {
  if (section !== null) {
    await frame.evaluate(`window.musaeumReaderE2E.goToSection(${section})`);
    await settle(frame);
  }
  const probe = await frame.evaluate(`window.musaeumReaderE2E.probeSelection(false, ${JSON.stringify(needle)})`);
  if (probe === null) {
    throw new Error(`"${needle.slice(0, 40)}" is not in a section on screen`);
  }
  await settle(frame);
  const again = await frame.evaluate(`window.musaeumReaderE2E.probeSelection(false, ${JSON.stringify(needle)})`);
  return { from: await toWindow(main, again.start), to: await toWindow(main, again.end) };
}

async function flowBook(main, row, book) {
  const result = { title: row.title, steps: [] };
  const step = (label, ok, detail = {}) => {
    result.steps.push({ label, ok, ...detail });
    check(ok, `${row.title}: ${label} ${JSON.stringify(detail)}`);
  };
  const state = await openBook(main, row.documentId);
  let frame = await flowFrame(main, state);
  const { targets } = book;
  // Continuous reading, where any part of a chapter can be brought into view.
  await press(main, 'c');
  await settle(frame);

  // 1. A drag over a paragraph's opening, highlighted green.
  const keptStart = targets.kept.slice(0, 60);
  let aim = await aimFlow(main, frame, keptStart, 0);
  await drag(main, aim.from, aim.to);
  let selection = await selectionNow(main, 'flow');
  step('a drag selects the text', selection.text.replace(/\s+/g, ' ').trim() === keptStart, { text: selection.text });
  await click(main, '#mark-color-green');
  let list = await written(main, 1, 'the highlight');
  step('the highlight keeps the quote', list[0].quote === keptStart, { quote: list[0].quote });
  await settle(frame);
  let marks = await frame.evaluate('window.musaeumReaderE2E.marks()');
  step('it is drawn', marks.drawn === 1 && marks.drift < 0.5, marks);

  // 2. A click on it: underlined instead.
  aim = await aimFlow(main, frame, keptStart, null);
  await clicks(main, { x: (aim.from.x * 3 + aim.to.x) / 4, y: aim.from.y }, 1);
  await waitFor(main, "document.querySelector('#mark-remove') !== null", { timeoutMs: 8000, what: 'the bar of the mark' });
  await click(main, '#mark-underline');
  await waitFor(main, "window.musaeumApp.reader().annotations.some((e) => e.kind === 'underline' && !e.pending)", { timeoutMs: 8000, what: 'the underline' });
  await press(main, 'Escape');

  // 3. A note on a sentence of chapter 3 that the second edition edits.
  const editedStart = targets.edited.slice(0, 70);
  aim = await aimFlow(main, frame, editedStart, state.flow.sections - 1);
  await drag(main, aim.from, aim.to);
  await selectionNow(main, 'flow');
  await click(main, '#mark-note');
  await waitFor(main, "document.querySelector('#note-text') !== null", { timeoutMs: 8000, what: 'the note editor' });
  await written(main, 2, 'the note, before its text');
  await typeInto(main, '#note-text', '- primeiro ponto\n- *segundo* ponto, sobre o coração');
  await click(main, '#note-save');
  await waitFor(main, "window.musaeumApp.reader().annotations.some((e) => e.note.includes('segundo'))", { timeoutMs: 8000, what: 'the note' });

  // 4. Across the normalizer's cut in chapter 2.
  const cuts = await frame.evaluate('window.musaeumReaderE2E.cuts()');
  step('chapter 2 is cut into parts', cuts.length > 0, { cuts });
  if (cuts.length > 0) {
    await frame.evaluate(`window.musaeumReaderE2E.goToSection(${cuts[0]})`);
    await settle(frame);
    await main.send('Input.dispatchMouseEvent', { type: 'mouseWheel', x: 700, y: 450, deltaX: 0, deltaY: -300 });
    await delay(300);
    await settle(frame);
    const seam = await frame.evaluate('window.musaeumReaderE2E.probeSelection(true)');
    step('two parts are on the screen', seam !== null);
    if (seam !== null) {
      await drag(main, await toWindow(main, seam.start), await toWindow(main, seam.end));
      selection = await selectionNow(main, 'flow');
      await click(main, '#mark-color-blue');
      list = await written(main, 3, 'the mark across the cut');
      await settle(frame);
      const acrossCut = list.find((entry) => entry.color === 'blue');
      marks = await frame.evaluate('window.musaeumReaderE2E.marks()');
      step('one annotation drawn over both parts', acrossCut !== undefined && (marks.frames[acrossCut.uuid] ?? 0) === 2 && marks.drift < 0.5, { marks, ranges: acrossCut?.ranges });
    }
  }

  // 5. The paragraph the second edition removes.
  const removedStart = targets.removed.slice(0, 60);
  aim = await aimFlow(main, frame, removedStart, 0);
  await drag(main, aim.from, aim.to);
  await selectionNow(main, 'flow');
  await click(main, '#mark-color-yellow');
  list = await written(main, 4, 'the fourth annotation');

  // 6. Typography and mode: the marks follow their text.
  const relayout = async (label, action) => {
    await action();
    await settle(frame);
    marks = await frame.evaluate('window.musaeumReaderE2E.marks()');
    step(`${label}: marks on their text`, marks.drift < 0.5 && marks.drawn >= 1, marks);
  };
  const typography = async (id) => {
    if ((await main.evaluate("document.querySelector('.typography') !== null")) === false) {
      await click(main, '#typography');
      await waitFor(main, "document.querySelector('.typography') !== null", { timeoutMs: 5000, what: 'the typography panel' });
    }
    await click(main, `#${id}`);
  };
  await relayout('font size +', () => typography('font-larger'));
  await relayout('wide margins', () => typography('margin-wide'));
  await relayout('serif', () => typography('font-serif'));
  await relayout('sepia', () => typography('theme-sepia'));
  await press(main, 'Escape');
  await relayout('paginated', () => press(main, 'c'));
  await frame.evaluate(`window.musaeumReaderE2E.goToSection(0)`);
  await settle(frame);
  marks = await frame.evaluate('window.musaeumReaderE2E.marks()');
  step('paginated, on chapter 1: its marks drawn', marks.drawn >= 1 && marks.drift < 0.5, marks);
  // Back to how a book opens.
  for (const id of ['font-smaller', 'margin-normal', 'font-publisher', 'theme-light']) {
    await typography(id);
  }
  await press(main, 'Escape');

  result.made = (await annotations(main)).map((entry) => ({ uuid: entry.uuid, kind: entry.kind, color: entry.color, quote: entry.quote, ranges: entry.ranges.length }));
  frame.close();
  await back(main);
  return result;
}

// --- The run ---

async function runFixtures(work) {
  const books = join(work, 'livros');
  const shelf = join(books, 'estante');
  mkdirSync(shelf, { recursive: true });
  const pdf = annotatedPdf();
  const epub = annotatedEpub();
  writeFileSync(join(shelf, 'anotar.pdf'), pdf.v1);
  writeFileSync(join(shelf, 'anotar.epub'), epub.v1);
  copyFileSync(join(root, 'fixtures', 'book.djvu'), join(shelf, 'book.djvu'));
  const library = join(work, 'biblioteca');
  const run = await launch(app, ['--library', library, '--e2e', '--width', '1400', '--height', '900'], 'window.musaeumApp !== undefined');
  const main = run.cdp;
  const result = { pdf: null, epub: null, djvu: null, survival: [] };
  report.fixtures = result;
  try {
    await waitFor(main, "document.querySelector('.welcome') !== null || document.querySelector('.library') !== null", { what: 'the first screen' });
    await main.evaluate(`window.musaeumApp.addRoot(${JSON.stringify(books)})`);
    const rows = (
      await waitFor(main, '(() => { const r = window.musaeumApp.rows(); const i = window.musaeumApp.library(); return r.total === 3 && i && !i.indexing && r.rows.every((b) => b.blake3 !== null) ? r : null; })()', {
        timeoutMs: 120_000,
        what: 'three books in the library',
      })
    ).rows;
    const pdfRow = rows.find((row) => row.format === 'pdf');
    const epubRow = rows.find((row) => row.format === 'epub');
    const djvuRow = rows.find((row) => row.format === 'djvu');

    console.log('  pdf');
    result.pdf = await fixedBook(main, pdfRow, pdf);
    console.log('  epub');
    result.epub = await flowBook(main, epubRow, epub);

    console.log('  djvu');
    result.djvu = { steps: [] };
    await openBook(main, djvuRow.documentId);
    await waitSharp(main);
    const layer = await pageChars(main, 0);
    const word = layer.text.split(/\s+/).find((candidate) => candidate.length >= 5);
    if (check(word !== undefined, `the DjVu fixture has no text on its first page: ${layer.text.slice(0, 80)}`)) {
      const aim = await aimFixed(main, 0, word);
      await clicks(main, { x: (aim.from.x + aim.to.x) / 2, y: aim.from.y }, 2);
      const selection = await selectionNow(main, 'fixed');
      check(selection.text === word, `DjVu: the double click selected ${JSON.stringify(selection.text)}, not ${word}`);
      await click(main, '#mark-color-purple');
      const list = await written(main, 1, 'the DjVu highlight');
      result.djvu.made = list.map((entry) => ({ uuid: entry.uuid, quote: entry.quote, kind: entry.kind, color: entry.color }));
    }
    await back(main);

    // Survival: reopen, rename, move, then a second edition by a safe save.
    const reopen = async (label, row, expectedPdf, expectedEpub) => {
      checkStillQuiet();
      const outcome = { label };
      await openBook(main, row.pdf.documentId);
      await waitSharp(main);
      outcome.pdf = await compare(main, result.pdf.made, expectedPdf, `${label} (PDF)`);
      const inspection = await main.evaluate('window.musaeumApp.inspectFixed()');
      outcome.pdfMarks = inspection.marks;
      await back(main);
      const state = await openBook(main, row.epub.documentId);
      const frame = await flowFrame(main, state);
      outcome.epub = await compare(main, result.epub.made, expectedEpub, `${label} (EPUB)`);
      frame.close();
      await back(main);
      if (result.djvu.made !== undefined) {
        await openBook(main, djvuRow.documentId);
        await waitSharp(main);
        outcome.djvu = await compare(main, result.djvu.made, () => 'exact', `${label} (DjVu)`);
        await back(main);
      }
      result.survival.push(outcome);
      console.log(`  ${label}: ${JSON.stringify({ pdf: outcome.pdf.states, epub: outcome.epub.states })}`);
    };
    const exact = () => 'exact';
    const ids = { pdf: pdfRow, epub: epubRow };
    await reopen('closed and opened again', ids, exact, exact);

    const listed = async (what, predicate) =>
      waitFor(main, `(() => { const r = window.musaeumApp.rows(); return r.rows.length === 3 && (${predicate})(r.rows) ? r.rows : null; })()`, {
        timeoutMs: 120_000,
        what,
      });
    renameSync(join(shelf, 'anotar.pdf'), join(shelf, 'anotado.pdf'));
    renameSync(join(shelf, 'anotar.epub'), join(shelf, 'anotado.epub'));
    await listed('the renamed files', "(rows) => rows.some((b) => b.path.endsWith('anotado.pdf')) && rows.some((b) => b.path.endsWith('anotado.epub'))");
    await reopen('renamed', ids, exact, exact);

    const moved = join(books, 'outra-pasta');
    mkdirSync(moved);
    renameSync(join(shelf, 'anotado.pdf'), join(moved, 'anotado.pdf'));
    renameSync(join(shelf, 'anotado.epub'), join(moved, 'anotado.epub'));
    await listed('the moved files', "(rows) => rows.filter((b) => b.path.includes('outra-pasta')).length === 2");
    await reopen('moved to another folder', ids, exact, exact);

    // A safe save: the second edition written beside the file, then put in its place.
    const before = await main.evaluate('window.musaeumApp.rows()');
    for (const [name, bytes] of [['anotado.pdf', pdf.v2], ['anotado.epub', epub.v2]]) {
      const target = join(moved, name);
      writeFileSync(`${target}.tmp`, bytes);
      renameSync(`${target}.tmp`, target);
    }
    const hashes = Object.fromEntries(before.rows.map((row) => [row.documentId, row.blake3]));
    await listed(
      'the second editions',
      `(rows) => rows.filter((b) => b.path.includes('outra-pasta') && b.blake3 !== null && b.blake3 !== ${JSON.stringify(hashes)}[b.documentId]).length === 2 && !window.musaeumApp.library().indexing`,
    );
    const after = await main.evaluate('window.musaeumApp.rows()');
    check(
      after.rows.find((row) => row.documentId === pdfRow.documentId) !== undefined && after.rows.find((row) => row.documentId === epubRow.documentId) !== undefined,
      'a second edition saved in place is not the same document any more',
    );
    const pdfState = (entry) => (entry.quote === pdf.targets.removed ? 'orphan' : entry.quote === pdf.targets.edited ? 'approximate' : 'exact');
    // A quote is kept with its spaces made single and trimmed; so are these, to be compared with it.
    const squeeze = (text) => text.replace(/\s+/g, ' ').trim();
    const epubState = (entry) =>
      entry.quote === squeeze(epub.targets.removed.slice(0, 60))
        ? 'orphan'
        : entry.quote === squeeze(epub.targets.edited.slice(0, 70))
          ? 'approximate'
          : 'exact';
    await reopen('a second edition, saved in place', ids, pdfState, epubState);

    // The orphans are still listed, searchable, with their quote.
    await openBook(main, pdfRow.documentId);
    await waitSharp(main);
    await click(main, '#annotations-toggle');
    await click(main, '#filter-orphans');
    const orphans = await waitFor(main, "(() => { const n = [...document.querySelectorAll('.annotation-item.orphan')]; return n.length > 0 ? n.map((e) => e.textContent) : null; })()", { timeoutMs: 5000, what: 'the orphan in the inspector' }).catch(() => null);
    result.orphanListed = orphans;
    check(orphans !== null && orphans.length === 1 && orphans[0].includes(pdf.targets.removed.slice(0, 20)), `the orphan in the inspector: ${JSON.stringify(orphans)}`);
    await click(main, '#annotations-toggle');
    await back(main);
  } finally {
    result.exitCode = await closeApp(run).catch((error) => String(error));
    result.log = run.log().slice(-20000);
    result.console = run.console.slice();
  }
  return result;
}

/** Opens a book of a large library through the search, since the grid only draws the cards in view. */
async function openFromSearch(main, row) {
  const words = row.title.replace(/[^\p{L}\p{N}\s]/gu, ' ').split(/\s+/).filter((w) => w.length > 0).slice(0, 4).join(' ');
  const text = words.length > 0 ? words : basename(row.path).replace(/\.[^.]+$/, '');
  await typeInto(main, '.search input', text);
  await waitFor(
    main,
    `(() => { const r = window.musaeumApp.rows(); return r.text === ${JSON.stringify(text)} && r.rows.some((b) => b.documentId === ${row.documentId}); })()`,
    { timeoutMs: 8000, what: `the search "${text}" to find ${row.title}` },
  );
  return openBook(main, row.documentId);
}

function listTree(directory) {
  const entries = [];
  const walk = (folder) => {
    for (const name of readdirSync(folder).sort()) {
      const path = join(folder, name);
      const stat = statSync(path, { bigint: true });
      entries.push({ path: relative(directory, path), directory: stat.isDirectory(), size: String(stat.size), mtime: String(stat.mtimeNs) });
      if (stat.isDirectory()) {
        walk(path);
      }
    }
  };
  walk(directory);
  return entries;
}

async function runCorpus(work) {
  const before = listTree(options.corpus);
  const library = join(work, 'acervo');
  const run = await launch(app, ['--library', library, '--e2e', '--width', '1400', '--height', '900'], 'window.musaeumApp !== undefined');
  const main = run.cdp;
  const result = { books: [] };
  report.corpus = result;
  try {
    await waitFor(main, "document.querySelector('.welcome') !== null || document.querySelector('.library') !== null", { what: 'the first screen' });
    await main.evaluate(`window.musaeumApp.addRoot(${JSON.stringify(options.corpus)})`);
    const rows = (
      await waitFor(main, '(() => { const r = window.musaeumApp.rows(); const i = window.musaeumApp.library(); return r.total > 0 && i && !i.indexing && r.rows.every((b) => b.blake3 !== null) ? r : null; })()', {
        timeoutMs: 600_000,
        what: 'the collection in the library',
      })
    ).rows;
    // A few EPUBs, and as many PDFs with text on the page they open at: a scan has nothing to select.
    const epubs = rows.filter((row) => row.format === 'epub').slice(0, options.samples);
    const pdfs = rows.filter((row) => row.format === 'pdf').slice(0, options.samples * 5);
    let annotatedPdfs = 0;
    result.skipped = [];
    for (const row of [...epubs, ...pdfs]) {
      if (row.format === 'pdf' && annotatedPdfs >= options.samples) {
        break;
      }
      checkStillQuiet();
      console.log(`  ${row.format}: ${basename(row.path)}`);
      const entry = { title: row.title, format: row.format };
      result.books.push(entry);
      const state = await openFromSearch(main, row);
      if (state.error !== null) {
        continue;
      }
      try {
        if (row.format === 'epub') {
          const frame = await flowFrame(main, state);
          await frame.evaluate('window.musaeumReaderE2E.goToFraction(0.3)');
          await settle(frame);
          const probe = await frame.evaluate('window.musaeumReaderE2E.probeSelection(false)');
          await drag(main, await toWindow(main, probe.start), await toWindow(main, probe.end));
          await selectionNow(main, 'flow');
          await click(main, '#mark-color-green');
          const list = await written(main, 1, 'the highlight');
          await click(main, '#typography');
          await click(main, '#font-larger');
          await press(main, 'Escape');
          await settle(frame);
          const marks = await frame.evaluate('window.musaeumReaderE2E.marks()');
          entry.quote = list[0].quote;
          entry.drift = marks.drift;
          check(marks.drawn === 1 && marks.drift < 0.5, `${row.title}: the mark after a larger font ${JSON.stringify(marks)}`);
          await click(main, '#typography');
          await click(main, '#font-smaller');
          await press(main, 'Escape');
          frame.close();
        } else {
          await waitSharp(main);
          const status = (await reader(main)).status;
          const page = status.page;
          const layer = await pageChars(main, page);
          const words = layer.text.match(/\p{L}{4,}(?: \p{L}{2,}){4}/u);
          if (words === null) {
            // A scan, or a page of pictures: another PDF instead.
            result.skipped.push({ title: row.title, page, characters: layer.chars.length });
            result.books.pop();
            await back(main);
            continue;
          }
          annotatedPdfs++;
          const aim = await aimFixed(main, page, words[0]);
          await drag(main, aim.from, aim.to);
          await selectionNow(main, 'fixed');
          await click(main, '#mark-color-green');
          const list = await written(main, 1, 'the highlight');
          entry.quote = list[0].quote;
          await click(main, 'button[aria-label="Aumentar"]');
          await waitSharp(main);
          entry.marks = (await main.evaluate('window.musaeumApp.inspectFixed()')).marks;
          check(entry.marks >= 1, `${row.title}: the mark is gone after zooming`);
          await click(main, 'button[aria-label="Diminuir"]');
        }
        entry.made = (await annotations(main)).map((made) => ({ uuid: made.uuid, quote: made.quote }));
        await back(main);
        const again = await openFromSearch(main, row);
        if (row.format === 'epub') {
          (await flowFrame(main, again)).close();
        } else {
          await waitSharp(main);
        }
        entry.reopened = await compare(main, entry.made, () => 'exact', `${row.title}, reopened`);
      } catch (error) {
        if (error instanceof Interrupted) throw error;
        check(false, `${row.title}: ${error.message}`);
      }
      await back(main).catch(() => {});
    }
  } finally {
    result.exitCode = await closeApp(run).catch((error) => String(error));
    result.console = run.console.slice();
    const after = listTree(options.corpus);
    result.unchanged = JSON.stringify(after) === JSON.stringify(before);
    result.listing = { entries: before.length };
    check(result.unchanged, 'the collection changed during the run');
  }
  return result;
}

/** A scanned book: lines found on its picture, marks along them, a box, a stroke. */
async function runLines(work) {
  const books = join(work, 'escaneados');
  mkdirSync(books, { recursive: true });
  const scan = scanPdf(2);
  writeFileSync(join(books, 'escaneado.pdf'), scan.pdf);
  const library = join(work, 'biblioteca-linhas');
  const run = await launch(app, ['--library', library, '--e2e', '--width', '1400', '--height', '900'], 'window.musaeumApp !== undefined');
  const main = run.cdp;
  const result = { steps: [] };
  report.lines = result;
  const step = (label, ok, detail = {}) => {
    result.steps.push({ label, ok, ...detail });
    check(ok, `lines: ${label} ${JSON.stringify(detail)}`);
  };
  try {
    await waitFor(main, "document.querySelector('.welcome') !== null || document.querySelector('.library') !== null", { what: 'the first screen' });
    await main.evaluate(`window.musaeumApp.addRoot(${JSON.stringify(books)})`);
    const [row] = (
      await waitFor(main, '(() => { const r = window.musaeumApp.rows(); const i = window.musaeumApp.library(); return r.total === 1 && i && !i.indexing && r.rows.every((b) => b.blake3 !== null) ? r : null; })()', {
        timeoutMs: 120_000,
        what: 'the scanned book in the library',
      })
    ).rows;
    await openBook(main, row.documentId);
    await waitSharp(main);

    // The lines tool: the page's lines are traced ahead of the rest of the background.
    await click(main, 'button[aria-label="Selecionar por linhas da imagem"]');
    const truth = scan.pages[0];
    const traced = await waitFor(main, '(async () => { const i = await window.musaeumApp.inspectFixed(); const p = i?.linePages?.find((x) => x.page === 0); return p ? { tool: i.tool, lines: p.lines } : null; })()', {
      timeoutMs: 90_000,
      what: "page 0's lines",
    });
    step('the lines found on the picture are the lines drawn on it', traced.tool === 'lines' && traced.lines === truth.length, { found: traced.lines, drawn: truth.length });

    const at = async (line, u) => screenOf(main, 0, u, truth[line].v - scan.xHeight / 2, 0);
    const middleOf = ([from, to]) => (from + to) / 2;
    // From the middle of the second word of line 3 to the middle of the third word of line 5.
    const from = await at(3, middleOf(truth[3].words[1]));
    const to = await at(5, middleOf(truth[5].words[2]));
    await drag(main, from, to);
    let selection = await selectionNow(main, 'region');
    let region = selection.region;
    const width = (line) => truth[line].words.at(-1)[1] - truth[line].words[0][0];
    const along = (line, u) => (u - truth[line].words[0][0]) / width(line);
    step(
      'a drag along lines takes the first from its word, the middle one whole, the last to its word',
      region.tool === 'lines' &&
        region.lines.map((piece) => piece.index).join() === '3,4,5' &&
        Math.abs(region.lines[0].from - along(3, truth[3].words[1][0])) < 0.01 &&
        region.lines[1].from === 0 &&
        region.lines[1].to === 1 &&
        Math.abs(region.lines[2].to - along(5, truth[5].words[2][1])) < 0.01,
      { lines: region.lines },
    );
    await click(main, '#mark-color-green');
    let list = await written(main, 1, 'the mark along lines');
    // Its quote is what the OCR reads under it, afterwards: on these made-up letters, whatever it reads.
    step('it is kept as an annotation with its three lines', list[0].marks.length === 1 && list[0].marks[0].quads === 3, list[0]);
    await waitSharp(main);
    // Between two words of the middle line: paper, under the mark.
    const gap = await at(4, (truth[4].words[1][1] + truth[4].words[2][0]) / 2);
    const green = await pixel(main, gap);
    step('the mark is drawn on the picture', near(green, GREEN, 16), { color: green });

    // Snapping off: the ends stay where the pointer is. The points are worked out again: the view
    // may have moved since.
    await click(main, '#line-snap');
    await waitSharp(main);
    await drag(main, await at(3, middleOf(truth[3].words[1])), await at(5, middleOf(truth[5].words[2])));
    selection = await selectionNow(main, 'region');
    region = selection.region;
    step(
      'with snapping off the ends are where the pointer was',
      Math.abs(region.lines[0].from - along(3, middleOf(truth[3].words[1]))) < 0.01 &&
        Math.abs(region.lines.at(-1).to - along(5, middleOf(truth[5].words[2]))) < 0.01,
      { lines: region.lines },
    );
    await press(main, 'Escape');
    await click(main, '#line-snap');

    // A box over three lines. Paper inside it, a space between two words of its middle line, is
    // read first: a scan's paper is not white, and the box takes the minimum with it.
    await click(main, 'button[aria-label="Caixa retangular"]');
    await waitSharp(main);
    const space = truth[11].words.findIndex((word, index) => index > 0 && truth[11].words[index - 1][1] > 0.32 && word[0] < 0.58);
    const inBox = await at(11, (truth[11].words[space - 1][1] + truth[11].words[space][0]) / 2);
    const paper = await pixel(main, inBox);
    const boxFrom = await screenOf(main, 0, 0.3, truth[10].v - scan.xHeight * 2, 0);
    const boxTo = await screenOf(main, 0, 0.6, truth[12].v + scan.xHeight, 0);
    await drag(main, boxFrom, boxTo);
    selection = await selectionNow(main, 'region');
    step('a box is a region of one quad', selection.region.tool === 'rect' && selection.region.quads.length === 1, { clip: selection.region.clip });
    await click(main, '#mark-color-blue');
    list = await written(main, 2, 'the box');
    await waitSharp(main);
    const blue = await pixel(main, inBox);
    const expected = BLUE.map((value, channel) => Math.min(value, paper[channel]));
    step('the box is drawn translucent over the paper', near(blue, expected, 12), { color: blue, paper, expected });

    // A stroke down the margin.
    await click(main, 'button[aria-label="Traço livre"]');
    await drag(main, await screenOf(main, 0, 0.06, truth[14].v, 0), await screenOf(main, 0, 0.07, truth[18].v, 0));
    selection = await selectionNow(main, 'region');
    step('a stroke keeps its path', selection.region.tool === 'free' && selection.region.path.length >= 4, { points: selection.region.path.length / 2 });
    await click(main, '#mark-color-yellow');
    list = await written(main, 3, 'the stroke');
    const inspected = await main.evaluate('window.musaeumApp.inspectFixed()');
    step('every mark is drawn', inspected.marks === 3, { marks: inspected.marks });

    // Closed and opened again: the three, exact, on the page they were made on.
    await back(main);
    await openBook(main, row.documentId);
    await waitSharp(main);
    list = await written(main, 3, 'the marks after reopening');
    step('closed and opened again, every mark is found exactly', list.every((entry) => entry.anchorState === 'exact' && entry.marks.length === 1 && entry.marks[0].page === 0), list.map((entry) => ({ state: entry.anchorState, marks: entry.marks })));
    await waitSharp(main);
    const again = await pixel(main, await at(4, (truth[4].words[1][1] + truth[4].words[2][0]) / 2));
    step('and drawn where it was', near(again, GREEN, 16), { color: again });
  } finally {
    result.exitCode = await closeApp(run).catch((error) => String(error));
    result.log = run.log().slice(-20000);
    result.console = run.console.slice();
  }
  return result;
}

// --- OCR: a native book and its scanned twin, side by side ---

/** Text as two copies are compared: line ends as `\n`, each line's words one space apart. */
const linesOf = (text) =>
  text
    .replace(/\r\n?/g, '\n')
    .split('\n')
    .map((line) => line.trim().replace(/\s+/g, ' '))
    .filter((line) => line !== '');
const sameLines = (a, b) => JSON.stringify(linesOf(a)) === JSON.stringify(linesOf(b));
const collapse = (text) => text.replace(/\s+/g, ' ').trim();
const plain = (text) => text.normalize('NFD').replace(/[̀-ͯ]/g, '');

/** Characters to change to make `a` into `b`, by Levenshtein's distance. */
function edits(a, b) {
  const x = [...a];
  const y = [...b];
  let row = Array.from({ length: y.length + 1 }, (_, at) => at);
  for (let i = 1; i <= x.length; i++) {
    const next = [i];
    for (let j = 1; j <= y.length; j++) {
      next[j] = Math.min(row[j] + 1, next[j - 1] + 1, row[j - 1] + (x[i - 1] === y[j - 1] ? 0 : 1));
    }
    row = next;
  }
  return row[y.length];
}

/** Where a point of a page of the OCR book, in points from its bottom-left corner, is on the screen. */
async function bookPoint(main, page, [x, y], size) {
  return screenOf(main, page, x / size[0], 1 - y / size[1], 0);
}

/** A point on a line of the OCR book `t` of the way along it, `rise` points up across it. */
function onLine(line, t, rise) {
  const [x0, y0] = line.start;
  const [x1, y1] = line.end;
  const length = Math.hypot(x1 - x0, y1 - y0);
  const [dx, dy] = [(x1 - x0) / length, (y1 - y0) / length];
  // Up across a line in PDF's frame, y growing upward: to the left of the way it goes.
  return [x0 + dx * length * t - dy * rise, y0 + dy * length * t + dx * rise];
}

/** The stored frame's point of the left part of a character, and of the right part of another. */
function charEnds(layer, start, end) {
  const first = layer.chars[start].quad;
  const last = layer.chars[end - 1].quad;
  const [u0, v0] = centerOf(first);
  const [u1, v1] = centerOf(last);
  return {
    from: [Math.max(first[0] + 0.0005, u0 - (first[2] - first[0]) * 0.35), v0],
    to: [u1 + (last[2] - last[0]) * 0.35, v1],
  };
}

const NOTHING_COPIED = 'musaeum-e2e: nada copiado';

/** Copies with Ctrl+C what is selected, and reads what the clipboard has then; null if nothing came. */
async function copied(main, work) {
  setClipboardText(NOTHING_COPIED, work);
  checkStillQuiet();
  await press(main, 'c', 2);
  const started = Date.now();
  while (Date.now() - started < 30_000) {
    const text = clipboardText(work);
    if (text !== null && text !== NOTHING_COPIED) {
      return text;
    }
    await delay(250);
  }
  return null;
}

/** The text a region selection will copy, once it is read. */
async function regionRead(main) {
  return waitFor(main, "(() => { const s = window.musaeumApp.reader()?.selection; return s && s.kind === 'region' && !s.reading ? s : null; })()", {
    timeoutMs: 120_000,
    what: 'the text under the region to be read',
  });
}

/** A page's line model traced, for the lines tool. */
async function linesTraced(main, page) {
  return waitFor(main, `(async () => { const i = await window.musaeumApp.inspectFixed(); const p = i?.linePages?.find((x) => x.page === ${page}); return p && p.lines > 0 ? p : null; })()`, {
    timeoutMs: 120_000,
    what: `page ${page}'s lines`,
  });
}

/**
 * The OCR book and its scanned twin, in one library. What a copy of lines of the native book puts
 * on the clipboard, a copy of the same lines of the twin with the lines tool puts there too, word
 * for word and line for line; so does a box; text at 90°, 45° and −30° is copied as it reads; a box
 * over the photograph copies nothing and a drag on it selects nothing; a highlight on the twin
 * gets the native quote, is found by the search without accents, and keeps it after reopening;
 * and no OCR worker outlives the app.
 */
async function runOcr(work) {
  const result = { steps: [], copies: [] };
  report.ocr = result;
  const step = (label, ok, detail = {}) => {
    result.steps.push({ label, ok, ...detail });
    check(ok, `ocr: ${label} ${JSON.stringify(detail)}`);
  };
  // The native book, from gen-fixtures, and its clean twin, scanned at 300 dpi by visual-check.
  const source = join(work, 'ocr-fonte');
  const books = join(work, 'ocr-livros');
  mkdirSync(books, { recursive: true });
  execFileSync(join(options.build, 'gen-fixtures.exe'), ['--only', 'ocr', '--out', source], { stdio: 'ignore', windowsHide: true });
  const truth = JSON.parse(readFileSync(join(source, 'ocr-native.json'), 'utf8'));
  const size = truth.page;
  copyFileSync(join(source, 'ocr-native.pdf'), join(books, 'nativo.pdf'));
  execFileSync(
    join(options.build, 'visual-check.exe'),
    ['scan', '--document', join(source, 'ocr-native.pdf'), '--out', join(books, 'escaneado.pdf'), '--dpi', '300', '--noise', '1', '--seed', '1'],
    { stdio: 'ignore', windowsHide: true },
  );
  const lines = (page, kind, block) => truth.lines.filter((line) => line.page === page && line.kind === kind && (block === undefined || line.block === block) && line.text !== '');

  // OCR workers already running belong to someone else (a measurement, say): only new ones count.
  const ocrPids = () =>
    execFileSync('tasklist', ['/FI', 'IMAGENAME eq musaeum-ocr.exe', '/FO', 'CSV', '/NH'], { encoding: 'utf8', windowsHide: true })
      .split('\n')
      .filter((line) => line.includes('musaeum-ocr.exe'))
      .map((line) => line.split('","')[1]);
  const before = new Set(ocrPids());
  const restore = saveClipboard(work);
  const library = join(work, 'biblioteca-ocr');
  const run = await launch(app, ['--library', library, '--e2e', '--width', '1400', '--height', '900'], 'window.musaeumApp !== undefined');
  const main = run.cdp;
  let ocrWorkers = [];
  try {
    await waitFor(main, "document.querySelector('.welcome') !== null || document.querySelector('.library') !== null", { what: 'the first screen' });
    await main.evaluate(`window.musaeumApp.addRoot(${JSON.stringify(books)})`);
    const rows = (
      await waitFor(main, '(() => { const r = window.musaeumApp.rows(); const i = window.musaeumApp.library(); return r.total === 2 && i && !i.indexing && r.rows.every((b) => b.blake3 !== null) ? r : null; })()', {
        timeoutMs: 120_000,
        what: 'both books in the library',
      })
    ).rows;
    const native = rows.find((row) => basename(row.path) === 'nativo.pdf');
    const twin = rows.find((row) => basename(row.path) === 'escaneado.pdf');

    // 1. The native book: three lines of prose selected with the text tool and copied.
    const body = lines(0, 'body', 1);
    const chosen = body.slice(1, 4);
    await openBook(main, native.documentId);
    await waitSharp(main);
    await waitFor(main, '(async () => (await window.musaeumApp.inspectFixed())?.charPages?.includes(0))()', { timeoutMs: 15_000, what: "page 0's characters" });
    const layer = await pageChars(main, 0);
    const start = layer.text.indexOf(chosen[0].text);
    const end = layer.text.indexOf(chosen[2].text) + chosen[2].text.length;
    step('the native book has the lines as written', start >= 0 && end > start, { start, end });
    const ends = charEnds(layer, start, end);
    await drag(main, await screenOf(main, 0, ...ends.from, 0), await screenOf(main, 0, ...ends.to, 0));
    const nativeSelection = await selectionNow(main, 'fixed');
    const nativeCopy = await copied(main, work);
    const expected = chosen.map((line) => line.text).join('\n');
    result.copies.push({ what: 'native, text tool', text: nativeCopy });
    step('the native copy is the three lines as written', nativeCopy !== null && sameLines(nativeCopy, expected), { copied: nativeCopy, selected: nativeSelection.text });
    await click(main, '#mark-color-green');
    const nativeMarks = await written(main, 1, 'the native highlight');
    const nativeQuote = nativeMarks[0].quote;
    await back(main);

    // 2. The twin: the same three lines with the lines tool, from the same places.
    await openBook(main, twin.documentId);
    await waitSharp(main);
    await click(main, 'button[aria-label="Selecionar por linhas da imagem"]');
    await linesTraced(main, 0);
    await waitSharp(main);
    await drag(main, await screenOf(main, 0, ...ends.from, 0), await screenOf(main, 0, ...ends.to, 0));
    let region = await regionRead(main);
    step('a drag along the lines takes the three, whole', region.region.lines.length === 3 && region.region.lines.every((piece, at) => (at > 0 || piece.from === 0) && (at < 2 || piece.to === 1)), { lines: region.region.lines });
    let twinCopy = await copied(main, work);
    result.copies.push({ what: 'scan, lines tool', text: twinCopy });
    step('the scan copies what the native book copies, word for word and line for line', twinCopy !== null && nativeCopy !== null && sameLines(twinCopy, nativeCopy), {
      native: nativeCopy,
      scan: twinCopy,
      edits: twinCopy === null || nativeCopy === null ? null : edits(linesOf(twinCopy).join('\n'), linesOf(nativeCopy).join('\n')),
    });

    // A highlight on them: its quote is read in the background, and is the native one.
    await click(main, '#mark-color-green');
    await written(main, 1, 'the highlight on the scan');
    const quoted = await waitFor(main, '(() => { const a = window.musaeumApp.reader()?.annotations ?? []; return a.length === 1 && a[0].quote !== null ? a[0] : null; })()', {
      timeoutMs: 60_000,
      what: 'the quote read by the OCR',
    }).catch(() => null);
    step('the mark on the scan quotes what the native one does', quoted !== null && quoted.quote === nativeQuote && quoted.quoteSource === 'ocr', { scan: quoted?.quote ?? null, native: nativeQuote });

    // 3. A box over three lines of the next paragraph.
    await click(main, 'button[aria-label="Caixa retangular"]');
    const boxed = lines(0, 'body', 2).slice(0, 3);
    const top = boxed[0].start[1] + boxed[0].size * 0.9;
    const bottom = boxed[2].start[1] - boxed[2].size * 0.4;
    await waitSharp(main);
    await drag(main, await bookPoint(main, 0, [64, top], size), await bookPoint(main, 0, [552, bottom], size));
    region = await regionRead(main);
    const boxCopy = await copied(main, work);
    result.copies.push({ what: 'scan, box', text: boxCopy });
    step('a box copies the lines inside it', boxCopy !== null && sameLines(boxCopy, boxed.map((line) => line.text).join('\n')), { copied: boxCopy });
    await press(main, 'Escape');

    // 4. The photograph: a box inside it copies nothing; a drag on it selects nothing.
    const photo = truth.graphics.find((graphic) => graphic.kind === 'photograph');
    const [left, lower, right, upper] = photo.rect;
    const middle = [(left + right) / 2, (lower + upper) / 2];
    await reveal(main, 1, 1 - middle[1] / size[1]);
    await waitSharp(main);
    await drag(main, await bookPoint(main, 1, [left + 12, upper - 12], size), await bookPoint(main, 1, [right - 12, lower + 12], size));
    region = await regionRead(main);
    step('a box over the photograph has no text', region.text === '', { text: region.text });
    setClipboardText(NOTHING_COPIED, work);
    await press(main, 'c', 2);
    await delay(1500);
    step('and a copy of it puts nothing on the clipboard', clipboardText(work) === NOTHING_COPIED);
    await press(main, 'Escape');
    await click(main, 'button[aria-label="Selecionar por linhas da imagem"]');
    await linesTraced(main, 1);
    await drag(main, await bookPoint(main, 1, [middle[0] - 60, middle[1] + 20], size), await bookPoint(main, 1, [middle[0] + 60, middle[1] - 20], size));
    await delay(500);
    const onPhoto = await main.evaluate('window.musaeumApp.reader()?.selection ?? null');
    step('a drag along lines on the photograph selects nothing', onPhoto === null, { selection: onPhoto });

    // 5. Text at other angles, on the third page: a box around the block read upward, and drags
    // along the blocks at 45° and at −30°.
    const upward = lines(2, 'turned', 1);
    const across = (line) => line.start[0];
    await reveal(main, 2, 0.5);
    await linesTraced(main, 2);
    await click(main, 'button[aria-label="Caixa retangular"]');
    await waitSharp(main);
    await drag(main, await bookPoint(main, 2, [across(upward[0]) - 11, 650], size), await bookPoint(main, 2, [across(upward.at(-1)) + 2, 112], size));
    region = await regionRead(main);
    const upwardCopy = await copied(main, work);
    result.copies.push({ what: 'scan, box, 90°', text: upwardCopy });
    const upwardText = upward.map((line) => line.text).join('\n');
    step('a box around text read upward copies it as it reads', upwardCopy !== null && sameLines(upwardCopy, upwardText), {
      copied: upwardCopy,
      edits: upwardCopy === null ? null : edits(linesOf(upwardCopy).join('\n'), upwardText),
      chars: upwardText.length,
    });
    await press(main, 'Escape');
    await click(main, 'button[aria-label="Selecionar por linhas da imagem"]');
    for (const [block, label] of [
      [3, '45°'],
      [4, '−30°'],
    ]) {
      const turned = lines(2, 'turned', block);
      const rise = turned[0].size * 0.22;
      await waitSharp(main);
      await drag(main, await bookPoint(main, 2, onLine(turned[0], 0.03, rise), size), await bookPoint(main, 2, onLine(turned.at(-1), 0.995, rise), size));
      region = await regionRead(main);
      const copy = await copied(main, work);
      const text = turned.map((line) => line.text).join('\n');
      result.copies.push({ what: `scan, lines tool, ${label}`, text: copy });
      step(`a drag along lines at ${label} copies them as they read`, copy !== null && sameLines(copy, text), {
        lines: region.region.lines,
        copied: copy,
        edits: copy === null ? null : edits(linesOf(copy).join('\n'), text),
        chars: text.length,
      });
      await press(main, 'Escape');
    }

    // 6. The search finds the mark on the scan by a word of its quote without its accent, and
    // closed and opened again it keeps its quote.
    const accented = (quoted?.quote ?? '').split(/[\s,.;:]+/).find((word) => word.length >= 5 && plain(word) !== word) ?? '';
    await click(main, '#annotations-toggle');
    await waitFor(main, "document.querySelector('.annotation-panel') !== null", { timeoutMs: 5000, what: 'the inspector' });
    await typeInto(main, '#annotation-search', plain(accented).toLowerCase());
    const found = await waitFor(main, "(() => { const n = document.querySelectorAll('.annotation-item'); return n.length === 1 ? n[0].textContent : null; })()", { timeoutMs: 8000, what: 'the search' }).catch(() => null);
    step('the search finds the mark on the scan by a word without its accent', accented !== '' && found !== null, { word: accented, found });
    await click(main, '#annotations-toggle');
    await back(main);
    await openBook(main, twin.documentId);
    await waitSharp(main);
    const again = await written(main, 1, 'the mark after reopening');
    step('closed and opened again, the mark keeps its quote and its place', again[0].quote === nativeQuote && again[0].anchorState === 'exact', again[0]);
  } finally {
    result.exitCode = await closeApp(run).catch((error) => String(error));
    result.log = run.log().slice(-20000);
    result.console = run.console.slice();
    // No worker outlives the app that started it.
    await delay(1000);
    ocrWorkers = ocrPids().filter((pid) => !before.has(pid));
    result.clipboardRestored = restore();
  }
  step('no OCR worker is left running', ocrWorkers.length === 0, { workers: ocrWorkers });
  return result;
}

/**
 * What to do on the real pages: the book (its folder and the start of its name), the page as the
 * reader numbers it from 1, the lines a drag starts and ends on (by what `corpus-check --inspect`
 * read on them) and how far along each, and what the copy must have and must not.
 */
const REAL_DRAGS = [
  {
    label: 'a table of contents keeps the line with a leader of dashes',
    book: 'TM9-1005-213-10',
    page: 82,
    from: { line: /TERIEL TO PREVENT/, t: 0.03 },
    to: { line: /^APPENDIX A\. REFERENCES/, t: 0.97 },
    has: ['TERIEL TO PREVENT', 'ENEMY USE', 'REFERENCES'],
  },
  {
    label: 'a table of contents keeps the short line in italics between two others',
    book: 'Vida Maravilhosa',
    page: 14,
    from: { line: /^Prossegue o desfile/, t: 0.02 },
    to: { line: /^Anomalocaris/, t: 0.97 },
    has: ['Prossegue o desfile', 'Wiwaxia, 215', 'Anomalocaris'],
  },
  {
    label: 'a line underlined in pencil, beside scribbles, copies its own words',
    book: 'Vida Maravilhosa',
    page: 17,
    from: { line: /^tivos dias de Burgess/, t: 0.55 },
    to: { line: /^tivos dias de Burgess/, t: 0.995 },
    has: ['novamente a partir desse'],
    hasNot: ['Sia', 'cia', 'mesmo ponto'],
  },
  {
    label: 'the next line keeps the end of its last word',
    book: 'Vida Maravilhosa',
    page: 17,
    from: { line: /^mesmo ponto/, t: 0.55 },
    to: { line: /^mesmo ponto/, t: 0.995 },
    has: ['semelhante à inteligência'],
  },
  {
    label: 'up the lines of a page printed the long way, whole, without the mark at 0° under them',
    book: 'TM9-1005-213-10',
    page: 79,
    from: { line: /^By Order of the Secretary/, t: 0.03 },
    to: { line: /^aircraft, M63/, t: 0.97 },
    has: ['By Order of the Secretary of the Army', 'To be distributed in accordance with DA Form', 'Operator Maintenance requirements for Machine Gun'],
    hasNot: ['Google'],
  },
];

/** A point of an inspected line `t` of the way along it, half its small letters up from it, in fractions of the page as shown. */
function onInspected(page, line, t) {
  const points = line.baseline;
  const lengths = points.slice(1).map((point, at) => Math.hypot(point[0] - points[at][0], point[1] - points[at][1]));
  const total = lengths.reduce((sum, length) => sum + length, 0);
  let walked = t * total;
  for (let at = 0; at < lengths.length; at++) {
    if (walked <= lengths[at] || at === lengths.length - 1) {
      const [a, b] = [points[at], points[at + 1]];
      const f = lengths[at] > 0 ? Math.min(1, walked / lengths[at]) : 0;
      const [dx, dy] = [(b[0] - a[0]) / lengths[at], (b[1] - a[1]) / lengths[at]];
      // Up across a line going right, with y growing downward: (dy, -dx).
      const rise = line.xHeight * 0.5;
      return [(a[0] + (b[0] - a[0]) * f + dy * rise) / page.width, (a[1] + (b[1] - a[1]) * f - dx * rise) / page.height];
    }
    walked -= lengths[at];
  }
  return [points[0][0] / page.width, points[0][1] / page.height];
}

/** A point of the page as shown, in the frame the page is stored in. */
function toStoredFrame([x, y], rotation) {
  return rotation === 1 ? [y, 1 - x] : rotation === 2 ? [1 - x, 1 - y] : rotation === 3 ? [1 - y, x] : [x, y];
}

/** Pages of two books of the real collection, only read: drags with the lines tool, and their copies. */
async function runReal(work) {
  const result = { drags: [] };
  report.real = result;
  const step = (label, ok, detail = {}) => {
    result.drags.push({ label, ok, ...detail });
    check(ok, `real: ${label} ${JSON.stringify(detail)}`);
  };
  if (options.real.length === 0) {
    throw new Error('--real: the reports of corpus-check --inspect --ocr on the pages are needed');
  }
  const inspected = options.real.map((path) => JSON.parse(readFileSync(path, 'utf8')));
  const before = listTree(options.corpus);
  const restore = saveClipboard(work);
  const library = join(work, 'biblioteca-real');
  const run = await launch(app, ['--library', library, '--e2e', '--width', '1400', '--height', '900'], 'window.musaeumApp !== undefined');
  const main = run.cdp;
  try {
    await waitFor(main, "document.querySelector('.welcome') !== null || document.querySelector('.library') !== null", { what: 'the first screen' });
    // Only the folders of the two books, so the library has little to index.
    const folders = [...new Set(inspected.map((report) => dirname(join(options.corpus, report.file))))];
    for (const folder of folders) {
      await main.evaluate(`window.musaeumApp.addRoot(${JSON.stringify(folder)})`);
    }
    const rows = (
      await waitFor(main, '(() => { const r = window.musaeumApp.rows(); const i = window.musaeumApp.library(); return r.total > 0 && i && !i.indexing && r.rows.every((b) => b.blake3 !== null) ? r : null; })()', {
        timeoutMs: 600_000,
        what: 'the books in the library',
      })
    ).rows;
    let open = null;
    for (const drag_ of REAL_DRAGS) {
      checkStillQuiet();
      const report_ = inspected.find((candidate) => basename(candidate.file).includes(drag_.book));
      const row = rows.find((candidate) => basename(candidate.path).includes(drag_.book));
      const page = report_?.pages.find((candidate) => candidate.page === drag_.page);
      if (report_ === undefined || row === undefined || page === undefined) {
        step(drag_.label, false, { missing: { report: report_ === undefined, book: row === undefined, page: page === undefined } });
        continue;
      }
      const from = page.lines.find((line) => drag_.from.line.test(line.text));
      const to = page.lines.find((line) => drag_.to.line.test(line.text));
      if (from === undefined || to === undefined) {
        step(drag_.label, false, { linesNotInspected: { from: from === undefined, to: to === undefined } });
        continue;
      }
      console.log(`  ${drag_.book}, page ${drag_.page}: ${drag_.label}`);
      if (open !== row.documentId) {
        if (open !== null) {
          await back(main);
        }
        await openBook(main, row.documentId);
        open = row.documentId;
        await click(main, 'button[aria-label="Selecionar por linhas da imagem"]');
      }
      const index = drag_.page - 1;
      await main.evaluate(`window.musaeumApp.goToPage(${index})`);
      await waitFor(main, `(async () => (await window.musaeumApp.inspectFixed())?.pages?.some((p) => p.page === ${index}))()`, { timeoutMs: 20_000, what: `page ${drag_.page} on the screen` });
      await linesTraced(main, index);
      await waitSharp(main);
      const start = await screenOf(main, index, ...toStoredFrame(onInspected(page, from, drag_.from.t), page.rotation), page.rotation);
      const end = await screenOf(main, index, ...toStoredFrame(onInspected(page, to, drag_.to.t), page.rotation), page.rotation);
      await drag(main, start, end);
      const region = await regionRead(main).catch(() => null);
      const copy = region === null ? null : await copied(main, work);
      const has = (drag_.has ?? []).filter((needle) => copy === null || !copy.includes(needle));
      const hasNot = (drag_.hasNot ?? []).filter((needle) => copy !== null && copy.includes(needle));
      step(drag_.label, copy !== null && has.length === 0 && hasNot.length === 0 && region.source === 'ocr', {
        page: drag_.page,
        copied: copy,
        source: region?.source ?? null,
        lines: region?.region?.lines ?? null,
        missing: has,
        unwanted: hasNot,
      });
      await press(main, 'Escape');
    }
  } finally {
    result.exitCode = await closeApp(run).catch((error) => String(error));
    result.log = run.log().slice(-20000);
    result.console = run.console.slice();
    result.clipboardRestored = restore();
    const after = listTree(options.corpus);
    result.unchanged = JSON.stringify(after) === JSON.stringify(before);
    result.listing = { entries: before.length };
    check(result.unchanged, 'the collection changed during the run');
  }
  return result;
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
  const work = mkdtempSync(join(tmpdir(), 'musaeum-e2e-annotations-'));
  try {
    if (options.phase === 'fixtures' || options.phase === 'all') {
      console.log('fixtures');
      await runFixtures(work);
    }
    if (options.phase === 'corpus' || options.phase === 'all') {
      console.log(`corpus: ${options.corpus}`);
      await runCorpus(work);
    }
    if (options.phase === 'lines' || options.phase === 'all') {
      console.log('lines');
      await runLines(work);
    }
    if (options.phase === 'ocr' || options.phase === 'all') {
      console.log('ocr');
      await runOcr(work);
    }
    if (options.phase === 'real') {
      console.log(`real pages: ${options.corpus}`);
      await runReal(work);
    }
  } finally {
    // The app's last writes may still hold a file when it has just closed: try again for a while,
    // and say so if something is left in %TEMP%.
    for (let attempt = 0; attempt < 20 && existsSync(work); attempt++) {
      try {
        rmSync(work, { recursive: true, force: true, maxRetries: 10, retryDelay: 300 });
      } catch {
        await delay(1000);
      }
    }
    report.workLeft = existsSync(work) ? work : null;
    if (report.workLeft !== null) {
      report.failures.push(`the work folder could not be removed: ${work}`);
    }
  }
}

let code = 0;
try {
  await main();
} catch (error) {
  report.error = error instanceof Error ? `${error.message}\n${error.stack}` : String(error);
  report.interrupted = error instanceof Interrupted;
  console.error(report.error);
  code = error instanceof Interrupted ? 3 : 2;
}
report.finished = new Date().toISOString();
mkdirSync(dirname(options.out), { recursive: true });
writeFileSync(options.out, JSON.stringify(report, null, 2));
if (code === 0 && report.failures.length > 0) {
  console.error(`${report.failures.length} failures`);
  code = 1;
} else if (code === 0) {
  console.log('All checks passed.');
}
process.exit(code);
