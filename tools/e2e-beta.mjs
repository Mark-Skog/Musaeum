#!/usr/bin/env node
// Phase 6, Part 2 (the Beta), in the real window, driven through the WebView2's debugging protocol
// with the input a person gives.
//
//   node tools/e2e-beta.mjs [--build target/release] [--out FILE.json] [--phase synthetic|real|all]
//                           [--corpus DIR] [--text-wait-minutes N] [--require-idle SECONDS]
//
// synthetic: books written for this go in a fresh library.
//   - Ctrl+click, Shift+click and Ctrl+A choose several books, and Escape puts the choice away.
//   - Two books chosen are edited together: series, year, rating, status and a new tag, saved on
//     both at once, each field the person's and no other.
//   - The books' text is searched from the library: a match on a page opens the PDF on that page;
//     a match in an EPUB opens it at the very place, where a note is then made. Back in the library,
//     the search is as it was left.
//   - What waits for review: two works with the same ISBN are merged, keeping the tag, the series
//     and the note of the one merged in; two with the same title and author are said to be
//     different, and are not suggested again.
//   - A Calibre library (metadata.db with custom columns, series, tags; one book with the same
//     bytes as one already in the library; one with no file) comes in through its dialog, and not a
//     byte of it changes.
//   - Closed and opened again, all of it is still there.
// real: the collection at --corpus, only read: listed, searched by title and in its text, a match
//   opened; the collection is listed before and after, and nothing in it changed.
//
// The libraries and the books live in a temporary folder, removed at the end. The run waits for
// the machine to be free and stops as soon as someone uses it (exit code 3).

import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, relative, resolve } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { setTimeout as delay } from 'node:timers/promises';

import { writeCalibreLibrary } from './lib/calibre-library.mjs';
import { click, closeApp, idleSeconds, launch, press, typeInto, waitFor } from './lib/cdp.mjs';
import { epub, sentence, textPdf } from './lib/test-books.mjs';

const root = resolve(import.meta.dirname, '..');
const CTRL = 2;
const SHIFT = 8;

function parseArguments(argv) {
  const options = {
    build: join(root, 'target', 'release'),
    out: join(root, 'bench-results', 'e2e-beta', 'report.json'),
    phase: 'all',
    corpus: 'C:\\Marte\\Archives\\DigitalLibrary\\Books',
    textWaitMinutes: 20,
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
    else if (flag === '--text-wait-minutes') options.textWaitMinutes = Number(value());
    else if (flag === '--require-idle') options.requireIdle = Number(value());
    else throw new Error(`unknown argument: ${flag}`);
  }
  if (!['synthetic', 'real', 'all'].includes(options.phase)) {
    throw new Error(`--phase is synthetic, real or all, not ${options.phase}`);
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
    console.error(`  FAILED: ${label} ${JSON.stringify(detail).slice(0, 2000)}`);
  } else {
    console.log(`  ok: ${label}`);
  }
  return ok;
}

/** A book of two chapters, by `creators`; `extra` is a paragraph added to the second. */
function book(title, creators, identifier, seed, extra = null) {
  return epub({
    title,
    identifier,
    creators,
    chapters: [1, 2].map((chapter) => ({
      title: `Capítulo ${chapter}`,
      paragraphs: [
        ...[0, 1, 2].map((paragraph) => sentence(`${seed}-${chapter}`, seed * 100 + chapter * 10 + paragraph, 14)),
        ...(chapter === 2 && extra !== null ? [extra] : []),
      ],
    })),
  });
}

/** Every file and folder under `directory`: size and times, and the bytes' hash when `hash`. */
function listTree(directory, { hash = false } = {}) {
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
        ...(hash && !stat.isDirectory() ? { sha256: createHash('sha256').update(readFileSync(path)).digest('hex') } : {}),
      });
      if (stat.isDirectory()) {
        walk(path);
      }
    }
  };
  walk(directory);
  return entries.sort((a, b) => a.path.localeCompare(b.path));
}

/** Marks the first element `selector` matches whose text is `text`, and clicks it. */
async function clickText(main, selector, text, modifiers = 0) {
  checkStillQuiet();
  await waitFor(
    main,
    `(() => {
      document.querySelectorAll('[data-e2e-target]').forEach((node) => node.removeAttribute('data-e2e-target'));
      const wanted = ${JSON.stringify(text)};
      const node = [...document.querySelectorAll(${JSON.stringify(selector)})].find((candidate) => candidate.textContent.trim() === wanted || candidate.getAttribute('aria-label') === wanted);
      if (!node) return null;
      node.setAttribute('data-e2e-target', '1');
      return true;
    })()`,
    { timeoutMs: 10_000, what: `${selector} “${text}”` },
  );
  await click(main, '[data-e2e-target="1"]', modifiers);
}

