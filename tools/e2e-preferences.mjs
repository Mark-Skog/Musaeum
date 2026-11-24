#!/usr/bin/env node
// Phase 8, Part 1, in the real window, driven through the WebView2's debugging protocol with the
// input a person gives: the Preferences, the two languages, the design's themes and motion, and the
// keyboard.
//
//   node tools/e2e-preferences.mjs [--build target/release] [--out FILE.json] [--require-idle SECONDS]
//
// - The defaults, with no preferences.db written until something changes.
// - The dialog by the keyboard: Ctrl+, opens it, Tab and Shift+Tab stay inside it, Escape closes it
//   and the focus goes back where it was. The details beside the books and the reader's contents
//   give the focus back to the books and to their button when they close.
// - The themes: data-theme on the root and the page's own background, light and dark; the system's
//   when none is chosen. Reduced motion: no transition on the page, and the fixed reader stops
//   animating its page turns while it keeps the person's own choice for later.
// - The language, at once and without starting again: the interface's words, and what the Rust
//   side writes (a refusal, the PDF's /Subj, the XFDF's subject, the study report's labels) in
//   pt-BR and then in English.
// - The annotations' author: in the PDF copy's /T, the XFDF's title, and the report's line.
// - Clearing the cache of lines and OCR: the annotations are all still there.
// - Everything again after the app is closed and opened: kept in preferences.db, beside the run's
//   library, never in a folder of books.
//
// Everything lives in a temporary folder, removed at the end. The run waits for the machine to be
// free and stops as soon as someone uses it (exit code 3).

import { execFileSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

import { click, closeApp, idleSeconds, launch, press, typeInto, waitFor } from './lib/cdp.mjs';

const root = resolve(import.meta.dirname, '..');
const SCALE_ONE = 4096;
const CTRL = 2;
const SHIFT = 8;
const FIXED_TIME = 1_795_466_405_123;

function parseArguments(argv) {
  const options = {
    build: join(root, 'target', 'release'),
    out: join(root, 'bench-results', 'e2e-preferences', 'report.json'),
    requireIdle: 60,
  };
  for (let at = 0; at < argv.length; at++) {
    const flag = argv[at];
    const value = () => argv[++at];
    if (flag === '--build') options.build = resolve(value());
    else if (flag === '--out') options.out = resolve(value());
    else if (flag === '--require-idle') options.requireIdle = Number(value());
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

// --- The catalogs, read from the sources, so the words checked are the ones the app shows ---

function catalog(locale) {
  const folder = join(root, 'apps', 'desktop', 'src', 'app', 'i18n', locale);
  const entries = {};
  for (const name of readdirSync(folder)) {
    const text = readFileSync(join(folder, name), 'utf8');
    for (const match of text.matchAll(/^\s+'([\w.]+)':\s*'((?:[^'\\]|\\.)*)',?$/gm)) {
      entries[match[1]] = match[2].replace(/\\'/g, "'");
    }
  }
  return entries;
}
const PT = catalog('pt-BR');
const EN = catalog('en');

// --- The app ---

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

async function waitSharp(main) {
  return waitFor(main, '(async () => { const i = await window.musaeumApp.inspectFixed(); return i && i.sharp && !i.framePending && !i.moving ? i : null; })()', {
    timeoutMs: 60_000,
    what: 'the pages to be drawn sharp',
  });
}

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

async function layerOf(main, page) {
  await waitFor(main, `(async () => (await window.musaeumApp.inspectFixed())?.charPages?.includes(${page}))()`, {
    timeoutMs: 20_000,
    what: `the compositor to have page ${page}'s characters`,
  });
  const bytes = await waitFor(main, `window.musaeumApp.pageChars(${page})`, { timeoutMs: 20_000, what: `page ${page}'s characters` });
  return decodeChars(bytes);
}

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

async function aim(main, page, needle) {
  const layer = await layerOf(main, page);
  const start = layer.text.indexOf(needle);
  if (start < 0) {
    throw new Error(`"${needle}" is not on page ${page}: ${layer.text.slice(0, 200)}`);
  }
  const end = start + needle.length - 1;
  const a = await screenOf(main, page, ...centerOf(layer.chars[start].quad), layer.rotation);
  const b = await screenOf(main, page, ...centerOf(layer.chars[end].quad), layer.rotation);
  const stride = { x: (b.x - a.x) / Math.max(1, needle.length - 1), y: (b.y - a.y) / Math.max(1, needle.length - 1) };
  return { from: { x: a.x - stride.x * 0.45, y: a.y - stride.y * 0.45 }, to: { x: b.x + stride.x * 0.45, y: b.y + stride.y * 0.45 } };
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

async function written(main, count, what) {
  return waitFor(main, `(() => { const a = window.musaeumApp.reader()?.annotations ?? []; return a.length === ${count} && a.every((e) => !e.pending) ? a : null; })()`, {
    timeoutMs: 30_000,
    what,
  });
}

async function pdf(main, action, path) {
  return waitFor(main, `window.musaeumApp.pdf(${JSON.stringify(action)}, ${JSON.stringify(path)})`, { timeoutMs: 180_000, what: action });
}

async function exchange(main, surface, action, path) {
  return waitFor(main, `window.musaeumApp.exchange(${JSON.stringify(surface)}, ${JSON.stringify(action)}, ${JSON.stringify(path)}, ${FIXED_TIME})`, {
    timeoutMs: 300_000,
    what: `${surface} ${action}`,
  });
}

/** Changes preferences as the dialog does, and waits for the page to have applied them. */
async function setPreferences(main, patch) {
  const answer = await main.evaluate(`window.musaeumApp.setPreferences(${JSON.stringify(patch)}).then((view) => view.preferences)`);
  if (patch.language !== undefined) {
    await waitFor(main, `document.documentElement.lang === ${JSON.stringify(patch.language)}`, { timeoutMs: 5000, what: `the page in ${patch.language}` });
  }
  return answer;
}

const activeInside = (main, selector) => main.evaluate(`document.activeElement?.closest(${JSON.stringify(selector)}) != null`);
const activeId = (main) => main.evaluate('document.activeElement?.id ?? null');
const textOf = (main, selector) => main.evaluate(`document.querySelector(${JSON.stringify(selector)})?.textContent?.trim() ?? null`);
const background = (main) => main.evaluate('getComputedStyle(document.body).backgroundColor');
const dialogOpen = (main) => main.evaluate("document.querySelector('[data-surface=\"preferences\"]') !== null");

async function keyboardInDialog(main, label) {
  // Opened from its button: the focus comes back to it.
  await click(main, '#open-preferences');
  await waitFor(main, "document.querySelector('[data-surface=\"preferences\"]') !== null", { timeoutMs: 5000, what: 'the Preferences' });
  await delay(200);
  const inside = [];
  for (let n = 0; n < 40; n++) {
    await press(main, 'Tab');
    inside.push(await activeInside(main, '[data-surface="preferences"]'));
  }
  for (let n = 0; n < 15; n++) {
    await press(main, 'Tab', SHIFT);
    inside.push(await activeInside(main, '[data-surface="preferences"]'));
  }
  await press(main, 'Escape');
  await waitFor(main, "document.querySelector('[data-surface=\"preferences\"]') === null", { timeoutMs: 5000, what: 'the Preferences to close' });
  await delay(150);
  const back = await activeId(main);
  step(`${label}: Tab and Shift+Tab stay in the Preferences, Escape closes them and the focus goes back to its button`, inside.every(Boolean) && back === 'open-preferences', {
    outside: inside.filter((value) => !value).length,
    back,
  });
  // And Ctrl+, opens them from anywhere.
  await press(main, ',', CTRL);
  const opened = await waitFor(main, "document.querySelector('[data-surface=\"preferences\"]') !== null", { timeoutMs: 5000, what: 'Ctrl+,' }).catch(() => false);
  const focusIn = opened ? await activeInside(main, '[data-surface="preferences"]') : false;
  await press(main, 'Escape');
  await waitFor(main, "document.querySelector('[data-surface=\"preferences\"]') === null", { timeoutMs: 5000, what: 'the Preferences to close' });
  step(`${label}: Ctrl+, opens the Preferences with the focus in them`, opened === true && focusIn);
}

async function run(work) {
  const generated = join(work, 'gerados');
  const shelf = join(work, 'estante');
  const exported = join(work, 'exportados');
  const library = join(work, 'biblioteca');
  for (const folder of [generated, shelf, exported]) {
    mkdirSync(folder, { recursive: true });
  }
  execFileSync(join(options.build, 'gen-fixtures.exe'), ['--only', 'annot', '--out', generated], { windowsHide: true });
  copyFileSync(join(generated, 'annot-streams.pdf'), join(shelf, 'livro.pdf'));
  const preferencesFile = join(library, 'preferences.db');
  const args = ['--library', library, '--e2e', '--width', '1400', '--height', '950'];

  let session = await launch(app, args, 'window.musaeumApp !== undefined');
  let main = session.cdp;
  try {
    await waitFor(main, "document.querySelector('.welcome') !== null || document.querySelector('.library') !== null", { what: 'the first screen' });

    // 1. The defaults, and nothing written for them.
    const defaults = await main.evaluate('window.musaeumApp.preferences().preferences');
    step(
      'the defaults: pt-BR, the system theme and motion, no author, OCR in por and eng, nothing written',
      defaults.language === 'pt-BR' && defaults.theme === 'system' && defaults.motion === 'system' && defaults.annotationAuthor === '' &&
        JSON.stringify(defaults.ocrLanguages) === '["por","eng"]' && !existsSync(preferencesFile),
      { defaults, written: existsSync(preferencesFile) },
    );

    await main.evaluate(`window.musaeumApp.addRoot(${JSON.stringify(shelf)})`);
    const row = await rowFor(main, 'livro.pdf');
    await waitForLibrary(main);

    // 2. The keyboard in the Preferences.
    await keyboardInDialog(main, 'library');

    // 3. The themes, chosen in the dialog.
    await click(main, '#open-preferences');
    await waitFor(main, "document.querySelector('[data-surface=\"preferences\"]') !== null", { timeoutMs: 5000, what: 'the Preferences' });
    await click(main, '#prefs-theme-dark');
    await waitFor(main, "document.documentElement.dataset.theme === 'dark'", { timeoutMs: 5000, what: 'the dark theme' });
    const dark = await background(main);
    await click(main, '#prefs-theme-light');
    await waitFor(main, "document.documentElement.dataset.theme === 'light'", { timeoutMs: 5000, what: 'the light theme' });
    const light = await background(main);
    await click(main, '#prefs-theme-system');
    await waitFor(main, 'document.documentElement.dataset.theme === undefined', { timeoutMs: 5000, what: 'the system theme' });
    step('the theme: dark and light paint the page at once, and the system\'s takes no data-theme', dark === 'rgb(24, 25, 27)' && light === 'rgb(246, 245, 242)', { dark, light });

    // 4. Reduced motion, in the dialog: no transition left on the page.
    await click(main, '#prefs-motion-reduce');
    await waitFor(main, "document.documentElement.dataset.motion === 'reduce'", { timeoutMs: 5000, what: 'reduced motion' });
    const still = await main.evaluate("getComputedStyle(document.querySelector('#prefs-close')).transitionDuration");
    await click(main, '#prefs-motion-system');
    await waitFor(main, "document.documentElement.dataset.motion !== 'reduce' || matchMedia('(prefers-reduced-motion: reduce)').matches", { timeoutMs: 5000, what: 'motion again' });
    const moving = await main.evaluate("getComputedStyle(document.querySelector('#prefs-close')).transitionDuration");
    step('reduced motion takes every transition away, and the system\'s choice gives them back', still.split(',').every((part) => part.trim() === '0s') && moving !== still, { still, moving });

    // 5. The author, typed and saved.
    await typeInto(main, 'input#prefs-author, #prefs-author input', 'Ana Lima');
    await click(main, '#prefs-author-save');
    await waitFor(main, "window.musaeumApp.preferences()?.preferences?.annotationAuthor === 'Ana Lima'", { timeoutMs: 5000, what: 'the author to be kept' });
    // Its button turns off once the name is kept; the focus goes back to the name, in the dialog.
    await delay(150);
    const focusAfterSave = await activeInside(main, '[data-surface="preferences"]');
    step('the author is kept, and preferences.db is written beside the library, not in the shelf', existsSync(preferencesFile) && !readdirSync(shelf).some((name) => name.startsWith('preferences')), {
      file: preferencesFile,
    });
    step('after saving the author, the focus stays in the dialog', focusAfterSave === true);

    // 6. The language, at once.
    await click(main, '#prefs-language-en');
    await waitFor(main, "document.documentElement.lang === 'en'", { timeoutMs: 5000, what: 'English' });
    const heading = await textOf(main, '[data-surface="preferences"] .dialog-title');
    await press(main, 'Escape');
    await waitFor(main, "document.querySelector('[data-surface=\"preferences\"]') === null", { timeoutMs: 5000, what: 'the Preferences to close' });
    const button = await textOf(main, '#open-preferences');
    const grid = await main.evaluate("document.querySelector('.book-grid, .book-table')?.getAttribute('aria-label') ?? null");
    step('English at once: the dialog, the toolbar and the books\' grid', heading === EN['prefs.title'] && button === EN['prefs.open'] && grid === EN['library.books.label'], { heading, button, grid });

    // 7. The details beside the books: Escape in them goes back to the books, and Escape there
    // puts them away, the focus staying with the books.
    await click(main, `[data-document="${row.documentId}"]`);
    await waitFor(main, "document.querySelector('.inspector') !== null", { timeoutMs: 10_000, what: 'the details' });
    // The details read the work first; their fields come after.
    await waitFor(main, "document.querySelector('.inspector input') !== null", { timeoutMs: 10_000, what: 'the fields of the details' });
    await main.evaluate("document.querySelector('.inspector input').focus()");
    const inDetails = await activeInside(main, '.inspector');
    await press(main, 'Escape');
    await delay(150);
    const toBooks = await activeInside(main, '.shelf');
    await press(main, 'Escape');
    await waitFor(main, "document.querySelector('.inspector') === null", { timeoutMs: 5000, what: 'the details to close' });
    await delay(150);
    const stillBooks = await activeInside(main, '.shelf');
    step('the details: Escape goes back to the books, Escape again puts the details away, the focus stays with the books', inDetails && toBooks && stillBooks, { inDetails, toBooks, stillBooks });

    // 8. In the reader, in English: a highlight, and everything the Rust side writes.
    await openBook(main, row.documentId);
    await waitSharp(main);
    const target = await aim(main, 0, 'a ação do coração');
    await drag(main, target.from, target.to);
    await waitFor(main, "(() => { const s = window.musaeumApp.reader()?.selection; return s && s.kind === 'fixed' && !s.reading ? s : null; })()", { timeoutMs: 8000, what: 'a selection' });
    await click(main, '#mark-color-yellow');
    const annotations = await written(main, 1, 'the highlight');
    const uuid = annotations[0].uuid;

    const back = await textOf(main, '#back-to-library');
    const toc = await textOf(main, '#toc-toggle');
    step('the reader in English', back === EN['reader.back'] && toc === EN['reader.toc'], { back, toc });

    // The reader's contents: closed by Escape, the focus back on its button.
    await click(main, '#toc-toggle');
    await waitFor(main, "document.querySelector('.toc-panel') !== null", { timeoutMs: 5000, what: 'the contents' });
    await main.evaluate("document.querySelector('.toc-panel button, .toc-panel [tabindex]')?.focus() ?? document.querySelector('.toc-panel').setAttribute('tabindex', '-1')");
    await main.evaluate("if (!document.activeElement?.closest('.toc-panel')) document.querySelector('.toc-panel').focus()");
    await press(main, 'Escape');
    await waitFor(main, "document.querySelector('.toc-panel') === null", { timeoutMs: 5000, what: 'the contents to close' });
    await delay(150);
    step('the contents close with Escape, and the focus goes back to their button', (await activeId(main)) === 'toc-toggle', { active: await activeId(main) });

    // Reduced motion in the fixed reader: paginated, page turns animate; reduced, they do not,
    // and the person's own choice stays.
    const paginated = await waitFor(main, "(() => { const s = window.musaeumApp.reader()?.status; return s ? s : null; })()", { timeoutMs: 5000, what: 'the status' });
    if (paginated.mode !== 'paginated') {
      await press(main, 'c');
      await waitFor(main, "window.musaeumApp.reader()?.status?.mode === 'paginated'", { timeoutMs: 8000, what: 'paginated' });
    }
    const before = await waitFor(main, 'window.musaeumApp.reader()?.status', { timeoutMs: 5000 });
    await setPreferences(main, { motion: 'reduce' });
    const reduced = await waitFor(main, "(() => { const s = window.musaeumApp.reader()?.status; return s && s.animating === false ? s : null; })()", { timeoutMs: 8000, what: 'the page turns to stop' }).catch(() => null);
    await setPreferences(main, { motion: 'system' });
    const restored = await waitFor(main, `(() => { const s = window.musaeumApp.reader()?.status; return s && s.animating === ${before.animationPreference} ? s : null; })()`, {
      timeoutMs: 8000,
      what: 'the page turns as the person chose',
    }).catch(() => null);
    step('reduced motion stops the fixed reader\'s page turns and keeps the person\'s own choice', reduced !== null && reduced.animationPreference === before.animationPreference && restored !== null, {
      before,
      reduced,
      restored,
    });

    // A refusal from the host, in English.
    const refusedEn = await pdf(main, 'export-pdf', join(shelf, 'inside.pdf'));
    // The PDF copy, the XFDF and the reports, in English.
    const copyEn = join(exported, 'copy-en.pdf');
    const xfdfEn = join(exported, 'annotations-en.xfdf');
    const mdEn = join(exported, 'report-en.md');
    const pdfReportEn = join(exported, 'report-en.pdf');
    const copyEnResult = await pdf(main, 'export-pdf', copyEn);
    const xfdfEnResult = await pdf(main, 'export-xfdf', xfdfEn);
    const mdEnResult = await exchange(main, 'reader', 'report-md', mdEn);
    const pdfReportEnResult = await exchange(main, 'reader', 'report-pdf', pdfReportEn);

    // And in Portuguese.
    await setPreferences(main, { language: 'pt-BR' });
    const backPt = await textOf(main, '#back-to-library');
    const refusedPt = await pdf(main, 'export-pdf', join(shelf, 'dentro.pdf'));
    const copyPt = join(exported, 'copia-pt.pdf');
    const xfdfPt = join(exported, 'anotacoes-pt.xfdf');
    const mdPt = join(exported, 'relatorio-pt.md');
    const pdfReportPt = join(exported, 'relatorio-pt.pdf');
    const copyPtResult = await pdf(main, 'export-pdf', copyPt);
    const xfdfPtResult = await pdf(main, 'export-xfdf', xfdfPt);
    const mdPtResult = await exchange(main, 'reader', 'report-md', mdPt);
    const pdfReportPtResult = await exchange(main, 'reader', 'report-pdf', pdfReportPt);

    step('the reader back in Portuguese at once', backPt === PT['reader.back'], { backPt });
    step(
      'a refusal from the host in the language shown: English, then Portuguese, and nothing written in the shelf',
      !refusedEn.ok && /is a folder of the library/.test(refusedEn.error) && !refusedPt.ok && /é uma pasta da biblioteca/.test(refusedPt.error) && readdirSync(shelf).join() === 'livro.pdf',
      { en: refusedEn.error, pt: refusedPt.error, shelf: readdirSync(shelf) },
    );

    const ours = async (path) => {
      const found = await main.evaluate(`window.musaeumApp.pdfAnnotations(${JSON.stringify(path)})`);
      return found.reading.annots.find((annot) => annot.name === uuid) ?? null;
    };
    const annotEn = copyEnResult.ok ? await ours(copyEn) : null;
    const annotPt = copyPtResult.ok ? await ours(copyPt) : null;
    step('the PDF copy: /T is the author chosen, /Subj in the language shown (Highlight, Realce)', annotEn?.author === 'Ana Lima' && annotEn?.subject === 'Highlight' && annotPt?.author === 'Ana Lima' && annotPt?.subject === 'Realce', {
      en: annotEn && { author: annotEn.author, subject: annotEn.subject },
      pt: annotPt && { author: annotPt.author, subject: annotPt.subject },
      errors: [copyEnResult.error, copyPtResult.error].filter(Boolean),
    });

    const xfdfTextEn = xfdfEnResult.ok ? readFileSync(xfdfEn, 'utf8') : '';
    const xfdfTextPt = xfdfPtResult.ok ? readFileSync(xfdfPt, 'utf8') : '';
    step('the XFDF: the author as its title, the subject in the language shown', /title="Ana Lima"/.test(xfdfTextEn) && /subject="Highlight"/.test(xfdfTextEn) && /title="Ana Lima"/.test(xfdfTextPt) && /subject="Realce"/.test(xfdfTextPt), {
      errors: [xfdfEnResult.error, xfdfPtResult.error].filter(Boolean),
    });

    const markdownEn = mdEnResult.ok ? readFileSync(mdEn, 'utf8') : '';
    const markdownPt = mdPtResult.ok ? readFileSync(mdPt, 'utf8') : '';
    step(
      'the Markdown report: its labels and dates in the language shown, and the author named',
      markdownEn.includes('*Study report made on 2026-') && markdownEn.includes('*Annotations by Ana Lima.*') && markdownEn.includes('**Highlight** · yellow') && markdownEn.includes('annotator: "Ana Lima"') &&
        markdownPt.includes('*Relatório de estudo gerado em ') && markdownPt.includes('*Anotações de Ana Lima.*') && markdownPt.includes('**Realce** · amarelo'),
      { en: markdownEn.slice(0, 1200), pt: markdownPt.slice(0, 1200), errors: [mdEnResult.error, mdPtResult.error].filter(Boolean) },
    );
    const textOfPdf = async (path) => (await main.evaluate(`window.musaeumApp.pdfText(${JSON.stringify(path)})`)).join(' ').replace(/\s+/g, ' ');
    const pagesEn = pdfReportEnResult.ok ? await textOfPdf(pdfReportEn) : '';
    const pagesPt = pdfReportPtResult.ok ? await textOfPdf(pdfReportPt) : '';
    step(
      'the PDF report, as PDFium reads it: in English, then in Portuguese, with the author',
      /Study report made on/.test(pagesEn) && /Annotations by Ana Lima/.test(pagesEn) && /page 1 of/.test(pagesEn) && /Relatório de estudo gerado em/.test(pagesPt) && /Anotações de Ana Lima/.test(pagesPt) && /página 1 de/.test(pagesPt),
      { en: pagesEn.slice(0, 600), pt: pagesPt.slice(0, 600), errors: [pdfReportEnResult.error, pdfReportPtResult.error].filter(Boolean) },
    );

    // 9. The cache cleared in the dialog: the annotations are all there.
    const annotationsBefore = (await main.evaluate('window.musaeumApp.fixedAnnotations()')).map((view) => view.annotation.uuid).sort();
    const cacheBefore = await main.evaluate('window.musaeumApp.cacheReport()');
    await press(main, ',', CTRL);
    await waitFor(main, "document.querySelector('[data-surface=\"preferences\"]') !== null", { timeoutMs: 5000, what: 'the Preferences' });
    // The dialog asks the host what the cache holds; the button comes with the answer.
    await waitFor(main, "document.querySelector('#prefs-cache-clear') !== null", { timeoutMs: 10_000, what: 'what the cache holds' });
    await click(main, '#prefs-cache-clear');
    await waitFor(main, "document.querySelector('#prefs-cache-confirm') !== null", { timeoutMs: 5000, what: 'the confirmation' });
    const confirmFocused = (await activeId(main)) === 'prefs-cache-cancel';
    await click(main, '#prefs-cache-confirm');
    await waitFor(main, "document.querySelector('#prefs-cache-confirm') === null && (document.querySelector('#prefs-status')?.textContent ?? '').length > 0", { timeoutMs: 30_000, what: 'the cache to be cleared' });
    const statusText = await textOf(main, '#prefs-status');
    const cacheAfter = await main.evaluate('window.musaeumApp.cacheReport()');
    // The confirmation went away with the focus in it: the focus is back on the button, and Escape
    // still closes the dialog.
    const focusAfterClear = await activeId(main);
    await press(main, 'Escape');
    const closedAfterClear = await waitFor(main, "document.querySelector('[data-surface=\"preferences\"]') === null", { timeoutMs: 5000, what: 'the Preferences to close' }).catch(() => false);
    const annotationsAfter = (await main.evaluate('window.musaeumApp.fixedAnnotations()')).map((view) => view.annotation.uuid).sort();
    step(
      'clearing the cache asks first (the safe choice focused), forgets the lines and the OCR, and every annotation stays',
      confirmFocused && cacheAfter.linePages === 0 && cacheAfter.ocrLines === 0 && JSON.stringify(annotationsAfter) === JSON.stringify(annotationsBefore) && annotationsAfter.includes(uuid),
      { cacheBefore, cacheAfter, statusText, annotationsBefore, annotationsAfter },
    );
    step('after clearing, the focus is back on its button, and Escape closes the Preferences', focusAfterClear === 'prefs-cache-clear' && closedAfterClear === true, { focusAfterClear });

    // 10. What will be read again: English, dark, reduced motion, the author, OCR in eng only.
    await setPreferences(main, { language: 'en', theme: 'dark', motion: 'reduce', ocrLanguages: ['eng'] });
    await click(main, '#back-to-library');
    await waitForLibrary(main);
  } finally {
    await closeApp(session);
  }

  // 11. Opened again: everything as it was left, from the first paint.
  session = await launch(app, args, 'window.musaeumApp !== undefined');
  main = session.cdp;
  try {
    await waitForLibrary(main);
    const kept = await main.evaluate('window.musaeumApp.preferences().preferences');
    const page = await main.evaluate("({ lang: document.documentElement.lang, theme: document.documentElement.dataset.theme ?? null, motion: document.documentElement.dataset.motion ?? null, bg: getComputedStyle(document.body).backgroundColor, button: document.querySelector('#open-preferences')?.textContent?.trim() })");
    step(
      'opened again: every preference kept, and the page in English, dark and still from the start',
      kept.language === 'en' && kept.theme === 'dark' && kept.motion === 'reduce' && kept.annotationAuthor === 'Ana Lima' && JSON.stringify(kept.ocrLanguages) === '["eng"]' &&
        page.lang === 'en' && page.theme === 'dark' && page.motion === 'reduce' && page.bg === 'rgb(24, 25, 27)' && page.button === EN['prefs.open'],
      { kept, page },
    );
    await keyboardInDialog(main, 'opened again, in English');
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
  const work = mkdtempSync(join(tmpdir(), 'musaeum-e2e-prefs-'));
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
