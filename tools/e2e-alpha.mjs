#!/usr/bin/env node
// The Alpha in the real window: the library, the reader over it, and the way back, driven from
// outside through the WebView2's debugging protocol, with the input a person would give.
//
//   node tools/e2e-alpha.mjs [--build target/release] [--out FILE.json] [--phase fixtures|corpus|all]
//                            [--corpus DIR] [--pdfs N] [--require-idle SECONDS]
//
// 1. Fixtures: a library made from a folder with one book in every flowing format (EPUB 3 and 2,
//    MOBI, AZW3, two FB2, TXT, Markdown) and a PDF, a comic and a DjVu book. It checks that the
//    library lists, searches and filters them, opens each one in the reader with a click and goes
//    back. In every flowing format it moves to the middle of the book and then changes the font
//    size, the line height, the margins, the font, the theme and the window, and switches between
//    paginated and continuous reading, checking after each change that the reading position is
//    the same character and that it is still on the page shown (paginated) or within one line of
//    the top of the screen (continuous). It closes and reopens each book and checks that the
//    library gave back exactly the position kept.
// 2. Corpus: a fresh library over a real collection, which is only read. It opens every EPUB and
//    every PDF in it from the library, and checks that the collection is the same afterwards.
//
// Every library and cache this creates is in a temporary folder, removed at the end. The window
// appears, so by default the run refuses to start unless nobody has touched the machine for a
// minute, and it stops as soon as someone does.

import { execFileSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, join, relative, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

import { click, closeApp, connectFrame, idleSeconds, launch, press, typeInto, waitFor } from './lib/cdp.mjs';
import { SEAM_TOLERANCE_PX, seamOracle } from './lib/seam-oracle.mjs';

const root = resolve(import.meta.dirname, '..');

function parseArguments(argv) {
  const options = {
    build: join(root, 'target', 'release'),
    out: join(root, 'bench-results', 'e2e-alpha', 'report.json'),
    phase: 'all',
    corpus: 'C:\\Marte\\Archives\\DigitalLibrary\\Books',
    pdfs: Infinity,
    requireIdle: 60,
  };
  for (let at = 0; at < argv.length; at++) {
    const flag = argv[at];
    const value = () => {
      const next = argv[++at];
      if (next === undefined) {
        throw new Error(`${flag} needs a value`);
      }
      return next;
    };
    if (flag === '--build') options.build = resolve(value());
    else if (flag === '--out') options.out = resolve(value());
    else if (flag === '--phase') options.phase = value();
    else if (flag === '--corpus') options.corpus = resolve(value());
    else if (flag === '--pdfs') options.pdfs = Number(value());
    else if (flag === '--require-idle') options.requireIdle = Number(value());
    else throw new Error(`unknown argument: ${flag}`);
  }
  return options;
}

const options = parseArguments(process.argv.slice(2));
const app = join(options.build, 'musaeum-desktop.exe');
const generator = join(options.build, 'gen-fixtures.exe');

/** Someone touched the machine: the run stops, closing its window, instead of getting in the way. */
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

const report = {
  started: new Date().toISOString(),
  options: { ...options, pdfs: Number.isFinite(options.pdfs) ? options.pdfs : 'all' },
  fixtures: null,
  corpus: null,
  failures: [],
};
const failures = report.failures;
function check(condition, message) {
  if (!condition) {
    failures.push(message);
    console.error(`  FAILED: ${message}`);
  }
  return condition;
}

// --- Page helpers ---

const appState = (cdp) => cdp.evaluate('window.musaeumApp.reader()');

async function waitForLibrary(cdp) {
  await waitFor(cdp, "window.musaeumApp.view() === 'library' && document.querySelector('.library') !== null", {
    what: 'the library',
  });
}

/** Opens a book from the library with a click on its card, and waits for the reader. */
async function openBook(cdp, documentId) {
  await waitFor(cdp, `document.querySelector('[data-document="${documentId}"]') !== null`, {
    timeoutMs: 15_000,
    what: `the card of document ${documentId}`,
  });
  const before = await cdp.evaluate('window.musaeumApp.opened()');
  await click(cdp, `[data-document="${documentId}"]`);
  // A click selects in a grid; the action is a click on a card already focused, or Enter.
  const started = Date.now();
  let opened = false;
  while (Date.now() - started < 2000) {
    if ((await cdp.evaluate('window.musaeumApp.opened()')) > before) {
      opened = true;
      break;
    }
    await delay(50);
  }
  if (!opened) {
    await press(cdp, 'Enter');
  }
  const state = await waitFor(
    cdp,
    `(() => { const r = window.musaeumApp.reader(); return r !== null && r.documentId === ${documentId} && (r.ready || r.error !== null) ? r : null; })()`,
    { timeoutMs: 60_000, what: `document ${documentId} to open` },
  ).catch(async (error) => {
    // What the page shows when a book does not open, to see why: its state and a picture of it.
    const name = `open-timeout-${documentId}-${Date.now()}`;
    const shot = await cdp.send('Page.captureScreenshot', { format: 'png' }).catch(() => null);
    if (shot !== null) {
      mkdirSync(dirname(options.out), { recursive: true });
      writeFileSync(join(dirname(options.out), `${name}.png`), Buffer.from(shot.data, 'base64'));
    }
    const facts = await cdp
      .evaluate(
        "({ view: window.musaeumApp.view(), opened: window.musaeumApp.opened(), reader: window.musaeumApp.reader(), iframes: [...document.querySelectorAll('iframe')].map((f) => f.src.slice(0, 90)), text: document.body.innerText.slice(0, 300) })",
      )
      .catch((reason) => String(reason));
    throw new Error(`${error.message}: ${JSON.stringify(facts).slice(0, 1500)} (picture: ${name}.png)`);
  });
  return { state, ms: Date.now() - started };
}

async function backToLibrary(cdp) {
  await click(cdp, '#back-to-library');
  await waitForLibrary(cdp);
}

/** Waits for the reader to finish what it was asked and for the screen to show it. */
async function settle(reader) {
  await reader.evaluate('window.musaeumReaderE2E.idle()');
  await reader.evaluate('new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))');
  await delay(450);
  await reader.evaluate('window.musaeumReaderE2E.idle()');
}

async function inspect(reader) {
  return reader.evaluate('window.musaeumReaderE2E.inspect()');
}

/** Whether the anchor is where it should be: on the page shown, or within a line of the top. */
function placed(view) {
  if (view.mode === 'continuous') {
    return view.top !== null && Math.abs(view.top) <= Math.max(view.lineHeight, 1) + 0.5;
  }
  return view.onPage === true;
}

async function typographyClick(cdp, id) {
  const open = await cdp.evaluate("document.querySelector('.typography') !== null");
  if (!open) {
    await click(cdp, '#typography');
    await waitFor(cdp, "document.querySelector('.typography') !== null", { timeoutMs: 5000, what: 'the typography panel' });
  }
  await click(cdp, `#${id}`);
}

async function closeTypography(cdp) {
  if (await cdp.evaluate("document.querySelector('.typography') !== null")) {
    await press(cdp, 'Escape');
    await delay(150);
  }
}

// --- The position gate, in one flowing book ---

async function flowGate(app_, book) {
  const main = app_.cdp;
  const result = { title: book.title, format: book.format, steps: [], restores: [], errors: [] };
  const step = async (reader, label, action, anchor) => {
    checkStillQuiet();
    await action();
    await settle(reader);
    const view = await inspect(reader);
    const same = view.anchor !== null && view.anchor.cfi === anchor.cfi;
    const ok = same && placed(view);
    result.steps.push({
      label,
      mode: view.mode,
      sameAnchor: same,
      placed: placed(view),
      top: view.top ?? null,
      left: view.left ?? null,
      lineHeight: view.lineHeight,
    });
    check(ok, `${book.format} ${book.path}: after ${label} (${view.mode}) the anchor ${same ? 'is the same but out of place' : 'moved'}: ${JSON.stringify({ before: anchor.cfi, after: view.anchor?.cfi, top: view.top, onPage: view.onPage })}`);
    return view;
  };

  // First opening: paginated with the page turn animated, which is how a new book opens.
  let opened = await openBook(main, book.documentId);
  if (!check(opened.state.error === null, `${book.path} did not open: ${opened.state.error}`)) {
    return result;
  }
  result.openMs = opened.ms;
  result.firstOpen = opened.state.flow;
  check(opened.state.flow.mode === 'paginated' && opened.state.flow.animated, `${book.path} did not open paginated and animated`);
  let reader = await connectFrame(app_, opened.state.flow.origin);
  await waitFor(reader, 'window.musaeumReaderE2E !== undefined', { what: 'the reader hooks' });
  await reader.evaluate('window.musaeumReaderE2E.goToFraction(0.37)');
  await settle(reader);
  const start = await inspect(reader);
  if (!check(start.anchor !== null && placed(start), `${book.path}: no anchor in the middle of the book`)) {
    reader.close();
    await backToLibrary(main);
    return result;
  }
  const anchor = start.anchor;

  // Paginated: everything that lays the text out again.
  await step(reader, 'font size +20%', async () => {
    await typographyClick(main, 'font-larger');
    await typographyClick(main, 'font-larger');
  }, anchor);
  await step(reader, 'line height 1.7', () => typographyClick(main, 'line-1-7'), anchor);
  await step(reader, 'wide margins', () => typographyClick(main, 'margin-wide'), anchor);
  await step(reader, 'serif font', () => typographyClick(main, 'font-serif'), anchor);
  await step(reader, 'sepia theme', () => typographyClick(main, 'theme-sepia'), anchor);
  await closeTypography(main);
  await step(reader, 'window 1100 × 760', () => main.evaluate('window.musaeumApp.resizeWindow(1100, 760)'), anchor);
  await step(reader, 'window 1400 × 900', () => main.evaluate('window.musaeumApp.resizeWindow(1400, 900)'), anchor);

  // C switches to continuous: the anchor's line goes to the top of the screen.
  await step(reader, 'C: continuous', () => press(main, 'c'), anchor);
  check((await appState(main)).flow.mode === 'continuous', `${book.path}: C did not switch to continuous`);
  await step(reader, 'font size −10%', async () => {
    await typographyClick(main, 'font-smaller');
  }, anchor);
  await step(reader, 'narrow margins', () => typographyClick(main, 'margin-narrow'), anchor);
  await step(reader, 'book font', () => typographyClick(main, 'font-publisher'), anchor);
  await step(reader, 'dark theme', () => typographyClick(main, 'theme-dark'), anchor);
  await closeTypography(main);
  await step(reader, 'window 1000 × 720', () => main.evaluate('window.musaeumApp.resizeWindow(1000, 720)'), anchor);
  await step(reader, 'window 1400 × 900', () => main.evaluate('window.musaeumApp.resizeWindow(1400, 900)'), anchor);
  await step(reader, 'C: paginated', () => press(main, 'c'), anchor);
  check((await appState(main)).flow.mode === 'paginated', `${book.path}: C did not switch back to paginated`);

  // A turns the page animation off; a page turn then moves the anchor to the next page.
  await press(main, 'a');
  await delay(200);
  check((await appState(main)).flow.animated === false, `${book.path}: A did not turn the animation off`);
  await press(main, 'ArrowRight');
  await settle(reader);
  const turned = await inspect(reader);
  result.turn = { from: anchor.cfi, to: turned.anchor?.cfi ?? null, placed: placed(turned), left: turned.left ?? null };
  check(
    turned.anchor !== null && turned.anchor.cfi !== anchor.cfi && placed(turned),
    `${book.path}: the page did not turn: ${JSON.stringify(result.turn)}`,
  );
  // Whatever the book carried, nothing in it ran: no section the reader loaded is marked.
  // How long each change took behind its crossfade, as the reader reported it.
  result.layouts = (await appState(main)).flow?.layouts ?? [];
  result.marked = await reader.evaluate(
    "[...document.querySelectorAll('iframe')].filter((frame) => frame.contentDocument?.documentElement?.hasAttribute('data-pwned')).length",
  );
  check(result.marked === 0, `${book.path}: a script of the book ran in ${result.marked} sections`);
  reader.close();

  // Closed and opened again: the library gives back exactly that position, in paginated mode.
  const restore = async (expected, mode) => {
    await backToLibrary(main);
    opened = await openBook(main, book.documentId);
    const flow = opened.state.flow;
    const again = await connectFrame(app_, flow.origin);
    await waitFor(again, 'window.musaeumReaderE2E !== undefined', { what: 'the reader hooks' });
    await settle(again);
    const view = await inspect(again);
    const exact = flow.restored !== null && flow.restored.readerCfi === expected.cfi;
    result.restores.push({
      mode,
      found: flow.restored?.found ?? null,
      exact,
      sameAnchor: view.anchor?.cfi === expected.cfi,
      placed: placed(view),
      cached: flow.cached,
      openMs: opened.ms,
    });
    check(exact && view.anchor?.cfi === expected.cfi && placed(view) && flow.mode === mode,
      `${book.path}: reopened ${flow.mode} at ${view.anchor?.cfi} (kept ${flow.restored?.readerCfi}), expected ${mode} at ${expected.cfi}`);
    return again;
  };
  reader = await restore(turned.anchor, 'paginated');

  // Now in continuous mode, scrolled by hand, then closed and opened again.
  await press(main, 'c');
  await settle(reader);
  await press(main, 'PageDown');
  await settle(reader);
  await press(main, 'ArrowDown');
  await settle(reader);
  const scrolled = await inspect(reader);
  check(scrolled.mode === 'continuous' && scrolled.anchor !== null, `${book.path}: no anchor after scrolling`);
  reader.close();
  reader = await restore(scrolled.anchor, 'continuous');
  // Where the normalizer cut a document, the parts meet as two paragraphs do inside one.
  result.seam = await checkSeam(reader, book.path);
  // A link out of the book (D12): the dialog shows the address, and the browser is asked to open it
  // only by the dialog's button. Under --e2e the app records the address instead of opening it. The
  // EPUB 3 fixture has a web link in every chapter; the section on the screen may not be one of them.
  let linked = false;
  const sectionCount = (await appState(main)).flow?.sections ?? 0;
  for (let section = 0; section < Math.min(sectionCount, 16) && !linked; section++) {
    await reader.evaluate(`window.musaeumReaderE2E.goToSection(${section})`);
    await settle(reader);
    linked = await reader.evaluate("window.musaeumReaderE2E.clickLink('https://example.org/')");
  }
  if (!linked) {
    check(basename(book.path) !== 'flow.epub', `${book.path}: no web link to click in any of the first sections`);
  }
  if (linked) {
    const before = (await main.evaluate('window.musaeumApp.openedLinks()')).length;
    await waitFor(main, "document.querySelector('.link-target') !== null", { timeoutMs: 5000, what: 'the link dialog' });
    const shown = await main.evaluate("document.querySelector('.link-target').textContent");
    const opensAlone = (await main.evaluate('window.musaeumApp.openedLinks()')).length !== before;
    await click(main, '#open-in-browser');
    await waitFor(main, "document.querySelector('.link-target') === null", { timeoutMs: 5000, what: 'the link dialog to close' });
    const opened = await main.evaluate('window.musaeumApp.openedLinks()');
    result.link = { shown, opensAlone, opened: opened.slice(before) };
    check(
      shown === 'https://example.org/' && !opensAlone && opened.length === before + 1 && opened.at(-1) === 'https://example.org/',
      `${book.path}: the link dialog did not open the address only from its button: ${JSON.stringify(result.link)}`,
    );
    await settle(reader);
  }
  // Back to how a new book opens, for the next one.
  await press(main, 'c');
  await press(main, 'a');
  await settle(reader);
  reader.close();
  await typographyClick(main, 'theme-light');
  await typographyClick(main, 'line-1-5');
  await typographyClick(main, 'margin-normal');
  await typographyClick(main, 'font-smaller');
  await closeTypography(main);
  await backToLibrary(main);
  return result;
}

/**
 * In continuous reading, goes to the first cut of the book and checks that the text after it is
 * where the uncut document has it.
 */
async function checkSeam(reader, label) {
  const cuts = await reader.evaluate('window.musaeumReaderE2E.cuts()');
  if (cuts.length === 0) {
    return null;
  }
  const cut = cuts[0];
  await reader.evaluate(`window.musaeumReaderE2E.goToSection(${cut})`);
  await settle(reader);
  const oracle = await reader.evaluate(`(${seamOracle})(${cut})`, 120_000);
  const seams = await reader.evaluate('window.musaeumReaderE2E.seams()');
  const seam = seams.find((candidate) => candidate.section === cut - 1) ?? null;
  check(
    oracle.measured && Math.abs(oracle.delta) <= SEAM_TOLERANCE_PX,
    `${label}: after the cut before section ${cut}, the text is not where the uncut document has it: ${JSON.stringify(oracle)}`,
  );
  return { ...oracle, gap: seam?.gap ?? null, space: seam?.space ?? null, lead: seam?.lead ?? null };
}

// --- A fixed-layout book, through the same library and the same bar ---

/** A PDF of three pages with a table of contents: two chapters and a section inside the first. */
function writeOutlinePdf(path) {
  const objects = [
    '<< /Type /Catalog /Pages 2 0 R /Outlines 6 0 R >>',
    '<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>',
    '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >>',
    '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >>',
    '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >>',
    '<< /Type /Outlines /First 7 0 R /Last 9 0 R /Count 3 >>',
    '<< /Title (Primeiro) /Parent 6 0 R /Next 9 0 R /First 8 0 R /Last 8 0 R /Count 1 /Dest [3 0 R /Fit] >>',
    '<< /Title (Uma se\\347\\343o) /Parent 7 0 R /A << /S /GoTo /D [4 0 R /XYZ 0 400 0] >> >>',
    '<< /Title (Segundo) /Parent 6 0 R /Prev 7 0 R /Dest [5 0 R /Fit] >>',
  ];
  let out = '%PDF-1.7\n';
  const offsets = [];
  objects.forEach((object, index) => {
    offsets.push(out.length);
    out += `${index + 1} 0 obj\n${object}\nendobj\n`;
  });
  const xref = out.length;
  out += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
  for (const offset of offsets) {
    out += `${String(offset).padStart(10, '0')} 00000 n \n`;
  }
  out += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  writeFileSync(path, out, 'latin1');
}

async function fixedGate(main, book) {
  const result = { title: book.title, format: book.format };
  let opened = await openBook(main, book.documentId);
  if (!check(opened.state.error === null, `${book.path} did not open: ${opened.state.error}`)) {
    return result;
  }
  result.openMs = opened.ms;
  const sharp = async () =>
    waitFor(main, `(async () => { const i = await window.musaeumApp.inspectFixed(); return i && i.sharp && !i.framePending ? i : null; })()`, {
      timeoutMs: 60_000,
      what: `${book.path} to be drawn`,
    });
  const first = await sharp();
  result.firstSharpMs = Date.now() - (Date.now() - opened.ms);
  const startMode = first.snapshot.mode?.kind ?? (await appState(main)).status?.mode;
  await press(main, 'c');
  await waitFor(main, `window.musaeumApp.reader().status?.mode !== ${JSON.stringify(startMode)}`, { timeoutMs: 5000, what: 'C to switch the mode' });
  await press(main, 'c');
  await waitFor(main, `window.musaeumApp.reader().status?.mode === ${JSON.stringify(startMode)}`, { timeoutMs: 5000, what: 'C to switch back' });
  await press(main, 'ArrowRight');
  await press(main, 'ArrowRight');
  await delay(900);
  // The table of contents, read from the bookmarks, and an entry followed to its page.
  const outline = await waitFor(main, 'window.musaeumApp.reader()?.outline ?? null', {
    timeoutMs: 20_000,
    what: `the outline of ${book.path}`,
  }).catch(() => null);
  result.outline = outline?.map((item) => `${item.level}:${item.page}:${item.label}`) ?? null;
  if (book.title === 'sumario') {
    check(
      JSON.stringify(result.outline) === JSON.stringify(['0:0:Primeiro', '1:1:Uma seção', '0:2:Segundo']),
      `${book.path}: the outline read was ${JSON.stringify(result.outline)}`,
    );
    await click(main, '#toc-toggle');
    await waitFor(main, "document.querySelector('.toc-flat') !== null", { timeoutMs: 5000, what: 'the contents panel' });
    await click(main, '.toc-flat > li:nth-child(3) .toc-entry');
    const reached = await waitFor(main, 'window.musaeumApp.reader()?.status?.page === 2', { timeoutMs: 10_000, what: 'the page of "Segundo"' }).catch(() => false);
    result.tocJump = reached === true;
    check(reached === true, `${book.path}: the entry "Segundo" did not lead to page 3`);
  }
  const before = (await appState(main)).status;
  result.pageCount = before?.pageCount;
  await backToLibrary(main);
  opened = await openBook(main, book.documentId);
  await sharp();
  const after = (await appState(main)).status;
  result.restoredPage = after?.page;
  check(before !== null && after !== null && after.page === before.page && after.mode === before.mode,
    `${book.path}: reopened at page ${after?.page} (${after?.mode}), left at ${before?.page} (${before?.mode})`);
  await backToLibrary(main);
  return result;
}

// --- Phase 1: fixtures ---

async function runFixtures(work) {
  const books = join(work, 'livros');
  const flowDir = join(books, 'fluidos');
  const fixedDir = join(books, 'fixos');
  mkdirSync(flowDir, { recursive: true });
  mkdirSync(fixedDir, { recursive: true });
  execFileSync(generator, ['--out', flowDir, '--only', 'flow'], { stdio: 'ignore', windowsHide: true });
  // The hostile book of Phase 0, through the app's own path: normalized, from the cache.
  copyFileSync(join(root, 'fixtures', 'hostile.epub'), join(flowDir, 'hostile.epub'));
  const fixtures = join(root, 'fixtures');
  for (const name of ['mixed.pdf', 'comic.cbz', 'book.djvu']) {
    const from = join(fixtures, name);
    if (!existsSync(from)) {
      throw new Error(`${from} is missing: run gen-fixtures first`);
    }
    copyFileSync(from, join(fixedDir, name));
  }
  writeOutlinePdf(join(fixedDir, 'sumario.pdf'));
  const expected = readdirSync(flowDir).length + readdirSync(fixedDir).length;

  const library = join(work, 'biblioteca');
  const run = await launch(app, ['--library', library, '--e2e', '--width', '1400', '--height', '900'], 'window.musaeumApp !== undefined');
  const main = run.cdp;
  const result = { expected, library: {}, flow: [], fixed: [] };
  report.fixtures = result;
  try {
    await waitFor(main, "document.querySelector('.welcome') !== null || document.querySelector('.library') !== null", { what: 'the first screen' });
    result.library.welcome = await main.evaluate("document.querySelector('.welcome') !== null");
    await main.evaluate(`window.musaeumApp.addRoot(${JSON.stringify(books)})`);
    let listed = await waitFor(main, `(() => { const r = window.musaeumApp.rows(); return r.total === ${expected} ? r : null; })()`, {
      timeoutMs: 120_000,
      what: `${expected} books in the library`,
    });
    await waitFor(main, `document.querySelectorAll('.book-card').length === ${expected}`, { what: 'every card on the grid' });
    result.library.listed = listed.total;
    // Titles and authors come from the files once they are read; searching waits for that.
    await waitFor(
      main,
      '(() => { const i = window.musaeumApp.library(); const r = window.musaeumApp.rows(); return i !== null && !i.indexing && r.rows.every((b) => b.blake3 !== null); })()',
      { timeoutMs: 180_000, what: 'the library to finish reading the files' },
    );
    listed = await main.evaluate('window.musaeumApp.rows()');
    result.library.titles = listed.rows.map((row) => `${row.format}: ${row.title}`);

    // Search, as typed. Every wait is for the listing of exactly that search and that folder.
    const listing = (text, folder) =>
      waitFor(
        main,
        `(() => { const r = window.musaeumApp.rows(); return r.text === ${JSON.stringify(text)} && r.folder === ${JSON.stringify(folder)} ? r : null; })()`,
        { timeoutMs: 8000, what: `the listing for "${text}" in folder ${folder}` },
      );
    const searches = [];
    for (const row of [
      listed.rows.find((candidate) => candidate.format === 'epub'),
      listed.rows.find((candidate) => candidate.format === 'pdf'),
      listed.rows.find((candidate) => candidate.format === 'djvu'),
    ].filter((candidate) => candidate !== undefined)) {
      const word = (row.title.split(/\s+/).find((part) => part.length >= 4) ?? row.title).toLowerCase();
      await typeInto(main, '.search input', word);
      let found = await listing(word, null).catch(() => null);
      let retried = null;
      if (found === null) {
        // What the page shows when the listing never came, then one more try.
        retried = await main.evaluate(
          "({ value: document.querySelector('.search input')?.value, active: document.activeElement?.tagName + '.' + document.activeElement?.className, rows: (({ text, folder, total }) => ({ text, folder, total }))(window.musaeumApp.rows()) })",
        );
        await typeInto(main, '.search input', word);
        found = await listing(word, null).catch(() => null);
      }
      searches.push({ word, total: found?.total ?? null, titles: found?.rows.map((b) => b.title) ?? [], retried });
      check(found !== null && found.rows.some((b) => b.documentId === row.documentId), `searching "${word}" did not find ${row.title}`);
    }
    await typeInto(main, '.search input', 'xyzzynothing');
    const nothing = await listing('xyzzynothing', null).catch(() => null);
    check(nothing !== null && nothing.total === 0, 'a search for nothing found something');
    await waitFor(main, "document.querySelector('.empty') !== null", { timeoutMs: 5000, what: 'the empty search message' });
    await click(main, '.search-clear');
    const cleared = await listing('', null).catch(() => null);
    check(cleared !== null && cleared.total === expected, 'clearing the search did not list every book');
    result.library.search = searches;

    // Folders.
    const folders = await main.evaluate('window.musaeumApp.folders()');
    const inFolder = async (name) => {
      const folder = folders.find((entry) => entry.name === name);
      await click(main, `.folder-item:has(.folder-name[title="${name}"])`);
      const shown = await listing('', folder?.id ?? -1).catch(() => null);
      return shown?.total ?? null;
    };
    result.library.folders = { fluidos: await inFolder('fluidos'), fixos: await inFolder('fixos') };
    check(result.library.folders.fluidos === readdirSync(flowDir).length, `the folder "fluidos" lists ${result.library.folders.fluidos}`);
    check(result.library.folders.fixos === 4, `the folder "fixos" lists ${result.library.folders.fixos}`);
    await click(main, '.folder-item:first-child');
    const all = await listing('', null).catch(() => null);
    check(all !== null && all.total === expected, '"Todos os livros" did not list every book');

    // The table, then back to covers.
    await click(main, '.segmented .segment:last-child');
    await waitFor(main, `document.querySelectorAll('.book-row').length === ${expected}`, { timeoutMs: 5000, what: 'the table' });
    await click(main, '.segmented .segment:first-child');
    await waitFor(main, `document.querySelectorAll('.book-card').length === ${expected}`, { timeoutMs: 5000, what: 'the covers' });

    // Covers, as the workers draw them.
    const covers = await waitFor(main, `(() => { const n = [...document.querySelectorAll('img.cover')].filter((i) => i.complete && i.naturalWidth > 0).length; return n >= 6 ? n : 0; })()`, {
      timeoutMs: 120_000,
      what: 'covers to be drawn',
    }).catch(() => 0);
    result.library.covers = covers;
    check(covers >= 6, `only ${covers} covers were drawn`);

    const rows = (await main.evaluate('window.musaeumApp.rows()')).rows;
    for (const row of rows.filter((candidate) => candidate.layout === 'flow')) {
      console.log(`  ${row.format}: ${basename(row.path)}`);
      try {
        result.flow.push(await flowGate(run, row));
      } catch (error) {
        if (error instanceof Interrupted) throw error;
        check(false, `${row.path}: ${error.message}`);
        await backToLibrary(main).catch(() => {});
      }
    }
    for (const row of rows.filter((candidate) => candidate.layout === 'fixed')) {
      console.log(`  ${row.format}: ${basename(row.path)}`);
      try {
        result.fixed.push(await fixedGate(main, row));
      } catch (error) {
        if (error instanceof Interrupted) throw error;
        check(false, `${row.path}: ${error.message}`);
        await backToLibrary(main).catch(() => {});
      }
    }
  } finally {
    result.exitCode = await closeApp(run).catch((error) => String(error));
    result.log = run.log().slice(-20000);
    result.console = run.console.slice();
  }
  return result;
}

// --- Phase 2: the real collection, only read ---

function listTree(directory) {
  const entries = [];
  const walk = (folder) => {
    for (const name of readdirSync(folder)) {
      const path = join(folder, name);
      const stat = statSync(path, { bigint: true });
      entries.push({
        path: relative(directory, path),
        directory: stat.isDirectory(),
        size: String(stat.size),
        modified: String(stat.mtimeNs),
        created: String(stat.birthtimeNs),
        mode: Number(stat.mode),
      });
      if (stat.isDirectory()) {
        walk(path);
      }
    }
  };
  walk(directory);
  return entries.sort((a, b) => a.path.localeCompare(b.path));
}

async function openFromSearch(main, row) {
  const words = row.title.replace(/[^\p{L}\p{N}\s]/gu, ' ').split(/\s+/).filter((w) => w.length > 0).slice(0, 4).join(' ');
  const text = words.length > 0 ? words : basename(row.path).replace(/\.[^.]+$/, '');
  await typeInto(main, '.search input', text);
  // The listing of exactly what was typed, so the click never lands while it changes.
  await waitFor(
    main,
    `(() => { const r = window.musaeumApp.rows(); return r.text === ${JSON.stringify(text)} && r.rows.some((b) => b.documentId === ${row.documentId}); })()`,
    { timeoutMs: 8000, what: `the search "${text}" to find ${row.title}` },
  );
  return openBook(main, row.documentId);
}

async function runCorpus(work) {
  const before = listTree(options.corpus);
  const library = join(work, 'biblioteca-acervo');
  const run = await launch(app, ['--library', library, '--e2e', '--width', '1400', '--height', '900'], 'window.musaeumApp !== undefined');
  const main = run.cdp;
  const result = { files: before.filter((entry) => !entry.directory).length, epubs: [], pdfs: [] };
  report.corpus = result;
  try {
    await waitFor(main, "document.querySelector('.welcome') !== null || document.querySelector('.library') !== null", { what: 'the first screen' });
    await main.evaluate(`window.musaeumApp.addRoot(${JSON.stringify(options.corpus)})`);
    // The listing is done when the count stops growing for a while.
    let total = -1;
    let stableSince = Date.now();
    const deadline = Date.now() + 600_000;
    while (Date.now() < deadline) {
      const now = (await main.evaluate('window.musaeumApp.rows()')).total;
      if (now !== total) {
        total = now;
        stableSince = Date.now();
      } else if (total > 0 && Date.now() - stableSince > 8000) {
        break;
      }
      await delay(500);
    }
    result.listed = total;
    // Titles come from the files once they are read, and the search finds books by them.
    const indexingStarted = Date.now();
    // A long wait with the window up: it stops as soon as someone uses the machine.
    for (const until = Date.now() + 1_800_000; ; ) {
      const done = await main.evaluate(
        '(() => { const i = window.musaeumApp.library(); const r = window.musaeumApp.rows(); return i !== null && !i.indexing && r.rows.every((b) => b.blake3 !== null); })()',
      );
      if (done) {
        break;
      }
      if (Date.now() > until) {
        throw new Error('the library did not finish reading the collection in 30 minutes');
      }
      checkStillQuiet();
      await delay(3000);
    }
    result.indexingMs = Date.now() - indexingStarted;
    const rows = (await main.evaluate('window.musaeumApp.rows()')).rows;
    const epubs = rows.filter((row) => row.format === 'epub');
    const pdfs = rows.filter((row) => row.format === 'pdf').slice(0, options.pdfs);
    for (const row of epubs) {
      checkStillQuiet();
      console.log(`  epub: ${row.title}`);
      const entry = { title: row.title, path: row.path };
      try {
        const opened = await openFromSearch(main, row);
        entry.openMs = opened.ms;
        entry.error = opened.state.error;
        if (check(opened.state.error === null, `${row.path}: ${opened.state.error}`)) {
          const flow = opened.state.flow;
          entry.sections = flow.sections;
          entry.cached = flow.cached;
          entry.hostOpenMs = flow.openMs;
          const reader = await connectFrame(run, flow.origin);
          await waitFor(reader, 'window.musaeumReaderE2E !== undefined', { what: 'the reader hooks' });
          await reader.evaluate('window.musaeumReaderE2E.goToFraction(0.5)');
          await settle(reader);
          const middle = await inspect(reader);
          await typographyClick(main, 'font-larger');
          await settle(reader);
          const larger = await inspect(reader);
          await typographyClick(main, 'font-smaller');
          await closeTypography(main);
          await settle(reader);
          entry.anchorKept = middle.anchor !== null && larger.anchor?.cfi === middle.anchor.cfi && placed(larger);
          check(entry.anchorKept, `${row.path}: the anchor moved with the font size`);
          reader.close();
          await backToLibrary(main);
          const again = await openFromSearch(main, row);
          entry.reopenMs = again.ms;
          entry.restoredExact = again.state.flow?.restored?.readerCfi === middle.anchor?.cfi;
          check(entry.restoredExact, `${row.path}: reopened at ${again.state.flow?.restored?.readerCfi}, kept ${middle.anchor?.cfi}`);
          // A book the normalizer cut: in continuous reading, the parts meet without a seam.
          if (again.state.flow !== null) {
            const reopened = await connectFrame(run, again.state.flow.origin);
            await waitFor(reopened, 'window.musaeumReaderE2E !== undefined', { what: 'the reader hooks' });
            await press(main, 'c');
            await settle(reopened);
            entry.seam = await checkSeam(reopened, row.path);
            await press(main, 'c');
            await settle(reopened);
          }
        }
      } catch (error) {
        if (error instanceof Interrupted) throw error;
        check(false, `${row.path}: ${error.message}`);
      }
      await backToLibrary(main).catch(() => {});
      result.epubs.push(entry);
    }
    for (const row of pdfs) {
      checkStillQuiet();
      const entry = { title: row.title, path: row.path };
      try {
        const opened = await openFromSearch(main, row);
        entry.error = opened.state.error;
        if (check(opened.state.error === null, `${row.path}: ${opened.state.error}`)) {
          const started = Date.now();
          const shown = await waitFor(main, `(async () => { const i = await window.musaeumApp.inspectFixed(); return i && i.sharp && !i.framePending ? i : null; })()`, {
            timeoutMs: 90_000,
            what: `${row.title} to be drawn`,
          });
          entry.openMs = opened.ms;
          entry.sharpAfterOpenMs = Date.now() - started;
          entry.pages = (await appState(main)).status?.pageCount ?? null;
          entry.shown = shown.pages.length;
          // The table of contents, from the bookmarks: read, shown, and followed to its page.
          const outline = await waitFor(main, 'window.musaeumApp.reader()?.outline ?? null', {
            timeoutMs: 20_000,
            what: `the outline of ${row.title}`,
          }).catch(() => null);
          entry.outline = outline === null ? null : outline.length;
          const target = outline?.findLast((item) => item.page !== null && item.page > 0) ?? null;
          if (target !== null) {
            await click(main, '#toc-toggle');
            await waitFor(main, "document.querySelector('.toc-flat') !== null", { timeoutMs: 5000, what: 'the contents panel' });
            await click(main, `.toc-flat > li:nth-child(${outline.indexOf(target) + 1}) .toc-entry`);
            const reached = await waitFor(main, `window.musaeumApp.reader()?.status?.page === ${target.page}`, {
              timeoutMs: 15_000,
              what: `the page of "${target.label}"`,
            }).catch(() => false);
            entry.tocJump = { label: target.label, page: target.page, reached: reached === true };
            check(reached === true, `${row.path}: the entry "${target.label}" did not lead to page ${target.page + 1}`);
          }
        }
      } catch (error) {
        if (error instanceof Interrupted) throw error;
        check(false, `${row.path}: ${error.message}`);
      }
      await backToLibrary(main).catch(() => {});
      result.pdfs.push(entry);
      process.stdout.write('.');
    }
    process.stdout.write('\n');
  } finally {
    result.exitCode = await closeApp(run).catch((error) => String(error));
    result.log = run.log().slice(-20000);
    result.console = run.console.slice();
    const after = listTree(options.corpus);
    const same = JSON.stringify(after) === JSON.stringify(before);
    result.unchanged = same;
    result.listing = { entries: before.length, bytes: before.reduce((sum, entry) => sum + (entry.directory ? 0 : Number(entry.size)), 0) };
    check(same, 'the collection changed during the run');
  }
  return result;
}

// --- Run ---

async function main() {
  for (const needed of [app, generator]) {
    if (!existsSync(needed)) {
      throw new Error(`${needed} is missing: run npm run build:app first`);
    }
  }
  if (options.requireIdle > 0) {
    const idle = idleSeconds();
    if (idle < options.requireIdle) {
      throw new Interrupted(`someone used the machine ${idle} s ago; this opens windows, so it waits for ${options.requireIdle} s of quiet`);
    }
    quietSince = idle;
  }
  const work = mkdtempSync(join(tmpdir(), 'musaeum-e2e-alpha-'));
  try {
    if (options.phase === 'fixtures' || options.phase === 'all') {
      console.log('fixtures');
      report.fixtures = await runFixtures(work);
    }
    if (options.phase === 'corpus' || options.phase === 'all') {
      console.log(`corpus: ${options.corpus}`);
      report.corpus = await runCorpus(work);
    }
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

let code = 0;
try {
  await main();
} catch (error) {
  report.error = error instanceof Error ? error.message : String(error);
  report.interrupted = error instanceof Interrupted;
  console.error(report.error);
  code = error instanceof Interrupted ? 3 : 2;
}
report.finished = new Date().toISOString();
mkdirSync(dirname(options.out), { recursive: true });
writeFileSync(options.out, JSON.stringify(report, null, 2));
if (failures.length > 0) {
  console.error(`${failures.length} failures:`);
  for (const failure of failures) {
    console.error(`  ${failure}`);
  }
  code = code || 1;
} else if (code === 0) {
  console.log('All checks passed.');
}
process.exit(code);