const taxonomy = (main) => main.evaluate('window.musaeumApp.taxonomy()');
const metadata = (main, work) => main.evaluate(`window.musaeumApp.workMetadata(${work})`);
const rows = (main) => main.evaluate('window.musaeumApp.rows()');
const selection = (main) => main.evaluate('window.musaeumApp.selection()');
/** Whether a work is there: a merge takes the other away. */
const workThere = (main, work) => main.evaluate(`window.musaeumApp.workMetadata(${work}).then(() => true, () => false)`);

/** The choice once it holds `count` books, or null after the wait. */
async function chosen(main, count, what) {
  return waitFor(main, `(() => { const s = window.musaeumApp.selection(); return s.length === ${count} ? s : null; })()`, { timeoutMs: 8000, what }).catch(() => null);
}

async function saveWorks(main) {
  await waitFor(main, "(() => { const b = document.querySelector('#save-works'); return b && !b.disabled; })()", { timeoutMs: 5000, what: 'something to save' });
  await click(main, '#save-works');
  await waitFor(main, "(() => { const b = document.querySelector('#save-works'); const a = document.querySelector('.inspector.batch'); return b && b.disabled && a && a.getAttribute('aria-busy') !== 'true'; })()", { timeoutMs: 8000, what: 'the save' });
}

async function addChip(main, label, typed) {
  await typeInto(main, `.inspector input[aria-label="${label}"], .inspector [aria-label="${label}"] input`, typed);
  await press(main, 'Enter');
}

async function waitForLibrary(main) {
  await waitFor(main, "window.musaeumApp.view() === 'library' && document.querySelector('.library') !== null", { timeoutMs: 15_000, what: 'the library' });
}

async function backToLibrary(main) {
  await click(main, '#back-to-library');
  await waitForLibrary(main);
}

/** Opens a listed book as a person does: a click chooses it, Enter opens it. */
async function openListed(main, documentId) {
  await waitFor(main, `document.querySelector('[data-document="${documentId}"]') !== null`, { timeoutMs: 15_000, what: `the card of ${documentId}` });
  const before = await main.evaluate('window.musaeumApp.opened()');
  await click(main, `[data-document="${documentId}"]`);
  await press(main, 'Enter');
  await waitFor(main, `window.musaeumApp.opened() > ${before}`, { timeoutMs: 5000, what: 'the book to open' });
  return readerReady(main, documentId);
}

async function readerReady(main, documentId) {
  return waitFor(main, `(() => { const r = window.musaeumApp.reader(); return r !== null && r.documentId === ${documentId} && (r.ready || r.error !== null) ? r : null; })()`, {
    timeoutMs: 30_000,
    what: `the reader of ${documentId}`,
  });
}

/** Types a search, in the titles or in the text. */
async function search(main, text, where) {
  await typeInto(main, '.search input', text);
  await clickText(main, '.segmented .segment', where === 'text' ? 'No texto' : 'Títulos');
}

/** The hits listed for a book once there are some, with where they are. */
async function hitsOf(main, documentId, timeoutMs = 15_000) {
  return waitFor(
    main,
    `(() => {
      const book = document.querySelector('.text-book[data-document="${documentId}"]');
      if (!book) return null;
      return [...book.querySelectorAll('.text-hit')].map((hit) => ({
        kind: hit.dataset.kind,
        unit: Number(hit.dataset.unit),
        where: hit.querySelector('.text-hit-where')?.textContent.trim(),
        marked: [...hit.querySelectorAll('mark')].map((mark) => mark.textContent),
      }));
    })()`,
    { timeoutMs, what: `the matches in ${documentId}` },
  );
}

// --- The synthetic library ---

async function runSynthetic(work) {
  const shelf = join(work, 'estante');
  mkdirSync(shelf, { recursive: true });
  const domBytes = book('Dom Casmurro', ['Machado de Assis'], 'urn:x:dom', 5);
  const files = {
    vida: ['vida.epub', book('Vida Maravilhosa', ['Stephen Jay Gould'], 'urn:isbn:9788535902778', 1)],
    vida2: ['vida-2.epub', book('Vida Maravilhosa', ['Stephen Jay Gould'], 'urn:isbn:9788535902778', 2, 'Um ornitorrinco atravessa o capítulo devagar.')],
    cosmos: ['cosmos.epub', book('Cosmos', ['Carl Sagan'], 'urn:x:cosmos', 3)],
    cosmos2: ['cosmos-outro.epub', book('Cosmos', ['Carl Sagan'], 'urn:x:cosmos-outro', 4)],
    dom: ['dom.epub', domBytes],
    astro: [
      'astronomia.pdf',
      textPdf([1, 2, 3, 4, 5].map((page) => [`Página ${page} do caderno de astronomia.`, ...(page === 4 ? ['Ao longe, um quasar distante brilha no fim.'] : []), sentence(`p${page}`, page * 7, 10)])),
    ],
  };
  for (const [name, bytes] of Object.values(files)) {
    writeFileSync(join(shelf, name), bytes);
  }
  const library = join(work, 'biblioteca');
  let session = await launch(app, ['--library', library, '--e2e', '--width', '1400', '--height', '900'], 'window.musaeumApp !== undefined');
  let main = session.cdp;
  const ids = {};
  try {
    await waitFor(main, "document.querySelector('.welcome') !== null || document.querySelector('.library') !== null", { what: 'the first screen' });
    await main.evaluate(`window.musaeumApp.addRoot(${JSON.stringify(shelf)})`);
    const first = await waitFor(main, '(() => { const r = window.musaeumApp.rows(); const i = window.musaeumApp.library(); return r.total === 6 && i && !i.indexing && r.rows.every((b) => b.blake3 !== null) ? r : null; })()', {
      timeoutMs: 120_000,
      what: 'the six books',
    });
    for (const [key, [name]] of Object.entries(files)) {
      ids[key] = first.rows.find((row) => row.path.endsWith(`\\${name}`) || row.path.endsWith(`/${name}`));
    }
    step('the six books are listed, each its own work', Object.values(ids).every((row) => row !== undefined) && new Set(Object.values(ids).map((row) => row.workId)).size === 6, { ids });

    // 1. Choosing several: Ctrl+click, Shift+click, Ctrl+A, Escape.
    const order = first.rows.map((row) => row.documentId);
    await click(main, `[data-document="${order[0]}"]`);
    await waitFor(main, "document.querySelector('.inspector[data-work]') !== null", { timeoutMs: 8000, what: 'the details of one book' });
    await click(main, `[data-document="${order[2]}"]`, CTRL);
    const two = await chosen(main, 2, 'two books');
    const batchShown = await waitFor(main, "document.querySelector('.inspector.batch[data-works=\"2\"]') !== null", { timeoutMs: 8000, what: 'the details of two books' }).catch(() => false);
    const status = await main.evaluate("document.querySelector('.toolbar-status')?.textContent ?? ''");
    step('Ctrl+click adds a book to the choice, and the details become those of both', two !== null && batchShown === true && status.includes('2 escolhidos'), { two, status });
    await click(main, `[data-document="${order[0]}"]`);
    await chosen(main, 1, 'one book again');
    await click(main, `[data-document="${order[3]}"]`, SHIFT);
    const range = await chosen(main, 4, 'four books');
    step(
      'Shift+click chooses the books between',
      range !== null && JSON.stringify(range.map((book) => book.documentId).sort()) === JSON.stringify(order.slice(0, 4).sort()),
      { range, order },
    );
    await press(main, 'a', CTRL);
    const all = await chosen(main, 6, 'every book');
    step('Ctrl+A chooses every book of the list', all !== null, { all });
    await press(main, 'Escape');
    const none = await waitFor(main, "window.musaeumApp.selection().length === 0 && document.querySelector('.inspector') === null", { timeoutMs: 5000, what: 'nothing chosen' }).catch(() => false);
    step('Escape puts the choice and the details away', none === true);

    // 2. Two books edited together.
    await click(main, `[data-document="${ids.vida.documentId}"]`);
    await click(main, `[data-document="${ids.vida2.documentId}"]`, CTRL);
    await waitFor(main, "document.querySelector('.inspector.batch[data-works=\"2\"]') !== null", { timeoutMs: 8000, what: 'the details of the two Vidas' });
    await typeInto(main, '.inspector input[data-field="batch-series"]', 'Paleontologia');
    await typeInto(main, '.inspector input[data-field="batch-year"]', '1989');
    await click(main, '.inspector [aria-label="4 estrelas"]');
    await click(main, '.inspector [data-picker="Estado de leitura"] .select-button');
    await clickText(main, '[role="option"]', 'Lido');
    await addChip(main, 'Acrescentar etiquetas', 'Fósseis');
    await saveWorks(main);
    const edited = [await metadata(main, ids.vida.workId), await metadata(main, ids.vida2.workId)];
    const userFields = ['series', 'year', 'rating', 'status'];
    step(
      'a change made to two books is saved on both, each field set the person’s and no other',
      edited.every(
        (work) =>
          work.series === 'Paleontologia' &&
          work.year === 1989 &&
          work.rating === 4 &&
          work.status === 'read' &&
          work.tagNames.includes('Fósseis') &&
          userFields.every((field) => work.provenance[field] === 'user') &&
          work.provenance.title !== 'user' &&
          work.provenance.creators !== 'user' &&
          work.title === 'Vida Maravilhosa',
      ),
      { edited: edited.map((work) => ({ series: work.series, year: work.year, rating: work.rating, status: work.status, tags: work.tagNames, provenance: work.provenance })) },
    );
    await press(main, 'Escape');

    // 3. The books' text.
    const indexed = await waitFor(main, '(async () => { const s = await window.musaeumApp.fulltextStatus(); return s.pending === 0 && s.indexed >= 6 ? s : null; })()', {
      timeoutMs: 180_000,
      what: 'the text of the six books in the index',
    }).catch(() => null);
    step('the text of every book is read into the index, in the background', indexed !== null, { indexed });
    await search(main, 'QUASAR', 'text');
    const pageHits = await hitsOf(main, ids.astro.documentId);
    step(
      'a word is found on the page it is on, with the match marked',
      pageHits.length === 1 && pageHits[0].kind === 'page' && pageHits[0].unit === 3 && pageHits[0].where === 'p. 4' && pageHits[0].marked.join() === 'quasar',
      { pageHits },
    );
    await click(main, `.text-book[data-document="${ids.astro.documentId}"] .text-hit`);
    await readerReady(main, ids.astro.documentId);
    const onPage = await waitFor(
      main,
      '(async () => { const i = await window.musaeumApp.inspectFixed(); return i && i.snapshot && i.snapshot.page === 3 && i.pages.some((p) => p.page === 3) ? { page: i.snapshot.page, shown: i.pages.map((p) => p.page) } : null; })()',
      { timeoutMs: 15_000, what: 'page 4 shown' },
    ).catch(() => null);
    step('a click on a match on a page opens the book on that page', onPage !== null, { onPage, target: await main.evaluate('window.musaeumApp.target()') });
    await backToLibrary(main);
    const kept = await waitFor(main, "(() => { const i = document.querySelector('.search input'); return i && i.value === 'QUASAR' && document.querySelector('.text-results') !== null; })()", { timeoutMs: 8000, what: 'the search as it was' }).catch(() => false);
    step('back in the library, the search in the text is as it was left', kept === true);

    await search(main, 'ornitorrinco', 'text');
    const flowHits = await hitsOf(main, ids.vida2.documentId);
    const found = await main.evaluate("window.musaeumApp.fulltextSearch('ornitorrinco')");
    const unit = found.books[0]?.units[0];
    step('a word is found in the section of a flowing book it is in', flowHits.length === 1 && flowHits[0].kind === 'document' && unit !== undefined && flowHits[0].unit === unit.index, { flowHits, unit });
    await click(main, `.text-book[data-document="${ids.vida2.documentId}"] .text-hit`);
    await readerReady(main, ids.vida2.documentId);
    const place = await main.evaluate(`window.musaeumApp.flowPlace(${ids.vida2.documentId}, ${unit.index}, ${unit.position}, ${unit.matched.length})`);
    const spineOf = (cfi) => String(cfi).replace(/^epubcfi\(/, '').split('!')[0];
    const there = await waitFor(
      main,
      `(() => { const r = window.musaeumApp.reader(); const at = r && r.flow && r.flow.relocation; return at && ${JSON.stringify(spineOf(place.start))} === String(at.cfi).replace(/^epubcfi\\(/, '').split('!')[0] ? at : null; })()`,
      { timeoutMs: 15_000, what: 'the reader at the match' },
    ).catch(async () => ({ missed: (await main.evaluate('window.musaeumApp.reader()'))?.flow?.relocation ?? null }));
    step('a click on a match in a flowing book opens it there', there.missed === undefined, { place, there });
    const note = await main.evaluate(`window.musaeumApp.annotateFlow(${JSON.stringify(place.start)}, ${JSON.stringify(place.end)}, 'Nota no ornitorrinco')`).catch((error) => ({ error: String(error) }));
    step('the place a match is at takes a note', note?.annotation?.quote?.includes('ornitorrinco') === true, { note });
    await backToLibrary(main);
    await search(main, '', 'titles');

    // 4. What waits for review.
    await clickText(main, '.folder-item .folder-name', 'Revisar');
    const vidaPair = [ids.vida.workId, ids.vida2.workId].sort((a, b) => a - b).join('-');
    const cosmosPair = [ids.cosmos.workId, ids.cosmos2.workId].sort((a, b) => a - b).join('-');
    const pairs = await waitFor(main, `(() => { const p = [...document.querySelectorAll('.review-item[data-pair]')].map((n) => ({ pair: n.dataset.pair, reason: n.querySelector('.review-reason')?.textContent })); return p.length >= 2 ? p : null; })()`, {
      timeoutMs: 10_000,
      what: 'the pairs to review',
    });
    const waiting = await main.evaluate("Number(document.querySelector('[data-waiting]')?.dataset.waiting)");
    // The second Vida has the first's identifier inside it and other bytes: the library also asks
    // whether it is the same file, changed (Phase 1).
    const suggestions = await main.evaluate("[...document.querySelectorAll('.review-item[data-suggestion]')].map((n) => ({ id: Number(n.dataset.suggestion), reason: n.querySelector('.review-reason')?.textContent, documents: [...n.querySelectorAll('[data-document]')].map((d) => Number(d.dataset.document)) }))");
    step(
      'the works that may be one book wait for review, each with why, and so do the files that may be one',
      pairs.some((pair) => pair.pair === vidaPair && pair.reason.includes('ISBN')) &&
        pairs.some((pair) => pair.pair === cosmosPair && pair.reason.includes('título')) &&
        suggestions.length === 1 &&
        suggestions[0].reason.includes('identificador interno') &&
        [ids.vida.documentId, ids.vida2.documentId].every((document) => suggestions[0].documents.includes(document)) &&
        waiting === pairs.length + suggestions.length,
      { pairs, suggestions, waiting },
    );
    if (suggestions.length > 0) {
      await clickText(main, `.review-item[data-suggestion="${suggestions[0].id}"] button`, 'São diferentes');
      const turnedDown = await waitFor(main, `document.querySelector('.review-item[data-suggestion="${suggestions[0].id}"]') === null`, { timeoutMs: 8000, what: 'the suggestion turned down' }).catch(() => false);
      step('a file said to be different from another is left as it is, and not asked about again', turnedDown === true && (await main.evaluate('window.musaeumApp.review()')).suggestions.length === 0);
    }
    await clickText(main, `.review-item[data-pair="${cosmosPair}"] button`, 'São livros diferentes');
    const dismissed = await waitFor(main, `document.querySelector('.review-item[data-pair="${cosmosPair}"]') === null`, { timeoutMs: 8000, what: 'the pair turned down' }).catch(() => false);
    step('two works said to be different books are not suggested again', dismissed === true);
    await clickText(main, `.review-item[data-pair="${vidaPair}"] button`, 'Fundir…');
    await waitFor(main, "document.querySelector('.merge-choices') !== null", { timeoutMs: 8000, what: 'the merge dialog' });
    await click(main, `.merge-choice[data-work="${ids.vida.workId}"]`);
    await clickText(main, '.library-dialog button[type="submit"]', 'Fundir');
    await waitFor(main, `document.querySelector('.library-dialog') === null && document.querySelector('.review-item[data-pair="${vidaPair}"]') === null`, { timeoutMs: 10_000, what: 'the merge' });
    const merged = await metadata(main, ids.vida.workId);
    const gone = !(await workThere(main, ids.vida2.workId));
    step(
      'merged, the two works are one, with both documents and what was on them',
      gone && merged.documents.length === 2 && merged.documents.includes(ids.vida2.documentId) && merged.tagNames.includes('Fósseis') && merged.series === 'Paleontologia',
      { merged: { documents: merged.documents, tags: merged.tagNames, series: merged.series }, gone },
    );
    await clickText(main, '.folder-item .folder-name', 'Todos os livros');
    await waitFor(main, `(() => { const r = window.musaeumApp.rows(); const b = r.filter && r.filter.kind === 'all' && r.rows.find((x) => x.documentId === ${ids.vida2.documentId}); return b && b.workId === ${ids.vida.workId}; })()`, {
      timeoutMs: 10_000,
      what: 'the merged book listed in its new work',
    });
    const reopened = await openListed(main, ids.vida2.documentId);
    const notes = reopened.annotations.filter((entry) => entry.note === 'Nota no ornitorrinco');
    step('the note made on the book merged in is still on it', notes.length === 1 && notes[0].anchorState === 'exact', { annotations: reopened.annotations });
    await backToLibrary(main);
    await press(main, 'Escape');

    // 5. A Calibre library.
    const calibre = join(work, 'Calibre');
    writeCalibreLibrary(calibre, [
      {
        title: 'O Cortiço',
        authors: ['Aluísio Azevedo'],
        series: ['Naturalismo', 2],
        tags: ['Romance', 'Clássico'],
        rating: 7,
        identifiers: [['isbn', '978-0-306-40615-7'], ['goodreads', '12345']],
        comments: '<div><p>Um <b>cortiço</b> no Rio.</p><p>Segunda parte.</p></div>',
        language: 'por',
        pubdate: '1890-01-01 00:00:00+00:00',
        folder: 'Aluisio Azevedo/O Cortico (1)',
        files: [
          ['epub', book('Cortiço do Arquivo', ['Autor do Arquivo'], 'urn:x:cortico', 10, 'A estalagem acordava cedo.')],
          ['pdf', textPdf([['O Cortiço, em PDF.', sentence('cortico-pdf', 11, 10)]])],
        ],
        custom: [
          ['lido', true],
          ['genero', 'Romance'],
          ['paginas', 312],
          ['nota', '<p>Lido na <i>escola</i>.</p>'],
          ['emprestado', '2024-03-01 10:00:00+00:00'],
          ['estrelas', 8],
        ],
      },
      {
        title: 'Dom Casmurro',
        authors: ['Machado de Assis'],
        series: ['Romances de Machado', 4],
        tags: ['Romance'],
        rating: 10,
        identifiers: [],
        comments: null,
        language: 'por',
        pubdate: '1899-01-01 00:00:00+00:00',
        folder: 'Machado de Assis/Dom Casmurro (2)',
        files: [['epub', domBytes]],
        custom: [['genero', 'Romance']],
      },
      {
        title: 'Livro Perdido',
        authors: ['Ninguém'],
        series: null,
        tags: [],
        rating: null,
        identifiers: [],
        comments: null,
        language: null,
        pubdate: '2001-01-01 00:00:00+00:00',
        folder: 'Ninguem/Livro Perdido (3)',
        files: [['epub', null]],
        custom: [],
      },
    ]);
    const calibreBefore = listTree(calibre, { hash: true });
    await main.evaluate(`window.musaeumApp.openCalibre(${JSON.stringify(calibre)})`);
    const preview = await waitFor(
      main,
      "(() => { const p = document.querySelector('.calibre-preview'); return p ? { books: Number(p.dataset.books), columns: [...p.querySelectorAll('.calibre-columns li')].map((n) => n.textContent.trim()) } : null; })()",
      { timeoutMs: 15_000, what: 'what the Calibre library holds' },
    );
    step(
      'the Calibre dialog says what the library holds, and that a calculated column is not brought',
      preview.books === 3 && preview.columns.length === 7 && preview.columns.some((text) => text.startsWith('Resumo') && text.includes('não é trazida')),
      { preview },
    );
    await clickText(main, '.library-dialog button[type="submit"]', 'Importar');
    const imported = await waitFor(
      main,
      "(() => { const r = document.querySelector('.calibre-report'); const b = document.querySelector('.library-dialog button[type=\"submit\"]'); return r && b && b.textContent.trim() === 'Fechar' ? { imported: Number(r.dataset.imported), waiting: Number(r.dataset.waiting), text: r.textContent.trim() } : null; })()",
      { timeoutMs: 180_000, what: 'the import' },
    );
    step('the books of the Calibre library with their files come in; the one without waits for nothing', imported.imported === 2 && imported.waiting === 0, { imported });
    await clickText(main, '.library-dialog button[type="submit"]', 'Fechar');
    const tree = await taxonomy(main);
    const field = (name) => tree.fields.find((candidate) => candidate.name === name);
    const listedNow = await waitFor(main, '(() => { const r = window.musaeumApp.rows(); return r.total === 8 && r.rows.every((b) => b.blake3 !== null) ? r : null; })()', { timeoutMs: 60_000, what: 'the Cortiço listed' });
    const cortico = listedNow.rows.filter((row) => row.path.includes('O Cortico (1)'));
    const corticoWork = cortico[0] === undefined ? null : await metadata(main, cortico[0].workId);
    const value = (name) => corticoWork?.custom.find((entry) => entry.fieldId === field(name)?.id)?.value;
    step(
      'a Calibre book comes in whole: its formats one work, its series, tags, rating, identifiers, comments and columns, each from Calibre',
      cortico.length === 2 &&
        cortico[0].workId === cortico[1].workId &&
        corticoWork.title === 'O Cortiço' &&
        corticoWork.creators[0]?.name === 'Aluísio Azevedo' &&
        corticoWork.series === 'Naturalismo' &&
        corticoWork.seriesPosition === 2 &&
        corticoWork.rating === 4 &&
        corticoWork.language === 'pt' &&
        corticoWork.year === 1890 &&
        corticoWork.description === 'Um cortiço no Rio.\n\nSegunda parte.' &&
        JSON.stringify(corticoWork.tagNames) === JSON.stringify(['Clássico', 'Romance']) &&
        corticoWork.identifiers.some((identifier) => identifier.scheme === 'isbn' && identifier.value === '9780306406157') &&
        value('Lido') === true &&
        value('Gênero') === 'Romance' &&
        value('Páginas') === 312 &&
        value('Nota') === 'Lido na escola.' &&
        value('Emprestado em') === '2024-03-01' &&
        value('Estrelas') === 4 &&
        field('Resumo') === undefined &&
        ['title', 'creators', 'series', 'rating', 'description', 'identifiers'].every((name) => corticoWork.provenance[name] === 'calibre'),
      { cortico, corticoWork, fields: tree.fields },
    );
    const domRows = listedNow.rows.filter((row) => row.blake3 === ids.dom.blake3);
    const domWork = await metadata(main, ids.dom.workId);
    step(
      'a Calibre book with the same bytes as one in the library is that book, with what Calibre knows of it',
      domRows.length === 1 && domRows[0].documentId === ids.dom.documentId && domRows[0].workId === ids.dom.workId && domWork.series === 'Romances de Machado' && domWork.tagNames.includes('Romance') && domWork.rating === 5,
      { domRows, domWork: { series: domWork.series, tags: domWork.tagNames, rating: domWork.rating, documents: domWork.documents } },
    );
    const calibreAfter = listTree(calibre, { hash: true });
    step(
      'not a byte of the Calibre library changed, and no journal was left beside its database',
      JSON.stringify(calibreAfter) === JSON.stringify(calibreBefore) && !['metadata.db-journal', 'metadata.db-wal', 'metadata.db-shm'].some((name) => existsSync(join(calibre, name))),
      { files: calibreBefore.length },
    );
    report.ids = Object.fromEntries(Object.entries(ids).map(([key, row]) => [key, { documentId: row.documentId, workId: row.workId }]));
    report.cortico = corticoWork?.id ?? null;
  } finally {
    report.exitCode = await closeApp(session).catch((error) => String(error));
    report.console = session.console.slice();
  }

  // 6. Closed and opened again.
  session = await launch(app, ['--library', library, '--e2e', '--width', '1400', '--height', '900'], 'window.musaeumApp !== undefined');
  main = session.cdp;
  try {
    await waitFor(main, "document.querySelector('.library') !== null", { what: 'the library' });
    await waitFor(main, '(() => { const r = window.musaeumApp.rows(); return r.total === 8 ? r : null; })()', { timeoutMs: 30_000, what: 'the books' });
    const review = await main.evaluate('window.musaeumApp.review()');
    const merged = await metadata(main, ids.vida.workId);
    // The Calibre books' text is read after they came in, in the background.
    await waitFor(main, '(async () => { const s = await window.musaeumApp.fulltextStatus(); return s.pending === 0 ? s : null; })()', { timeoutMs: 180_000, what: 'the text index' }).catch(() => null);
    const cortico = report.cortico === null ? null : await metadata(main, report.cortico);
    const text = await main.evaluate("window.musaeumApp.fulltextSearch('estalagem')");
    step(
      'closed and opened again: the merge, the pair turned down and the Calibre books are still there, and the Calibre book’s text is in the index',
      merged.documents.length === 2 &&
        !review.duplicates.some((pair) => pair.works.includes(ids.cosmos.workId) && pair.works.includes(ids.cosmos2.workId)) &&
        cortico?.series === 'Naturalismo' &&
        text.books.length === 1,
      { duplicates: review.duplicates, cortico: cortico?.title, text: text.books.length },
    );
  } finally {
    report.reopenExitCode = await closeApp(session).catch((error) => String(error));
  }
}

// --- The real collection, only read ---

async function runReal(work) {
  const before = listTree(options.corpus);
  const result = { files: before.filter((entry) => !entry.directory).length };
  report.real = result;
  const library = join(work, 'biblioteca-acervo');
  const session = await launch(app, ['--library', library, '--e2e', '--width', '1400', '--height', '900'], 'window.musaeumApp !== undefined');
  const main = session.cdp;
  let listing = null;
  try {
    await waitFor(main, "document.querySelector('.welcome') !== null || document.querySelector('.library') !== null", { what: 'the first screen' });
    await main.evaluate(`window.musaeumApp.addRoot(${JSON.stringify(options.corpus)})`);
    const started = Date.now();
    for (const until = Date.now() + 1_800_000; ; ) {
      const done = await main.evaluate('(() => { const i = window.musaeumApp.library(); const r = window.musaeumApp.rows(); return i !== null && !i.indexing && r.total > 0 && r.rows.every((b) => b.blake3 !== null); })()');
      if (done) {
        break;
      }
      if (Date.now() > until) {
        throw new Error('the library did not finish reading the collection in 30 minutes');
      }
      checkStillQuiet();
      await delay(3000);
    }
    listing = await rows(main);
    result.listed = listing.total;
    result.listingMs = Date.now() - started;
    step('the collection is listed, every file of a book a book', listing.total > 0 && listing.total <= result.files, { listed: listing.total, files: result.files });

    // A search by title: a word of a book's title finds it.
    const sample = listing.rows.find((row) => /\p{L}{5,}/u.test(row.title)) ?? listing.rows[0];
    const word = sample.title.match(/\p{L}{5,}/u)?.[0] ?? sample.title;
    await search(main, word, 'titles');
    const byTitle = await waitFor(main, `(() => { const r = window.musaeumApp.rows(); return r.text === ${JSON.stringify(word)} ? r : null; })()`, { timeoutMs: 10_000, what: `the search “${word}”` });
    step('a search by title finds the book the word is from', byTitle.rows.some((row) => row.documentId === sample.documentId), { word, found: byTitle.total, sample: sample.title });

    // The text of the books, read in the background: searched once there is enough of it. The
    // search in the text is shown meanwhile, so the window says how far the reading is (a person
    // looking sees it move) and its results grow as books are read.
    const words = ['the', 'de'];
    await search(main, words[0], 'text');
    const textStarted = Date.now();
    let status = null;
    let told = 0;
    for (const until = Date.now() + options.textWaitMinutes * 60_000; Date.now() < until; ) {
      status = await main.evaluate('window.musaeumApp.fulltextStatus()');
      result.text = { status, waitedMs: Date.now() - textStarted };
      if (status.pending === 0) {
        break;
      }
      if (Date.now() - told >= 60_000) {
        told = Date.now();
        const read = status.indexed + status.empty + status.failed;
        console.log(`  text: ${read} of ${read + status.pending} books read (${status.indexed} with text, ${status.units} pages or sections) after ${Math.round((Date.now() - textStarted) / 1000)} s${status.current ? `; reading “${status.current}”` : ''}`);
        mkdirSync(dirname(options.out), { recursive: true });
        writeFileSync(options.out, JSON.stringify(report, null, 2));
      }
      checkStillQuiet();
      await delay(5000);
    }
    result.text = { status, waitedMs: Date.now() - textStarted };
    let textHits = null;
    for (const candidate of words) {
      textHits = await main.evaluate(`window.musaeumApp.fulltextSearch(${JSON.stringify(candidate)})`);
      if (textHits.books.length > 0) {
        result.text.word = candidate;
        break;
      }
    }
    result.text.books = textHits?.books.length ?? 0;
    result.text.elapsedMs = textHits?.elapsedMs ?? null;
    step('the books’ text is read into the index and searched', status !== null && status.indexed > 0 && result.text.books > 0, { text: result.text });
    if (result.text.books > 0) {
      if (result.text.word !== words[0]) {
        await search(main, result.text.word, 'text');
      }
      const shown = await waitFor(
        main,
        "(() => { const hits = [...document.querySelectorAll('.text-hit[data-kind=\"page\"]')]; const hit = hits[0]; if (!hit) return null; hit.setAttribute('data-e2e-hit', '1'); return { document: Number(hit.closest('.text-book').dataset.document), unit: Number(hit.dataset.unit) }; })()",
        { timeoutMs: 15_000, what: 'a match on a page' },
      ).catch(() => null);
      if (shown !== null) {
        await click(main, '[data-e2e-hit="1"]');
        await readerReady(main, shown.document);
        const onPage = await waitFor(
          main,
          `(async () => { const i = await window.musaeumApp.inspectFixed(); return i && i.snapshot && i.snapshot.page === ${shown.unit} && i.pages.some((p) => p.page === ${shown.unit}) ? i.snapshot.page : null; })()`,
          { timeoutMs: 20_000, what: 'the page of the match' },
        ).catch(() => null);
        step('a match in the real collection opens its book on its page', onPage !== null, { shown });
        await backToLibrary(main);
      }
    }
  } finally {
    result.exitCode = await closeApp(session).catch((error) => String(error));
    result.console = session.console.slice(-50);
    // Which books gave no text, and why: from the test library's own cache, never the collection.
    try {
      const cache = new DatabaseSync(join(library, 'cache', 'cache.db'), { readOnly: true });
      const titles = new Map((listing?.rows ?? []).map((row) => [row.blake3, row.title]));
      const unread = cache.prepare("SELECT lower(hex(blake3)) AS blake3, state, error FROM fulltext_content WHERE state <> 'done' ORDER BY state, error").all();
      cache.close();
      result.unread = unread.map((entry) => ({ title: titles.get(entry.blake3) ?? entry.blake3, state: entry.state, error: entry.error }));
    } catch (error) {
      result.unread = String(error);
    }
    const after = listTree(options.corpus);
    result.unchanged = JSON.stringify(after) === JSON.stringify(before);
    result.entries = before.length;
    result.bytes = before.reduce((sum, entry) => sum + (entry.directory ? 0 : Number(entry.size)), 0);
    step('the collection is exactly as it was: not a file added, changed, moved or taken away', result.unchanged, { entries: result.entries, files: result.files });
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
  const work = mkdtempSync(join(tmpdir(), 'musaeum-e2e-beta-'));
  try {
    if (options.phase === 'synthetic' || options.phase === 'all') {
      console.log('synthetic');
      await runSynthetic(work);
    }
    if (options.phase === 'real' || options.phase === 'all') {
      console.log(`real: ${options.corpus}`);
      await runReal(work);
    }
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
